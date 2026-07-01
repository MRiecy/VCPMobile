use sqlx::{sqlite::SqlitePoolOptions, Pool, Sqlite, Row};
use std::fs;
use tauri::AppHandle;
use tauri::Manager;

pub struct DbState {
    pub pool: Pool<Sqlite>,
    pub path: std::path::PathBuf,
}

impl DbState {
    /// 执行 SQLite 物理页面碎片分批回收与查询规划器索引优化
    pub async fn run_incremental_vacuum_optimize(
        &self,
        pages_to_vacuum: i32,
    ) -> Result<(), sqlx::Error> {
        // 1. 分批页整理碎片，防堵大面积 I/O 阻塞
        sqlx::query(&format!("PRAGMA incremental_vacuum({})", pages_to_vacuum))
            .execute(&self.pool)
            .await?;
        // 2. 重构索引规划器
        sqlx::query("PRAGMA optimize").execute(&self.pool).await?;
        Ok(())
    }
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

    log::info!("[DBManager] Initializing SQLite at: {:?}", db_path);

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
    // 7. foreign_keys: 开启外键约束，以支持级联删除
    connect_options = connect_options
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
        .busy_timeout(std::time::Duration::from_secs(30))
        .pragma("mmap_size", "268435456")
        .pragma("temp_store", "2")
        .pragma("page_size", "16384")
        .pragma("cache_size", "-8000")
        .pragma("auto_vacuum", "2")
        .pragma("foreign_keys", "1");

    let pool = match open_and_check_db(&connect_options, &db_path).await {
        Ok(p) => p,
        Err(e) => {
            log::warn!(
                "[DBManager] Database open/integrity check failed: {}. Attempting self-healing...",
                e
            );
            // 自愈处理：归档损坏的数据库并清空 WAL 文件
            archive_corrupt_db(&db_path);

            // 重新尝试创建并建立干净连接
            open_and_check_db(&connect_options, &db_path)
                .await
                .map_err(|err| format!("数据库损坏且重建失败: {}", err))?
        }
    };

    // 运行结构版本迁移引擎
    run_migrations(&pool).await?;

    // 运行系统内置高级规则的多模态无损同步器
    crate::vcp_modules::chat::context_injection::sync_system_preset_rules(&pool)
        .await
        .map_err(|e| format!("[DBManager] Failed to sync preset rules: {}", e))?;

    Ok((pool, db_path))
}

async fn open_and_check_db(
    connect_options: &sqlx::sqlite::SqliteConnectOptions,
    db_path: &std::path::Path,
) -> Result<Pool<Sqlite>, String> {
    let mut retry_count = 0;
    let pool = loop {
        match SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(connect_options.clone())
            .await
        {
            Ok(p) => break p,
            Err(e) => {
                retry_count += 1;
                if retry_count >= 3 {
                    return Err(format!(
                        "数据库连接重试失败 (已重试 {} 次): {}",
                        retry_count, e
                    ));
                }
                log::warn!(
                    "[DBManager] Connection failed: {}. Retrying in {}ms... (Attempt {})",
                    e,
                    retry_count * 50,
                    retry_count
                );
                tokio::time::sleep(std::time::Duration::from_millis(retry_count * 50)).await;
            }
        }
    };

    // 如果数据库文件先前已存在，则在冷启动时运行轻量化快速自检
    if db_path.exists() {
        if !check_integrity(&pool).await {
            return Err("PRAGMA quick_check(1) failed".to_string());
        }
    }

    Ok(pool)
}

async fn check_integrity(pool: &Pool<Sqlite>) -> bool {
    let check: Result<String, sqlx::Error> = sqlx::query_scalar("PRAGMA quick_check(1)")
        .fetch_one(pool)
        .await;
    match check {
        Ok(result) => result.to_lowercase() == "ok",
        Err(e) => {
            log::error!("[DBManager] Integrity quick check failed: {}", e);
            false
        }
    }
}

fn archive_corrupt_db(db_path: &std::path::Path) {
    let now = chrono::Utc::now().timestamp_millis();
    let corrupt_path = db_path.with_extension(format!("db.corrupt.{}", now));
    log::warn!(
        "[DBManager] Archiving corrupt database from {:?} to {:?}",
        db_path,
        corrupt_path
    );
    if let Err(e) = fs::rename(db_path, &corrupt_path) {
        log::error!("[DBManager] Failed to rename corrupt database file: {}", e);
    }

    // 物理清除关联的 WAL / SHM 临时缓存，防止损坏指针残留
    let wal_path = db_path.with_extension("db-wal");
    if wal_path.exists() {
        let _ = fs::remove_file(&wal_path);
    }
    let shm_path = db_path.with_extension("db-shm");
    if shm_path.exists() {
        let _ = fs::remove_file(&shm_path);
    }
}

struct Migration {
    version: i32,
    description: &'static str,
    sql: &'static str,
}

static MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        description: "create_initial_tables",
        sql: "
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

        -- 2. agents 表 (智能体配置 - 物理删除了 current_topic_id)
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

        -- 3. groups 表 (群组配置 - 物理删除了 current_topic_id)
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

        -- 6. messages 表 (消息历史 - 已物理删除 is_thinking 列)
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
            content_hash TEXT NOT NULL DEFAULT '',  -- 消息内容指纹 (用于快速 Diff 和聚合指纹计算,包含附件指纹)
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
        "
    },
    Migration {
        version: 2,
        description: "add_deleted_at_to_message_attachments",
        sql: "-- ⚡ 增量热迁移：为 message_attachments 表增加 deleted_at 逻辑删除字段
              ALTER TABLE message_attachments ADD COLUMN deleted_at BIGINT;"
    },
    Migration {
        version: 3,
        description: "create_messages_fts_table",
        sql: "CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                  msg_id UNINDEXED,
                  topic_id UNINDEXED,
                  content,
                  tokenize = 'unicode61'
              );

              CREATE TRIGGER IF NOT EXISTS after_messages_physical_delete
              AFTER DELETE ON messages
              BEGIN
                  DELETE FROM messages_fts WHERE msg_id = old.msg_id;
              END;

              CREATE TRIGGER IF NOT EXISTS after_messages_logical_delete
              AFTER UPDATE OF deleted_at ON messages
              WHEN new.deleted_at IS NOT NULL
              BEGIN
                  DELETE FROM messages_fts WHERE msg_id = new.msg_id;
              END;"
    }
];

async fn run_migrations(pool: &Pool<Sqlite>) -> Result<(), String> {
    // 1. 确保版本控制系统表 schema_migrations 存在
    let has_migration_table: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations')"
    )
    .fetch_one(pool)
    .await
    .map_err(|e| format!("Failed to check schema_migrations table: {}", e))?;

    if !has_migration_table {
        // 检测是否是无迁移表的历史老版本数据库
        let has_legacy_messages: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='messages')"
        )
        .fetch_one(pool)
        .await
        .unwrap_or(false);

        // 创建版本控制元数据表
        sqlx::query(
            "CREATE TABLE schema_migrations (
                version INTEGER PRIMARY KEY,
                description TEXT NOT NULL,
                applied_at BIGINT NOT NULL
            )"
        )
        .execute(pool)
        .await
        .map_err(|e| format!("Failed to create schema_migrations table: {}", e))?;

        if has_legacy_messages {
            log::info!("[DBManager] Legacy database detected. Reconciling schema state...");
            // 老版本可能已经通过旧有的 setup_tables 执行过 ALTER 增加了 deleted_at。
            // 我们通过检查列信息来判断应该打上哪些版本标记。
            let columns = sqlx::query("PRAGMA table_info(message_attachments)")
                .fetch_all(pool)
                .await
                .unwrap_or_default();
            
            let has_deleted_at = columns.iter().any(|row| {
                let name: String = row.get("name");
                name == "deleted_at"
            });

            let now = chrono::Utc::now().timestamp_millis();
            sqlx::query(
                "INSERT INTO schema_migrations (version, description, applied_at) VALUES (1, 'create_initial_tables', ?)"
            )
            .bind(now)
            .execute(pool)
            .await
            .map_err(|e| format!("Failed to seed legacy migration 1: {}", e))?;

            if has_deleted_at {
                sqlx::query(
                    "INSERT INTO schema_migrations (version, description, applied_at) VALUES (2, 'add_deleted_at_to_message_attachments', ?)"
                )
                .bind(now)
                .execute(pool)
                .await
                .map_err(|e| format!("Failed to seed legacy migration 2: {}", e))?;
                log::info!("[DBManager] Legacy database successfully seeded at version 2.");
            } else {
                log::info!("[DBManager] Legacy database seeded at version 1 (missing deleted_at).");
            }
        }
    }

    // 2. 获取已应用的迁移历史
    let applied_versions: std::collections::HashSet<i32> = sqlx::query_scalar(
        "SELECT version FROM schema_migrations"
    )
    .fetch_all(pool)
    .await
    .map_err(|e| format!("Failed to query applied migrations: {}", e))?
    .into_iter()
    .collect();

    // 3. 按版本顺序执行所有缺失的迁移
    for migration in MIGRATIONS {
        if !applied_versions.contains(&migration.version) {
            log::info!(
                "[DBManager] Applying migration {}: {}",
                migration.version,
                migration.description
            );

            // 开启排他性事务运行该迁移
            let mut tx = pool.begin().await.map_err(|e| format!("Failed to start migration transaction: {}", e))?;

            // SQLite 无法在一个 prepared statement 中同时处理多条以分号分隔的 SQL
            // 故在内存中安全分割后逐条执行
            for statement in migration.sql.split(';') {
                let trimmed = statement.trim();
                if !trimmed.is_empty() {
                    sqlx::query(trimmed)
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| format!("Failed to execute query in migration {}: {}", migration.version, e))?;
                }
            }

            // 记录版本日志
            let now = chrono::Utc::now().timestamp_millis();
            sqlx::query(
                "INSERT INTO schema_migrations (version, description, applied_at) VALUES (?, ?, ?)"
            )
            .bind(migration.version)
            .bind(migration.description)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("Failed to log migration {} to schema_migrations: {}", migration.version, e))?;

            tx.commit().await.map_err(|e| format!("Failed to commit migration transaction: {}", e))?;
            log::info!("[DBManager] Migration {} applied successfully.", migration.version);
        }
    }
    Ok(())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FtsSearchResult {
    pub msg_id: String,
    pub topic_id: String,
    pub role: String,
    pub content: String,
    pub timestamp: i64,
    pub topic_title: String,
}

pub fn preprocess_fts_text(text: &str) -> String {
    let mut result = String::with_capacity(text.len() * 2);
    let mut last_was_cjk = false;

    for c in text.chars() {
        let is_cjk = ('\u{4e00}'..='\u{9fff}').contains(&c)
            || ('\u{3400}'..='\u{4dbf}').contains(&c)
            || ('\u{20000}'..='\u{2a6df}').contains(&c);

        if is_cjk {
            if !result.is_empty() && !last_was_cjk && !result.ends_with(' ') {
                result.push(' ');
            }
            result.push(c);
            result.push(' ');
            last_was_cjk = true;
        } else {
            if last_was_cjk && c != ' ' && !result.ends_with(' ') {
                result.push(' ');
            }
            result.push(c);
            last_was_cjk = false;
        }
    }
    result.trim().to_string()
}

#[tauri::command]
pub async fn search_messages_fts(
    db_state: tauri::State<'_, DbState>,
    query: String,
) -> Result<Vec<FtsSearchResult>, String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    // 编译全文检索 MATCH 条件
    let processed = preprocess_fts_text(trimmed);
    let terms: Vec<String> = processed
        .split_whitespace()
        .map(|s| format!("\"{}\"", s))
        .collect();
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let fts_query = terms.join(" AND ");

    // 执行全文检索，并联查 messages 获取压缩正文，最后由 Rust 统一解压，确保格式完美
    let rows = sqlx::query(
        "SELECT 
            m.msg_id, 
            m.topic_id, 
            m.role, 
            m.content AS compressed_content, 
            m.timestamp, 
            t.title AS topic_title
         FROM messages_fts fts
         INNER JOIN messages m ON fts.msg_id = m.msg_id
         INNER JOIN topics t ON m.topic_id = t.topic_id
         WHERE fts.content MATCH ? AND m.deleted_at IS NULL AND t.deleted_at IS NULL
         ORDER BY m.timestamp DESC
         LIMIT 100"
    )
    .bind(&fts_query)
    .fetch_all(&db_state.pool)
    .await
    .map_err(|e| format!("全文检索执行失败: {}", e))?;

    let mut results = Vec::with_capacity(rows.len());
    for row in rows {
        let msg_id: String = row.get("msg_id");
        let topic_id: String = row.get("topic_id");
        let role: String = row.get("role");
        let compressed_content: Vec<u8> = row.get("compressed_content");
        let timestamp: i64 = row.get("timestamp");
        let topic_title: String = row.get("topic_title");

        // 使用 ContentCompressor 解压正文明文
        let content = crate::vcp_modules::persistence::message_repository::ContentCompressor::decompress(&compressed_content)
            .unwrap_or_else(|_| String::from_utf8_lossy(&compressed_content).to_string());

        results.push(FtsSearchResult {
            msg_id,
            topic_id,
            role,
            content,
            timestamp,
            topic_title,
        });
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preprocess_fts_text() {
        assert_eq!(preprocess_fts_text("我喜欢AI"), "我 喜 欢 AI");
        assert_eq!(preprocess_fts_text("AI智能体"), "AI 智 能 体");
        assert_eq!(preprocess_fts_text("Hello World"), "Hello World");
        assert_eq!(preprocess_fts_text(""), "");
    }
}


