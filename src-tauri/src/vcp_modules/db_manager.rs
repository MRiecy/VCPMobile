use sqlx::{sqlite::SqlitePoolOptions, Pool, Sqlite};
use std::fs;
use tauri::AppHandle;
use tauri::Manager;

pub struct DbState {
    pub pool: Pool<Sqlite>,
}

pub async fn init_db(app_handle: &AppHandle) -> Result<Pool<Sqlite>, String> {
    // 获取应用配置目录 (Android 下通常为 /data/user/0/com.vcp.avatar/files)
    let config_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| format!("Config dir failed: {}", e))?;

    // 确保父目录存在
    if !config_dir.exists() {
        fs::create_dir_all(&config_dir).map_err(|e| format!("Create dir failed: {}", e))?;
    }

    let mut db_path = config_dir.clone();
    db_path.push("vcp_avatar.db");

    println!("[DBManager] Initializing SQLite at: {:?}", db_path);

    // 配置连接选项
    let mut connect_options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(true);

    // 性能优化：禁用同步以减少磁盘 IO 压力 (适合移动端)
    connect_options = connect_options.journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(connect_options)
        .await
        .map_err(|e| format!("Connect failed: {}", e))?;

    // 运行初始化建表
    setup_tables(&pool).await?;

    Ok(pool)
}

async fn setup_tables(pool: &Pool<Sqlite>) -> Result<(), String> {
    // 1. avatars 全局多态头像表 (真理之源)
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS avatars (
            owner_type TEXT NOT NULL,     -- 'agent', 'group', 'user', 'system'
            owner_id TEXT NOT NULL,       -- 对应实体的 UUID 或 'default_user'
            avatar_hash TEXT NOT NULL,    -- SHA-256 摘要，用于 WS 快速 Diff
            mime_type TEXT NOT NULL,      -- e.g., 'image/webp', 'image/png'
            image_data BLOB NOT NULL,     -- 物理二进制数据
            dominant_color TEXT,          -- 预计算的主色调 (rgb/hex)
            updated_at BIGINT NOT NULL,   -- 逻辑时钟/时间戳
            PRIMARY KEY (owner_type, owner_id)
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 2. agents 表 (智能体配置)
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS agents (
            agent_id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            system_prompt TEXT NOT NULL DEFAULT '',
            model TEXT NOT NULL,
            temperature REAL NOT NULL DEFAULT 1,
            context_token_limit INTEGER NOT NULL DEFAULT 0,
            max_output_tokens INTEGER NOT NULL DEFAULT 0,
            extra_json TEXT,
            created_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL,
            deleted_at BIGINT
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;
    // 3. groups 表 (群组配置 - 已移除冗余头像字段)
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS groups (
            group_id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            mode TEXT NOT NULL DEFAULT 'sequential',
            group_prompt TEXT,
            invite_prompt TEXT,
            use_unified_model INTEGER NOT NULL DEFAULT 0,
            unified_model TEXT,
            tag_match_mode TEXT,
            extra_json TEXT,
            created_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL,
            deleted_at BIGINT
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 4. group_members 表
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS group_members (
            group_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            member_tag TEXT,
            sort_order INTEGER NOT NULL DEFAULT 0,
            created_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL,
            deleted_at BIGINT,
            PRIMARY KEY (group_id, agent_id)
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 5. topics 表
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS topics (
            topic_id TEXT PRIMARY KEY,
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            title TEXT NOT NULL,
            created_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL,
            locked INTEGER NOT NULL DEFAULT 0,
            unread INTEGER NOT NULL DEFAULT 0,
            unread_count INTEGER NOT NULL DEFAULT 0,
            msg_count INTEGER NOT NULL DEFAULT 0,
            creator_source TEXT,
            revision INTEGER NOT NULL DEFAULT 0,
            extra_json TEXT,
            deleted_at BIGINT
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 6. messages 表 (消息历史 - 已移除冗余 avatar_url 和 avatar_color)
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS messages (
            msg_id TEXT PRIMARY KEY,
            topic_id TEXT NOT NULL,
            role TEXT NOT NULL,
            name TEXT,
            agent_id TEXT,
            content TEXT NOT NULL,
            timestamp BIGINT NOT NULL,
            is_thinking INTEGER,
            is_group_message INTEGER NOT NULL DEFAULT 0,
            group_id TEXT,
            render_format TEXT,
            render_content BLOB,
            render_version INTEGER NOT NULL DEFAULT 1,
            extra_json TEXT,
            created_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL,
            deleted_at BIGINT
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 6. attachments 表 (替代 attachment_index)
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS attachments (
            attachment_hash TEXT PRIMARY KEY,
            attachment_id TEXT NOT NULL,
            name TEXT,
            internal_file_name TEXT,
            local_path TEXT NOT NULL,
            src TEXT,
            mime_type TEXT,
            size BIGINT NOT NULL,
            extracted_text TEXT,
            thumbnail_path TEXT,
            image_frames TEXT,
            extra_json TEXT,
            created_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL,
            deleted_at BIGINT
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 7. message_attachments 表 (更名自 message_attachment_ref)
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS message_attachments (
            msg_id TEXT NOT NULL,
            attachment_hash TEXT NOT NULL,
            attachment_order INTEGER NOT NULL,
            extra_json TEXT,
            created_at BIGINT NOT NULL,
            PRIMARY KEY (msg_id, attachment_order)
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 8. agent_regex_rules 表 (正式业务化)
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS agent_regex_rules (
            rule_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            title TEXT,
            find_pattern TEXT NOT NULL,
            replace_with TEXT,
            apply_to_roles TEXT,
            apply_to_frontend INTEGER NOT NULL DEFAULT 1,
            apply_to_context INTEGER NOT NULL DEFAULT 1,
            min_depth INTEGER,
            max_depth INTEGER,
            sort_order INTEGER NOT NULL DEFAULT 0,
            extra_json TEXT,
            created_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL,
            deleted_at BIGINT,
            PRIMARY KEY (agent_id, rule_id)
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 9. app_settings 表 (替代 settings.json)
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS app_settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at BIGINT NOT NULL
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 10. model_favorites 表 (替代 model_favorites.json)
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS model_favorites (
            model_id TEXT PRIMARY KEY,
            created_at BIGINT NOT NULL
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 11. model_usage_stats 表 (替代 model_usage_stats.json)
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS model_usage_stats (
            model_id TEXT PRIMARY KEY,
            usage_count INTEGER NOT NULL DEFAULT 0,
            updated_at BIGINT NOT NULL
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 索引
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_topics_owner
         ON topics(owner_type, owner_id, updated_at DESC)",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_messages_topic_time
         ON messages(topic_id, timestamp DESC)",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_messages_updated_at
         ON messages(updated_at)",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_group_members_agent
         ON group_members(agent_id)",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_message_attachments_hash
         ON message_attachments(attachment_hash)",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 删除旧表 (如果存在)
    let drop_tables = vec![
        "DROP TABLE IF EXISTS agent_index",
        "DROP TABLE IF EXISTS topic_state",
        "DROP TABLE IF EXISTS message_index",
        "DROP TABLE IF EXISTS attachment_index",
        "DROP TABLE IF EXISTS message_attachment_ref",
    ];

    for drop_query in drop_tables {
        sqlx::query(drop_query)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}
