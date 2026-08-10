ALTER TABLE render_cache ADD COLUMN content_hash TEXT NOT NULL DEFAULT '';
ALTER TABLE render_cache ADD COLUMN renderer_schema_version INTEGER NOT NULL DEFAULT 0;
