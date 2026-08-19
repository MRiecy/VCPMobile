-- Migration 0008: 重建 messages_fts 为 trigram 分词 + 补消息过滤索引
--
-- 背景：0003 建立的 unicode61 FTS 依赖应用层 preprocess_fts_text() 逐 CJK 字插空格，
-- 等价于单字索引：不保字序、假阳性高（"机器学习"与"学习机器"无法区分）。
-- bundled SQLite 3.46.0 内置 FTS5 trigram 分词器（≥3.34 起），任意 3 字滑窗，
-- 保局部字序、支持子串匹配，写入路径从此直接存原文。
--
-- 本迁移只换表结构，不回填存量数据：回填由 rebuild_messages_fts 命令
-- 在首次打开全局搜索页时分批执行（避免启动路径长事务占锁）。
-- 注意：重建后 FTS 索引为空，全局搜索在回填完成前结果不全，
-- 前端需通过索引覆盖率状态向用户展示"索引构建中"。

-- 1. 摘除旧触发器与旧 FTS 表（0003/0004 版本，unicode61）
DROP TRIGGER IF EXISTS after_messages_physical_delete;
DROP TRIGGER IF EXISTS after_messages_logical_delete;
DROP TABLE IF EXISTS messages_fts;

-- 2. 重建 trigram 版 FTS 虚表（列契约不变：msg_id/topic_id UNINDEXED + content）
CREATE VIRTUAL TABLE messages_fts USING fts5(
    msg_id UNINDEXED,
    topic_id UNINDEXED,
    content,
    tokenize = 'trigram'
);

-- 3. 重建删除同步触发器（沿用 0004 的复合键匹配，防跨话题误删）
CREATE TRIGGER after_messages_physical_delete
AFTER DELETE ON messages
BEGIN
    DELETE FROM messages_fts WHERE msg_id = old.msg_id AND topic_id = old.topic_id;
END;

CREATE TRIGGER after_messages_logical_delete
AFTER UPDATE OF deleted_at ON messages
WHEN new.deleted_at IS NOT NULL
BEGIN
    DELETE FROM messages_fts WHERE msg_id = new.msg_id AND topic_id = new.topic_id;
END;

-- 4. 搜索过滤条件索引（全局搜索按 agent/role 过滤时避免全表扫）
CREATE INDEX IF NOT EXISTS idx_messages_agent_id ON messages(agent_id);
CREATE INDEX IF NOT EXISTS idx_messages_role ON messages(role);
