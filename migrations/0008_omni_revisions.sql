-- Diff akışı · config katmanı · araştırma · persona cache
CREATE TABLE IF NOT EXISTS file_touches (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id INTEGER NOT NULL REFERENCES agents(id),
    path TEXT NOT NULL,
    outside_workspace INTEGER NOT NULL DEFAULT 0,
    added INTEGER NOT NULL DEFAULT 0,
    removed INTEGER NOT NULL DEFAULT 0,
    pre_ref TEXT,
    post_ref TEXT,
    ts TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS config_kv (
    key TEXT PRIMARY KEY,
    value_json TEXT NOT NULL,
    source TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS research_findings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id INTEGER NOT NULL REFERENCES tasks(id),
    mode TEXT NOT NULL,
    query TEXT NOT NULL,
    result_ref TEXT,
    format TEXT,
    ts TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS personas (
    name TEXT PRIMARY KEY,
    path TEXT NOT NULL,
    checksum TEXT NOT NULL,
    loaded_at TEXT NOT NULL DEFAULT (datetime('now'))
);
