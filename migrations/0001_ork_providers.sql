-- Provider & key yönetimi
CREATE TABLE IF NOT EXISTS providers (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,  -- openai, anthropic, google, openrouter, litellm, etc.
    base_url TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS api_keys (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider_id INTEGER NOT NULL REFERENCES providers(id),
    key_ref TEXT NOT NULL,  -- keyring reference, not the actual key
    label TEXT,
    status TEXT NOT NULL DEFAULT 'active' CHECK(status IN ('active', 'revoked', 'expired')),
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
