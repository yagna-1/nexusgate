-- NexusGate Schema
-- All monetary values stored as micro-USD (1 USD = 1,000,000 micro-USD) for precision

CREATE TABLE IF NOT EXISTS api_keys (
    id          TEXT PRIMARY KEY,
    key_hash    TEXT NOT NULL UNIQUE,       -- SHA-256 of raw key, never store plaintext
    name        TEXT NOT NULL,
    -- Budget limits (null = unlimited)
    budget_total_micro_usd    INTEGER,      -- lifetime total
    budget_daily_micro_usd    INTEGER,      -- per calendar day
    budget_monthly_micro_usd  INTEGER,      -- per calendar month
    max_tokens_per_request    INTEGER,
    allowed_tiers             TEXT,         -- JSON array: ["economy","standard"] or NULL = all
    is_active   INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    last_used_at TEXT
);

CREATE TABLE IF NOT EXISTS cost_records (
    id              TEXT PRIMARY KEY,
    api_key_id      TEXT NOT NULL,
    provider        TEXT NOT NULL,          -- openai | anthropic | gemini
    model           TEXT NOT NULL,
    input_tokens    INTEGER NOT NULL,
    output_tokens   INTEGER NOT NULL,
    cost_micro_usd  INTEGER NOT NULL,
    request_id      TEXT NOT NULL,
    fallback_used   INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (api_key_id) REFERENCES api_keys(id)
);

-- Indexes for cost queries
CREATE INDEX IF NOT EXISTS idx_cost_key_id    ON cost_records(api_key_id);
CREATE INDEX IF NOT EXISTS idx_cost_created   ON cost_records(created_at);
CREATE INDEX IF NOT EXISTS idx_cost_provider  ON cost_records(provider);
CREATE INDEX IF NOT EXISTS idx_cost_request   ON cost_records(request_id);

-- Seed a default admin API key for first-run (will be printed to logs on startup)
-- The actual seeding is done in code for proper key generation
