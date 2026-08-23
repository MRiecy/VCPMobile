use sqlx::{sqlite::SqlitePoolOptions, Pool, Row, Sqlite};
use std::fs;
use std::path::{Path, PathBuf};
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

    validate_sqlite_file_set(&db_path)?;

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
        Err(DbOpenFailure::Unavailable(e)) => {
            return Err(format!("数据库暂不可用，已保留原文件且不会自动重建: {}", e));
        }
        Err(DbOpenFailure::ConfirmedCorruption { reason, pool }) => {
            log::warn!(
                "[DBManager] Confirmed database corruption: {}. Preserving recovery unit...",
                reason
            );

            if let Some(pool) = pool {
                pool.close().await;
            }

            let archive_path = archive_corrupt_db(&db_path).await?;
            let recovery_message = format!(
                "检测到数据库损坏，原 DB/WAL/SHM 已完整归档至 {}，正在创建干净数据库",
                archive_path.display()
            );
            log::error!("[DBManager] {}", recovery_message);
            let recovered_at = chrono::Utc::now().timestamp_millis();
            let recovery_notice =
                crate::vcp_modules::infra::lifecycle_state::DatabaseRecoveryNotice {
                    message: recovery_message.clone(),
                    archive_path: archive_path.to_string_lossy().into_owned(),
                    recovered_at,
                };
            {
                let lifecycle = app_handle
                    .state::<crate::vcp_modules::infra::lifecycle_state::LifecycleState>(
                );
                *lifecycle.database_recovery.write().await = Some(recovery_notice.clone());
            }
            let _ = app_handle.emit(
                "vcp-system-event",
                serde_json::json!({
                    "type": "vcp-database-recovery",
                    "status": "warning",
                    "message": recovery_message,
                    "archivePath": recovery_notice.archive_path,
                    "recoveredAt": recovered_at,
                    "source": "Core"
                }),
            );

            // 重新尝试创建并建立干净连接
            match open_and_check_db(&connect_options, &db_path).await {
                Ok(pool) => pool,
                Err(err) => {
                    return Err(format!(
                        "数据库已归档至 {}，但干净数据库创建失败: {}",
                        archive_path.display(),
                        err.message()
                    ));
                }
            }
        }
    };

    // 运行结构版本迁移引擎
    run_migrations(&pool).await?;

    // 检测 page_size 是否需要物理升级至 16KB 闪存友好对齐
    let page_size: i32 = sqlx::query_scalar("PRAGMA page_size")
        .fetch_one(&pool)
        .await
        .unwrap_or(4096);

    let pool = if page_size != 16384 {
        log::info!(
            "[DBManager] Legacy page_size {} detected. Running page size VACUUM optimization...",
            page_size
        );

        let lifecycle =
            app_handle.state::<crate::vcp_modules::infra::lifecycle_state::LifecycleState>();
        {
            let mut status_lock = lifecycle.status.write().await;
            *status_lock = crate::vcp_modules::infra::lifecycle_state::CoreStatus::Optimizing;
            let mut msg_lock = lifecycle.status_message.write().await;
            *msg_lock = "正在优化数据库存储以提高运行效率...".to_string();
        }

        let _ = app_handle.emit(
            "vcp-system-event",
            serde_json::json!({
                "type": "vcp-core-status",
                "status": "optimizing",
                "message": "正在优化数据库存储以提高运行效率...",
                "source": "Core"
            }),
        );

        // SQLite 在 WAL 模式下不允许变更 page_size。
        // 我们必须彻底关闭当前 pool 释放所有锁，然后使用单连接临时切换出 WAL 模式并执行 VACUUM。
        pool.close().await;

        let temp_options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Delete);

        use sqlx::Connection;
        match sqlx::sqlite::SqliteConnection::connect_with(&temp_options).await {
            Ok(mut temp_conn) => {
                let _ = sqlx::query("PRAGMA page_size = 16384")
                    .execute(&mut temp_conn)
                    .await;
                if let Err(e) = sqlx::query("VACUUM").execute(&mut temp_conn).await {
                    log::error!("[DBManager] Page size VACUUM optimization failed: {}", e);
                } else {
                    log::info!("[DBManager] Page size successfully upgraded to 16KB.");
                }
                let _ = temp_conn.close().await;
            }
            Err(e) => {
                log::error!(
                    "[DBManager] Failed to open temp connection for page size optimization: {}",
                    e
                );
            }
        }

        // 重新打开正常的 WAL 连接池并接管连接
        let pool = match open_and_check_db(&connect_options, &db_path).await {
            Ok(p) => p,
            Err(err) => return Err(format!("重建连接池失败: {}", err.message())),
        };

        // 整理完成后重置回 Initializing 状态以继续引导
        {
            let mut status_lock = lifecycle.status.write().await;
            *status_lock = crate::vcp_modules::infra::lifecycle_state::CoreStatus::Initializing;
        }
        pool
    } else {
        pool
    };

    // 运行系统内置高级规则的多模态无损同步器
    crate::vcp_modules::chat::context_injection::sync_system_preset_rules(&pool)
        .await
        .map_err(|e| format!("[DBManager] Failed to sync preset rules: {}", e))?;

    Ok((pool, db_path))
}

async fn open_and_check_db(
    connect_options: &sqlx::sqlite::SqliteConnectOptions,
    db_path: &std::path::Path,
) -> Result<Pool<Sqlite>, DbOpenFailure> {
    let mut retry_count = 0;
    let pool = loop {
        match SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(connect_options.clone())
            .await
        {
            Ok(p) => break p,
            Err(e) => {
                if is_confirmed_sqlite_corruption(&e) {
                    return Err(DbOpenFailure::ConfirmedCorruption {
                        reason: e.to_string(),
                        pool: None,
                    });
                }
                retry_count += 1;
                if retry_count >= 3 {
                    return Err(DbOpenFailure::Unavailable(format!(
                        "数据库连接重试失败 (已重试 {} 次): {}",
                        retry_count, e
                    )));
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

    // 冷启动时运行轻量化快速自检；SQL 执行错误与明确损坏必须分流。
    if db_path.exists() {
        match check_integrity(&pool).await {
            Ok(()) => {}
            Err(IntegrityFailure::Confirmed(reason)) => {
                return Err(DbOpenFailure::ConfirmedCorruption {
                    reason,
                    pool: Some(pool),
                });
            }
            Err(IntegrityFailure::Unavailable(reason)) => {
                pool.close().await;
                return Err(DbOpenFailure::Unavailable(reason));
            }
        }
    }

    Ok(pool)
}

enum DbOpenFailure {
    Unavailable(String),
    ConfirmedCorruption {
        reason: String,
        pool: Option<Pool<Sqlite>>,
    },
}

impl DbOpenFailure {
    fn message(&self) -> &str {
        match self {
            Self::Unavailable(message) => message,
            Self::ConfirmedCorruption { reason, .. } => reason,
        }
    }
}

enum IntegrityFailure {
    Unavailable(String),
    Confirmed(String),
}

async fn check_integrity(pool: &Pool<Sqlite>) -> Result<(), IntegrityFailure> {
    let check: Result<String, sqlx::Error> = sqlx::query_scalar("PRAGMA quick_check(1)")
        .fetch_one(pool)
        .await;
    match check {
        Ok(result) if result.trim().eq_ignore_ascii_case("ok") => Ok(()),
        Ok(result) => Err(IntegrityFailure::Confirmed(format!(
            "PRAGMA quick_check(1) reported corruption: {}",
            result
        ))),
        Err(e) => {
            log::error!("[DBManager] Integrity quick check failed: {}", e);
            if is_confirmed_sqlite_corruption(&e) {
                Err(IntegrityFailure::Confirmed(e.to_string()))
            } else {
                Err(IntegrityFailure::Unavailable(format!(
                    "PRAGMA quick_check(1) could not run: {}",
                    e
                )))
            }
        }
    }
}

fn is_confirmed_sqlite_corruption(error: &sqlx::Error) -> bool {
    let Some(database_error) = error.as_database_error() else {
        return false;
    };
    let Some(code) = database_error
        .code()
        .and_then(|code| code.parse::<i32>().ok())
    else {
        return false;
    };

    // SQLite extended result codes keep the primary code in the low byte.
    // SQLITE_CORRUPT=11 and SQLITE_NOTADB=26 are the only open/check errors
    // that authorize destructive-path recovery. BUSY/LOCKED/IOERR remain
    // unavailable errors and must never cause a fresh database to be created.
    is_confirmed_sqlite_corruption_code(code)
}

fn is_confirmed_sqlite_corruption_code(code: i32) -> bool {
    matches!(code & 0xff, 11 | 26)
}

fn sqlite_sidecar_path(db_path: &Path, suffix: &str) -> PathBuf {
    let mut path = db_path.as_os_str().to_os_string();
    path.push(suffix);
    PathBuf::from(path)
}

fn validate_sqlite_file_set(db_path: &Path) -> Result<(), String> {
    if db_path.exists() {
        return Ok(());
    }

    let wal_path = sqlite_sidecar_path(db_path, "-wal");
    let shm_path = sqlite_sidecar_path(db_path, "-shm");
    if wal_path.exists() || shm_path.exists() {
        return Err(format!(
            "数据库主文件缺失，但检测到未合并的 WAL/SHM；为避免覆盖已提交数据，已停止自动建库。请保全并人工恢复: {}, {}, {}",
            db_path.display(),
            wal_path.display(),
            shm_path.display()
        ));
    }

    Ok(())
}

async fn archive_corrupt_db(db_path: &Path) -> Result<PathBuf, String> {
    if !db_path.exists() {
        return Err(format!(
            "已确认数据库损坏，但主数据库文件不存在: {}",
            db_path.display()
        ));
    }

    let parent = db_path
        .parent()
        .ok_or_else(|| format!("数据库路径没有父目录: {}", db_path.display()))?;
    let file_name = db_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("数据库文件名无效: {}", db_path.display()))?;
    let archive_path = parent.join(format!(
        "{}.recovery.{}-{}",
        file_name,
        chrono::Utc::now().timestamp_millis(),
        uuid::Uuid::new_v4().simple()
    ));

    log::warn!(
        "[DBManager] Archiving SQLite recovery unit from {:?} to {:?}",
        db_path,
        archive_path
    );

    tokio::fs::create_dir(&archive_path)
        .await
        .map_err(|e| format!("创建数据库恢复目录失败: {}", e))?;

    let wal_path = sqlite_sidecar_path(db_path, "-wal");
    let shm_path = sqlite_sidecar_path(db_path, "-shm");
    let sources = [db_path.to_path_buf(), wal_path, shm_path];
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();

    for source in sources.into_iter().filter(|path| path.exists()) {
        let destination = archive_path.join(
            source
                .file_name()
                .ok_or_else(|| format!("恢复单元文件名无效: {}", source.display()))?,
        );
        if let Err(error) = tokio::fs::rename(&source, &destination).await {
            let mut rollback_errors = Vec::new();
            for (original, archived) in moved.iter().rev() {
                if let Err(rollback_error) = tokio::fs::rename(archived, original).await {
                    rollback_errors.push(format!(
                        "{} -> {}: {}",
                        archived.display(),
                        original.display(),
                        rollback_error
                    ));
                }
            }
            let rollback_detail = if rollback_errors.is_empty() {
                "已回滚此前移动".to_string()
            } else {
                format!("回滚失败: {}", rollback_errors.join("; "))
            };
            return Err(format!(
                "归档恢复单元失败 {} -> {}: {}; {}",
                source.display(),
                destination.display(),
                error,
                rollback_detail
            ));
        }
        moved.push((source, destination));
    }

    if moved.is_empty() {
        return Err(format!("数据库恢复单元为空: {}", db_path.display()));
    }

    Ok(archive_path)
}

/// Baseline 版本号：全新安装快速路径只执行该版本文件（终态 schema 存档点）。
const BASELINE_VERSION: i64 = 100;
/// baseline 创建时增量链的最大版本号。区间 (BASELINE_INCREMENTAL_MAX, BASELINE_VERSION)
/// 内严禁出现新迁移（会被全新安装快速路径 seed 跳过），由单元测试钉死。
const BASELINE_INCREMENTAL_MAX: i64 = 8;

async fn run_migrations(pool: &Pool<Sqlite>) -> Result<(), String> {
    let migrator = sqlx::migrate!("./migrations");
    if is_fresh_install(pool).await? {
        // 全新安装：直接执行 baseline 终态 schema，seed 全部既有迁移记录，
        // 跳过 0001→0008 增量链（永不经过 unicode61 FTS 中间态）
        bootstrap_fresh_install(pool, &migrator).await?;
    } else {
        ensure_current_baseline(pool).await?;
    }
    // sqlx 内置迁移引擎：底层用 sqlite3_exec()，原生支持触发器等多语句 DDL
    migrator
        .run(pool)
        .await
        .map_err(|e| format!("数据库初始化失败: {}", e))
}

/// 全新安装判定：无 messages 业务表且无任何已应用迁移记录。
/// 若 _sqlx_migrations 有记录但 messages 缺失（极端半迁移状态），
/// 交给 migrator 正常续跑，不走快速路径。
async fn is_fresh_install(pool: &Pool<Sqlite>) -> Result<bool, String> {
    let has_messages: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='messages')",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| format!("Fresh check: failed to inspect messages table: {e}"))?;
    if has_messages {
        return Ok(false);
    }
    let has_tracking: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations')",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| format!("Fresh check: failed to inspect tracking table: {e}"))?;
    if !has_tracking {
        return Ok(true);
    }
    let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(pool)
        .await
        .map_err(|e| format!("Fresh check: failed to count applied migrations: {e}"))?;
    Ok(applied == 0)
}

/// 0100 已进行不兼容改写。非空数据库必须已经记录 baseline，随后由 sqlx
/// checksum 判断其是否属于当前 schema；不再为无追踪记录的旧库伪造迁移状态。
async fn ensure_current_baseline(pool: &Pool<Sqlite>) -> Result<(), String> {
    let has_tracking: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations')",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| format!("Baseline check: failed to inspect tracking table: {e}"))?;
    if !has_tracking {
        return Err("数据库版本不兼容：本版本不迁移旧数据库，请清除应用数据或重装".to_string());
    }

    let has_baseline: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM _sqlx_migrations WHERE version = ? AND success = 1)",
    )
    .bind(BASELINE_VERSION)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("Baseline check: failed to inspect migration record: {e}"))?;
    if !has_baseline {
        return Err("数据库版本不兼容：本版本不迁移旧数据库，请清除应用数据或重装".to_string());
    }
    Ok(())
}

/// 全新安装快速路径：直接执行 baseline 文件，并将「baseline 创建时已存在」的
/// 全部迁移 seed 为已应用。
///
/// Seed 规则（铁律）：只覆盖 version <= BASELINE_INCREMENTAL_MAX 的增量迁移
/// 和 baseline 自身；baseline 之后新增的迁移（版本号均 > BASELINE_VERSION）
/// 不在 seed 范围内，由 migrator 正常排队执行。
async fn bootstrap_fresh_install(
    pool: &Pool<Sqlite>,
    migrator: &sqlx::migrate::Migrator,
) -> Result<(), String> {
    let baseline = migrator
        .migrations
        .iter()
        .find(|m| m.version == BASELINE_VERSION)
        .ok_or_else(|| {
            format!(
                "Fresh bootstrap: baseline migration v{} not found in ./migrations",
                BASELINE_VERSION
            )
        })?;

    log::info!(
        "[DBManager] Fresh install detected. Running baseline v{} (full schema snapshot)...",
        BASELINE_VERSION
    );

    // 整份执行 baseline（sqlx::raw_sql 支持多语句 DDL）
    sqlx::raw_sql(baseline.sql.as_ref())
        .execute(pool)
        .await
        .map_err(|e| {
            format!(
                "Fresh bootstrap: baseline v{} execution failed: {e}",
                BASELINE_VERSION
            )
        })?;

    // 单事务内 seed 迁移记录（原子、可重试），checksum 与 sqlx 运行期校验同源
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| format!("Fresh bootstrap: failed to begin transaction: {e}"))?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations (
            version        BIGINT PRIMARY KEY,
            description    TEXT NOT NULL,
            installed_on   TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            success        BOOLEAN NOT NULL,
            checksum       BLOB NOT NULL,
            execution_time BIGINT NOT NULL
        )",
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| format!("Fresh bootstrap: failed to create _sqlx_migrations: {e}"))?;

    for migration in migrator.migrations.iter() {
        let covered =
            migration.version <= BASELINE_INCREMENTAL_MAX || migration.version == BASELINE_VERSION;
        if !covered {
            continue;
        }
        sqlx::query(
            "INSERT OR IGNORE INTO _sqlx_migrations
             (version, description, installed_on, success, checksum, execution_time)
             VALUES (?, ?, datetime('now'), 1, ?, 0)",
        )
        .bind(migration.version)
        .bind(migration.description.as_ref())
        .bind(migration.checksum.as_ref())
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            format!(
                "Fresh bootstrap: failed to seed migration v{}: {}",
                migration.version, e
            )
        })?;
    }
    tx.commit()
        .await
        .map_err(|e| format!("Fresh bootstrap: failed to commit seed records: {e}"))?;

    log::info!(
        "[DBManager] Fresh bootstrap complete: baseline v{} applied, {} migrations seeded.",
        BASELINE_VERSION,
        migrator
            .migrations
            .iter()
            .filter(|m| m.version <= BASELINE_INCREMENTAL_MAX || m.version == BASELINE_VERSION)
            .count()
    );
    Ok(())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FtsSearchResult {
    pub msg_id: String,
    pub topic_id: String,
    pub role: String,
    pub timestamp: i64,
    pub topic_title: String,
    pub owner_id: String,
    pub owner_type: String,
    /// FTS5 snippet() 生成的命中摘要（含 <mark> 高亮标记），非完整正文
    pub snippet: String,
}

/// 构造 FTS5 MATCH 查询串：每个词转义后包成双引号短语，AND 连接。
/// trigram 分词器（migration 0008 起）下短语即"保序子串"语义，存原文直接匹配；
/// 双引号加倍转义防 MATCH 语法注入。
pub fn build_fts_match_query(terms: &[String]) -> Option<String> {
    let quoted: Vec<String> = terms
        .iter()
        .map(|t| t.replace('"', "\"\""))
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"", t))
        .collect();
    if quoted.is_empty() {
        None
    } else {
        Some(quoted.join(" AND "))
    }
}

/// trigram 分词器的硬性边界：MATCH 模式少于 3 个字符时**静默返回零结果**
///（无全表扫描兜底，已在 bundled SQLite 上实测验证）。
/// 中文双字词（部署/模型/天气…）是搜索高频词，必须绕开 FTS 走 instr() 子串匹配。
const TRIGRAM_MIN_TERM_CHARS: usize = 3;

/// 按词长分流：(走 FTS 的长词, 走 instr() 子串匹配的短词)
pub fn split_trigram_terms(input: &str) -> (Vec<String>, Vec<String>) {
    let mut long_terms = Vec::new();
    let mut short_terms = Vec::new();
    for term in input.split_whitespace() {
        if term.chars().count() >= TRIGRAM_MIN_TERM_CHARS {
            long_terms.push(term.to_string());
        } else if !term.is_empty() {
            short_terms.push(term.to_string());
        }
    }
    (long_terms, short_terms)
}

/// 纯短词查询的本地 snippet：以首个命中词为中心截取窗口，全部命中包 <mark>。
/// 匹配语义与 SQL instr() 一致（区分大小写；FTS 路径的大小写折叠由 snippet() 自理）。
pub fn build_local_snippet(content: &str, terms: &[String], window_chars: usize) -> String {
    let valid: Vec<&String> = terms.iter().filter(|t| !t.is_empty()).collect();
    let first_hit = valid.iter().filter_map(|t| content.find(t.as_str())).min();
    let total_chars = content.chars().count();
    let (start_char, prefix_ellipsis) = match first_hit {
        Some(pos) => {
            let char_pos = content[..pos].chars().count();
            let half = window_chars / 2;
            if char_pos > half {
                (char_pos - half, true)
            } else {
                (0, false)
            }
        }
        None => (0, false),
    };
    let window: String = content
        .chars()
        .skip(start_char)
        .take(window_chars)
        .collect();
    let suffix_ellipsis = start_char + window_chars < total_chars;

    // 收集窗口内全部命中区间（按起点排序，跳过重叠）
    let mut marks: Vec<(usize, usize)> = Vec::new();
    for term in &valid {
        let mut from = 0;
        while let Some(i) = window[from..].find(term.as_str()) {
            let start = from + i;
            let end = start + term.len();
            marks.push((start, end));
            from = end;
        }
    }
    marks.sort_unstable();

    let mut out = String::with_capacity(window.len() + 64);
    let mut cursor = 0;
    for (start, end) in marks {
        if start < cursor {
            continue; // 与上一命中区间重叠，跳过
        }
        out.push_str(&window[cursor..start]);
        out.push_str("<mark>");
        out.push_str(&window[start..end]);
        out.push_str("</mark>");
        cursor = end;
    }
    out.push_str(&window[cursor..]);

    format!(
        "{}{}{}",
        if prefix_ellipsis { "…" } else { "" },
        out,
        if suffix_ellipsis { "…" } else { "" }
    )
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FtsSearchFilter {
    pub query: String,
    pub topic_id: Option<String>,
    /// 会话归属过滤（决策 B）：topics.owner_id / owner_type，而非消息发送者
    pub owner_id: Option<String>,
    pub owner_type: Option<String>,
    pub role: Option<String>,
    pub start_time: Option<i64>,
    pub end_time: Option<i64>,
    pub limit: Option<i64>,
    /// keyset 分页游标（仅时间倒序模式有效，须成对出现）
    pub before_timestamp: Option<i64>,
    pub before_owner_type: Option<String>,
    pub before_owner_id: Option<String>,
    pub before_topic_id: Option<String>,
    pub before_message_id: Option<String>,
    /// 排序："time"（默认，时间倒序）| "rank"（bm25 相关度，游标分页不可用）
    pub sort: Option<String>,
}

#[tauri::command]
pub async fn search_messages_fts(
    db_state: tauri::State<'_, DbState>,
    filter: FtsSearchFilter,
) -> Result<Vec<FtsSearchResult>, String> {
    let trimmed = filter.query.trim();
    if trimmed.chars().count() < 2 {
        return Ok(Vec::new());
    }
    if filter.owner_id.is_some() != filter.owner_type.is_some() {
        return Err("search owner filter requires both ownerId and ownerType".to_string());
    }
    if filter.topic_id.is_some() && filter.owner_id.is_none() {
        return Err("search topic filter requires ownerId and ownerType".to_string());
    }
    let cursor_present = [
        filter.before_timestamp.is_some(),
        filter.before_owner_type.is_some(),
        filter.before_owner_id.is_some(),
        filter.before_topic_id.is_some(),
        filter.before_message_id.is_some(),
    ];
    if cursor_present.iter().any(|present| *present)
        && cursor_present.iter().any(|present| !*present)
    {
        return Err(
            "search cursor requires timestamp, ownerType, ownerId, topicId and messageId"
                .to_string(),
        );
    }
    let limit_val = filter.limit.unwrap_or(50).clamp(1, 200);

    // 按词长分流：≥3 字走 trigram FTS，<3 字（中文双字高频词）走 instr() 子串匹配。
    // trigram 对 <3 字 MATCH 静默返回零结果（无扫描兜底），这是硬性分流而非优化。
    let (long_terms, short_terms) = split_trigram_terms(trimmed);
    let use_fts = !long_terms.is_empty();
    let fts_query = build_fts_match_query(&long_terms);
    // bm25 仅在 FTS 路径有意义；纯短词路径强制时间倒序
    let sort_by_rank = use_fts && filter.sort.as_deref() == Some("rank");
    if sort_by_rank && filter.before_timestamp.is_some() {
        return Err("cursor pagination is only supported in time sort mode".to_string());
    }

    let mut sql = if use_fts {
        String::from(
            "SELECT
                m.msg_id,
                m.topic_id,
                m.role,
                m.timestamp,
                t.title AS topic_title,
                t.owner_id,
                t.owner_type,
                snippet(messages_fts, 2, '<mark>', '</mark>', '…', 48) AS snippet,
                NULL AS full_content
             FROM messages_fts
             INNER JOIN messages m ON messages_fts.owner_type = m.owner_type
                AND messages_fts.owner_id = m.owner_id
                AND messages_fts.topic_id = m.topic_id
                AND messages_fts.msg_id = m.msg_id
             INNER JOIN topics t ON m.owner_type = t.owner_type
                AND m.owner_id = t.owner_id
                AND m.topic_id = t.topic_id
             WHERE messages_fts.content MATCH ? AND m.deleted_at IS NULL AND t.deleted_at IS NULL",
        )
    } else {
        String::from(
            "SELECT
                m.msg_id,
                m.topic_id,
                m.role,
                m.timestamp,
                t.title AS topic_title,
                t.owner_id,
                t.owner_type,
                '' AS snippet,
                substr(m.content, 1, 262144) AS full_content
             FROM messages m
             INNER JOIN topics t ON m.owner_type = t.owner_type
                AND m.owner_id = t.owner_id
                AND m.topic_id = t.topic_id
             WHERE m.deleted_at IS NULL AND t.deleted_at IS NULL",
        )
    };

    // 短词子串条件（两条路径通用，全部参数化绑定）
    for _ in &short_terms {
        sql.push_str(" AND instr(m.content, ?) > 0");
    }

    // 动态过滤条件（全部参数化绑定，无注入面）
    if filter.topic_id.is_some() {
        sql.push_str(" AND m.topic_id = ?");
    }
    if filter.owner_id.is_some() {
        sql.push_str(" AND m.owner_id = ?");
    }
    if filter.owner_type.is_some() {
        sql.push_str(" AND m.owner_type = ?");
    }
    if filter.role.is_some() {
        sql.push_str(" AND m.role = ?");
    }
    if filter.start_time.is_some() {
        sql.push_str(" AND m.timestamp >= ?");
    }
    if filter.end_time.is_some() {
        sql.push_str(" AND m.timestamp <= ?");
    }
    if filter.before_timestamp.is_some() {
        sql.push_str(
            " AND (m.timestamp, m.owner_type, m.owner_id, m.topic_id, m.msg_id) < (?, ?, ?, ?, ?)",
        );
    }

    if sort_by_rank {
        sql.push_str(
            " ORDER BY bm25(messages_fts), m.timestamp DESC, m.owner_type DESC, m.owner_id DESC, m.topic_id DESC, m.msg_id DESC",
        );
    } else {
        sql.push_str(
            " ORDER BY m.timestamp DESC, m.owner_type DESC, m.owner_id DESC, m.topic_id DESC, m.msg_id DESC",
        );
    }
    sql.push_str(" LIMIT ?");

    // 按 SQL 文本顺序绑定：MATCH 词 → 短词 instr → 过滤器 → 游标 → limit
    let mut final_query = sqlx::query(&sql);
    if let Some(ref fq) = fts_query {
        final_query = final_query.bind(fq);
    }
    for term in &short_terms {
        final_query = final_query.bind(term);
    }

    if let Some(ref tid) = filter.topic_id {
        final_query = final_query.bind(tid);
    }
    if let Some(ref oid) = filter.owner_id {
        final_query = final_query.bind(oid);
    }
    if let Some(ref ot) = filter.owner_type {
        final_query = final_query.bind(ot);
    }
    if let Some(ref r) = filter.role {
        final_query = final_query.bind(r);
    }
    if let Some(st) = filter.start_time {
        final_query = final_query.bind(st);
    }
    if let Some(et) = filter.end_time {
        final_query = final_query.bind(et);
    }
    if let (Some(bts), Some(ref bot), Some(ref boi), Some(ref bti), Some(ref bmi)) = (
        filter.before_timestamp,
        &filter.before_owner_type,
        &filter.before_owner_id,
        &filter.before_topic_id,
        &filter.before_message_id,
    ) {
        final_query = final_query
            .bind(bts)
            .bind(bot)
            .bind(boi)
            .bind(bti)
            .bind(bmi);
    }
    final_query = final_query.bind(limit_val);

    let rows = final_query
        .fetch_all(&db_state.pool)
        .await
        .map_err(|e| format!("全文检索执行失败: {}", e))?;

    let mut results = Vec::with_capacity(rows.len());
    for row in rows {
        // FTS 路径用 snippet()；纯短词路径用本地窗口摘要（高亮全部短词命中）
        let snippet: String = if use_fts {
            row.get("snippet")
        } else {
            let full_content: String = row.get("full_content");
            build_local_snippet(&full_content, &short_terms, 96)
        };
        results.push(FtsSearchResult {
            msg_id: row.get("msg_id"),
            topic_id: row.get("topic_id"),
            role: row.get("role"),
            timestamp: row.get("timestamp"),
            topic_title: row.get("topic_title"),
            owner_id: row.get("owner_id"),
            owner_type: row.get("owner_type"),
            snippet,
        });
    }

    Ok(results)
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FtsIndexStatus {
    pub total_messages: i64,
    pub indexed_messages: i64,
    pub rebuilding: bool,
}

static FTS_REBUILD_RUNNING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

async fn fts_index_status(pool: &Pool<Sqlite>) -> Result<FtsIndexStatus, String> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE deleted_at IS NULL")
        .fetch_one(pool)
        .await
        .map_err(|e| format!("统计消息总数失败: {}", e))?;
    let indexed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages_fts")
        .fetch_one(pool)
        .await
        .map_err(|e| format!("统计索引条目失败: {}", e))?;
    Ok(FtsIndexStatus {
        total_messages: total,
        indexed_messages: indexed,
        rebuilding: FTS_REBUILD_RUNNING.load(std::sync::atomic::Ordering::SeqCst),
    })
}

/// 查询 FTS 索引覆盖率状态（前端搜索页首开时调用，决定是否展示"索引构建中"）
#[tauri::command]
pub async fn get_fts_index_status(
    db_state: tauri::State<'_, DbState>,
) -> Result<FtsIndexStatus, String> {
    fts_index_status(&db_state.pool).await
}

/// 回填 FTS 索引（决策 G：首次打开搜索页时由前端触发，不在启动路径执行）。
/// 幂等断点续跑：NOT EXISTS 跳过已索引条目，任意时刻中断后重入安全。
#[tauri::command]
pub async fn rebuild_messages_fts(
    app_handle: AppHandle,
    db_state: tauri::State<'_, DbState>,
) -> Result<FtsIndexStatus, String> {
    use std::sync::atomic::Ordering;
    // 并发护栏：同一时刻只允许一个回填任务，重入直接返回当前状态
    if FTS_REBUILD_RUNNING.swap(true, Ordering::SeqCst) {
        return fts_index_status(&db_state.pool).await;
    }
    let result = rebuild_messages_fts_inner(&app_handle, &db_state.pool).await;
    FTS_REBUILD_RUNNING.store(false, Ordering::SeqCst);
    result
}

async fn rebuild_messages_fts_inner(
    app_handle: &AppHandle,
    pool: &Pool<Sqlite>,
) -> Result<FtsIndexStatus, String> {
    const BATCH_SIZE: i64 = 500;
    loop {
        // 单事务内 SELECT+INSERT：WAL 快照一致；中断后已提交批次不回滚（断点续跑）
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| format!("回填事务开启失败: {}", e))?;
        let inserted = sqlx::query(
            "INSERT INTO messages_fts (msg_id, topic_id, content, owner_type, owner_id)
             SELECT m.msg_id, m.topic_id, m.content, m.owner_type, m.owner_id FROM messages m
             WHERE m.deleted_at IS NULL
               AND NOT EXISTS (
                   SELECT 1 FROM messages_fts f
                   WHERE f.owner_type = m.owner_type AND f.owner_id = m.owner_id
                     AND f.topic_id = m.topic_id AND f.msg_id = m.msg_id
               )
             LIMIT ?",
        )
        .bind(BATCH_SIZE)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("回填批次执行失败: {}", e))?;
        let batch_count = inserted.rows_affected();
        tx.commit()
            .await
            .map_err(|e| format!("回填事务提交失败: {}", e))?;
        if batch_count == 0 {
            break;
        }
        // 批次间发射进度事件，驱动搜索页"索引构建中"进度 UI
        let status = fts_index_status(pool).await?;
        let _ = app_handle.emit(
            "vcp-system-event",
            serde_json::json!({
                "type": "vcp-fts-rebuild",
                "indexedMessages": status.indexed_messages,
                "totalMessages": status.total_messages,
                "source": "GlobalSearch"
            }),
        );
    }
    fts_index_status(pool).await
}

pub async fn decompress_database_migration(app_handle: &AppHandle) -> Result<bool, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    // 1. 检测是否含有需要升级的压缩数据
    let needs_upgrade: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM messages WHERE typeof(content) = 'blob')")
            .fetch_one(pool)
            .await
            .unwrap_or(false);

    if !needs_upgrade {
        return Ok(false);
    }

    log::info!("[DBManager] Compressed messages detected in database. Intercepting bootstrap for decompression migration...");

    let lifecycle =
        app_handle.state::<crate::vcp_modules::infra::lifecycle_state::LifecycleState>();
    {
        let mut status_lock = lifecycle.status.write().await;
        *status_lock = crate::vcp_modules::infra::lifecycle_state::CoreStatus::Decompressing;
        let mut msg_lock = lifecycle.status_message.write().await;
        *msg_lock = "正在准备解压历史消息... 0%".to_string();
    }

    // 2. 查询待解压总条数
    let total_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE typeof(content) = 'blob'")
            .fetch_one(pool)
            .await
            .map_err(|e| format!("Failed to query compressed messages count: {}", e))?;

    if total_count == 0 {
        return Ok(false);
    }

    log::info!(
        "[DBManager] Decompressing {} messages in background...",
        total_count
    );

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
        // 读取未解压的批次，获取 deleted_at 以避免 FTS 索引污染
        let rows = sqlx::query(
            "SELECT owner_type, owner_id, topic_id, msg_id, content, deleted_at
             FROM messages WHERE typeof(content) = 'blob' ORDER BY rowid LIMIT ?",
        )
        .bind(batch_size)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("Failed to fetch compressed batch: {}", e))?;

        if rows.is_empty() {
            break;
        }

        // 开启本批次事务
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| format!("Failed to start batch transaction: {}", e))?;

        for row in &rows {
            let owner_type: String = row.get("owner_type");
            let owner_id: String = row.get("owner_id");
            let msg_id: String = row.get("msg_id");
            let topic_id: String = row.get("topic_id");
            let content_bytes: Vec<u8> = row.get("content");
            let deleted_at: Option<i64> = row.get("deleted_at");

            // 校验 zstd 压缩魔数头：[0x28, 0xB5, 0x2F, 0xFD] (Little Endian for 0xFD2FB528)
            let is_zstd = content_bytes.len() >= 4
                && content_bytes[0] == 0x28
                && content_bytes[1] == 0xB5
                && content_bytes[2] == 0x2F
                && content_bytes[3] == 0xFD;

            let content = if is_zstd {
                match crate::vcp_modules::persistence::message_repository::ContentCompressor::decompress(&content_bytes) {
                    Ok(decompressed) => decompressed,
                    Err(e) => {
                        return Err(format!(
                            "Failed to decompress message {} in topic {}: {}. Migration aborted to prevent data corruption.",
                            msg_id, topic_id, e
                        ));
                    }
                }
            } else {
                // 如果不是 zstd 压缩的，说明是原本就作为 BLOB 插入的明文文本（或损坏的文本）
                String::from_utf8_lossy(&content_bytes).to_string()
            };

            // 1. 更新写回为明文 String (SQLite 动态类型会自动将 typeof 转为 'text')
            sqlx::query(
                "UPDATE messages SET content = ?
                 WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
            )
            .bind(&content)
            .bind(&owner_type)
            .bind(&owner_id)
            .bind(&topic_id)
            .bind(&msg_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("Failed to update decompressed message: {}", e))?;

            // 2. 同步写入 FTS5 虚拟索引表，删除陈旧的索引项，仅在消息未逻辑删除时插入，防止软删除索引泄漏
            sqlx::query(
                "DELETE FROM messages_fts
                 WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
            )
            .bind(&owner_type)
            .bind(&owner_id)
            .bind(&topic_id)
            .bind(&msg_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("Failed to delete stale FTS entry: {}", e))?;

            if deleted_at.is_none() {
                // trigram 分词器（migration 0008 起）直接索引原文，无需 CJK 预处理
                sqlx::query(
                    "INSERT INTO messages_fts (msg_id, topic_id, content, owner_type, owner_id)
                     VALUES (?, ?, ?, ?, ?)",
                )
                .bind(&msg_id)
                .bind(&topic_id)
                .bind(&content)
                .bind(&owner_type)
                .bind(&owner_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| format!("Failed to insert FTS entry: {}", e))?;
            }
        }

        tx.commit()
            .await
            .map_err(|e| format!("Failed to commit batch transaction: {}", e))?;

        processed_count += rows.len();

        // 4. 定期发射 progress 信号
        let pct = (processed_count * 100) / (total_count as usize);
        log::info!(
            "[DBManager] Decompression progress: {}% ({}/{})",
            pct,
            processed_count,
            total_count
        );

        let msg = format!("正在重构本地数据库... {}%", pct);
        {
            let mut msg_lock = lifecycle.status_message.write().await;
            *msg_lock = msg.clone();
        }

        let _ = app_handle.emit(
            "vcp-system-event",
            serde_json::json!({
                "type": "vcp-core-status",
                "status": "decompressing",
                "message": msg,
                "source": "Core"
            }),
        );
    }

    // 5. 物理页收尾整理
    log::info!("[DBManager] Decompression complete. Reclaiming database disk space via VACUUM...");
    {
        let mut msg_lock = lifecycle.status_message.write().await;
        *msg_lock = "正在优化数据库存储空间...".to_string();
    }
    let _ = app_handle.emit(
        "vcp-system-event",
        serde_json::json!({
            "type": "vcp-core-status",
            "status": "decompressing",
            "message": "正在优化数据库存储空间...",
            "source": "Core"
        }),
    );
    sqlx::query("VACUUM")
        .execute(pool)
        .await
        .unwrap_or_default();

    // 6. 发射升级完成信号，等待重启
    log::info!("[DBManager] Database migration completed successfully. Waiting for user restart confirmation...");
    let final_msg = "本地数据库格式重构成功，请确认重启应用。".to_string();
    {
        let mut status_lock = lifecycle.status.write().await;
        *status_lock =
            crate::vcp_modules::infra::lifecycle_state::CoreStatus::DecompressionComplete;
        let mut msg_lock = lifecycle.status_message.write().await;
        *msg_lock = final_msg.clone();
    }
    let _ = app_handle.emit(
        "vcp-system-event",
        serde_json::json!({
            "type": "vcp-core-status",
            "status": "decompression-complete",
            "message": final_msg,
            "source": "Core"
        }),
    );

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_fts_match_query() {
        // 单词短语
        assert_eq!(
            build_fts_match_query(&["机器学习".to_string()]),
            Some("\"机器学习\"".into())
        );
        // 多词 AND
        assert_eq!(
            build_fts_match_query(&["机器学习".to_string(), "部署".to_string()]),
            Some("\"机器学习\" AND \"部署\"".into())
        );
        // 双引号加倍转义，防 MATCH 语法注入
        assert_eq!(
            build_fts_match_query(&["a\"b".to_string()]),
            Some("\"a\"\"b\"".into())
        );
        // 空输入
        assert_eq!(build_fts_match_query(&[]), None);
    }

    #[test]
    fn test_split_trigram_terms() {
        // ≥3 字进 FTS，<3 字走 instr()
        let (long, short) = split_trigram_terms("机器学习 部署 ai 模型训练");
        assert_eq!(long, vec!["机器学习", "模型训练"]);
        assert_eq!(short, vec!["部署", "ai"]);
        // 纯长词 / 纯短词
        let (long, short) = split_trigram_terms("神经网络");
        assert_eq!(long, vec!["神经网络"]);
        assert!(short.is_empty());
        let (long, short) = split_trigram_terms("部署 天气");
        assert!(long.is_empty());
        assert_eq!(short, vec!["部署", "天气"]);
        // 空输入
        let (long, short) = split_trigram_terms("   ");
        assert!(long.is_empty());
        assert!(short.is_empty());
    }

    #[test]
    fn test_build_local_snippet() {
        let terms = vec!["部署".to_string()];
        // 命中居中 + 双侧省略号 + 高亮
        let content = "今天天气不错，我们来讨论一下服务的部署问题，部署上线后还要观察日志。";
        let snippet = build_local_snippet(content, &terms, 20);
        assert!(
            snippet.contains("<mark>部署</mark>"),
            "snippet: {}",
            snippet
        );
        assert!(snippet.ends_with('…'), "snippet: {}", snippet);
        // 窗口内多次命中全部高亮
        assert_eq!(snippet.matches("<mark>").count(), 2, "snippet: {}", snippet);
        // 无命中时取开头窗口，无 <mark>
        let miss = build_local_snippet(content, &["火星".to_string()], 20);
        assert!(!miss.contains("<mark>"));
        // 短内容无省略号
        let short = build_local_snippet("部署吧", &terms, 96);
        assert_eq!(short, "<mark>部署</mark>吧");
        // 重叠区间跳过（"aba" 在 "ababa" 中两处重叠命中只取第一处）
        let overlap = build_local_snippet("ababa", &["aba".to_string()], 96);
        assert_eq!(overlap, "<mark>aba</mark>ba");
    }

    #[test]
    fn only_corrupt_and_notadb_codes_authorize_recovery() {
        assert!(is_confirmed_sqlite_corruption_code(11));
        assert!(is_confirmed_sqlite_corruption_code(26));
        assert!(is_confirmed_sqlite_corruption_code(11 | (1 << 8)));
        assert!(!is_confirmed_sqlite_corruption_code(5)); // SQLITE_BUSY
        assert!(!is_confirmed_sqlite_corruption_code(6)); // SQLITE_LOCKED
        assert!(!is_confirmed_sqlite_corruption_code(10)); // SQLITE_IOERR
    }

    /// 铁律守护：(BASELINE_INCREMENTAL_MAX, BASELINE_VERSION) 版本号区间内严禁出现迁移，
    /// 否则全新安装快速路径会将其 seed 跳过，造成永久 schema 缺失。
    #[test]
    fn baseline_version_gap_is_free_of_migrations() {
        let migrator = sqlx::migrate!("./migrations");
        for migration in migrator.migrations.iter() {
            assert!(
                migration.version <= BASELINE_INCREMENTAL_MAX
                    || migration.version >= BASELINE_VERSION,
                "migration v{} falls in the forbidden gap ({}, {}); \
                 versions below baseline are seeded-and-skipped on fresh installs",
                migration.version,
                BASELINE_INCREMENTAL_MAX,
                BASELINE_VERSION
            );
        }
        // baseline 文件必须存在
        assert!(
            migrator
                .migrations
                .iter()
                .any(|m| m.version == BASELINE_VERSION),
            "baseline migration v{} must exist",
            BASELINE_VERSION
        );
    }

    /// 全新安装快速路径：空库 → 只执行 baseline → 全部既有迁移 seed 为已应用，
    /// 直接得到 trigram 终态 FTS，不经过 unicode61 中间态。
    #[tokio::test]
    async fn fresh_install_fast_path_runs_only_baseline() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open test database");

        run_migrations(&pool).await.expect("run migrations");

        // 全部 1..=8 + baseline 记录已 seed，migrator 无事可做
        let versions: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&pool)
                .await
                .expect("read seeded versions");
        let mut expected: Vec<i64> = (1..=BASELINE_INCREMENTAL_MAX).collect();
        expected.push(BASELINE_VERSION);
        assert_eq!(versions, expected);

        // FTS 直接是 trigram 终态
        let fts_ddl: String =
            sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE name='messages_fts'")
                .fetch_one(&pool)
                .await
                .expect("read fts ddl");
        assert!(fts_ddl.contains("trigram"), "baseline FTS must be trigram");

        // 代表性表与 0007/0008 产物齐全
        for name in ["messages", "topics", "active_generations", "render_cache"] {
            let exists: bool = sqlx::query_scalar(&format!(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='{}')",
                name
            ))
            .fetch_one(&pool)
            .await
            .expect("inspect table");
            assert!(exists, "baseline must create {}", name);
        }
        let has_agent_idx: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='idx_messages_agent_id')",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect index");
        assert!(has_agent_idx, "baseline must include idx_messages_agent_id");

        let agent_pk: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM pragma_table_info('agents') WHERE pk > 0 ORDER BY pk",
        )
        .fetch_all(&pool)
        .await
        .expect("read agent primary key");
        assert_eq!(agent_pk, ["owner_type", "agent_id"]);

        let group_pk: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM pragma_table_info('groups') WHERE pk > 0 ORDER BY pk",
        )
        .fetch_all(&pool)
        .await
        .expect("read group primary key");
        assert_eq!(group_pk, ["owner_type", "group_id"]);

        let topic_pk: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM pragma_table_info('topics') WHERE pk > 0 ORDER BY pk",
        )
        .fetch_all(&pool)
        .await
        .expect("read topic primary key");
        assert_eq!(topic_pk, ["owner_type", "owner_id", "topic_id"]);

        let message_pk: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM pragma_table_info('messages') WHERE pk > 0 ORDER BY pk",
        )
        .fetch_all(&pool)
        .await
        .expect("read message primary key");
        assert_eq!(message_pk, ["owner_type", "owner_id", "topic_id", "msg_id"]);

        assert!(fts_ddl.contains("owner_type UNINDEXED"));
        assert!(fts_ddl.contains("owner_id UNINDEXED"));
    }

    #[tokio::test]
    async fn legacy_database_is_rejected_without_mutation() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open test database");
        sqlx::query("CREATE TABLE messages (topic_id TEXT, msg_id TEXT)")
            .execute(&pool)
            .await
            .expect("create legacy fixture");

        let error = run_migrations(&pool)
            .await
            .expect_err("legacy database must be rejected");
        assert!(error.contains("数据库版本不兼容"));

        let has_tracking: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='_sqlx_migrations')",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect tracking table");
        assert!(
            !has_tracking,
            "rejection must not mutate the legacy database"
        );
    }

    /// trigram FTS 端到端验证：保序子串命中、乱序不命中、逻辑删除触发器清索引。
    #[tokio::test]
    async fn trigram_fts_matches_cjk_in_order_and_respects_delete_trigger() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open test database");
        run_migrations(&pool).await.expect("run migrations");

        sqlx::query(
            "INSERT INTO topics (owner_type, owner_id, topic_id, title, created_at, updated_at)
             VALUES ('agent', 'a1', 't1', '测试话题', 0, 0)",
        )
        .execute(&pool)
        .await
        .expect("insert topic");
        sqlx::query(
            "INSERT INTO messages
             (owner_type, owner_id, topic_id, msg_id, role, content, timestamp, created_at, updated_at)
             VALUES ('agent', 'a1', 't1', 'm1', 'assistant',
                     '我想讨论机器学习模型的部署问题', 1000, 1000, 1000)",
        )
        .execute(&pool)
        .await
        .expect("insert message");
        sqlx::query(
            "INSERT INTO messages_fts (msg_id, topic_id, content, owner_type, owner_id)
             SELECT msg_id, topic_id, content, owner_type, owner_id FROM messages",
        )
        .execute(&pool)
        .await
        .expect("backfill fts");

        // 保序子串命中
        let hit: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messages_fts WHERE content MATCH ?")
                .bind(build_fts_match_query(&["机器学习".to_string()]).unwrap())
                .fetch_one(&pool)
                .await
                .expect("fts match");
        assert_eq!(hit, 1, "in-order substring must match");

        // 乱序不命中（unicode61 单字方案的假阳性在此被消灭）
        let miss: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messages_fts WHERE content MATCH ?")
                .bind(build_fts_match_query(&["学习机器".to_string()]).unwrap())
                .fetch_one(&pool)
                .await
                .expect("fts match");
        assert_eq!(miss, 0, "out-of-order query must not match");

        // 硬性边界文档化：<3 字 MATCH 静默返回零结果（无扫描兜底），
        // 这是 search_messages_fts 对短词走 instr() 分流的原因
        let short_zero: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messages_fts WHERE content MATCH ?")
                .bind(build_fts_match_query(&["部署".to_string()]).unwrap())
                .fetch_one(&pool)
                .await
                .expect("fts match");
        assert_eq!(short_zero, 0, "trigram <3-char MATCH must yield zero rows");
        // 同一短词经 instr() 可命中（分流路径的正确性）
        let instr_hit: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE instr(content, '部署') > 0")
                .fetch_one(&pool)
                .await
                .expect("instr match");
        assert_eq!(instr_hit, 1, "instr() fallback must hit short term");

        // 逻辑删除触发器清索引
        sqlx::query(
            "UPDATE messages SET deleted_at = 2000
             WHERE owner_type='agent' AND owner_id='a1' AND topic_id='t1' AND msg_id='m1'",
        )
        .execute(&pool)
        .await
        .expect("soft delete");
        let after_delete: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messages_fts WHERE content MATCH ?")
                .bind(build_fts_match_query(&["机器学习".to_string()]).unwrap())
                .fetch_one(&pool)
                .await
                .expect("fts match");
        assert_eq!(after_delete, 0, "logical delete must purge fts entry");
    }

    #[tokio::test]
    async fn archive_preserves_database_wal_and_shm_as_one_unit() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("vcp_avatar.db");
        let wal_path = sqlite_sidecar_path(&db_path, "-wal");
        let shm_path = sqlite_sidecar_path(&db_path, "-shm");

        tokio::fs::write(&db_path, b"main-db")
            .await
            .expect("write database fixture");
        tokio::fs::write(&wal_path, b"committed-wal")
            .await
            .expect("write wal fixture");
        tokio::fs::write(&shm_path, b"shared-memory")
            .await
            .expect("write shm fixture");

        let archive = archive_corrupt_db(&db_path).await.expect("archive unit");

        assert!(!db_path.exists());
        assert!(!wal_path.exists());
        assert!(!shm_path.exists());
        assert_eq!(
            tokio::fs::read(archive.join("vcp_avatar.db"))
                .await
                .expect("read archived database"),
            b"main-db"
        );
        assert_eq!(
            tokio::fs::read(archive.join("vcp_avatar.db-wal"))
                .await
                .expect("read archived wal"),
            b"committed-wal"
        );
        assert_eq!(
            tokio::fs::read(archive.join("vcp_avatar.db-shm"))
                .await
                .expect("read archived shm"),
            b"shared-memory"
        );
    }

    #[tokio::test]
    async fn missing_main_database_never_discards_sidecars() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("vcp_avatar.db");
        let wal_path = sqlite_sidecar_path(&db_path, "-wal");
        tokio::fs::write(&wal_path, b"orphaned-committed-wal")
            .await
            .expect("write wal fixture");

        let error = archive_corrupt_db(&db_path)
            .await
            .expect_err("missing main database must fail closed");

        assert!(error.contains("主数据库文件不存在"));
        assert_eq!(
            tokio::fs::read(wal_path).await.expect("wal remains"),
            b"orphaned-committed-wal"
        );
    }

    #[tokio::test]
    async fn orphaned_sidecars_are_rejected_before_sqlite_can_create_a_new_main_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("vcp_avatar.db");
        let wal_path = sqlite_sidecar_path(&db_path, "-wal");
        let shm_path = sqlite_sidecar_path(&db_path, "-shm");
        tokio::fs::write(&wal_path, b"committed-wal")
            .await
            .expect("write wal fixture");
        tokio::fs::write(&shm_path, b"shared-memory")
            .await
            .expect("write shm fixture");

        let error = validate_sqlite_file_set(&db_path)
            .expect_err("orphaned sidecars must stop initialization before SQLite open");

        assert!(error.contains("主文件缺失"));
        assert!(!db_path.exists());
        assert_eq!(
            tokio::fs::read(&wal_path).await.expect("wal remains"),
            b"committed-wal"
        );
        assert_eq!(
            tokio::fs::read(&shm_path).await.expect("shm remains"),
            b"shared-memory"
        );
    }
}
