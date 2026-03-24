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
    // 1. 话题索引表
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS topic_index (
            topic_id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL,
            title TEXT,
            mtime BIGINT NOT NULL,
            file_hash TEXT,
            last_msg_preview TEXT,
            msg_count INTEGER DEFAULT 0,
            locked BOOLEAN DEFAULT 0,
            unread BOOLEAN DEFAULT 0,
            unread_count INTEGER DEFAULT 0
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // Migration: Add unread_count if it doesn't exist
    let _ = sqlx::query("ALTER TABLE topic_index ADD COLUMN unread_count INTEGER DEFAULT 0")
        .execute(pool)
        .await;

    // 2. 附件索引表
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

    // 3. Agent 索引表
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

    // 4. 正则规则表
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

    Ok(())
}
