-- Migration 0100: Baseline v2 —— 全新安装的完整终态 schema 存档点
--
-- 设计（见 plan/vcpmobile-global-search-research/01 Phase 0 附）：
--   全新安装由 bootstrap_fresh_if_needed 直接执行本文件并 seed 全部既有迁移记录，
--   跳过 0001→0008 增量链（永不经过 unicode61 FTS 中间态、永不 ALTER）；
--   存量安装由 bootstrap_legacy_if_needed 将 v100 seed 为已应用，本文件不执行。
--
-- 铁律：
--   1. 本文件必须与 0001→0008 增量链的终态保持 schema 等价（有 CI 级漂移防护测试）；
--   2. 全部语句使用 IF NOT EXISTS，保证任何情况下重复执行无害；
--   3. 后续增量迁移使用 >0100 的版本号（0101、0102…）；
--      严禁新增版本号 < 0100 的迁移（会被全新安装快速路径 seed 跳过）。

-- ============ 表（0001 终态 + 0002/0005/0006/0007 增量列） ============

-- 1. avatars 全局多态头像表（含 0006 deleted_at）
CREATE TABLE IF NOT EXISTS avatars (
    owner_type TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    avatar_hash TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    image_data BLOB NOT NULL,
    dominant_color TEXT,
    updated_at BIGINT NOT NULL,
    deleted_at BIGINT,
    PRIMARY KEY (owner_type, owner_id)
);

-- 2. agents 表 (智能体配置)
CREATE TABLE IF NOT EXISTS agents (
    agent_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    system_prompt TEXT NOT NULL DEFAULT '',
    mobile_system_prompt TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL,
    temperature REAL NOT NULL DEFAULT 1,
    context_token_limit INTEGER NOT NULL DEFAULT 0,
    max_output_tokens INTEGER NOT NULL DEFAULT 0,
    stream_output INTEGER NOT NULL DEFAULT 1,
    use_temperature INTEGER NOT NULL DEFAULT 0,
    config_hash TEXT NOT NULL DEFAULT '',
    content_hash TEXT NOT NULL DEFAULT '',
    updated_at BIGINT NOT NULL,
    deleted_at BIGINT
);

-- 3. groups 表 (群组配置)
CREATE TABLE IF NOT EXISTS groups (
    group_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    mode TEXT NOT NULL DEFAULT 'sequential',
    group_prompt TEXT,
    invite_prompt TEXT,
    use_unified_model INTEGER NOT NULL DEFAULT 0,
    unified_model TEXT,
    tag_match_mode TEXT,
    config_hash TEXT NOT NULL DEFAULT '',
    content_hash TEXT NOT NULL DEFAULT '',
    created_at BIGINT NOT NULL DEFAULT 0,
    updated_at BIGINT NOT NULL,
    deleted_at BIGINT
);

-- 4. group_members 表
CREATE TABLE IF NOT EXISTS group_members (
    group_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    member_tag TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (group_id, agent_id)
);

-- 5. topics 表 (主题管理)
CREATE TABLE IF NOT EXISTS topics (
    topic_id TEXT PRIMARY KEY,
    owner_type TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    title TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    locked INTEGER NOT NULL DEFAULT 1,
    unread INTEGER NOT NULL DEFAULT 0,
    unread_count INTEGER NOT NULL DEFAULT 0,
    msg_count INTEGER NOT NULL DEFAULT 0,
    config_hash TEXT NOT NULL DEFAULT '',
    content_hash TEXT NOT NULL DEFAULT '',
    deleted_at BIGINT
);

-- 6. messages 表 (消息历史)
CREATE TABLE IF NOT EXISTS messages (
    msg_id TEXT NOT NULL,
    topic_id TEXT NOT NULL,
    role TEXT NOT NULL,
    name TEXT,
    agent_id TEXT,
    content TEXT NOT NULL,
    timestamp BIGINT NOT NULL,
    is_group_message INTEGER NOT NULL DEFAULT 0,
    group_id TEXT,
    finish_reason TEXT,
    content_hash TEXT NOT NULL DEFAULT '',
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    deleted_at BIGINT,
    PRIMARY KEY (topic_id, msg_id)
);

-- 7. render_cache 表（含 0005 content_hash / renderer_schema_version）
CREATE TABLE IF NOT EXISTS render_cache (
    topic_id TEXT NOT NULL,
    msg_id TEXT NOT NULL,
    render_content BLOB,
    updated_at BIGINT NOT NULL,
    content_hash TEXT NOT NULL DEFAULT '',
    renderer_schema_version INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (topic_id, msg_id),
    FOREIGN KEY (topic_id, msg_id) REFERENCES messages(topic_id, msg_id) ON DELETE CASCADE
);

-- 8. message_attachments 表（含 0002 deleted_at）
CREATE TABLE IF NOT EXISTS message_attachments (
    topic_id TEXT NOT NULL,
    msg_id TEXT NOT NULL,
    hash TEXT NOT NULL,
    attachment_order INTEGER NOT NULL,
    display_name TEXT NOT NULL,
    src TEXT,
    status TEXT,
    created_at BIGINT NOT NULL,
    deleted_at BIGINT,
    PRIMARY KEY (topic_id, msg_id, attachment_order),
    FOREIGN KEY (topic_id, msg_id) REFERENCES messages(topic_id, msg_id) ON DELETE CASCADE
);

-- 9. attachments 表 (物理文件真理之源)
CREATE TABLE IF NOT EXISTS attachments (
    hash TEXT PRIMARY KEY,
    mime_type TEXT NOT NULL,
    size BIGINT NOT NULL,
    internal_path TEXT NOT NULL,
    extracted_text TEXT,
    image_frames TEXT,
    thumbnail_path TEXT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

-- 10. settings 表 (存储全局配置)
CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at BIGINT NOT NULL
);

-- 11. model_favorites 表
CREATE TABLE IF NOT EXISTS model_favorites (
    model_id TEXT PRIMARY KEY,
    created_at BIGINT NOT NULL
);

-- 12. model_usage_stats 表
CREATE TABLE IF NOT EXISTS model_usage_stats (
    model_id TEXT PRIMARY KEY,
    usage_count INTEGER NOT NULL DEFAULT 0,
    updated_at BIGINT NOT NULL
);

-- 13. emoticon_library 表 (表情包修复库)
CREATE TABLE IF NOT EXISTS emoticon_library (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    category TEXT NOT NULL,
    filename TEXT NOT NULL,
    url TEXT NOT NULL UNIQUE,
    search_key TEXT NOT NULL
);

-- 14. tarven_rules 表 (VCPChatTarven 规则库)
CREATE TABLE IF NOT EXISTS tarven_rules (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    rule_type TEXT NOT NULL,
    is_enabled INTEGER NOT NULL DEFAULT 1,
    content TEXT NOT NULL,
    scope TEXT NOT NULL,
    wrap INTEGER NOT NULL DEFAULT 1,
    role TEXT,
    depth INTEGER,
    position TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

-- 15. active_generations 活跃生成注册表 (0007 终态)
CREATE TABLE IF NOT EXISTS active_generations (
    msg_id TEXT PRIMARY KEY,
    topic_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    owner_type TEXT NOT NULL,
    created_at BIGINT NOT NULL
);

-- ============ 全文索引（0008 终态：trigram，跳过 unicode61 中间态） ============

CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    msg_id UNINDEXED,
    topic_id UNINDEXED,
    content,
    tokenize = 'trigram'
);

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

-- ============ 索引（0001 的 9 个 + 0008 的 2 个） ============

CREATE INDEX IF NOT EXISTS idx_topics_owner ON topics(owner_id, owner_type, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_emoticon_category ON emoticon_library(category);
CREATE INDEX IF NOT EXISTS idx_messages_topic_time ON messages(topic_id, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_messages_updated_at ON messages(updated_at);
CREATE INDEX IF NOT EXISTS idx_group_members_agent ON group_members(agent_id);
CREATE INDEX IF NOT EXISTS idx_message_attachments_hash ON message_attachments(hash);
CREATE INDEX IF NOT EXISTS idx_message_attachments_msg ON message_attachments(topic_id, msg_id);
CREATE INDEX IF NOT EXISTS idx_render_cache_msg ON render_cache(topic_id, msg_id);
CREATE INDEX IF NOT EXISTS idx_tarven_rules_active ON tarven_rules(rule_type, is_enabled, sort_order ASC);
CREATE INDEX IF NOT EXISTS idx_messages_agent_id ON messages(agent_id);
CREATE INDEX IF NOT EXISTS idx_messages_role ON messages(role);
