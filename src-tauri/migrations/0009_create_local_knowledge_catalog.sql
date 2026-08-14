-- Migration 0009: app-private, device-local knowledge grants for VCPMobileCLI vref.
-- Canonical source bytes live in the dedicated knowledge CAS; these tables are not synced.

CREATE TABLE IF NOT EXISTS local_knowledge_meta (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    catalog_generation INTEGER NOT NULL CHECK (catalog_generation >= 0),
    used_bytes INTEGER NOT NULL CHECK (used_bytes >= 0),
    updated_at_ms INTEGER NOT NULL
);

INSERT OR IGNORE INTO local_knowledge_meta(
    singleton, schema_version, catalog_generation, used_bytes, updated_at_ms
) VALUES (1, 1, 0, 0, 0);

CREATE TABLE IF NOT EXISTS local_knowledge_sources (
    source_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL CHECK (length(display_name) BETWEEN 1 AND 240),
    mime_type TEXT NOT NULL CHECK (length(mime_type) BETWEEN 1 AND 128),
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0 AND size_bytes <= 33554432),
    source_sha256 TEXT NOT NULL CHECK (length(source_sha256) = 64),
    object_name TEXT NOT NULL CHECK (length(object_name) = 64),
    index_status TEXT NOT NULL CHECK (index_status IN ('indexing', 'ready', 'failed')),
    failure_code TEXT CHECK (failure_code IS NULL OR length(failure_code) <= 96),
    index_text_truncated INTEGER NOT NULL CHECK (index_text_truncated IN (0, 1)),
    chunk_count INTEGER NOT NULL CHECK (chunk_count BETWEEN 0 AND 300),
    granted_at_ms INTEGER NOT NULL,
    revoked_at_ms INTEGER,
    UNIQUE(source_sha256)
);

CREATE INDEX IF NOT EXISTS idx_local_knowledge_sources_active
    ON local_knowledge_sources(revoked_at_ms, index_status, source_id);

CREATE TABLE IF NOT EXISTS local_knowledge_chunks (
    source_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 299),
    content TEXT NOT NULL CHECK (length(CAST(content AS BLOB)) <= 4096),
    content_sha256 TEXT NOT NULL CHECK (length(content_sha256) = 64),
    PRIMARY KEY (source_id, ordinal),
    FOREIGN KEY (source_id) REFERENCES local_knowledge_sources(source_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_local_knowledge_chunks_content
    ON local_knowledge_chunks(content_sha256, source_id);

CREATE TABLE IF NOT EXISTS local_knowledge_import_candidates (
    token TEXT PRIMARY KEY,
    inspect_operation_id TEXT NOT NULL UNIQUE,
    candidate_sha256 TEXT NOT NULL CHECK (length(candidate_sha256) = 64),
    source_sha256 TEXT NOT NULL CHECK (length(source_sha256) = 64),
    staging_name TEXT NOT NULL CHECK (length(staging_name) BETWEEN 1 AND 96),
    display_name TEXT NOT NULL CHECK (length(display_name) BETWEEN 1 AND 240),
    mime_type TEXT NOT NULL CHECK (length(mime_type) BETWEEN 1 AND 128),
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0 AND size_bytes <= 33554432),
    catalog_generation INTEGER NOT NULL CHECK (catalog_generation >= 0),
    index_text_truncated INTEGER NOT NULL CHECK (index_text_truncated IN (0, 1)),
    chunk_count INTEGER NOT NULL CHECK (chunk_count BETWEEN 0 AND 300),
    created_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms >= created_at_ms)
);

CREATE INDEX IF NOT EXISTS idx_local_knowledge_candidates_expiry
    ON local_knowledge_import_candidates(expires_at_ms ASC);

CREATE TABLE IF NOT EXISTS local_knowledge_operations (
    operation_id TEXT PRIMARY KEY,
    operation_kind TEXT NOT NULL CHECK (operation_kind IN ('inspect', 'commit', 'discard', 'revoke')),
    request_sha256 TEXT NOT NULL CHECK (length(request_sha256) = 64),
    source_id TEXT,
    result_json TEXT NOT NULL CHECK (length(CAST(result_json AS BLOB)) <= 262144),
    created_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_local_knowledge_operations_created
    ON local_knowledge_operations(created_at_ms ASC);

CREATE TABLE IF NOT EXISTS local_knowledge_attempt_holds (
    turn_attempt TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    source_sha256 TEXT NOT NULL CHECK (length(source_sha256) = 64),
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (turn_attempt, operation_id, source_id),
    FOREIGN KEY (source_id) REFERENCES local_knowledge_sources(source_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_local_knowledge_holds_source
    ON local_knowledge_attempt_holds(source_id);
