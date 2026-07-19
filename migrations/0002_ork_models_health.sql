-- Model & sağlık
CREATE TABLE IF NOT EXISTS models (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider_id INTEGER NOT NULL REFERENCES providers(id),
    name TEXT NOT NULL,
    ctx_len INTEGER,
    price_in REAL,
    price_out REAL,
    caps_json TEXT,  -- JSON capability flags
    UNIQUE(provider_id, name)
);

CREATE TABLE IF NOT EXISTS provider_health (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider_id INTEGER NOT NULL REFERENCES providers(id),
    model TEXT,
    state TEXT NOT NULL CHECK(state IN ('healthy', 'degraded', 'down', 'quota_exhausted')),
    latency_ms INTEGER,
    checked_at TEXT NOT NULL DEFAULT (datetime('now')),
    detail TEXT
);

CREATE TABLE IF NOT EXISTS routing_policies (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    strategy TEXT NOT NULL CHECK(strategy IN ('round_robin', 'weighted', 'fallback', 'jep')),
    config_json TEXT NOT NULL  -- JSON strategy configuration
);
