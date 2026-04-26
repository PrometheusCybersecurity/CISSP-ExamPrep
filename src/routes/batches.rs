//! /api/batches/* — quiz batch lifecycle.

use axum::{
    extract::{Path, State},
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use futures_util::stream::Stream;
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::convert::Infallible;
use std::sync::Arc;
use uuid::Uuid;

use super::{dashboard, ApiError};
use crate::{
    db,
    engine,
    llm::{self, ChatMessage},
    pdf,
    AppState,
};

// ─── Models ─────────────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
pub struct Question {
    pub id: String,
    pub domain: u8,
    pub subtopic: Option<String>,
    pub difficulty: u8,
    pub question: String,
    pub options: serde_json::Value,
    pub correct: String,
    pub explanation: Option<String>,
    pub user_answer: Option<String>,
    pub is_correct: Option<bool>,
    pub answered_at: Option<i64>,
    pub created_at: i64,
    pub batch_id: String,
}

#[derive(Serialize)]
pub struct CurrentBatch {
    pub batch_id: String,
    pub idx: u32,
    pub total: u32,
    pub question: Option<Question>,
    pub finished: bool,
}

// ─── GET /api/batches/current ───────────────────────────────────────────────

pub async fn current(State(state): State<AppState>) -> Result<Json<Option<CurrentBatch>>, ApiError> {
    let conn = state.pool.get()?;
    let active = match db::get_state(&conn, "active_batch_id")? {
        Some(id) => id,
        None => return Ok(Json(None)),
    };
    let batch = match load_batch(&conn, &active)? {
        Some(b) => b,
        None => return Ok(Json(None)),
    };
    let total = batch.question_ids.len() as u32;
    let finished = batch.finished;
    let question = if (batch.idx as usize) < batch.question_ids.len() && !finished {
        let qid = &batch.question_ids[batch.idx as usize];
        load_question(&conn, qid)?
    } else {
        None
    };
    Ok(Json(Some(CurrentBatch {
        batch_id: batch.id,
        idx: batch.idx,
        total,
        question,
        finished,
    })))
}

// ─── POST /api/batches/:id/answer ──────────────────────────────────────────

#[derive(Deserialize)]
pub struct AnswerPayload {
    pub letter: String,
}

#[derive(Serialize)]
pub struct AnswerResponse {
    pub question: Question,
    pub idx: u32,
    pub total: u32,
}

pub async fn answer(
    State(state): State<AppState>,
    Path(batch_id): Path<String>,
    Json(body): Json<AnswerPayload>,
) -> Result<Json<AnswerResponse>, ApiError> {
    let letter = body.letter.trim().to_uppercase();
    if !["A", "B", "C", "D"].contains(&letter.as_str()) {
        return Err(ApiError::bad("letter must be one of A, B, C, D"));
    }

    let mut conn = state.pool.get()?;
    let tx = conn.transaction()?;

    let batch = load_batch(&tx, &batch_id)?
        .ok_or_else(|| ApiError::not_found("batch not found"))?;
    let qid = batch
        .question_ids
        .get(batch.idx as usize)
        .ok_or_else(|| ApiError::bad("no current question for this batch"))?
        .clone();
    let mut q = load_question(&tx, &qid)?
        .ok_or_else(|| ApiError::not_found("current question missing"))?;
    if q.user_answer.is_some() {
        return Err(ApiError::bad("question already answered"));
    }

    let is_correct = letter == q.correct;
    let now = db::now_ms();
    tx.execute(
        "UPDATE questions
            SET user_answer = ?1, is_correct = ?2, answered_at = ?3
          WHERE id = ?4",
        rusqlite::params![letter, is_correct as i64, now, qid],
    )?;
    q.user_answer = Some(letter.clone());
    q.is_correct = Some(is_correct);
    q.answered_at = Some(now);

    // Update domain_stats: attempted+1, correct+(0|1), recent_correct rolling.
    let mut stat: engine::DomainStat = {
        let json_recent: String = tx.query_row(
            "SELECT recent_correct_json FROM domain_stats WHERE domain = ?1",
            [q.domain as i64],
            |r| r.get(0),
        )?;
        let recent: Vec<u8> = serde_json::from_str(&json_recent).unwrap_or_default();
        engine::DomainStat {
            attempted: 0,
            correct: 0,
            recent_correct: recent,
        }
    };
    let prev: (i64, i64) = tx.query_row(
        "SELECT attempted, correct FROM domain_stats WHERE domain = ?1",
        [q.domain as i64],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    stat.attempted = prev.0 as u32 + 1;
    stat.correct = prev.1 as u32 + if is_correct { 1 } else { 0 };
    stat.recent_correct.push(if is_correct { 1 } else { 0 });
    while stat.recent_correct.len() > engine::ROLLING_WINDOW {
        stat.recent_correct.remove(0);
    }
    let recent_json = serde_json::to_string(&stat.recent_correct)?;
    tx.execute(
        "UPDATE domain_stats
            SET attempted = ?1, correct = ?2, recent_correct_json = ?3
          WHERE domain = ?4",
        rusqlite::params![
            stat.attempted as i64,
            stat.correct as i64,
            recent_json,
            q.domain as i64
        ],
    )?;
    tx.commit()?;

    Ok(Json(AnswerResponse {
        question: q,
        idx: batch.idx,
        total: batch.question_ids.len() as u32,
    }))
}

// ─── POST /api/batches/:id/skip ─────────────────────────────────────────────

#[derive(Serialize)]
pub struct AdvanceResponse {
    pub batch_id: String,
    pub idx: u32,
    pub total: u32,
    pub question: Option<Question>,
    pub finished: bool,
}

pub async fn skip(
    State(state): State<AppState>,
    Path(batch_id): Path<String>,
) -> Result<Json<AdvanceResponse>, ApiError> {
    advance_idx(&state, &batch_id).await
}

async fn advance_idx(state: &AppState, batch_id: &str) -> Result<Json<AdvanceResponse>, ApiError> {
    let conn = state.pool.get()?;
    let mut batch = load_batch(&conn, batch_id)?
        .ok_or_else(|| ApiError::not_found("batch not found"))?;
    if batch.finished {
        return Err(ApiError::bad("batch already finished"));
    }
    batch.idx += 1;
    conn.execute(
        "UPDATE batches SET current_idx = ?1 WHERE id = ?2",
        rusqlite::params![batch.idx as i64, batch_id],
    )?;
    let total = batch.question_ids.len() as u32;
    let next_q = if (batch.idx as usize) < batch.question_ids.len() {
        load_question(&conn, &batch.question_ids[batch.idx as usize])?
    } else {
        None
    };
    Ok(Json(AdvanceResponse {
        batch_id: batch.id,
        idx: batch.idx,
        total,
        question: next_q,
        finished: false,
    }))
}

// ─── POST /api/batches/:id/cancel ──────────────────────────────────────

/// Mark a batch as finished WITHOUT applying difficulty progression and clear
/// the active-batch pointer. Already-answered questions remain in the DB and
/// the stats they updated stay (since `answer` updates `domain_stats` live).
pub async fn cancel(
    State(state): State<AppState>,
    Path(batch_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let mut conn = state.pool.get()?;
    let tx = conn.transaction()?;
    let exists: i64 = tx.query_row(
        "SELECT COUNT(1) FROM batches WHERE id = ?1",
        [&batch_id],
        |r| r.get(0),
    )?;
    if exists == 0 {
        return Err(ApiError::not_found("batch not found"));
    }
    tx.execute(
        "UPDATE batches SET finished = 1 WHERE id = ?1",
        rusqlite::params![batch_id],
    )?;
    db::set_state(&tx, "active_batch_id", "")?;
    tx.commit()?;
    Ok(Json(json!({ "ok": true, "batch_id": batch_id })))
}

// ─── POST /api/batches/:id/finish ───────────────────────────────────────

pub async fn finish(
    State(state): State<AppState>,
    Path(batch_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    // Scope all SQLite work so the (non-Send) connection is dropped before any await.
    {
        let mut conn = state.pool.get()?;
        let tx = conn.transaction()?;

        let stats = dashboard::load_stats(&tx)?;
        let mut diff = dashboard::load_difficulty(&tx)?;
        let changes = engine::apply_difficulty_progression(&stats, &mut diff);
        for (d, t) in &diff {
            tx.execute(
                "UPDATE difficulty SET tier = ?1 WHERE domain = ?2",
                rusqlite::params![*t as i64, *d as i64],
            )?;
        }
        let changes_json = serde_json::to_string(&changes)?;
        tx.execute(
            "UPDATE batches SET tier_changes_json = ?1, finished = 1 WHERE id = ?2",
            rusqlite::params![changes_json, batch_id],
        )?;
        db::set_state(&tx, "active_batch_id", "")?; // clear active
        tx.commit()?;
    }

    summary_inner(&state, &batch_id).await
}

// ─── POST /api/batches/:id/study-guide ───────────────────────────

/// Build a personalised study-guide PDF for the missed questions in a finished
/// (or cancelled) batch. Calls the LLM to synthesise per-domain study notes,
/// then hands a JSON payload to `scripts/study_guide.py` (ReportLab) which
/// renders the actual PDF. Response is raw `application/pdf` so the browser
/// can save it directly.
pub async fn study_guide(
    State(state): State<AppState>,
    Path(batch_id): Path<String>,
) -> Result<axum::response::Response, ApiError> {
    // 1. Pull batch metadata + missed questions from the DB synchronously.
    let (total, answered, correct, missed_payload, provider, model) = {
        let conn = state.pool.get()?;
        let batch = load_batch(&conn, &batch_id)?
            .ok_or_else(|| ApiError::not_found("batch not found"))?;
        let mut questions = Vec::with_capacity(batch.question_ids.len());
        for qid in &batch.question_ids {
            if let Some(q) = load_question(&conn, qid)? {
                questions.push(q);
            }
        }
        let total = questions.len();
        let answered: Vec<&Question> =
            questions.iter().filter(|q| q.user_answer.is_some()).collect();
        let correct: usize = answered
            .iter()
            .filter(|q| q.is_correct.unwrap_or(false))
            .count();
        let answered_count = answered.len();

        // Missed = answered but wrong. Skipped questions don't go on the study
        // guide because the user never saw their best guess.
        let missed: Vec<Value> = answered
            .iter()
            .filter(|q| !q.is_correct.unwrap_or(false))
            .map(|q| {
                json!({
                    "id": q.id,
                    "domain": q.domain,
                    "domain_name": engine::domain_name(q.domain),
                    "tier": q.difficulty,
                    "tier_name": engine::tier_name(q.difficulty),
                    "subtopic": q.subtopic,
                    "question": q.question,
                    "options": q.options,
                    "user_answer": q.user_answer,
                    "correct": q.correct,
                    "explanation": q.explanation,
                })
            })
            .collect();

        // Sort missed by domain so the PDF groups cleanly.
        let mut sorted = missed;
        sorted.sort_by_key(|v| v.get("domain").and_then(|x| x.as_i64()).unwrap_or(0));

        let provider = db::get_state(&conn, "provider")
            .ok()
            .flatten()
            .unwrap_or_else(|| state.cfg.default_provider.clone());
        let model = db::get_state(&conn, "model")
            .ok()
            .flatten()
            .unwrap_or_else(|| state.cfg.default_model.clone());

        (total, answered_count, correct, sorted, provider, model)
    };

    // 2. LLM synthesis (skipped when there are no misses).
    let synthesis_md = if missed_payload.is_empty() {
        "# Nice work\n\nNo missed questions in this batch. Generate the next batch \
         to keep stretching the harder tiers."
            .to_string()
    } else {
        let prompt = build_study_guide_prompt(&missed_payload);
        run_synthesis(&state, &provider, &model, &prompt).await.unwrap_or_else(|e| {
            // If the LLM fails the PDF still ships with verbatim missed questions;
            // we just include the error inline so the user knows synthesis failed.
            format!(
                "# Study Notes\n\nThe AI synthesis step failed; the verbatim missed \
                 questions on the following pages are still authoritative.\n\n_Reason:_ {e}"
            )
        })
    };

    // 3. Build payload for the Python renderer.
    let accuracy_pct = if answered > 0 {
        (correct as f64 / answered as f64) * 100.0
    } else {
        0.0
    };
    let payload = json!({
        "batch_id": batch_id,
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "total": total,
        "answered": answered,
        "correct": correct,
        "accuracy_pct": accuracy_pct,
        "synthesis_md": synthesis_md,
        "missed": missed_payload,
    });

    // 4. Render PDF on a blocking thread (subprocess + ReportLab can take a
    //    couple of seconds; we don't want to stall the tokio runtime).
    let payload_clone = payload.clone();
    let pdf_bytes = tokio::task::spawn_blocking(move || pdf::render_study_guide(&payload_clone))
        .await
        .map_err(|e| ApiError::internal(format!("pdf task join: {e}")))??;

    // 5. Return as application/pdf attachment.
    let filename = format!(
        "cissp-study-guide-{}-{}.pdf",
        chrono::Utc::now().format("%Y%m%d"),
        batch_id.chars().take(8).collect::<String>()
    );
    use axum::http::header;
    let response = axum::response::Response::builder()
        .status(200)
        .header(header::CONTENT_TYPE, "application/pdf")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header(header::CONTENT_LENGTH, pdf_bytes.len().to_string())
        .body(axum::body::Body::from(pdf_bytes))
        .map_err(|e| ApiError::internal(format!("response: {e}")))?;
    Ok(response)
}

fn build_study_guide_prompt(missed: &[Value]) -> String {
    // Compact summary of misses for the LLM. Full stems are included so it can
    // identify themes; explanations are NOT included (they're already in the
    // PDF verbatim section, no need to spend tokens repeating them).
    let mut summary = String::new();
    for (i, q) in missed.iter().enumerate() {
        summary.push_str(&format!(
            "\n{}. D{} ({}) [{}]: {}\n   user picked {}; correct was {}",
            i + 1,
            q.get("domain").and_then(|v| v.as_i64()).unwrap_or(0),
            q.get("domain_name").and_then(|v| v.as_str()).unwrap_or("?"),
            q.get("tier_name").and_then(|v| v.as_str()).unwrap_or("?"),
            q.get("question").and_then(|v| v.as_str()).unwrap_or(""),
            q.get("user_answer").and_then(|v| v.as_str()).unwrap_or("-"),
            q.get("correct").and_then(|v| v.as_str()).unwrap_or("-"),
        ));
    }

    format!(
        "You are writing a personalised CISSP study guide based on questions a candidate\n\
         JUST got wrong. Output ONLY GitHub-flavoured markdown (no preface, no closing).\n\
         The reader is a security manager preparing for the ISC2 CISSP exam.\n\n\
         Use this structure (omit a section if it doesn't apply):\n\n\
         ## Patterns across these misses\n\
         2-4 sentences identifying the *manager-thinking* gap (e.g. \"jumping to technical\n\
         controls before risk acceptance\", \"confusing due care with due diligence\",\n\
         \"choosing the technically-correct option that ignores lifecycle order\"). Be\n\
         specific about which questions illustrate each pattern.\n\n\
         ## Domain-by-domain review\n\
         For each affected CISSP domain that appears in the miss list, a `### Domain N — Name`\n\
         heading followed by:\n\
         - one short paragraph naming the underlying concept the candidate should re-study\n\
         - a bullet list of 3-6 specific topic anchors to review (frameworks, models,\n\
           lifecycle phases, named standards — not vendor products)\n\
         - one sentence on the *exam-style trap* the missed questions used\n\n\
         ## Suggested study order\n\
         A numbered list of 3-5 concrete next actions, ordered by impact on this batch's\n\
         misses (e.g. \"1. Re-read NIST SP 800-37 RMF steps; 2. Drill BIA → BCP → DR ordering\n\
         flashcards\"). Each item one line.\n\n\
         Tone: direct, manager-level, no fluff. No emoji. No bullet headers in shouty caps.\n\
         Never reference \"the user\" — write in second person (\"you missed…\", \"focus on…\").\n\n\
         MISSED QUESTIONS:{summary}"
    )
}

async fn run_synthesis(
    state: &AppState,
    provider: &str,
    model: &str,
    prompt: &str,
) -> Result<String, String> {
    let messages = vec![
        ChatMessage {
            role: "system".into(),
            content: "You are an expert CISSP exam tutor. Output GitHub-flavoured markdown only."
                .into(),
        },
        ChatMessage {
            role: "user".into(),
            content: prompt.to_string(),
        },
    ];
    let on_progress = |_buf: &str| {};
    match provider {
        "anthropic" => match state.cfg.anthropic_key.clone() {
            Some(k) => llm::anthropic::complete_chat(
                state.http.clone(),
                k,
                llm::anthropic::AnthropicOptions {
                    model,
                    max_tokens: 4096,
                    system: "You are an expert CISSP exam tutor. Output GitHub-flavoured markdown only.",
                },
                vec![ChatMessage { role: "user".into(), content: prompt.to_string() }],
                on_progress,
            )
            .await
            .map_err(|e| e.to_string()),
            None => Err("anthropic key missing".into()),
        },
        _ => match state.cfg.openai_key.clone() {
            Some(k) => llm::openai::complete_chat(
                state.http.clone(),
                k,
                llm::openai::OpenAiOptions {
                    model,
                    max_tokens: 4096,
                    temperature: 0.5,
                    json_mode: false,
                },
                messages,
                on_progress,
            )
            .await
            .map_err(|e| e.to_string()),
            None => Err("openai key missing".into()),
        },
    }
}

// ─── POST /api/study-guide/all-misses ──────────────────────────

/// Build a study-guide PDF that aggregates **every** missed question in the
/// database (across all batches), not just one batch. Same renderer as the
/// per-batch endpoint; payload uses `scope: "all-time"` so the cover wording
/// reads correctly.
pub async fn global_study_guide(
    State(state): State<AppState>,
) -> Result<axum::response::Response, ApiError> {
    // Cap to keep LLM context reasonable. Most-recent first.
    const MAX_MISSES: usize = 200;

    let (total_answered, correct_count, missed_payload, provider, model) = {
        let conn = state.pool.get()?;

        // Global stats
        let total_answered: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM questions WHERE answered_at IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let correct_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM questions WHERE is_correct = 1",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        // All misses, newest first, capped.
        let mut stmt = conn.prepare(
            "SELECT id, domain, subtopic, difficulty, question, options_json, correct,
                    explanation, user_answer
             FROM questions
             WHERE answered_at IS NOT NULL AND is_correct = 0
             ORDER BY answered_at DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map([MAX_MISSES as i64], |r| {
            let opts_str: String = r.get(5)?;
            let opts: Value = serde_json::from_str(&opts_str).unwrap_or(Value::Null);
            Ok(json!({
                "id":          r.get::<_, String>(0)?,
                "domain":      r.get::<_, i64>(1)?,
                "domain_name": engine::domain_name(r.get::<_, i64>(1)? as u8),
                "tier":        r.get::<_, i64>(3)?,
                "tier_name":   engine::tier_name(r.get::<_, i64>(3)? as u8),
                "subtopic":    r.get::<_, Option<String>>(2)?,
                "question":    r.get::<_, String>(4)?,
                "options":     opts,
                "correct":     r.get::<_, String>(6)?,
                "explanation": r.get::<_, Option<String>>(7)?,
                "user_answer": r.get::<_, Option<String>>(8)?,
            }))
        })?;
        let mut missed: Vec<Value> = rows.collect::<Result<Vec<_>, _>>()?;

        // Group by domain in the PDF for readability.
        missed.sort_by_key(|v| v.get("domain").and_then(|x| x.as_i64()).unwrap_or(0));

        let provider = db::get_state(&conn, "provider")
            .ok()
            .flatten()
            .unwrap_or_else(|| state.cfg.default_provider.clone());
        let model = db::get_state(&conn, "model")
            .ok()
            .flatten()
            .unwrap_or_else(|| state.cfg.default_model.clone());

        (total_answered, correct_count, missed, provider, model)
    };

    if missed_payload.is_empty() {
        return Err(ApiError::bad(
            "no missed questions in the database — nothing to put in a study guide yet.",
        ));
    }

    // LLM synthesis.
    let prompt = build_study_guide_prompt(&missed_payload);
    let synthesis_md = run_synthesis(&state, &provider, &model, &prompt)
        .await
        .unwrap_or_else(|e| {
            format!(
                "# Study Notes\n\nThe AI synthesis step failed; the verbatim missed \
                 questions on the following pages are still authoritative.\n\n_Reason:_ {e}"
            )
        });

    let accuracy_pct = if total_answered > 0 {
        (correct_count as f64 / total_answered as f64) * 100.0
    } else {
        0.0
    };

    let payload = json!({
        "batch_id": "all-misses",
        "scope": "all-time",
        "miss_count": missed_payload.len(),
        "capped": missed_payload.len() >= MAX_MISSES,
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "total": total_answered,
        "answered": total_answered,
        "correct": correct_count,
        "accuracy_pct": accuracy_pct,
        "synthesis_md": synthesis_md,
        "missed": missed_payload,
    });

    let payload_clone = payload.clone();
    let pdf_bytes = tokio::task::spawn_blocking(move || pdf::render_study_guide(&payload_clone))
        .await
        .map_err(|e| ApiError::internal(format!("pdf task join: {e}")))??;

    let filename = format!(
        "cissp-all-misses-{}.pdf",
        chrono::Utc::now().format("%Y%m%d")
    );
    use axum::http::header;
    let response = axum::response::Response::builder()
        .status(200)
        .header(header::CONTENT_TYPE, "application/pdf")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header(header::CONTENT_LENGTH, pdf_bytes.len().to_string())
        .body(axum::body::Body::from(pdf_bytes))
        .map_err(|e| ApiError::internal(format!("response: {e}")))?;
    Ok(response)
}

// ─── GET /api/batches/:id/summary ───────────────────────────

pub async fn summary(
    State(state): State<AppState>,
    Path(batch_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    summary_inner(&state, &batch_id).await
}

async fn summary_inner(state: &AppState, batch_id: &str) -> Result<Json<Value>, ApiError> {
    let conn = state.pool.get()?;
    let batch = load_batch(&conn, batch_id)?
        .ok_or_else(|| ApiError::not_found("batch not found"))?;
    let mut questions = Vec::with_capacity(batch.question_ids.len());
    for qid in &batch.question_ids {
        if let Some(q) = load_question(&conn, qid)? {
            questions.push(q);
        }
    }
    let answered: Vec<&Question> = questions.iter().filter(|q| q.user_answer.is_some()).collect();
    let correct: usize = answered.iter().filter(|q| q.is_correct.unwrap_or(false)).count();

    // Per-domain
    let mut per_dom: std::collections::BTreeMap<u8, (u32, u32)> = std::collections::BTreeMap::new();
    for q in &answered {
        let entry = per_dom.entry(q.domain).or_insert((0, 0));
        entry.0 += 1;
        if q.is_correct.unwrap_or(false) {
            entry.1 += 1;
        }
    }
    let per_dom_json: Vec<Value> = per_dom
        .into_iter()
        .map(|(d, (att, cor))| json!({
            "domain": d,
            "name": engine::domain_name(d),
            "short": engine::domain_short(d),
            "attempted": att,
            "correct": cor,
        }))
        .collect();

    let tier_changes: Value = batch
        .tier_changes_json
        .as_deref()
        .map(|s| serde_json::from_str::<Value>(s).unwrap_or(Value::Null))
        .unwrap_or(Value::Null);

    Ok(Json(json!({
        "batch_id": batch.id,
        "total": batch.question_ids.len(),
        "answered": answered.len(),
        "correct": correct,
        "per_domain": per_dom_json,
        "tier_changes": tier_changes,
        "missed": answered.iter()
            .filter(|q| !q.is_correct.unwrap_or(false))
            .map(|q| json!({
                "id": q.id,
                "domain": q.domain,
                "domain_short": engine::domain_short(q.domain),
                "tier": q.difficulty,
                "tier_name": engine::tier_name(q.difficulty),
                "user_answer": q.user_answer,
                "correct": q.correct,
                "question": q.question,
            }))
            .collect::<Vec<_>>(),
        "questions": questions,
    })))
}

// ─── POST /api/batches/generate (SSE) ───────────────────────────────────────

pub async fn generate(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = async_stream::stream! {
        // 1. Build the plan.
        let conn = match state.pool.get() {
            Ok(c) => c,
            Err(e) => {
                yield sse_error(format!("db: {e}"));
                return;
            }
        };
        let stats = match dashboard::load_stats(&conn) {
            Ok(s) => s,
            Err(e) => { yield sse_error(format!("{e:?}")); return; }
        };
        let diff = match dashboard::load_difficulty(&conn) {
            Ok(d) => d,
            Err(e) => { yield sse_error(format!("{e:?}")); return; }
        };
        let plan = engine::build_batch_plan(&stats, &diff, engine::BATCH_SIZE);
        let total_requested: u32 = plan.iter().map(|p| p.total()).sum();

        // 2. Provider/model.
        let provider = db::get_state(&conn, "provider")
            .ok().flatten()
            .unwrap_or_else(|| state.cfg.default_provider.clone());
        let model = db::get_state(&conn, "model")
            .ok().flatten()
            .unwrap_or_else(|| state.cfg.default_model.clone());
        drop(conn);

        // 3. Emit initial plan event.
        yield sse_named("plan", &json!({
            "total": total_requested,
            "provider": provider,
            "model": model,
            "plan": plan,
        }));

        // 4. Build prompt.
        let prompt = build_prompt(&plan, total_requested);

        // 5. Run LLM, stream progress.
        let progress = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let progress_cb = progress.clone();
        let on_progress = move |buf: &str| {
            let n = buf.matches("\"correct\"").count() as u32;
            progress_cb.store(n.min(total_requested), std::sync::atomic::Ordering::Relaxed);
        };

        // Box+pin the LLM future so we can poll it inside `tokio::select!` and
        // interleave periodic progress yields. The shared `progress` atomic is
        // updated by `on_progress` as bytes stream in, and we publish its
        // current value to the SSE channel every ~400ms.
        let llm_fut: std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, llm::LlmError>> + Send>> =
            match provider.as_str() {
                "anthropic" => match state.cfg.anthropic_key.clone() {
                    Some(k) => {
                        let http = state.http.clone();
                        let model_owned = model.clone();
                        let prompt_owned = prompt.clone();
                        Box::pin(async move {
                            llm::anthropic::complete_chat(
                                http,
                                k,
                                llm::anthropic::AnthropicOptions {
                                    model: &model_owned,
                                    // 50 fully-explained CISSP questions run
                                    // ~15-20k output tokens; current Anthropic
                                    // models cap at 64k+ so 32k is a safe ceiling.
                                    max_tokens: 32768,
                                    system: "You output STRICT JSON only. No markdown, no prose, no code fences.",
                                },
                                vec![ChatMessage { role: "user".into(), content: prompt_owned }],
                                on_progress,
                            )
                            .await
                        })
                    }
                    None => Box::pin(async { Err(llm::LlmError::MissingKey("anthropic")) }),
                },
                _ => match state.cfg.openai_key.clone() {
                    Some(k) => {
                        let http = state.http.clone();
                        let model_owned = model.clone();
                        let prompt_owned = prompt.clone();
                        Box::pin(async move {
                            llm::openai::complete_chat(
                                http,
                                k,
                                llm::openai::OpenAiOptions {
                                    model: &model_owned,
                                    max_tokens: 16384,
                                    temperature: 0.85,
                                    json_mode: true,
                                },
                                vec![
                                    ChatMessage {
                                        role: "system".into(),
                                        content: "You output STRICT JSON only. No markdown, no prose.".into(),
                                    },
                                    ChatMessage { role: "user".into(), content: prompt_owned },
                                ],
                                on_progress,
                            )
                            .await
                        })
                    }
                    None => Box::pin(async { Err(llm::LlmError::MissingKey("openai")) }),
                },
            };

        let mut llm_fut = llm_fut;
        let mut tick =
            tokio::time::interval(std::time::Duration::from_millis(400));
        // The first tick fires immediately — burn it so we don't emit a 0%
        // progress event before any data has arrived.
        tick.tick().await;

        let raw_result: Result<String, llm::LlmError> = loop {
            tokio::select! {
                res = &mut llm_fut => break res,
                _ = tick.tick() => {
                    let parsed = progress.load(std::sync::atomic::Ordering::Relaxed);
                    yield sse_named("progress", &json!({
                        "parsed": parsed,
                        "total": total_requested
                    }));
                }
            }
        };

        // Final progress snapshot so the ring snaps to 100% before "done".
        let parsed_now = progress.load(std::sync::atomic::Ordering::Relaxed);
        yield sse_named("progress", &json!({
            "parsed": parsed_now.max(total_requested),
            "total": total_requested
        }));

        let raw = match raw_result {
            Ok(r) => r,
            Err(e) => { yield sse_error(format!("{e}")); return; }
        };

        // 6. Parse JSON. If the response was truncated mid-array (token cap),
        // try salvaging completed objects before giving up.
        let cleaned = clean_llm_json(&raw);
        let parsed_value: serde_json::Value = match serde_json::from_str(&cleaned) {
            Ok(v) => v,
            Err(_) => match salvage_truncated_json(&cleaned) {
                Some(v) => v,
                None => {
                    yield sse_error(
                        "model JSON parse failed and could not salvage partial output".to_string()
                    );
                    return;
                }
            },
        };
        let questions_arr: Vec<serde_json::Value> = parsed_value
            .get("questions")
            .and_then(|v| v.as_array().cloned())
            .or_else(|| parsed_value.as_array().cloned())
            .unwrap_or_default();

        // 7. Validate + dedupe + persist.
        let conn = match state.pool.get() {
            Ok(c) => c,
            Err(e) => { yield sse_error(format!("db: {e}")); return; }
        };
        let validated = match validate_and_persist(&conn, questions_arr, &plan) {
            Ok(v) => v,
            Err(e) => { yield sse_error(e.message); return; }
        };

        if validated.ids.is_empty() {
            yield sse_error("model returned no valid questions".to_string());
            return;
        }

        // 8. Done.
        let summary = json!({
            "batch_id": validated.batch_id,
            "count": validated.ids.len(),
        });
        yield sse_named("done", &summary);
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

struct PersistedBatch {
    batch_id: String,
    ids: Vec<String>,
}

fn validate_and_persist(
    conn: &rusqlite::Connection,
    raw: Vec<serde_json::Value>,
    plan: &[engine::DomainPlan],
) -> Result<PersistedBatch, ApiError> {
    let mut known: std::collections::HashSet<String> = std::collections::HashSet::new();
    {
        let mut stmt = conn.prepare("SELECT question FROM questions")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for r in rows {
            known.insert(normalize_q(&r?));
        }
    }

    let batch_id = format!("b-{}", Uuid::new_v4().simple());
    let now = db::now_ms();
    let mut ids: Vec<String> = Vec::new();
    let mut to_insert: Vec<(String, u8, Option<String>, u8, String, String, String, Option<String>)> =
        Vec::new();

    for q in raw {
        let domain = q.get("domain").and_then(|v| v.as_i64()).unwrap_or(-1);
        let difficulty = q.get("difficulty").and_then(|v| v.as_i64()).unwrap_or(-1);
        let correct = q
            .get("correct")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_uppercase();
        let opts = q.get("options").cloned().unwrap_or(Value::Null);
        let stem = q
            .get("question")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let subtopic = q.get("subtopic").and_then(|v| v.as_str()).map(|s| s.to_string());
        let explanation = q.get("explanation").and_then(|v| v.as_str()).map(|s| s.to_string());

        if !(1..=8).contains(&domain) || !(1..=4).contains(&difficulty) {
            continue;
        }
        if !matches!(correct.as_str(), "A" | "B" | "C" | "D") {
            continue;
        }
        if stem.is_empty() {
            continue;
        }
        if !["A", "B", "C", "D"]
            .iter()
            .all(|k| opts.get(k).and_then(|v| v.as_str()).is_some())
        {
            continue;
        }
        let n = normalize_q(&stem);
        if known.contains(&n) {
            continue;
        }
        known.insert(n);
        let id = format!("q-{}", Uuid::new_v4().simple());
        ids.push(id.clone());
        to_insert.push((
            id,
            domain as u8,
            subtopic,
            difficulty as u8,
            stem,
            opts.to_string(),
            correct,
            explanation,
        ));
    }

    if to_insert.is_empty() {
        return Err(ApiError::internal("no valid questions in model output"));
    }

    // The LLM tends to anchor the correct answer to letter "A". Rebalance
    // server-side: assign each question a target correct letter via balanced
    // round-robin (shuffled), keep the correct option text on that letter, and
    // randomly permute the three distractors. This guarantees uniform letter
    // distribution and removes positional tells regardless of model bias.
    rebalance_correct_letters(&mut to_insert);

    {
        let mut stmt = conn.prepare(
            "INSERT INTO questions
              (id, domain, subtopic, difficulty, question, options_json, correct,
               explanation, user_answer, is_correct, answered_at, created_at, batch_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, NULL, NULL, ?9, ?10)",
        )?;
        for (id, domain, subtopic, difficulty, question, options_json, correct, explanation) in
            &to_insert
        {
            stmt.execute(rusqlite::params![
                id,
                *domain as i64,
                subtopic,
                *difficulty as i64,
                question,
                options_json,
                correct,
                explanation,
                now,
                batch_id,
            ])?;
        }
    }

    let dist_json = {
        let mut m = std::collections::BTreeMap::new();
        for p in plan {
            m.insert(p.domain.to_string(), p.total());
        }
        serde_json::to_string(&m)?
    };
    let diff_json = {
        let mut m = std::collections::BTreeMap::new();
        for p in plan {
            m.insert(p.domain.to_string(), p.tier);
        }
        serde_json::to_string(&m)?
    };
    let ids_json = serde_json::to_string(&ids)?;
    conn.execute(
        "INSERT INTO batches
           (id, created_at, distribution_json, difficulty_by_domain_json,
            question_ids_json, tier_changes_json, current_idx, finished)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, 0, 0)",
        rusqlite::params![batch_id, now, dist_json, diff_json, ids_json],
    )?;
    db::set_state(conn, "active_batch_id", &batch_id)?;

    Ok(PersistedBatch { batch_id, ids })
}

// ─── Helpers ────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct BatchRow {
    id: String,
    idx: u32,
    finished: bool,
    question_ids: Vec<String>,
    tier_changes_json: Option<String>,
}

fn load_batch(conn: &rusqlite::Connection, id: &str) -> Result<Option<BatchRow>, ApiError> {
    let row = conn
        .query_row(
            "SELECT id, current_idx, finished, question_ids_json, tier_changes_json
             FROM batches WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    let Some((id, idx, fin, ids_json, tier_changes_json)) = row else {
        return Ok(None);
    };
    let question_ids: Vec<String> = serde_json::from_str(&ids_json).unwrap_or_default();
    Ok(Some(BatchRow {
        id,
        idx: idx as u32,
        finished: fin != 0,
        question_ids,
        tier_changes_json,
    }))
}

fn load_question(conn: &rusqlite::Connection, id: &str) -> Result<Option<Question>, ApiError> {
    let row = conn
        .query_row(
            "SELECT id, domain, subtopic, difficulty, question, options_json, correct,
                    explanation, user_answer, is_correct, answered_at, created_at, batch_id
             FROM questions WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)? as u8,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, i64>(3)? as u8,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, Option<String>>(8)?,
                    r.get::<_, Option<i64>>(9)?,
                    r.get::<_, Option<i64>>(10)?,
                    r.get::<_, i64>(11)?,
                    r.get::<_, String>(12)?,
                ))
            },
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    let Some(t) = row else { return Ok(None) };
    let options: Value = serde_json::from_str(&t.5).unwrap_or(Value::Null);
    Ok(Some(Question {
        id: t.0,
        domain: t.1,
        subtopic: t.2,
        difficulty: t.3,
        question: t.4,
        options,
        correct: t.6,
        explanation: t.7,
        user_answer: t.8,
        is_correct: t.9.map(|v| v != 0),
        answered_at: t.10,
        created_at: t.11,
        batch_id: t.12,
    }))
}

fn build_prompt(plan: &[engine::DomainPlan], total: u32) -> String {
    // Per-domain breakdown with topic anchors. Anchors stay attached to the
    // domain block so the model can pick a different one per question and
    // avoid generating five flavors of the same risk-register stem.
    let mut lines = String::new();
    for p in plan {
        let tn = engine::tier_name(p.tier);
        let dn = engine::domain_name(p.domain);
        lines.push_str(&format!(
            "- Domain {} ({dn}): {} question{} at tier {} ({tn})",
            p.domain,
            p.count,
            if p.count == 1 { "" } else { "s" },
            p.tier,
        ));
        if let Some(s) = &p.stretch {
            let stn = engine::tier_name(s.tier);
            lines.push_str(&format!(
                " PLUS {} stretch question{} at tier {} ({stn})",
                s.count,
                if s.count == 1 { "" } else { "s" },
                s.tier,
            ));
        }
        lines.push('\n');
        let anchors = engine::domain_anchors(p.domain);
        if !anchors.is_empty() {
            lines.push_str("    Pick distinct subtopics from this anchor list (do not repeat within this domain block):\n");
            for a in anchors {
                lines.push_str("      • ");
                lines.push_str(a);
                lines.push('\n');
            }
        }
    }

    let rubric = (1..=4u8)
        .map(|t| format!("  - Tier {t} ({}): {}", engine::tier_name(t), engine::tier_desc(t)))
        .collect::<Vec<_>>()
        .join("\n");

    // The worked example is provided as a REFERENCE for style only. The model
    // must NOT echo it back. JSON output remains the single object below.
    let worked_example = r#"WORKED REFERENCE EXAMPLE (style only — DO NOT copy verbatim, DO NOT include in output):

  Stem (Domain 1, Tier 3):
    "A multinational manufacturer is moving its ERP platform to a public-cloud
     IaaS provider. The CFO has signed the contract before the security team
     was engaged, and go-live is in six weeks. Regulators in two of the
     company's operating regions require demonstrable evidence of ongoing
     vendor risk management for any system processing financial records.
     Which action should the CISO take FIRST?"
  Options:
    A. Engage an external auditor to perform a SOC 2 Type II readiness review.
    B. Require the provider to complete the company's standard third-party
       risk questionnaire and review their existing SOC 2 Type II report.
    C. Insist the contract be re-negotiated to add a right-to-audit clause
       before any data is migrated.
    D. Configure cloud-native logging and forward events to the corporate SIEM.
  Correct: B
  Why it works:
    • B is the FIRST defensible governance step under a signed contract:
      gather evidence already in hand (their SOC 2) before commissioning new work.
    • A is correct in spirit but premature and aimed at the wrong party.
    • C is the manager-instinct trap — re-negotiation comes AFTER risk review,
      not before, and the contract is already executed.
    • D is the technician trap: control-implementation before risk understanding.
  Why it's exam-grade:
    • Multi-sentence scenario with cost/time/regulatory pressure.
    • Two distractors are technically reasonable but solve a different problem.
    • Uses \"FIRST\" qualifier; rewards lifecycle ordering.
    • References a real artifact (SOC 2 Type II) without trivia-grade detail."#;

    let anti_patterns = r#"ANTI-PATTERNS — these will be REJECTED by validation. Do NOT produce questions that:
  • Test rote memorization of port numbers, RFC numbers, or vendor product names.
  • Have an obvious \"call security/CISO\" or \"perform risk assessment\" answer
    in EVERY scenario regardless of the stem.
  • Use the phrase \"all of the above\" or \"none of the above\".
  • Contain options of wildly different lengths (the longest option becomes the tell).
  • Include negatively-phrased stems (\"Which is NOT…\") at tier 3 or 4.
  • Reuse the same correct letter four+ times in a row.
  • Contain trick wording where two options are functionally identical.
  • Use technician language at tier 3-4 (e.g. specific CLI flags, exact CVE IDs,
    exact NIST control numbers like \"AC-2(7)\") — CISSP is a manager exam.
  • Reference current events, named real-world breaches, or vendor branding.
  • Have explanations that just restate the correct option without justifying
    why each distractor is wrong.
  • Generate questions that violate the assigned domain (e.g. a Domain 4
    question that's really an IAM question)."#;

    format!(
        "You are generating high-fidelity CISSP exam practice questions for a study app.
The target is the ISC2 CISSP exam (current CBK), where every question is judged from
the perspective of a security manager / CISO weighing CIA, business impact, regulatory
obligation, and lifecycle phase — not a sysadmin running commands.

OUTPUT FORMAT — return ONE JSON object, NO markdown fences, NO commentary, NO worked
example echoes:
{{
  \"questions\": [
    {{
      \"domain\": <integer 1-8>,
      \"subtopic\": \"<short topic name, ~3-6 words>\",
      \"difficulty\": <integer 1-4>,
      \"question\": \"<full question stem>\",
      \"options\": {{ \"A\": \"<text>\", \"B\": \"<text>\", \"C\": \"<text>\", \"D\": \"<text>\" }},
      \"correct\": \"<one of A|B|C|D>\",
      \"explanation\": \"<2-4 sentences justifying the correct answer AND why each
                          distractor is wrong>\"
    }}
  ]
}}

Generate EXACTLY {total} questions distributed as:
{lines}

Difficulty rubric:
{rubric}

{worked_example}

{anti_patterns}

HARD RULES (validation will drop violators silently):
  1. \"domain\" MUST equal the requested CISSP domain (1-8) for that question.
  2. \"difficulty\" MUST equal the requested tier for that question.
  3. Exactly four options A/B/C/D per question; \"correct\" is one letter.
  4. The correct letter must be roughly evenly distributed across the batch
     (each of A/B/C/D appears ~25% of the time, ±1).
  5. Tier 1: ≤1 sentence stems. Tier 2: 2-4 sentence stems. Tier 3: 3-6 sentence
     stems with at least one source of pressure (cost, time, regulation,
     executive demand). Tier 4: 4-7 sentence stems with at least one piece of
     deliberate misdirection.
  6. At tier 3 and 4, the stem MUST end with an explicit qualifier — one of
     MOST, FIRST, BEST, NEXT, or PRIMARY — and the question must be answerable
     from the manager's chair.
  7. Distractors must be plausible (a candidate who half-knows the topic could
     pick them). Avoid joke distractors and avoid two distractors that mean
     the same thing.
  8. Use the anchor list above to vary subtopics within each domain block. Each
     question's \"subtopic\" must reference the chosen anchor in 3-6 words.
  9. Explanations: 2-4 sentences. Justify the correct option AND say in one
     short clause why each of the other three is wrong (\"A solves a different
     problem; C is premature; D is a technician control before risk review\").
 10. Output ONLY the JSON object. NO preface, NO closing remarks, NO worked
     example reproduction, NO markdown fences.",
    )
}

/// Re-assign correct letters across the batch so each of A/B/C/D appears
/// roughly N/4 times, and randomize distractor positions for each question.
///
/// Operates on the `to_insert` tuple shape used by `validate_and_persist`:
/// `(id, domain, subtopic, difficulty, question, options_json, correct, explanation)`.
fn rebalance_correct_letters(
    items: &mut [(String, u8, Option<String>, u8, String, String, String, Option<String>)],
) {
    let n = items.len();
    if n == 0 {
        return;
    }
    let letters = ['A', 'B', 'C', 'D'];
    let mut targets: Vec<char> = (0..n).map(|i| letters[i % 4]).collect();
    let mut rng = rand::thread_rng();
    targets.shuffle(&mut rng);

    for (item, target) in items.iter_mut().zip(targets.iter()) {
        let opts: Value = match serde_json::from_str(&item.5) {
            Ok(v) => v,
            Err(_) => continue, // leave malformed entries alone (validator should have caught it)
        };
        let cur_correct = item.6.chars().next().unwrap_or('A');
        let text_at = |l: char| -> String {
            opts.get(l.to_string())
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        let correct_text = text_at(cur_correct);
        let mut distractor_texts: Vec<String> = letters
            .iter()
            .filter(|&&l| l != cur_correct)
            .map(|&l| text_at(l))
            .collect();
        distractor_texts.shuffle(&mut rng);

        let mut new_opts = serde_json::Map::with_capacity(4);
        new_opts.insert(target.to_string(), Value::String(correct_text));
        let other_slots: Vec<char> = letters.iter().copied().filter(|l| l != target).collect();
        for (slot, txt) in other_slots.into_iter().zip(distractor_texts.into_iter()) {
            new_opts.insert(slot.to_string(), Value::String(txt));
        }

        item.5 = serde_json::to_string(&Value::Object(new_opts)).unwrap_or_else(|_| item.5.clone());
        item.6 = target.to_string();
    }
}

fn normalize_q(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = false;
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            for c in ch.to_lowercase() {
                out.push(c);
            }
            last_space = false;
        } else if ch.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        }
    }
    out.trim().to_string()
}

fn clean_llm_json(s: &str) -> String {
    let mut t = s.trim().to_string();
    if t.starts_with("```") {
        t = t.trim_start_matches("```json").trim_start_matches("```").trim().to_string();
        if let Some(idx) = t.rfind("```") {
            t.truncate(idx);
            t = t.trim().to_string();
        }
    }
    if let (Some(a), Some(b)) = (t.find('{'), t.rfind('}')) {
        if a > 0 || b < t.len() - 1 {
            t = t[a..=b].to_string();
        }
    }
    t
}

/// Attempt to recover usable JSON from a response that was cut off mid-array
/// (e.g. provider hit `max_tokens`). We walk the string while tracking string
/// state and brace/bracket depth, remember the byte offset of the last point
/// where we were inside the `"questions"` array at depth 1 (i.e. just after a
/// completed question object) and rebuild a valid envelope from that point.
///
/// Returns `Some(json)` only if at least one complete question object was
/// recovered.
fn salvage_truncated_json(s: &str) -> Option<serde_json::Value> {
    let bytes = s.as_bytes();
    let mut in_str = false;
    let mut esc = false;
    let mut depth_obj: i32 = 0;
    let mut depth_arr: i32 = 0;
    let mut last_complete_obj_end: Option<usize> = None;

    for (i, &b) in bytes.iter().enumerate() {
        if in_str {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth_obj += 1,
            b'}' => {
                depth_obj -= 1;
                // Completed a question object inside the questions array.
                // (Outer envelope object is depth_obj==1 when we close the
                // last question; bare-array form has depth_obj==0.)
                if depth_arr >= 1 && depth_obj <= 1 {
                    last_complete_obj_end = Some(i);
                }
            }
            b'[' => depth_arr += 1,
            b']' => depth_arr -= 1,
            _ => {}
        }
    }

    let end = last_complete_obj_end?;
    let prefix = &s[..=end];
    // Detect envelope vs bare-array shape by looking for the questions key
    // before the last completed object.
    let rebuilt = if prefix.contains("\"questions\"") {
        format!("{prefix}]}}")
    } else if prefix.trim_start().starts_with('[') {
        format!("{prefix}]")
    } else {
        return None;
    };
    serde_json::from_str::<serde_json::Value>(&rebuilt).ok()
}

fn sse_error(msg: String) -> Result<Event, Infallible> {
    Ok(Event::default()
        .event("error")
        .data(json!({ "message": msg }).to_string()))
}

fn sse_named(name: &str, payload: &Value) -> Result<Event, Infallible> {
    Ok(Event::default().event(name).data(payload.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn item(correct_letter: char, a: &str, b: &str, c: &str, d: &str) -> (String, u8, Option<String>, u8, String, String, String, Option<String>) {
        let opts = serde_json::json!({ "A": a, "B": b, "C": c, "D": d });
        (
            "q-test".to_string(),
            1,
            None,
            1,
            "stem".to_string(),
            opts.to_string(),
            correct_letter.to_string(),
            None,
        )
    }

    #[test]
    fn rebalance_distributes_correct_letters_uniformly_for_50() {
        let mut items = Vec::new();
        // Worst-case input: every question's correct answer is "A".
        for _ in 0..50 {
            items.push(item('A', "correct-text", "d1", "d2", "d3"));
        }
        rebalance_correct_letters(&mut items);
        let mut counts: BTreeMap<String, u32> = BTreeMap::new();
        for it in &items {
            *counts.entry(it.6.clone()).or_insert(0) += 1;
        }
        // 50 / 4 → 12 or 13 per letter (round-robin over 50: 13,13,12,12).
        for letter in ["A", "B", "C", "D"] {
            let c = counts.get(letter).copied().unwrap_or(0);
            assert!(
                (12..=13).contains(&c),
                "letter {letter} count {c} outside [12,13] for n=50"
            );
        }
    }

    #[test]
    fn rebalance_preserves_correct_text_at_new_letter() {
        let mut items = vec![item('A', "THE-RIGHT-ANSWER", "wrong1", "wrong2", "wrong3")];
        rebalance_correct_letters(&mut items);
        let it = &items[0];
        let opts: Value = serde_json::from_str(&it.5).unwrap();
        // Whatever the new correct letter is, its text must still be the original correct text.
        let new_letter = &it.6;
        assert_eq!(
            opts.get(new_letter).and_then(|v| v.as_str()).unwrap(),
            "THE-RIGHT-ANSWER"
        );
        // And the four option texts remain the same set.
        let mut got: Vec<String> = ["A", "B", "C", "D"]
            .iter()
            .map(|k| opts.get(*k).and_then(|v| v.as_str()).unwrap_or("").to_string())
            .collect();
        got.sort();
        let mut want = vec![
            "THE-RIGHT-ANSWER".to_string(),
            "wrong1".to_string(),
            "wrong2".to_string(),
            "wrong3".to_string(),
        ];
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn rebalance_is_a_noop_on_empty() {
        let mut items: Vec<(String, u8, Option<String>, u8, String, String, String, Option<String>)> =
            Vec::new();
        rebalance_correct_letters(&mut items);
        assert!(items.is_empty());
    }

    #[test]
    fn salvage_recovers_envelope_truncated_mid_array() {
        // Two complete questions, third was cut off mid-string.
        let truncated = r#"{
  "questions": [
    {"domain":1,"subtopic":"x","difficulty":1,"question":"q1","options":{"A":"a","B":"b","C":"c","D":"d"},"correct":"A","explanation":"e"},
    {"domain":2,"subtopic":"y","difficulty":2,"question":"q2","options":{"A":"a","B":"b","C":"c","D":"d"},"correct":"B","explanation":"e"},
    {"domain":3,"subtopic":"z","difficulty":3,"question":"q3 with a partial "#;
        // Sanity check: this is not valid JSON yet.
        assert!(serde_json::from_str::<Value>(truncated).is_err());
        let recovered = salvage_truncated_json(truncated).expect("should salvage envelope");
        let arr = recovered.get("questions").and_then(|v| v.as_array()).unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].get("question").and_then(|v| v.as_str()), Some("q1"));
        assert_eq!(arr[1].get("question").and_then(|v| v.as_str()), Some("q2"));
    }

    #[test]
    fn salvage_recovers_bare_array_truncated_mid_array() {
        let truncated = r#"[
  {"domain":1,"correct":"A"},
  {"domain":2,"correct":"B"},
  {"domain":3,"correc"#;
        assert!(serde_json::from_str::<Value>(truncated).is_err());
        let recovered = salvage_truncated_json(truncated).expect("should salvage array");
        let arr = recovered.as_array().unwrap();
        assert_eq!(arr.len(), 2);
    }

    #[test]
    fn salvage_returns_none_when_no_complete_objects() {
        // Truncated before any object closed.
        let truncated = r#"{"questions": [{"domain":1, "correct":"A""#;
        assert!(salvage_truncated_json(truncated).is_none());
    }

    #[test]
    fn salvage_handles_escaped_quotes_in_strings() {
        // Question text contains an escaped quote and a brace inside a string.
        let truncated = r#"{
  "questions": [
    {"domain":1,"question":"He said \"hello {world}\" loudly","correct":"A"},
    {"domain":2,"question":"q2 partial "#;
        let recovered = salvage_truncated_json(truncated).expect("should salvage");
        let arr = recovered.get("questions").and_then(|v| v.as_array()).unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(
            arr[0].get("question").and_then(|v| v.as_str()),
            Some("He said \"hello {world}\" loudly")
        );
    }
}
