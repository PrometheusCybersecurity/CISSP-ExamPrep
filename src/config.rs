//! Configuration loaded from environment variables (populated by `.env`).

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub openai_key: Option<String>,
    pub anthropic_key: Option<String>,
    pub bind_addr: String,
    pub data_dir: PathBuf,
    pub default_provider: String,
    pub default_model: String,
}

impl Config {
    pub fn from_env() -> eyre::Result<Self> {
        let openai_key = std::env::var("OPENAI_API_KEY").ok().filter(|s| !s.is_empty());
        let anthropic_key = std::env::var("ANTHROPIC_API_KEY").ok().filter(|s| !s.is_empty());

        if openai_key.is_none() && anthropic_key.is_none() {
            eyre::bail!(
                "no LLM API key configured — set OPENAI_API_KEY and/or ANTHROPIC_API_KEY in .env"
            );
        }

        let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:7878".to_string());
        let data_dir = PathBuf::from(std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".into()));
        let default_provider =
            std::env::var("DEFAULT_PROVIDER").unwrap_or_else(|_| "openai".to_string());
        let default_model = std::env::var("DEFAULT_MODEL").unwrap_or_else(|_| "gpt-4o".to_string());

        Ok(Config {
            openai_key,
            anthropic_key,
            bind_addr,
            data_dir,
            default_provider,
            default_model,
        })
    }

    pub fn log_summary(&self) {
        tracing::info!(
            openai = self.openai_key.is_some(),
            anthropic = self.anthropic_key.is_some(),
            data_dir = %self.data_dir.display(),
            bind_addr = %self.bind_addr,
            default_provider = %self.default_provider,
            default_model = %self.default_model,
            "config loaded",
        );
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("cissp.db")
    }
}
