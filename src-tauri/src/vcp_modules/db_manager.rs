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
    // 1. 附件索引表
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS attachment_index (
            hash TEXT PRIMARY KEY,
            local_path TEXT NOT NULL,
            mime_type TEXT,
            size BIGINT NOT NULL,
            created_at BIGINT NOT NULL
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 2. Agent 索引表
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS agent_index (
            agent_id TEXT PRIMARY KEY,
            name TEXT,
            mtime BIGINT NOT NULL
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 3. 正则规则表
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS agent_regex_rules (
            rule_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            title TEXT,
            find_pattern TEXT NOT NULL,
            replace_with TEXT,
            apply_to_roles TEXT,
            apply_to_frontend BOOLEAN,
            apply_to_context BOOLEAN,
            min_depth INTEGER,
            max_depth INTEGER,
            PRIMARY KEY (agent_id, rule_id)
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 4. Project Leviathan: topic_state
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS topic_state (
            topic_id TEXT PRIMARY KEY,
            item_id TEXT NOT NULL,
            title TEXT,
            created_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL,
            revision INTEGER NOT NULL DEFAULT 0,
            msg_count INTEGER NOT NULL DEFAULT 0,
            locked BOOLEAN NOT NULL DEFAULT 0,
            unread BOOLEAN NOT NULL DEFAULT 0,
            unread_count INTEGER NOT NULL DEFAULT 0
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 5. Project Leviathan: message_index
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS message_index (
            msg_id TEXT PRIMARY KEY,
            topic_id TEXT NOT NULL,
            item_id TEXT NOT NULL,
            role TEXT NOT NULL,
            created_at BIGINT NOT NULL,
            raw_byte_offset INTEGER NOT NULL,
            raw_byte_length INTEGER NOT NULL,
            render_byte_offset INTEGER,
            render_byte_length INTEGER,
            has_attachments INTEGER NOT NULL DEFAULT 0,
            is_deleted INTEGER NOT NULL DEFAULT 0,
            extra_json TEXT
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_message_topic_time
         ON message_index(topic_id, created_at)",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // 7. Project Leviathan: message_attachment_ref
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS message_attachment_ref (
            msg_id TEXT NOT NULL,
            attachment_hash TEXT NOT NULL,
            attachment_order INTEGER NOT NULL,
            PRIMARY KEY (msg_id, attachment_order)
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_attachment_ref_hash
         ON message_attachment_ref(attachment_hash)",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    Ok(())
}
