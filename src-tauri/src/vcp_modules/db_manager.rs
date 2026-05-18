use sqlx::{sqlite::SqlitePoolOptions, Pool, Sqlite};
use std::fs;
use tauri::AppHandle;
use tauri::Manager;

pub struct DbState {
    pub pool: Pool<Sqlite>,
    pub path: std::path::PathBuf,
}

pub async fn init_db(app_handle: &AppHandle) -> Result<(Pool<Sqlite>, std::path::PathBuf), String> {
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

    // 自动迁移旧版附件 (Android)
    let _ = crate::vcp_modules::file_manager::migrate_legacy_attachments(app_handle);

    // 配置连接选项
    let mut connect_options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(true);

    // 深度性能优化：
    // 1. WAL 模式：允许读写并发，极大提升 UI 相应速度
    // 2. Normal 同步：在 WAL 模式下兼顾安全性与速度
    // 3. mmap_size: 开启内存映射 I/O (256MB)，将磁盘读取变为内存访问
    // 4. temp_store: 将临时表、排序操作强制放在内存中
    // 5. page_size: 提升至 16KB，优化现代闪存 I/O 效率
    // 6. auto_vacuum: 开启增量清理逻辑，配合维护任务物理回收空间
    connect_options = connect_options
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
        .busy_timeout(std::time::Duration::from_secs(30))
        .pragma("mmap_size", "268435456")
        .pragma("temp_store", "2")
        .pragma("page_size", "16384")
        .pragma("cache_size", "-8000")
        .pragma("auto_vacuum", "2");

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(connect_options)
        .await
        .map_err(|e| format!("Connect failed: {}", e))?;

    // 运行初始化建表
    setup_tables(&pool).await?;

    // 挂载到 App State (注意：由于 init_db 返回 pool，我们需要在外部构建 DbState)
    Ok((pool, db_path))
}

async fn setup_tables(pool: &Pool<Sqlite>) -> Result<(), String> {
    // 1. avatars 全局多态头像表 (真理之源)
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS avatars (
            owner_type TEXT NOT NULL,     -- 'agent', 'group', 'user', 'system'
            owner_id TEXT NOT NULL,       -- 对应实体的 UUID 或 'user_avatar'
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
            mobile_system_prompt TEXT NOT NULL DEFAULT '',
            model TEXT NOT NULL,
            temperature REAL NOT NULL DEFAULT 1,
            context_token_limit INTEGER NOT NULL DEFAULT 0,
            max_output_tokens INTEGER NOT NULL DEFAULT 0,
            stream_output INTEGER NOT NULL DEFAULT 1,
            config_hash TEXT NOT NULL DEFAULT '', -- 配置内容指纹
            content_hash TEXT NOT NULL DEFAULT '', -- 聚合指纹 (Config + Topics)
            current_topic_id TEXT,                 -- 记录最后一次打开的话题 ID
            updated_at BIGINT NOT NULL,
            deleted_at BIGINT
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 确保字段存在 (用于存量 DB 升级)
    let _ = sqlx::query("ALTER TABLE agents ADD COLUMN current_topic_id TEXT")
        .execute(pool)
        .await;
    let _ =
        sqlx::query("ALTER TABLE agents ADD COLUMN mobile_system_prompt TEXT NOT NULL DEFAULT ''")
            .execute(pool)
            .await;

    // 3. groups 表 (群组配置)
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
            config_hash TEXT NOT NULL DEFAULT '', -- 配置内容指纹
            content_hash TEXT NOT NULL DEFAULT '', -- 聚合指纹 (Config + Topics)
            current_topic_id TEXT,                 -- 记录最后一次打开的话题 ID
            created_at BIGINT NOT NULL DEFAULT 0,
            updated_at BIGINT NOT NULL,
            deleted_at BIGINT
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 确保字段存在 (用于存量 DB 升级)
    let _ = sqlx::query("ALTER TABLE groups ADD COLUMN current_topic_id TEXT")
        .execute(pool)
        .await;

    // 4. group_members 表
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS group_members (
            group_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            member_tag TEXT,
            sort_order INTEGER NOT NULL DEFAULT 0,
            updated_at BIGINT NOT NULL,
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
            locked INTEGER NOT NULL DEFAULT 1,
            unread INTEGER NOT NULL DEFAULT 0,
            unread_count INTEGER NOT NULL DEFAULT 0,
            msg_count INTEGER NOT NULL DEFAULT 0,
            config_hash TEXT NOT NULL DEFAULT '',  -- 配置内容指纹 (Topic Meta Hash)
            content_hash TEXT NOT NULL DEFAULT '', -- 聚合指纹 (Messages Root)
            deleted_at BIGINT
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 确保字段存在 (用于存量 DB 升级)
    let _ = sqlx::query("ALTER TABLE topics ADD COLUMN config_hash TEXT NOT NULL DEFAULT ''")
        .execute(pool)
        .await;

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
            is_thinking INTEGER NOT NULL DEFAULT 0,
            is_group_message INTEGER NOT NULL DEFAULT 0,
            group_id TEXT,
            finish_reason TEXT,
            content_hash TEXT NOT NULL DEFAULT '', -- 消息内容指纹
            created_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL,
            deleted_at BIGINT
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 6.1 render_cache 表 (预渲染内容缓存 - 独立表以防止 B-Tree 溢出)
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS render_cache (
            msg_id TEXT PRIMARY KEY,
            render_content BLOB,
            updated_at BIGINT NOT NULL,
            FOREIGN KEY (msg_id) REFERENCES messages(msg_id) ON DELETE CASCADE
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 迁移：将 messages 表中的旧 render_content 移动到新表
    let has_old_render_column: bool = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name = 'render_content'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0)
        > 0;

    if has_old_render_column {
        println!("[DBManager] Migrating render_content to render_cache...");
        // 复制数据
        let _ = sqlx::query(
            "INSERT OR IGNORE INTO render_cache (msg_id, render_content, updated_at) 
             SELECT msg_id, render_content, updated_at FROM messages WHERE render_content IS NOT NULL",
        )
        .execute(pool)
        .await;

        // 移除旧列 (需要 SQLite 3.35+)
        let _ = sqlx::query("ALTER TABLE messages DROP COLUMN render_content")
            .execute(pool)
            .await;
    }

    // 6. attachments 表 (物理文件真理之源)
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS attachments (
            hash TEXT PRIMARY KEY,            -- 内容摘要 SHA-256
            mime_type TEXT NOT NULL,          -- e.g., 'image/webp'
            size BIGINT NOT NULL,             -- 文件大小
            internal_path TEXT NOT NULL,      -- 本地物理存储路径
            extracted_text TEXT,              -- OCR 或解析文本
            image_frames TEXT,                -- 视频帧或 PDF 图片 (JSON Array)
            thumbnail_path TEXT,              -- 缩略图路径
            created_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 7. message_attachments 表 (逻辑引用上下文)
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS message_attachments (
            msg_id TEXT NOT NULL,
            hash TEXT NOT NULL,               -- 指向 attachments.hash
            attachment_order INTEGER NOT NULL, -- 排序
            display_name TEXT NOT NULL,       -- 原始文件名
            src TEXT,                         -- 来源 URL
            status TEXT,                      -- 状态 (如 'removed')
            created_at BIGINT NOT NULL,
            PRIMARY KEY (msg_id, attachment_order)
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 8. settings 表 (存储全局配置)
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at BIGINT NOT NULL
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 9. model_favorites 表
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS model_favorites (
            model_id TEXT PRIMARY KEY,
            created_at BIGINT NOT NULL
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 10. model_usage_stats 表
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

    // 11. emoticon_library 表 (表情包修复库)
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS emoticon_library (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            category TEXT NOT NULL,
            filename TEXT NOT NULL,
            url TEXT NOT NULL UNIQUE,
            search_key TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 索引
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_topics_owner
         ON topics(owner_id, owner_type, created_at DESC)",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_emoticon_category
         ON emoticon_library(category)",
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
         ON message_attachments(hash)",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    Ok(())
}
