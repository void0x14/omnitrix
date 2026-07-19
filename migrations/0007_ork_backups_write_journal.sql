-- Yedekleme & WAL kontrol
CREATE TABLE IF NOT EXISTS backups (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    scope TEXT NOT NULL,
    destination TEXT NOT NULL,
    status TEXT NOT NULL,
    checksum TEXT,
    size INTEGER,
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    finished_at TEXT
);

CREATE TABLE IF NOT EXISTS write_journal (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    op_id TEXT NOT NULL UNIQUE,
    op_kind TEXT NOT NULL,
    payload_ref TEXT,
    applied INTEGER NOT NULL DEFAULT 0,
    ts TEXT NOT NULL DEFAULT (datetime('now'))
);
