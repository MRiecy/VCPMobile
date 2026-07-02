-- Migration 0001: 初始化全部业务表与索引
-- 15 张表 + 9 条索引

-- 1. avatars 全局多态头像表 (真理之源)
CREATE TABLE IF NOT EXISTS avatars (
    owner_type TEXT NOT NULL,     -- 'agent', 'group', 'user', 'system'
    owner_id TEXT NOT NULL,       -- 对应实体的 UUID 或 'user_avatar'
    avatar_hash TEXT NOT NULL,    -- SHA-256 摘要，用于 WS 快速 Diff
    mime_type TEXT NOT NULL,      -- e.g., 'image/webp', 'image/png'
    image_data BLOB NOT NULL,     -- 物理二进制数据
    dominant_color TEXT,          -- 预计算的主色调 (rgb/hex)
    updated_at BIGINT NOT NULL,   -- 逻辑时钟/时间戳
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
    config_hash TEXT NOT NULL DEFAULT '',  -- 配置内容指纹
    content_hash TEXT NOT NULL DEFAULT '', -- 聚合指纹 (Config + Topics)
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
    config_hash TEXT NOT NULL DEFAULT '',  -- 配置内容指纹
    content_hash TEXT NOT NULL DEFAULT '', -- 聚合指纹 (Config + Topics)
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
    config_hash TEXT NOT NULL DEFAULT '',  -- 配置内容指纹 (Topic Meta Hash)
    content_hash TEXT NOT NULL DEFAULT '', -- 聚合指纹 (Messages Root)
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
    content_hash TEXT NOT NULL DEFAULT '',  -- 消息内容指纹
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    deleted_at BIGINT,
    PRIMARY KEY (topic_id, msg_id)
);

-- 7. render_cache 表
CREATE TABLE IF NOT EXISTS render_cache (
    topic_id TEXT NOT NULL,
    msg_id TEXT NOT NULL,
    render_content BLOB,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (topic_id, msg_id),
    FOREIGN KEY (topic_id, msg_id) REFERENCES messages(topic_id, msg_id) ON DELETE CASCADE
);

-- 8. message_attachments 表
CREATE TABLE IF NOT EXISTS message_attachments (
    topic_id TEXT NOT NULL,
    msg_id TEXT NOT NULL,
    hash TEXT NOT NULL,
    attachment_order INTEGER NOT NULL,
    display_name TEXT NOT NULL,
    src TEXT,
    status TEXT,
    created_at BIGINT NOT NULL,
    PRIMARY KEY (topic_id, msg_id, attachment_order),
    FOREIGN KEY (topic_id, msg_id) REFERENCES messages(topic_id, msg_id) ON DELETE CASCADE
);

-- 9. attachments 表 (物理文件真理之源)
CREATE TABLE IF NOT EXISTS attachments (
    hash TEXT PRIMARY KEY,            -- 内容摘要 SHA-256
    mime_type TEXT NOT NULL,          -- e.g., 'image/webp'
    size BIGINT NOT NULL,             -- 文件大小
    internal_path TEXT NOT NULL,      -- 本地物理存储路径
    extracted_text TEXT,              -- OCR 或解析文本
    image_frames TEXT,                -- 视频帧或 PDF 图片 (JSON Array)
    thumbnail_path TEXT,              -- 缩略图路径
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

-- 15. active_generations 活跃生成注册表 (用于云端无状态断点恢复的事务日志)
CREATE TABLE IF NOT EXISTS active_generations (
    msg_id TEXT PRIMARY KEY,
    topic_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    owner_type TEXT NOT NULL,
    created_at BIGINT NOT NULL
);

-- 索引 (共 9 个)
CREATE INDEX IF NOT EXISTS idx_topics_owner ON topics(owner_id, owner_type, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_emoticon_category ON emoticon_library(category);
CREATE INDEX IF NOT EXISTS idx_messages_topic_time ON messages(topic_id, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_messages_updated_at ON messages(updated_at);
CREATE INDEX IF NOT EXISTS idx_group_members_agent ON group_members(agent_id);
CREATE INDEX IF NOT EXISTS idx_message_attachments_hash ON message_attachments(hash);
CREATE INDEX IF NOT EXISTS idx_message_attachments_msg ON message_attachments(topic_id, msg_id);
CREATE INDEX IF NOT EXISTS idx_render_cache_msg ON render_cache(topic_id, msg_id);
CREATE INDEX IF NOT EXISTS idx_tarven_rules_active ON tarven_rules(rule_type, is_enabled, sort_order ASC);
