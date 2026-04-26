-- Idempotent initial schema for CISSP Coach.
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS questions (
  id              TEXT    PRIMARY KEY,
  domain          INTEGER NOT NULL,
  subtopic        TEXT,
  difficulty      INTEGER NOT NULL,
  question        TEXT    NOT NULL,
  options_json    TEXT    NOT NULL,
  correct         TEXT    NOT NULL,
  explanation     TEXT,
  user_answer     TEXT,
  is_correct      INTEGER,
  answered_at     INTEGER,
  created_at      INTEGER NOT NULL,
  batch_id        TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_questions_batch  ON questions(batch_id);
CREATE INDEX IF NOT EXISTS idx_questions_domain ON questions(domain);

CREATE TABLE IF NOT EXISTS domain_stats (
  domain               INTEGER PRIMARY KEY,
  attempted            INTEGER NOT NULL DEFAULT 0,
  correct              INTEGER NOT NULL DEFAULT 0,
  recent_correct_json  TEXT    NOT NULL DEFAULT '[]'
);

CREATE TABLE IF NOT EXISTS difficulty (
  domain INTEGER PRIMARY KEY,
  tier   INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS batches (
  id                         TEXT    PRIMARY KEY,
  created_at                 INTEGER NOT NULL,
  distribution_json          TEXT    NOT NULL,
  difficulty_by_domain_json  TEXT    NOT NULL,
  question_ids_json          TEXT    NOT NULL DEFAULT '[]',
  tier_changes_json          TEXT,
  current_idx                INTEGER NOT NULL DEFAULT 0,
  finished                   INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS app_state (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS chat_messages (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  role       TEXT    NOT NULL,
  content    TEXT    NOT NULL,
  created_at INTEGER NOT NULL
);

-- Seed difficulty rows for all 8 domains at tier 1 (Easy).
INSERT OR IGNORE INTO difficulty (domain, tier) VALUES
  (1, 1), (2, 1), (3, 1), (4, 1), (5, 1), (6, 1), (7, 1), (8, 1);

-- Seed empty domain_stats rows.
INSERT OR IGNORE INTO domain_stats (domain, attempted, correct, recent_correct_json) VALUES
  (1, 0, 0, '[]'), (2, 0, 0, '[]'), (3, 0, 0, '[]'), (4, 0, 0, '[]'),
  (5, 0, 0, '[]'), (6, 0, 0, '[]'), (7, 0, 0, '[]'), (8, 0, 0, '[]');
