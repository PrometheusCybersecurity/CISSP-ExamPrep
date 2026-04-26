//! SQLite connection pool + migration runner.

use crate::config::Config;
use r2d2_sqlite::SqliteConnectionManager;

pub type Pool = r2d2::Pool<SqliteConnectionManager>;
pub type Conn = r2d2::PooledConnection<SqliteConnectionManager>;

const INIT_SQL: &str = include_str!("../migrations/0001_init.sql");

pub fn init(cfg: &Config) -> eyre::Result<Pool> {
    std::fs::create_dir_all(&cfg.data_dir)?;
    let manager = SqliteConnectionManager::file(cfg.db_path()).with_init(|c| {
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
    });
    let pool = r2d2::Pool::builder().max_size(8).build(manager)?;

    // Run migration once.
    let conn = pool.get()?;
    conn.execute_batch(INIT_SQL)?;
    tracing::info!(path = %cfg.db_path().display(), "database ready");
    Ok(pool)
}

/// Read a value from `app_state` keyed by `key`.
/// Accepts any `&rusqlite::Connection`-like value (pooled conn or transaction).
pub fn get_state(conn: &rusqlite::Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM app_state WHERE key = ?1",
        [key],
        |row| row.get::<_, String>(0),
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    })
}

/// Upsert a value into `app_state`.
pub fn set_state(conn: &rusqlite::Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO app_state(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![key, value],
    )?;
    Ok(())
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
