-- A pending generation may outlive its Rust request owner. Preserve an explicit
-- user stop so reconnection only settles that generation instead of continuing it.
ALTER TABLE active_generations ADD COLUMN stop_requested INTEGER NOT NULL DEFAULT 0;
-- Written only when stopping cannot be confirmed, never on every SSE chunk.
ALTER TABLE active_generations ADD COLUMN stop_partial_content TEXT;
