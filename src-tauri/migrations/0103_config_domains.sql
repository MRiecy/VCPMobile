-- Configuration is stored per domain. The settings table remains owned by its
-- cache/state consumers; only its former 'global' row is moved by Rust once.
CREATE TABLE config_domains (
    domain TEXT PRIMARY KEY NOT NULL,
    schema_version INTEGER NOT NULL CHECK (schema_version >= 1),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    value TEXT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE TABLE config_recovery (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    domain TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    revision INTEGER NOT NULL,
    value TEXT NOT NULL,
    reason TEXT NOT NULL,
    created_at BIGINT NOT NULL
);

CREATE TABLE config_metadata (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);
