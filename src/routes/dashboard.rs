//! GET /api/dashboard — overview shown on the Quiz tab empty state.

use axum::{extract::State, Json};
use serde::Serialize;
use serde_json::Value;

use super::ApiError;
use crate::{engine, AppState};

#[derive(Serialize)]
pub struct DomainSummary {
    pub domain: u8,
    pub name: &'static str,
    pub short: &'static str,
    pub attempted: u32,
    pub correct: u32,
    pub lifetime_accuracy: Option<f64>,
    pub rolling_accuracy: f64,
    pub tier: u8,
    pub tier_name: &'static str,
    pub planned_count: u32,
    pub stretch_count: u32,
    pub stretch_tier: Option<u8>,
}

#[derive(Serialize)]
pub struct Dashboard {
    pub total_questions: u32,
    pub overall_accuracy: Option<f64>,
    pub weakest_domain: Option<u8>,
    pub domains: Vec<DomainSummary>,
}

pub async fn get(State(state): State<AppState>) -> Result<Json<Dashboard>, ApiError> {
    let conn = state.pool.get()?;
    let stats = load_stats(&conn)?;
    let diff = load_difficulty(&conn)?;
    let plan = engine::build_batch_plan(&stats, &diff, engine::BATCH_SIZE);

    let total_questions: u32 = conn
        .query_row("SELECT COUNT(*) FROM questions", [], |r| r.get::<_, i64>(0))?
        as u32;

    let mut total_attempted = 0u32;
    let mut total_correct = 0u32;
    let mut weakest: Option<(u8, f64)> = None;
    let mut domains = Vec::with_capacity(8);

    for (d, name, short) in engine::DOMAINS {
        let s = stats.get(&d).cloned().unwrap_or_default();
        total_attempted += s.attempted;
        total_correct += s.correct;
        let lifetime = s.lifetime_accuracy();
        if let Some(acc) = lifetime {
            match weakest {
                None => weakest = Some((d, acc)),
                Some((_, prev)) if acc < prev => weakest = Some((d, acc)),
                _ => {}
            }
        }
        let tier = diff.get(&d).copied().unwrap_or(1);
        let p = plan.iter().find(|p| p.domain == d);
        let planned_count = p.map(|p| p.count).unwrap_or(0);
        let (stretch_count, stretch_tier) = match p.and_then(|p| p.stretch.as_ref()) {
            Some(s) => (s.count, Some(s.tier)),
            None => (0, None),
        };
        domains.push(DomainSummary {
            domain: d,
            name,
            short,
            attempted: s.attempted,
            correct: s.correct,
            lifetime_accuracy: lifetime,
            rolling_accuracy: s.rolling_accuracy(),
            tier,
            tier_name: engine::tier_name(tier),
            planned_count,
            stretch_count,
            stretch_tier,
        });
    }

    let overall = if total_attempted == 0 {
        None
    } else {
        Some(total_correct as f64 / total_attempted as f64)
    };

    Ok(Json(Dashboard {
        total_questions,
        overall_accuracy: overall,
        weakest_domain: weakest.map(|(d, _)| d),
        domains,
    }))
}

pub fn load_stats(conn: &rusqlite::Connection) -> Result<engine::Stats, ApiError> {
    let mut stmt = conn.prepare(
        "SELECT domain, attempted, correct, recent_correct_json FROM domain_stats",
    )?;
    let rows = stmt
        .query_map([], |r| {
            let domain: u8 = r.get::<_, i64>(0)? as u8;
            let attempted: u32 = r.get::<_, i64>(1)? as u32;
            let correct: u32 = r.get::<_, i64>(2)? as u32;
            let recent_json: String = r.get(3)?;
            Ok((domain, attempted, correct, recent_json))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = engine::Stats::new();
    for (d, attempted, correct, recent_json) in rows {
        let recent: Vec<u8> = serde_json::from_str(&recent_json).unwrap_or_default();
        out.insert(
            d,
            engine::DomainStat {
                attempted,
                correct,
                recent_correct: recent,
            },
        );
    }
    Ok(out)
}

pub fn load_difficulty(conn: &rusqlite::Connection) -> Result<engine::Difficulty, ApiError> {
    let mut stmt = conn.prepare("SELECT domain, tier FROM difficulty")?;
    let rows = stmt
        .query_map([], |r| {
            let d: u8 = r.get::<_, i64>(0)? as u8;
            let t: u8 = r.get::<_, i64>(1)? as u8;
            Ok((d, t))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = engine::Difficulty::new();
    for (d, t) in rows {
        out.insert(d, t.clamp(1, 4));
    }
    Ok(out)
}

pub fn _stub_unused(_v: Value) {}
