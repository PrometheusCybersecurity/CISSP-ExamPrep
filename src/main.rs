//! CISSP Coach — local server.
//!
//! Loads `.env`, opens (or creates) a SQLite database, runs the embedded
//! migration, and serves both the API and the embedded `static/index.html`
//! on `BIND_ADDR`.

use std::sync::Arc;

mod config;
mod db;
mod engine;
mod llm;
mod pdf;
mod routes;
mod static_assets;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<config::Config>,
    pub pool: db::Pool,
    pub http: reqwest::Client,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // 1. Load `.env` (best effort — missing file is fine, we'll error
    //    later if no API key was set).
    let _ = dotenvy::dotenv();

    // 2. Tracing.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,cissp_coach=debug,tower_http=info".into()),
        )
        .with_target(false)
        .compact()
        .init();

    // 3. Config + sanity check on keys.
    let cfg = config::Config::from_env()?;
    cfg.log_summary();

    // 4. DB.
    let pool = db::init(&cfg)?;

    // 5. Reqwest client (shared HTTP connection pool for outbound LLM calls).
    let http = reqwest::Client::builder()
        .user_agent("cissp-coach/0.1")
        .build()?;

    let state = AppState {
        cfg: Arc::new(cfg),
        pool,
        http,
    };

    // 6. Router.
    let app = routes::build_router(state.clone());

    // 7. Serve.
    let addr: std::net::SocketAddr = state.cfg.bind_addr.parse()?;
    tracing::info!("listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
