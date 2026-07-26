-- Denetim & güven
CREATE TABLE IF NOT EXISTS penalty_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id INTEGER NOT NULL REFERENCES agents(id),
    level TEXT NOT NULL,
    reason TEXT NOT NULL,
    applied_by TEXT NOT NULL,
    ts TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS interrupts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id INTEGER NOT NULL REFERENCES agents(id),
    kind TEXT NOT NULL,
    source TEXT NOT NULL,
    ts TEXT NOT NULL DEFAULT (datetime('now')),
    resolved_at TEXT
);

CREATE TABLE IF NOT EXISTS trust_scores (
    subject_id TEXT NOT NULL,
    subject_kind TEXT NOT NULL,
    score REAL NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (subject_id, subject_kind)
);
