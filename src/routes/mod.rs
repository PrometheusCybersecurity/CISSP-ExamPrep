//! HTTP routes.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::json;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::{static_assets, AppState};

mod batches;
mod chat;
mod dashboard;
mod data;
mod settings;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self { status, message: message.into() }
    }
    pub fn bad(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, msg)
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, msg)
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, msg)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(json!({ "error": self.message }));
        (self.status, body).into_response()
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        ApiError::internal(format!("db: {e}"))
    }
}

impl From<r2d2::Error> for ApiError {
    fn from(e: r2d2::Error) -> Self {
        ApiError::internal(format!("db pool: {e}"))
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        ApiError::internal(format!("json: {e}"))
    }
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        // Frontend
        .route("/", get(static_assets::index))
        // API
        .route("/api/settings", get(settings::get).patch(settings::patch))
        .route("/api/dashboard", get(dashboard::get))
        .route("/api/batches/generate", post(batches::generate))
        .route("/api/batches/current", get(batches::current))
        .route("/api/batches/:id/answer", post(batches::answer))
        .route("/api/batches/:id/skip", post(batches::skip))
        .route("/api/batches/:id/finish", post(batches::finish))
        .route("/api/batches/:id/cancel", post(batches::cancel))
        .route("/api/batches/:id/summary", get(batches::summary))
        .route("/api/batches/:id/study-guide", post(batches::study_guide))
        .route("/api/study-guide/all-misses", post(batches::global_study_guide))
        .route("/api/chat/stream", post(chat::stream))
        .route("/api/chat/history", get(chat::history).delete(chat::clear))
        .route("/api/export", get(data::export))
        .route("/api/import", post(data::import))
        .route("/api/data/reset", post(data::reset))
        // Static assets fallback (any unmatched GET serves embedded files; index for SPA).
        .fallback(static_assets::asset)
        .with_state(state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}
