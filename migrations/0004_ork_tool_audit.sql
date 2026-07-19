-- Tool denetimi
CREATE TABLE IF NOT EXISTS tool_calls (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id INTEGER NOT NULL REFERENCES agents(id),
    tool TEXT NOT NULL,
    args_json TEXT,
    result_ref TEXT,
    status TEXT NOT NULL,
    capability_ok INTEGER,
    ts TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS capability_audit (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id INTEGER NOT NULL REFERENCES agents(id),
    capability TEXT NOT NULL,
    target TEXT NOT NULL,
    decision TEXT NOT NULL,
    approver TEXT,
    ts TEXT NOT NULL DEFAULT (datetime('now'))
);
