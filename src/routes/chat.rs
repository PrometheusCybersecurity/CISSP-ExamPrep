//! /api/chat/* — chat coach streaming + history.
//!
//! All SQLite work happens in sync helpers (off the async path) so the SSE
//! handler's future stays `Send`, which axum requires.

use axum::{
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::convert::Infallible;

use super::ApiError;
use crate::{
    db::{self, Pool},
    llm::{self, ChatMessage, LlmEvent},
    AppState,
};

const SYSTEM_PROMPT: &str = include_str!("../../static/system_prompt.txt");

#[derive(Deserialize)]
pub struct StreamPayload {
    pub message: String,
    pub provider: Option<String>,
    pub model: Option<String>,
}

#[derive(Serialize)]
pub struct HistoryMessage {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub created_at: i64,
}

pub async fn history(State(state): State<AppState>) -> Result<Json<Vec<HistoryMessage>>, ApiError> {
    let conn = state.pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, role, content, created_at FROM chat_messages ORDER BY id ASC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(HistoryMessage {
                id: r.get(0)?,
                role: r.get(1)?,
                content: r.get(2)?,
                created_at: r.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(rows))
}

pub async fn clear(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let conn = state.pool.get()?;
    conn.execute("DELETE FROM chat_messages", [])?;
    Ok(Json(json!({ "ok": true })))
}

/// Sync: append the user message and return (history, provider, model).
fn prepare_chat(
    pool: &Pool,
    cfg: &crate::config::Config,
    user_msg: &str,
    override_provider: Option<&str>,
    override_model: Option<&str>,
) -> Result<(Vec<ChatMessage>, String, String), ApiError> {
    let conn = pool.get()?;
    let now = db::now_ms();
    conn.execute(
        "INSERT INTO chat_messages(role, content, created_at) VALUES('user', ?1, ?2)",
        rusqlite::params![user_msg, now],
    )?;

    let history: Vec<ChatMessage> = {
        let mut stmt = conn.prepare(
            "SELECT role, content FROM chat_messages
              WHERE id IN (SELECT id FROM chat_messages ORDER BY id DESC LIMIT 30)
              ORDER BY id ASC",
        )?;
        let rows: Vec<ChatMessage> = stmt
            .query_map([], |r| {
                Ok(ChatMessage { role: r.get(0)?, content: r.get(1)? })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };

    let provider = override_provider
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            db::get_state(&conn, "provider")
                .ok()
                .flatten()
                .unwrap_or_else(|| cfg.default_provider.clone())
        });
    let model = override_model.map(|s| s.to_string()).unwrap_or_else(|| {
        db::get_state(&conn, "model")
            .ok()
            .flatten()
            .unwrap_or_else(|| cfg.default_model.clone())
    });

    Ok((history, provider, model))
}

/// Sync: persist a final assistant message.
fn persist_assistant(pool: &Pool, content: &str) -> Result<(), ApiError> {
    if content.is_empty() {
        return Ok(());
    }
    let conn = pool.get()?;
    conn.execute(
        "INSERT INTO chat_messages(role, content, created_at) VALUES('assistant', ?1, ?2)",
        rusqlite::params![content, db::now_ms()],
    )?;
    Ok(())
}

pub async fn stream(
    State(state): State<AppState>,
    Json(body): Json<StreamPayload>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let prepared = prepare_chat(
        &state.pool,
        &state.cfg,
        &body.message,
        body.provider.as_deref(),
        body.model.as_deref(),
    );

    let s = async_stream::stream! {
        let (history, provider, model) = match prepared {
            Ok(v) => v,
            Err(e) => { yield sse_error(e.message); return; }
        };

        let mut full = String::new();
        match provider.as_str() {
            "anthropic" => {
                let key = match state.cfg.anthropic_key.clone() {
                    Some(k) => k,
                    None => { yield sse_error("anthropic key not configured".into()); return; }
                };
                let inner = llm::anthropic::stream_chat(
                    state.http.clone(),
                    key,
                    llm::anthropic::AnthropicOptions {
                        model: &model,
                        max_tokens: 4096,
                        system: SYSTEM_PROMPT,
                    },
                    history,
                );
                futures_util::pin_mut!(inner);
                while let Some(ev) = futures_util::StreamExt::next(&mut inner).await {
                    match ev {
                        LlmEvent::Token { text } => {
                            full.push_str(&text);
                            yield sse_named("token", &json!({"text": text}));
                        }
                        LlmEvent::Done => break,
                        LlmEvent::Error { message } => {
                            yield sse_error(message);
                            return;
                        }
                    }
                }
            }
            _ => {
                let key = match state.cfg.openai_key.clone() {
                    Some(k) => k,
                    None => { yield sse_error("openai key not configured".into()); return; }
                };
                let mut msgs: Vec<ChatMessage> = vec![ChatMessage {
                    role: "system".into(),
                    content: SYSTEM_PROMPT.to_string(),
                }];
                msgs.extend(history);
                let inner = llm::openai::stream_chat(
                    state.http.clone(),
                    key,
                    llm::openai::OpenAiOptions {
                        model: &model,
                        max_tokens: 4096,
                        temperature: 0.7,
                        json_mode: false,
                    },
                    msgs,
                );
                futures_util::pin_mut!(inner);
                while let Some(ev) = futures_util::StreamExt::next(&mut inner).await {
                    match ev {
                        LlmEvent::Token { text } => {
                            full.push_str(&text);
                            yield sse_named("token", &json!({"text": text}));
                        }
                        LlmEvent::Done => break,
                        LlmEvent::Error { message } => {
                            yield sse_error(message);
                            return;
                        }
                    }
                }
            }
        }

        // Persist assistant reply (sync, off the async path).
        let _ = persist_assistant(&state.pool, &full);
        yield sse_named("done", &json!({}));
    };
    Sse::new(s).keep_alive(KeepAlive::default())
}

fn sse_error(msg: String) -> Result<Event, Infallible> {
    Ok(Event::default()
        .event("error")
        .data(json!({ "message": msg }).to_string()))
}

fn sse_named(name: &str, payload: &Value) -> Result<Event, Infallible> {
    Ok(Event::default().event(name).data(payload.to_string()))
}
