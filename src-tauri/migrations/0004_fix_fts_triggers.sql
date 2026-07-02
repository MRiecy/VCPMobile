-- Migration 0004: Recreate FTS5 triggers with proper composite primary key (topic_id, msg_id) matching
-- Drop legacy single-key triggers from Migration 0003
DROP TRIGGER IF EXISTS after_messages_physical_delete;
DROP TRIGGER IF EXISTS after_messages_logical_delete;

-- Recreate with composite key matching to prevent cross-topic collision deletion
CREATE TRIGGER IF NOT EXISTS after_messages_physical_delete
AFTER DELETE ON messages
BEGIN
    DELETE FROM messages_fts WHERE msg_id = old.msg_id AND topic_id = old.topic_id;
END;

CREATE TRIGGER IF NOT EXISTS after_messages_logical_delete
AFTER UPDATE OF deleted_at ON messages
WHEN new.deleted_at IS NOT NULL
BEGIN
    DELETE FROM messages_fts WHERE msg_id = new.msg_id AND topic_id = new.topic_id;
END;
