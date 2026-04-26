//! /api/settings — non-secret app settings.

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use super::ApiError;
use crate::{db, AppState};

#[derive(Serialize)]
pub struct Settings {
    pub provider: String,
    pub model: String,
    pub has_openai: bool,
    pub has_anthropic: bool,
}

#[derive(Deserialize)]
pub struct PatchPayload {
    pub provider: Option<String>,
    pub model: Option<String>,
}

pub async fn get(State(state): State<AppState>) -> Result<Json<Settings>, ApiError> {
    let conn = state.pool.get()?;
    let provider = db::get_state(&conn, "provider")?
        .unwrap_or_else(|| state.cfg.default_provider.clone());
    let model = db::get_state(&conn, "model")?
        .unwrap_or_else(|| state.cfg.default_model.clone());
    Ok(Json(Settings {
        provider,
        model,
        has_openai: state.cfg.openai_key.is_some(),
        has_anthropic: state.cfg.anthropic_key.is_some(),
    }))
}

pub async fn patch(
    State(state): State<AppState>,
    Json(body): Json<PatchPayload>,
) -> Result<Json<Settings>, ApiError> {
    let conn = state.pool.get()?;
    if let Some(p) = body.provider {
        if p != "openai" && p != "anthropic" {
            return Err(ApiError::bad("provider must be 'openai' or 'anthropic'"));
        }
        db::set_state(&conn, "provider", &p)?;
    }
    if let Some(m) = body.model {
        db::set_state(&conn, "model", &m)?;
    }
    drop(conn);
    get(State(state)).await
}
