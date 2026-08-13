-- Migration 0008: device-local, disposable vectors for river=semantic:N.
-- No source text or query is stored, and this table is intentionally absent from sync.

CREATE TABLE IF NOT EXISTS local_semantic_embedding_cache (
    model_id TEXT NOT NULL,
    content_sha256 TEXT NOT NULL CHECK (length(content_sha256) = 64),
    dimension INTEGER NOT NULL CHECK (dimension > 0 AND dimension <= 4096),
    vector BLOB NOT NULL CHECK (length(vector) = dimension * 4),
    created_at_ms INTEGER NOT NULL,
    last_used_at_ms INTEGER NOT NULL,
    PRIMARY KEY (model_id, content_sha256)
);

CREATE INDEX IF NOT EXISTS idx_local_semantic_cache_lru
    ON local_semantic_embedding_cache(last_used_at_ms ASC);
