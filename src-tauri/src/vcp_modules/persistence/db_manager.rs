use sqlx::{sqlite::SqlitePoolOptions, Pool, Sqlite, Row};
use std::fs;
use tauri::{AppHandle, Emitter, Manager};

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

async fn run_migrations(pool: &Pool<Sqlite>) -> Result<(), String> {
    let migrator = sqlx::migrate!("./migrations");
    // 处理 1.1.2 存量用户（有业务表但无任何迁移追踪记录）
    bootstrap_legacy_if_needed(pool, &migrator).await?;
    // sqlx 内置迁移引擎：底层用 sqlite3_exec()，原生支持触发器等多语句 DDL
    migrator
        .run(pool)
        .await
        .map_err(|e| format!("数据库初始化失败: {}", e))
}

/// 为 1.1.2 原始用户（有业务表但无迁移追踪表）构建初始迁移状态。
///
/// sqlx::migrate!() 使用 _sqlx_migrations 表追踪版本，并通过 SHA-384
/// checksum 校验每个已执行迁移的文件内容。此函数通过检查 Schema 状态推断
/// 哪些迁移已在历史上执行过，并向 _sqlx_migrations 写入带真实 checksum
/// 的虚拟记录，告知 sqlx「这些迁移已执行，跳过它们」。
///
/// Checksum 直接取自 migrator.migrations[i].checksum，这是编译期由
/// sqlx::migrate!() 宏对 .sql 文件内容计算的 SHA-384，与 sqlx 运行期
/// 校验使用的值完全一致，无需手动计算。
async fn bootstrap_legacy_if_needed(
    pool: &Pool<Sqlite>,
    migrator: &sqlx::migrate::Migrator,
) -> Result<(), String> {
    // 检测是否为 1.1.2 用户：有业务表但没有 sqlx 迁移追踪表
    let has_messages: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='messages')"
    )
    .fetch_one(pool)
    .await
    .unwrap_or(false);

    let has_sqlx_table: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations')"
    )
    .fetch_one(pool)
    .await
    .unwrap_or(false);

    // 不是遗留用户（全新安装），或桥接已完成
    if !has_messages || has_sqlx_table {
        return Ok(());
    }

    log::info!("[DBManager] Legacy 1.1.2 database detected. Bootstrapping migration state...");

    // 检测各迁移在历史上是否已执行（通过当前 Schema 状态推断）
    let columns = sqlx::query("PRAGMA table_info(message_attachments)")
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    let has_deleted_at = columns.iter().any(|row| {
        row.try_get::<String, _>("name").unwrap_or_default() == "deleted_at"
    });

    let has_fts: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='messages_fts')"
    )
    .fetch_one(pool)
    .await
    .unwrap_or(false);

    // 向 _sqlx_migrations 写入虚拟记录（migrator.run() 会自动建表后再读取）
    // 此处借用 pool 直接执行，因为 _sqlx_migrations 尚不存在，
    // 所以先让 migrator 自己建表，再插入记录。
    // 实际顺序：run() → 建表 → 读记录（发现已有记录）→ 跳过对应版本
    //
    // 由于 _sqlx_migrations 在此时不存在，我们需要手动创建它，
    // 或者利用 sqlx 提供的 ensure_migrations_table() 方法。
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations (
            version        BIGINT PRIMARY KEY,
            description    TEXT NOT NULL,
            installed_on   TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            success        BOOLEAN NOT NULL,
            checksum       BLOB NOT NULL,
            execution_time BIGINT NOT NULL
        )"
    )
    .execute(pool)
    .await
    .map_err(|e| format!("Bootstrap: failed to create _sqlx_migrations: {}", e))?;

    for migration in migrator.migrations.iter() {
        let already_applied = match migration.version {
            1 => true,           // 初始表必然存在（用户能运行说明 Migration 1 已执行）
            2 => has_deleted_at, // deleted_at 列存在则 Migration 2 已执行
            3 => has_fts,        // messages_fts 表存在则 Migration 3 已执行
            _ => false,
        };

        if already_applied {
            sqlx::query(
                "INSERT OR IGNORE INTO _sqlx_migrations
                 (version, description, installed_on, success, checksum, execution_time)
                 VALUES (?, ?, datetime('now'), 1, ?, 0)"
            )
            .bind(migration.version)
            .bind(migration.description.as_ref())
            .bind(migration.checksum.as_ref())
            .execute(pool)
            .await
            .map_err(|e| format!(
                "Bootstrap: failed to seed migration v{}: {}", migration.version, e
            ))?;

            log::info!(
                "[DBManager] Bootstrap: seeded migration v{} ({}).",
                migration.version,
                migration.description
            );
        }
    }

    log::info!("[DBManager] Legacy bootstrap complete. Handing over to sqlx migrator.");
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

    // 执行全文检索，并联查 messages 获取正文明文，确保格式完美
    let rows = sqlx::query(
        "SELECT 
            m.msg_id, 
            m.topic_id, 
            m.role, 
            m.content, 
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
        let content: String = row.get("content");
        let timestamp: i64 = row.get("timestamp");
        let topic_title: String = row.get("topic_title");

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

pub async fn decompress_database_migration(app_handle: &AppHandle) -> Result<(), String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    // 1. 检测是否含有需要升级的压缩数据
    let needs_upgrade: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM messages WHERE typeof(content) = 'blob')"
    )
    .fetch_one(pool)
    .await
    .unwrap_or(false);

    if !needs_upgrade {
        return Ok(());
    }

    log::info!("[DBManager] Compressed messages detected in database. Intercepting bootstrap for decompression migration...");

    // 2. 查询待解压总条数
    let total_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE typeof(content) = 'blob'"
    )
    .fetch_one(pool)
    .await
    .map_err(|e| format!("Failed to query compressed messages count: {}", e))?;

    if total_count == 0 {
        return Ok(());
    }

    log::info!("[DBManager] Decompressing {} messages in background...", total_count);

    // 发射初始进度
    let _ = app_handle.emit(
        "vcp-system-event",
        serde_json::json!({
            "type": "vcp-core-status",
            "status": "decompressing",
            "message": "正在准备解压历史消息... 0%",
            "source": "Core"
        }),
    );

    // 3. 分批解压并写回
    let mut processed_count = 0;
    let batch_size = 200;

    loop {
        // 读取未解压的批次
        let rows = sqlx::query(
            "SELECT msg_id, topic_id, content FROM messages WHERE typeof(content) = 'blob' ORDER BY rowid LIMIT ?"
        )
        .bind(batch_size)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("Failed to fetch compressed batch: {}", e))?;

        if rows.is_empty() {
            break;
        }

        // 开启本批次事务
        let mut tx = pool.begin().await.map_err(|e| format!("Failed to start batch transaction: {}", e))?;

        for row in &rows {
            let msg_id: String = row.get("msg_id");
            let topic_id: String = row.get("topic_id");
            let content_bytes: Vec<u8> = row.get("content");

            // 解压缩旧的二进制数据
            let content = crate::vcp_modules::persistence::message_repository::ContentCompressor::decompress(&content_bytes)
                .unwrap_or_else(|_| String::from_utf8_lossy(&content_bytes).to_string());

            // 1. 更新写回为明文 String (SQLite 动态类型会自动将 typeof 转为 'text')
            sqlx::query("UPDATE messages SET content = ? WHERE msg_id = ? AND topic_id = ?")
                .bind(&content)
                .bind(&msg_id)
                .bind(&topic_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| format!("Failed to update decompressed message: {}", e))?;

            // 2. 同步写入 FTS5 虚拟索引表，解决历史数据无法检索的逻辑漏洞
            let search_content = preprocess_fts_text(&content);
            sqlx::query("DELETE FROM messages_fts WHERE msg_id = ?")
                .bind(&msg_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| format!("Failed to delete stale FTS entry: {}", e))?;

            sqlx::query("INSERT INTO messages_fts (msg_id, topic_id, content) VALUES (?, ?, ?)")
                .bind(&msg_id)
                .bind(&topic_id)
                .bind(&search_content)
                .execute(&mut *tx)
                .await
                .map_err(|e| format!("Failed to insert FTS entry: {}", e))?;
        }

        tx.commit().await.map_err(|e| format!("Failed to commit batch transaction: {}", e))?;

        processed_count += rows.len();

        // 4. 定期发射 progress 信号
        let pct = (processed_count * 100) / (total_count as usize);
        log::info!("[DBManager] Decompression progress: {}% ({}/{})", pct, processed_count, total_count);

        let _ = app_handle.emit(
            "vcp-system-event",
            serde_json::json!({
                "type": "vcp-core-status",
                "status": "decompressing",
                "message": format!("正在重构本地数据库... {}%", pct),
                "source": "Core"
            }),
        );
    }

    // 5. 迁移完成后执行 VACUUM 释放物理空间
    log::info!("[DBManager] Decompression complete. Reclaiming database disk space via VACUUM...");
    let _ = app_handle.emit(
        "vcp-system-event",
        serde_json::json!({
            "type": "vcp-core-status",
            "status": "decompressing",
            "message": "正在优化数据库存储空间...",
            "source": "Core"
        }),
    );
    sqlx::query("VACUUM").execute(pool).await.unwrap_or_default();

    // 6. 发射升级完成信号，等待重启
    log::info!("[DBManager] Database migration completed successfully. Waiting for user restart confirmation...");
    let _ = app_handle.emit(
        "vcp-system-event",
        serde_json::json!({
            "type": "vcp-core-status",
            "status": "decompression-complete",
            "message": "本地数据库格式重构成功，请确认重启应用。",
            "source": "Core"
        }),
    );

    // 7. 进入无限睡眠状态，阻塞后续初始化进程，避免业务逻辑调用报错
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
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


