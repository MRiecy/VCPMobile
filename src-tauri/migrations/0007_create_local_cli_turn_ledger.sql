-- Migration 0007: durable owner for the bounded local VCPMobileCLI model/tool loop.
-- This is device-local execution state and is intentionally excluded from sync tables.

CREATE TABLE IF NOT EXISTS local_cli_turn_ledger (
    turn_attempt TEXT PRIMARY KEY,
    outer_message_id TEXT NOT NULL,
    topic_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    owner_type TEXT NOT NULL CHECK (owner_type IN ('agent', 'group')),
    speaker_agent_id TEXT,
    route TEXT NOT NULL CHECK (route IN ('local_loopback', 'vcp_plugin')),
    state TEXT NOT NULL CHECK (state IN (
        'claimed', 'running', 'result_ready', 'continuation_pending',
        'continued', 'finalizing', 'terminal', 'interrupted'
    )),
    step_index INTEGER NOT NULL CHECK (step_index >= 0),
    tool_steps INTEGER NOT NULL CHECK (tool_steps >= 0 AND tool_steps <= 8),
    started_at_ms INTEGER NOT NULL,
    deadline_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    version INTEGER NOT NULL DEFAULT 0,
    frozen_request_json TEXT NOT NULL,
    continuation_messages_json TEXT,
    expected_calls INTEGER NOT NULL DEFAULT 0 CHECK (expected_calls >= 0),
    step_records_json TEXT NOT NULL DEFAULT '[]',
    marked_history_json TEXT NOT NULL DEFAULT '[]',
    final_content TEXT,
    terminal_reason TEXT,
    UNIQUE (topic_id, outer_message_id),
    FOREIGN KEY (topic_id, outer_message_id)
        REFERENCES messages(topic_id, msg_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_local_cli_turn_pending
    ON local_cli_turn_ledger(outer_message_id, state, updated_at_ms);

CREATE INDEX IF NOT EXISTS idx_local_cli_turn_topic
    ON local_cli_turn_ledger(topic_id, updated_at_ms DESC);

-- Messages and topics use soft deletion, so ON DELETE CASCADE is insufficient.
-- These triggers cover local delete, regeneration truncation, and sync-applied tombstones.
CREATE TRIGGER IF NOT EXISTS trg_local_cli_turn_message_tombstone
AFTER UPDATE OF deleted_at ON messages
WHEN NEW.deleted_at IS NOT NULL
BEGIN
    DELETE FROM local_cli_turn_ledger
    WHERE topic_id = NEW.topic_id AND outer_message_id = NEW.msg_id;
END;

CREATE TRIGGER IF NOT EXISTS trg_local_cli_turn_topic_tombstone
AFTER UPDATE OF deleted_at ON topics
WHEN NEW.deleted_at IS NOT NULL
BEGIN
    DELETE FROM local_cli_turn_ledger WHERE topic_id = NEW.topic_id;
END;
