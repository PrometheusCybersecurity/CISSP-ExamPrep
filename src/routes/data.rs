//! /api/export and /api/import — full DB JSON dump.
//!
//! Import accepts:
//! 1. Native server format: `{ "schema": "cissp-coach-rs-v1", ... }`
//! 2. The old browser localStorage `Export DB` format from the earlier
//!    single-file app: `{ "schema": "cissp-coach-v1", qbank, domainStats,
//!    difficulty, batches, currentBatch }`.

use axum::{extract::State, Json};
use serde_json::{json, Value};

use super::ApiError;
use crate::AppState;

pub async fn export(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let conn = state.pool.get()?;

    // Questions
    let mut stmt = conn.prepare(
        "SELECT id, domain, subtopic, difficulty, question, options_json, correct,
                explanation, user_answer, is_correct, answered_at, created_at, batch_id
         FROM questions",
    )?;
    let qbank: Vec<Value> = stmt
        .query_map([], |r| {
            let opts_json: String = r.get(5)?;
            let opts: Value = serde_json::from_str(&opts_json).unwrap_or(Value::Null);
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "domain": r.get::<_, i64>(1)?,
                "subtopic": r.get::<_, Option<String>>(2)?,
                "difficulty": r.get::<_, i64>(3)?,
                "question": r.get::<_, String>(4)?,
                "options": opts,
                "correct": r.get::<_, String>(6)?,
                "explanation": r.get::<_, Option<String>>(7)?,
                "userAnswer": r.get::<_, Option<String>>(8)?,
                "isCorrect": r.get::<_, Option<i64>>(9)?.map(|v| v != 0),
                "answeredAt": r.get::<_, Option<i64>>(10)?,
                "createdAt": r.get::<_, i64>(11)?,
                "batchId": r.get::<_, String>(12)?,
            }))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    // Domain stats
    let mut stmt = conn.prepare(
        "SELECT domain, attempted, correct, recent_correct_json FROM domain_stats",
    )?;
    let mut domain_stats = serde_json::Map::new();
    for row in stmt.query_map([], |r| {
        let recent: String = r.get(3)?;
        let recent_v: Value = serde_json::from_str(&recent).unwrap_or(json!([]));
        Ok((
            r.get::<_, i64>(0)?.to_string(),
            json!({
                "attempted": r.get::<_, i64>(1)?,
                "correct": r.get::<_, i64>(2)?,
                "recentCorrect": recent_v,
            }),
        ))
    })? {
        let (k, v) = row?;
        domain_stats.insert(k, v);
    }

    // Difficulty
    let mut stmt = conn.prepare("SELECT domain, tier FROM difficulty")?;
    let mut difficulty = serde_json::Map::new();
    for row in stmt.query_map([], |r| {
        Ok((r.get::<_, i64>(0)?.to_string(), json!(r.get::<_, i64>(1)?)))
    })? {
        let (k, v) = row?;
        difficulty.insert(k, v);
    }

    // Batches
    let mut stmt = conn.prepare(
        "SELECT id, created_at, distribution_json, difficulty_by_domain_json,
                question_ids_json, tier_changes_json, current_idx, finished
         FROM batches",
    )?;
    let batches: Vec<Value> = stmt
        .query_map([], |r| {
            let dist: Value =
                serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or(Value::Null);
            let diff: Value =
                serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or(Value::Null);
            let qids: Value =
                serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or(json!([]));
            let tc: Value = match r.get::<_, Option<String>>(5)? {
                Some(s) => serde_json::from_str(&s).unwrap_or(Value::Null),
                None => Value::Null,
            };
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "createdAt": r.get::<_, i64>(1)?,
                "distribution": dist,
                "difficultyByDomain": diff,
                "questionIds": qids,
                "tierChanges": tc,
                "currentIdx": r.get::<_, i64>(6)?,
                "finished": r.get::<_, i64>(7)? != 0,
            }))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    // Chat messages
    let mut stmt = conn.prepare(
        "SELECT role, content, created_at FROM chat_messages ORDER BY id ASC",
    )?;
    let chat: Vec<Value> = stmt
        .query_map([], |r| {
            Ok(json!({
                "role": r.get::<_, String>(0)?,
                "content": r.get::<_, String>(1)?,
                "createdAt": r.get::<_, i64>(2)?,
            }))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Json(json!({
        "schema": "cissp-coach-rs-v1",
        "exportedAt": chrono::Utc::now().to_rfc3339(),
        "qbank": qbank,
        "domainStats": domain_stats,
        "difficulty": difficulty,
        "batches": batches,
        "chatHistory": chat,
    })))
}

pub async fn import(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut conn = state.pool.get()?;
    let tx = conn.transaction()?;

    // Wipe existing rows we're going to replace.
    tx.execute("DELETE FROM questions", [])?;
    tx.execute("DELETE FROM batches", [])?;
    tx.execute("DELETE FROM domain_stats", [])?;
    tx.execute("DELETE FROM difficulty", [])?;
    // Keep app_state and chat_messages by default.

    // qbank
    if let Some(arr) = body.get("qbank").and_then(|v| v.as_array()) {
        for q in arr {
            let id = q.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let id = if id.is_empty() {
                format!("q-{}", uuid::Uuid::new_v4().simple())
            } else {
                id
            };
            let domain = q.get("domain").and_then(|v| v.as_i64()).unwrap_or(0);
            let subtopic = q.get("subtopic").and_then(|v| v.as_str()).map(|s| s.to_string());
            let difficulty = q.get("difficulty").and_then(|v| v.as_i64()).unwrap_or(1);
            let question = q.get("question").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let options = q.get("options").cloned().unwrap_or(Value::Null).to_string();
            let correct = q.get("correct").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let explanation = q.get("explanation").and_then(|v| v.as_str()).map(|s| s.to_string());
            let user_answer = q
                .get("userAnswer")
                .or_else(|| q.get("user_answer"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let is_correct = q
                .get("isCorrect")
                .or_else(|| q.get("is_correct"))
                .and_then(|v| v.as_bool())
                .map(|b| b as i64);
            let answered_at = q
                .get("answeredAt")
                .or_else(|| q.get("answered_at"))
                .and_then(|v| v.as_i64());
            let created_at = q
                .get("createdAt")
                .or_else(|| q.get("created_at"))
                .and_then(|v| v.as_i64())
                .unwrap_or_else(crate::db::now_ms);
            let batch_id = q
                .get("batchId")
                .or_else(|| q.get("batch_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("imported")
                .to_string();
            tx.execute(
                "INSERT INTO questions
                  (id, domain, subtopic, difficulty, question, options_json, correct,
                   explanation, user_answer, is_correct, answered_at, created_at, batch_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                rusqlite::params![
                    id,
                    domain,
                    subtopic,
                    difficulty,
                    question,
                    options,
                    correct,
                    explanation,
                    user_answer,
                    is_correct,
                    answered_at,
                    created_at,
                    batch_id,
                ],
            )?;
        }
    }

    // domainStats
    let stats_obj = body.get("domainStats").and_then(|v| v.as_object());
    for d in 1..=8u8 {
        let key = d.to_string();
        let (attempted, correct, recent) = match stats_obj.and_then(|m| m.get(&key)) {
            Some(v) => {
                let attempted = v.get("attempted").and_then(|x| x.as_i64()).unwrap_or(0);
                let correct = v.get("correct").and_then(|x| x.as_i64()).unwrap_or(0);
                let recent = v.get("recentCorrect").cloned().unwrap_or(json!([]));
                (attempted, correct, recent.to_string())
            }
            None => (0, 0, "[]".to_string()),
        };
        tx.execute(
            "INSERT INTO domain_stats(domain, attempted, correct, recent_correct_json)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![d as i64, attempted, correct, recent],
        )?;
    }

    // difficulty
    let diff_obj = body.get("difficulty").and_then(|v| v.as_object());
    for d in 1..=8u8 {
        let key = d.to_string();
        let tier = diff_obj
            .and_then(|m| m.get(&key))
            .and_then(|v| v.as_i64())
            .unwrap_or(1)
            .clamp(1, 4);
        tx.execute(
            "INSERT INTO difficulty(domain, tier) VALUES (?1, ?2)",
            rusqlite::params![d as i64, tier],
        )?;
    }

    // batches
    if let Some(arr) = body.get("batches").and_then(|v| v.as_array()) {
        for b in arr {
            let id = b
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("imported")
                .to_string();
            let created_at = b
                .get("createdAt")
                .or_else(|| b.get("created_at"))
                .and_then(|v| v.as_i64())
                .unwrap_or_else(crate::db::now_ms);
            let distribution = b.get("distribution").cloned().unwrap_or(json!({})).to_string();
            let difficulty_by_domain = b
                .get("difficultyByDomain")
                .or_else(|| b.get("difficulty_by_domain"))
                .cloned()
                .unwrap_or(json!({}))
                .to_string();
            let question_ids = b
                .get("questionIds")
                .or_else(|| b.get("question_ids"))
                .cloned()
                .unwrap_or(json!([]))
                .to_string();
            let tier_changes = b
                .get("tierChanges")
                .or_else(|| b.get("tier_changes"))
                .map(|v| v.to_string());
            let current_idx = b
                .get("currentIdx")
                .or_else(|| b.get("current_idx"))
                .or_else(|| b.get("idx"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let finished = b
                .get("finished")
                .and_then(|v| v.as_bool())
                .unwrap_or(false) as i64;
            tx.execute(
                "INSERT INTO batches
                   (id, created_at, distribution_json, difficulty_by_domain_json,
                    question_ids_json, tier_changes_json, current_idx, finished)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    id,
                    created_at,
                    distribution,
                    difficulty_by_domain,
                    question_ids,
                    tier_changes,
                    current_idx,
                    finished,
                ],
            )?;
        }
    }

    // currentBatch (legacy single-batch reference)
    if let Some(cb) = body.get("currentBatch") {
        if let Some(id) = cb.get("batchId").and_then(|v| v.as_str()) {
            crate::db::set_state(&tx, "active_batch_id", id)?;
        }
    }

    tx.commit()?;
    Ok(Json(json!({ "ok": true })))
}

/// Wipe every piece of user-generated data: questions, batches, chat messages,
/// active-batch pointer; reset domain_stats and difficulty back to defaults.
/// Preserves only the `provider` / `model` user preferences in `app_state`.
pub async fn reset(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let mut conn = state.pool.get()?;
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM questions", [])?;
    tx.execute("DELETE FROM batches", [])?;
    tx.execute("DELETE FROM chat_messages", [])?;
    tx.execute(
        "DELETE FROM app_state WHERE key NOT IN ('provider', 'model')",
        [],
    )?;
    tx.execute(
        "UPDATE domain_stats SET attempted = 0, correct = 0, recent_correct_json = '[]'",
        [],
    )?;
    tx.execute("UPDATE difficulty SET tier = 1", [])?;
    tx.commit()?;
    Ok(Json(json!({ "ok": true })))
}
