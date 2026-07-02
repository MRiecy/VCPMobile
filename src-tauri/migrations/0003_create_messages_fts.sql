-- Migration 0003: 全文搜索 FTS5 虚拟表 + 同步触发器
-- sqlite3_exec() 原生支持多语句与 BEGIN...END 块，无需任何分割技巧

CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    msg_id UNINDEXED,
    topic_id UNINDEXED,
    content,
    tokenize = 'unicode61'
);

-- 物理删除同步：消息被物理删除时从 FTS 索引中移除
CREATE TRIGGER IF NOT EXISTS after_messages_physical_delete
AFTER DELETE ON messages
BEGIN
    DELETE FROM messages_fts WHERE msg_id = old.msg_id;
END;

-- 逻辑删除同步：消息设置 deleted_at 时从 FTS 索引中移除
CREATE TRIGGER IF NOT EXISTS after_messages_logical_delete
AFTER UPDATE OF deleted_at ON messages
WHEN new.deleted_at IS NOT NULL
BEGIN
    DELETE FROM messages_fts WHERE msg_id = new.msg_id;
END;
