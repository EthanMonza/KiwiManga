-- KiwiManga initial schema.
-- Timestamps are unix seconds (INTEGER) to avoid extra date/time crates.

CREATE TABLE IF NOT EXISTS users (
  chat_id      INTEGER PRIMARY KEY,
  locale       TEXT,                       -- manual choice via /language (NULL = auto)
  auto_locale  TEXT NOT NULL DEFAULT 'en', -- last whatlang-based guess
  pack_mode    TEXT NOT NULL DEFAULT 'merged',
  chapter_lang TEXT,                       -- override via /settings (NULL = follow UI locale)
  created_at   INTEGER NOT NULL,
  updated_at   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS jobs (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  chat_id         INTEGER NOT NULL,
  kind            TEXT NOT NULL,            -- last10 | range | volumes | single
  payload         TEXT NOT NULL,            -- JSON JobPayload
  status          TEXT NOT NULL DEFAULT 'pending', -- pending | running | done | failed | canceled
  progress_done   INTEGER NOT NULL DEFAULT 0,
  progress_total  INTEGER NOT NULL DEFAULT 0,
  step            TEXT NOT NULL DEFAULT 'queued',  -- queued | download | pack | upload | done | failed | canceled
  progress_msg_id INTEGER,                  -- telegram message id for progress edits
  error           TEXT,
  attempts        INTEGER NOT NULL DEFAULT 0,
  created_at      INTEGER NOT NULL,
  updated_at      INTEGER NOT NULL,
  started_at      INTEGER,
  finished_at     INTEGER
);
CREATE INDEX IF NOT EXISTS idx_jobs_status ON jobs(status);
CREATE INDEX IF NOT EXISTS idx_jobs_chat_status ON jobs(chat_id, status);
-- Dedup: the same job cannot be queued/running twice for one chat.
CREATE UNIQUE INDEX IF NOT EXISTS idx_jobs_dedup
  ON jobs(chat_id, kind, payload) WHERE status IN ('pending', 'running');

CREATE TABLE IF NOT EXISTS chapters_cache (
  source     TEXT NOT NULL,  -- mangadex | ranobelib
  title_id   TEXT NOT NULL,  -- source-side title id (uuid / slug)
  chapter_id TEXT NOT NULL,  -- source-side chapter id
  lang       TEXT NOT NULL,  -- chapter language actually downloaded
  kind       TEXT NOT NULL,  -- manga | ranobe
  path       TEXT NOT NULL,  -- absolute path under /data/cache
  bytes      INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (source, title_id, chapter_id, lang)
);
