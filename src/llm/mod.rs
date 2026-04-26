//! Outbound LLM client wrappers.
//!
//! Two providers (OpenAI, Anthropic) with a unified streaming protocol the
//! routes layer can pipe into SSE.

pub mod anthropic;
pub mod openai;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum LlmError {
    #[error("missing api key for provider: {0}")]
    MissingKey(&'static str),

    #[error("upstream {provider} returned {status}: {body}")]
    Upstream {
        provider: &'static str,
        status: u16,
        body: String,
    },

    #[error("network: {0}")]
    Network(String),

    #[error("parse: {0}")]
    Parse(String),
}

/// Unified streaming event the routes layer forwards to the browser.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LlmEvent {
    Token { text: String },
    Done,
    Error { message: String },
}
