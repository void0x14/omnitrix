-- Kayıt
CREATE TABLE IF NOT EXISTS recordings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id INTEGER NOT NULL REFERENCES agents(id),
    media_type TEXT NOT NULL,
    blob_ref TEXT NOT NULL,
    bytes INTEGER,
    codec TEXT,
    started_at TEXT NOT NULL,
    ended_at TEXT
);
