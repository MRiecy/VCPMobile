// maintenance_manager.rs - 负责系统维护、垃圾回收与缓存清理的核心模块
// 职责: 聚合所有低频但关键的系统维护任务，对齐前端 MaintenanceSection 领域。

use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::file_manager::{
    delete_attachment_physical, get_attachments_root_dir, get_cache_root_dir,
    get_multimodal_cache_dir, get_thumbnails_root_dir,
};
use crate::vcp_modules::infra::utils::{is_valid_cas_hash, now_secs, YieldCounter};
use std::collections::HashSet;
use tauri::{AppHandle, Manager, State};

struct AttachmentGcSnapshot {
    live_hashes: HashSet<String>,
    indexed_attachments: Vec<(String, String)>,
}

const LAST_AUTOMATIC_MAINTENANCE_KEY: &str = "last_automatic_maintenance";

async fn last_automatic_maintenance(pool: &sqlx::SqlitePool) -> Result<i64, String> {
    let value: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
        .bind(LAST_AUTOMATIC_MAINTENANCE_KEY)
        .fetch_optional(pool)
        .await
        .map_err(|error| format!("读取自动维护时间失败: {error}"))?;
    value.map_or(Ok(0), |value| {
        value
            .parse::<i64>()
            .map_err(|_| "自动维护时间格式无效".to_string())
    })
}

async fn record_automatic_maintenance(db_state: &DbState, time: i64) -> Result<(), String> {
    let mut tx = db_state
        .write_transaction("maintenance.record-completion")
        .await?;
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, ?) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(LAST_AUTOMATIC_MAINTENANCE_KEY)
    .bind(time.to_string())
    .bind(time)
    .execute(&mut *tx)
    .await
    .map_err(|error| format!("记录自动维护时间失败: {error}"))?;
    tx.commit().await
}

#[derive(Debug, Default)]
pub struct AttachmentGcReport {
    pub retained: usize,
    pub reclaimed: usize,
    pub ghost_files: usize,
}

/// 在同一 SQLite 快照中确定存活消息和当前分享草稿持有的 CAS。
async fn load_attachment_gc_snapshot(
    tx: &mut sqlx::SqliteConnection,
) -> Result<AttachmentGcSnapshot, String> {
    let mut live_hashes: HashSet<String> = sqlx::query_as::<_, (String,)>(
        "SELECT DISTINCT ma.hash FROM message_attachments ma \
             INNER JOIN messages m ON ma.owner_type = m.owner_type AND ma.owner_id = m.owner_id \
                AND ma.topic_id = m.topic_id AND ma.msg_id = m.msg_id \
             INNER JOIN topics t ON m.owner_type = t.owner_type AND m.owner_id = t.owner_id \
                AND m.topic_id = t.topic_id \
             WHERE m.deleted_at IS NULL \
               AND t.deleted_at IS NULL \
               AND (\
                 (t.owner_type = 'agent' AND EXISTS (\
                   SELECT 1 FROM agents a WHERE a.owner_type = 'agent' \
                     AND a.agent_id = t.owner_id AND a.deleted_at IS NULL\
                 )) OR \
                 (t.owner_type = 'group' AND EXISTS (\
                   SELECT 1 FROM groups g WHERE g.owner_type = 'group' \
                     AND g.group_id = t.owner_id AND g.deleted_at IS NULL\
                 ))\
               )",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|error| format!("读取附件 GC 有效引用失败: {error}"))?
    .into_iter()
    .map(|(hash,)| hash)
    .collect();

    let current_share: Option<String> =
        sqlx::query_scalar("SELECT record FROM current_share WHERE singleton = 1")
            .fetch_optional(&mut *tx)
            .await
            .map_err(|error| format!("读取附件 GC 分享引用失败: {error}"))?;
    if let Some(record) = current_share {
        // A recoverable share may have imported CAS files before its first
        // message exists. Its persisted draft owns those files across restart.
        live_hashes.extend(super::share_inbox::active_attachment_hashes(&record)?);
    }

    let indexed_attachments =
        sqlx::query_as::<_, (String, String)>("SELECT hash, internal_path FROM attachments")
            .fetch_all(&mut *tx)
            .await
            .map_err(|error| format!("读取附件 GC 索引失败: {error}"))?;

    Ok(AttachmentGcSnapshot {
        live_hashes,
        indexed_attachments,
    })
}

fn is_hash_uuid_temp(value: &str) -> bool {
    let Some(hash) = value.get(..64) else {
        return false;
    };
    is_valid_cas_hash(hash)
        && value.as_bytes().get(64) == Some(&b'-')
        && value
            .get(65..)
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
}

fn is_managed_attachment_temp(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".tmp") else {
        return false;
    };
    if let Some(value) = stem.strip_prefix(".ingest-") {
        return uuid::Uuid::parse_str(value).is_ok() || is_hash_uuid_temp(value);
    }
    stem.strip_prefix(".thumb-").is_some_and(is_hash_uuid_temp)
}

async fn sweep_attachment_files(
    root: &std::path::Path,
    protected_hashes: &HashSet<String>,
) -> usize {
    let Ok(mut entries) = tokio::fs::read_dir(root).await else {
        return 0;
    };
    let mut removed = 0;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let Ok(metadata) = tokio::fs::symlink_metadata(&path).await else {
            continue;
        };
        if !metadata.file_type().is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let is_managed_temp = is_managed_attachment_temp(name);
        let is_unindexed_cas = std::path::Path::new(name)
            .file_stem()
            .and_then(|value| value.to_str())
            .is_some_and(|hash| is_valid_cas_hash(hash) && !protected_hashes.contains(hash));
        if (is_managed_temp || is_unindexed_cas) && tokio::fs::remove_file(&path).await.is_ok() {
            removed += 1;
        }
    }
    removed
}

async fn sweep_unindexed_derived_files(
    root: &std::path::Path,
    indexed_hashes: &HashSet<String>,
    hash_from_name: fn(&str) -> Option<&str>,
) -> usize {
    let Ok(mut entries) = tokio::fs::read_dir(root).await else {
        return 0;
    };
    let mut removed = 0;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let Ok(metadata) = tokio::fs::symlink_metadata(&path).await else {
            continue;
        };
        if !metadata.file_type().is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if is_managed_attachment_temp(name) {
            if tokio::fs::remove_file(&path).await.is_ok() {
                removed += 1;
            }
            continue;
        }
        let Some(hash) = hash_from_name(name).filter(|hash| is_valid_cas_hash(hash)) else {
            continue;
        };
        if !indexed_hashes.contains(hash) && tokio::fs::remove_file(&path).await.is_ok() {
            removed += 1;
        }
    }
    removed
}

/// 在 Core Ready 之前回收上一会话遗留的无引用 CAS。
/// 此时上传 staging 已清空，前端尚不能创建新的暂存附件，因此无需在线租约或墓碑。
pub async fn reclaim_orphaned_attachments<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    db_state: &DbState,
) -> Result<AttachmentGcReport, String> {
    let pool = &db_state.pool;
    let mut tx = pool
        .begin()
        .await
        .map_err(|error| format!("开启附件 GC 快照失败: {error}"))?;
    let snapshot = load_attachment_gc_snapshot(&mut tx).await?;
    tx.commit()
        .await
        .map_err(|error| format!("结束附件 GC 快照失败: {error}"))?;

    let mut report = AttachmentGcReport {
        retained: snapshot.live_hashes.len(),
        ..Default::default()
    };
    let mut reclaimed_hashes = Vec::new();
    for (hash, internal_path) in snapshot.indexed_attachments {
        if snapshot.live_hashes.contains(&hash) || !is_valid_cas_hash(&hash) {
            continue;
        }
        if let Err(error) = delete_attachment_physical(app_handle, &hash, &internal_path).await {
            log::warn!("[Maintenance] Attachment GC kept {hash}: {error}");
            continue;
        }
        reclaimed_hashes.push(hash);
    }
    if !reclaimed_hashes.is_empty() {
        let mut write_tx = db_state
            .write_transaction("maintenance.attachment-gc")
            .await?;
        for hash in &reclaimed_hashes {
            sqlx::query("DELETE FROM attachments WHERE hash = ?")
                .bind(hash)
                .execute(&mut *write_tx)
                .await
                .map_err(|error| format!("删除附件索引 {hash} 失败: {error}"))?;
        }
        write_tx
            .commit()
            .await
            .map_err(|error| format!("提交附件 GC 索引清理失败: {error}"))?;
        report.reclaimed = reclaimed_hashes.len();
    }

    let indexed_attachments =
        sqlx::query_as::<_, (String, String)>("SELECT hash, internal_path FROM attachments")
            .fetch_all(pool)
            .await
            .map_err(|error| format!("读取附件 GC 最终索引失败: {error}"))?;
    let indexed_hashes: HashSet<String> = indexed_attachments
        .iter()
        .map(|(hash, _)| hash.clone())
        .collect();
    // Local roots can change after an iOS update/restore. A referenced hash is
    // never a ghost merely because its previous absolute path differs.
    let protected_hashes = indexed_hashes.union(&snapshot.live_hashes).cloned().collect();
    let attachments_root = get_attachments_root_dir(app_handle)?;
    let thumbnails_root = get_thumbnails_root_dir(app_handle)?;
    let multimodal_root = get_multimodal_cache_dir(app_handle)?;
    report.ghost_files += sweep_attachment_files(&attachments_root, &protected_hashes).await;
    report.ghost_files +=
        sweep_unindexed_derived_files(&thumbnails_root, &indexed_hashes, |name| {
            name.strip_suffix("_thumb.webp")
                .or_else(|| name.strip_suffix("_thumb.jpg"))
                .or_else(|| name.strip_suffix("_thumb.png"))
        })
        .await;
    report.ghost_files +=
        sweep_unindexed_derived_files(&multimodal_root, &indexed_hashes, |name| {
            name.strip_suffix(".json")
        })
        .await;

    Ok(report)
}

/// 辅助函数：异步深度遍历计算目录大小（带协作式 CPU 挂起出让，每 200 个文件出让一次时间片）
#[cfg(not(target_os = "ios"))]
async fn calculate_dir_size(path: &std::path::Path) -> u64 {
    let mut total_size = 0;
    let mut stack = vec![path.to_path_buf()];
    let mut yield_ctrl = YieldCounter::new(200);

    while let Some(current_path) = stack.pop() {
        if current_path.is_file() {
            yield_ctrl.tick().await;
            if let Ok(meta) = tokio::fs::metadata(&current_path).await {
                total_size += meta.len();
            }
        } else if current_path.is_dir() {
            if let Ok(mut entries) = tokio::fs::read_dir(&current_path).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    stack.push(entry.path());
                }
            }
        }
    }
    total_size
}

/// 清理 WebView HTTP 缓存，不触碰 LocalStorage 等应用持久状态。
#[cfg(not(target_os = "ios"))]
#[tauri::command]
pub async fn clear_webview_cache(app: AppHandle) -> Result<String, String> {
    let mut cleared_details = String::new();
    let mut freed_size = 0u64;

    let cache_dir =
        get_cache_root_dir(&app).map_err(|error| format!("WebView 缓存目录不可用: {error}"))?;
    let http_cache_dir = cache_dir.join("WebView").join("Default").join("HTTP Cache");
    if http_cache_dir.exists() {
        freed_size = calculate_dir_size(&http_cache_dir).await;

        if tokio::fs::remove_dir_all(&http_cache_dir).await.is_ok() {
            cleared_details.push_str("HTTP Cache 已清除；");
        } else {
            freed_size = 0;
            cleared_details.push_str("部分 HTTP Cache 正在使用，暂未清除；");
        }
    } else {
        cleared_details.push_str("HTTP Cache 无残余文件；");
    }

    let freed_size_mb = (freed_size as f64) / 1024.0 / 1024.0;
    Ok(format!(
        "WebView HTTP 缓存清理完成 ({})，释放空间: {:.2} MB",
        cleared_details.trim_end_matches('；'),
        freed_size_mb
    ))
}

#[cfg(target_os = "ios")]
#[tauri::command]
pub async fn clear_webview_cache(app: AppHandle) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        tauri_plugin_vcp_mobile::system::clear_webview_cache_native(&app)
    })
    .await
    .map_err(|error| format!("WebKit 缓存清理任务失败: {error}"))??;
    Ok("已完成 WebKit 磁盘与内存浏览缓存清理；聊天、附件与设置保留".to_string())
}

/// 3. 重建系统缓存与性能物理整理 (Level 3)
/// 物理抹除 V8 code_cache 字节码编译缓存并运行 SQLite 碎片整理，坚决不清理同步日志诊断数据。
#[tauri::command]
pub async fn reconstruct_system_cache(
    app: AppHandle,
    db_state: State<'_, DbState>,
) -> Result<String, String> {
    let mut cleared_details = String::new();

    // 1. 保留 Android 和 host 开发分支原有 V8 清理；iOS WebKit 不使用此目录。
    #[cfg(not(target_os = "ios"))]
    if let Ok(cache_dir) = get_cache_root_dir(&app) {
        let code_cache_dir = cache_dir.join("code_cache");
        if code_cache_dir.exists() {
            if tokio::fs::remove_dir_all(&code_cache_dir).await.is_ok() {
                cleared_details.push_str("V8 code_cache 已彻底物理清除；");
            } else {
                cleared_details.push_str("V8 code_cache 部分锁定，已标记失效；");
            }
        } else {
            cleared_details.push_str("V8 code_cache 无残余物理文件；");
        }
    }
    #[cfg(target_os = "ios")]
    let _ = app;

    // 2. SQLite 物理空间碎片真空整理与查询规划器优化
    // 分批回收 500 个 Page，避免造成单次大 Vacuum 导致长时间的 I/O 阻塞与锁竞争
    db_state
        .run_incremental_vacuum_optimize(500)
        .await
        .map_err(|error| format!("数据库维护失败: {error}"))?;
    cleared_details.push_str("SQLite 空间碎片整理与索引规划器重构已执行；");

    #[cfg(not(target_os = "ios"))]
    let result = format!(
        "系统缓存重建与数据库真空物理收缩完成 ({})",
        cleared_details.trim_end_matches('；')
    );
    #[cfg(target_os = "ios")]
    let result = format!(
        "数据库维护完成 ({})",
        cleared_details.trim_end_matches('；')
    );
    Ok(result)
}

/// 初始化自动维护逻辑（在 App 启动时调用）。
pub async fn init_automatic_maintenance(app: AppHandle) {
    // 异步清理超过 24 小时的孤立 SSE 缓存文件，防止磁盘文件泄露
    let app_clone = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Ok(cache_dir) = get_cache_root_dir(&app_clone) else {
            return;
        };
        let sse_cache_dir = cache_dir.join("sse_cache");
        if !sse_cache_dir.exists() {
            return;
        }
        let Ok(entries) = std::fs::read_dir(sse_cache_dir) else {
            return;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let Ok(modified) = metadata.modified() else {
                continue;
            };
            let Ok(elapsed) = modified.elapsed() else {
                continue;
            };
            if elapsed.as_secs() > 24 * 3600 {
                log::info!(
                    "[Maintenance] Deleting orphaned SSE cache file older than 24 hours: {:?}",
                    path
                );
                let _ = std::fs::remove_file(path);
            }
        }
    });

    let db_state = app.state::<DbState>();
    let last_maintenance = match last_automatic_maintenance(&db_state.pool).await {
        Ok(time) => time,
        Err(error) => {
            log::warn!("[Maintenance] Scheduled maintenance state unavailable: {error}");
            return;
        }
    };

    let now = now_secs();

    let three_days_secs = 3 * 24 * 60 * 60;

    if now - last_maintenance > three_days_secs {
        log::info!("[Maintenance] Triggering scheduled SQLite maintenance...");

        // 1. SQLite 物理空间回收与查询规划器优化
        let _ = db_state.run_incremental_vacuum_optimize(100).await;

        // 2. 自动清除已删除消息的多余附件关联
        let cleanup_result = async {
            let mut tx = db_state
                .write_transaction("maintenance.attachment-relations")
                .await?;
            sqlx::query(
                "DELETE FROM message_attachments WHERE (owner_type, owner_id, topic_id, msg_id) IN (\
                 SELECT ma.owner_type, ma.owner_id, ma.topic_id, ma.msg_id FROM message_attachments ma \
                 INNER JOIN messages m ON ma.owner_type = m.owner_type AND ma.owner_id = m.owner_id \
                    AND ma.topic_id = m.topic_id AND ma.msg_id = m.msg_id \
                 WHERE m.deleted_at IS NOT NULL)",
            )
            .execute(&mut *tx)
            .await
            .map_err(|error| error.to_string())?;
            tx.commit().await.map_err(|error| error.to_string())?;
            Ok::<(), String>(())
        }
        .await;
        if let Err(error) = cleanup_result {
            log::warn!("[Maintenance] Attachment relation cleanup failed: {error}");
        }

        // 3. 更新时间戳
        if let Err(error) = record_automatic_maintenance(&db_state, now).await {
            log::warn!("[Maintenance] Failed to record maintenance completion: {error}");
        }
        log::info!("[Maintenance] Scheduled maintenance complete.");
    }
}

#[cfg(test)]
mod tests {
    use super::{
        last_automatic_maintenance, load_attachment_gc_snapshot, record_automatic_maintenance,
    };

    #[tokio::test]
    async fn maintenance_completion_uses_its_own_record_without_rewriting_configuration() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open test database");
        sqlx::raw_sql(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at BIGINT NOT NULL);\
             CREATE TABLE config_domains (domain TEXT PRIMARY KEY, revision BIGINT, value TEXT);\
             INSERT INTO config_domains VALUES ('voice', 9, 'preserve raw value');\
             INSERT INTO settings VALUES ('cached_models', 'preserve cache', 1);",
        )
        .execute(&pool)
        .await
        .expect("create storage records");
        let db = crate::vcp_modules::db_manager::DbState::new(pool, Default::default());
        assert_eq!(last_automatic_maintenance(&db.pool).await.unwrap(), 0);
        record_automatic_maintenance(&db, 100).await.unwrap();
        record_automatic_maintenance(&db, 200).await.unwrap();
        assert_eq!(last_automatic_maintenance(&db.pool).await.unwrap(), 200);
        let domain: (i64, String) =
            sqlx::query_as("SELECT revision, value FROM config_domains WHERE domain = 'voice'")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(domain, (9, "preserve raw value".into()));
        let cache: String = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'cached_models'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(cache, "preserve cache");
        let global: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM settings WHERE key = 'global'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(global, 0);
    }

    async fn create_live_set_tables(pool: &sqlx::SqlitePool) {
        sqlx::query(
            "CREATE TABLE message_attachments (
                owner_type TEXT NOT NULL,
                owner_id TEXT NOT NULL,
                topic_id TEXT NOT NULL,
                msg_id TEXT NOT NULL,
                hash TEXT NOT NULL,
                display_name TEXT,
                src TEXT,
                status TEXT
             )",
        )
        .execute(pool)
        .await
        .expect("create message_attachments");
        sqlx::query(
            "CREATE TABLE messages (
                owner_type TEXT NOT NULL,
                owner_id TEXT NOT NULL,
                topic_id TEXT NOT NULL,
                msg_id TEXT NOT NULL,
                deleted_at INTEGER
             )",
        )
        .execute(pool)
        .await
        .expect("create messages");
        sqlx::query(
            "CREATE TABLE topics (
                owner_type TEXT NOT NULL,
                owner_id TEXT NOT NULL,
                topic_id TEXT NOT NULL,
                deleted_at INTEGER
             )",
        )
        .execute(pool)
        .await
        .expect("create topics");
        sqlx::query(
            "CREATE TABLE agents (owner_type TEXT, agent_id TEXT PRIMARY KEY, deleted_at INTEGER)",
        )
        .execute(pool)
        .await
        .expect("create agents");
        sqlx::query(
            "CREATE TABLE groups (owner_type TEXT, group_id TEXT PRIMARY KEY, deleted_at INTEGER)",
        )
        .execute(pool)
        .await
        .expect("create groups");
        sqlx::query(
            "CREATE TABLE current_share (singleton INTEGER PRIMARY KEY, record TEXT NOT NULL)",
        )
        .execute(pool)
        .await
        .expect("create current share");
    }

    #[tokio::test]
    async fn restart_gc_retains_unsent_share_files_until_the_share_is_discarded() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open database");
        create_live_set_tables(&pool).await;
        sqlx::query(
            "CREATE TABLE attachments (hash TEXT PRIMARY KEY, internal_path TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .expect("create attachment index");
        let mut record = serde_json::json!({
            "generation": 1, "deliveryId": "share-1", "state": "draft",
            "attachments": [{"hash": "draft-hash", "type": "image/png", "src": "/draft", "name": "draft.png", "size": 4}],
            "items": [{"itemId": "item-1", "fileName": "import.png", "attachment": {"hash": "import-hash", "type": "image/png", "src": "/import", "name": "import.png", "size": 4}}]
        });
        sqlx::query("INSERT INTO current_share VALUES (1, ?)")
            .bind(record.to_string())
            .execute(&pool)
            .await
            .expect("persist draft");
        let mut tx = pool.begin().await.expect("begin snapshot");
        let snapshot = load_attachment_gc_snapshot(&mut tx)
            .await
            .expect("read live refs");
        assert!(snapshot.live_hashes.contains("draft-hash"));
        assert!(snapshot.live_hashes.contains("import-hash"));
        tx.commit().await.expect("finish snapshot");

        record["state"] = "discarded".into();
        sqlx::query("UPDATE current_share SET record = ? WHERE singleton = 1")
            .bind(record.to_string())
            .execute(&pool)
            .await
            .expect("discard share");
        let mut tx = pool.begin().await.expect("begin later snapshot");
        assert!(load_attachment_gc_snapshot(&mut tx)
            .await
            .expect("read terminal refs")
            .live_hashes
            .is_empty());
    }

    #[tokio::test]
    async fn gc_snapshot_query_failure_yields_no_destructive_plan() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open test database");
        create_live_set_tables(&pool).await;

        // The first live-reference query is valid, but the attachment index query fails.
        // The caller receives no partial snapshot and therefore has nothing it may delete.
        let mut tx = pool.begin().await.expect("begin snapshot");
        let error = load_attachment_gc_snapshot(&mut tx)
            .await
            .err()
            .expect("missing attachments table must fail closed");
        assert!(error.contains("附件 GC 索引"));
    }

    #[tokio::test]
    async fn gc_snapshot_keeps_only_fully_live_attachment_hashes() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open test database");
        create_live_set_tables(&pool).await;
        sqlx::query(
            "CREATE TABLE attachments (hash TEXT PRIMARY KEY, internal_path TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .expect("create attachments");
        sqlx::query(
            "INSERT INTO agents VALUES
                ('agent', 'agent-live', NULL), ('agent', 'agent-deleted', 7)",
        )
        .execute(&pool)
        .await
        .expect("insert owners");
        sqlx::query(
            "INSERT INTO topics VALUES
                ('agent', 'agent-live', 'topic-live', NULL),
                ('agent', 'agent-deleted', 'topic-deleted', NULL)",
        )
        .execute(&pool)
        .await
        .expect("insert topics");
        sqlx::query(
            "INSERT INTO messages VALUES
                ('agent', 'agent-live', 'topic-live', 'message-live', NULL),
                ('agent', 'agent-deleted', 'topic-deleted', 'message-deleted', NULL)",
        )
        .execute(&pool)
        .await
        .expect("insert messages");
        sqlx::query(
            "INSERT INTO message_attachments
                (owner_type, owner_id, topic_id, msg_id, hash) VALUES
                ('agent', 'agent-live', 'topic-live', 'message-live', 'live-hash'),
                ('agent', 'agent-deleted', 'topic-deleted', 'message-deleted', 'deleted-hash')",
        )
        .execute(&pool)
        .await
        .expect("insert relations");
        sqlx::query(
            "INSERT INTO attachments VALUES
                ('live-hash', '/live'),
                ('deleted-hash', '/deleted')",
        )
        .execute(&pool)
        .await
        .expect("insert attachment index");

        let mut tx = pool.begin().await.expect("begin snapshot");
        let snapshot = load_attachment_gc_snapshot(&mut tx)
            .await
            .expect("complete live-set snapshot");
        assert!(snapshot.live_hashes.contains("live-hash"));
        assert!(!snapshot.live_hashes.contains("deleted-hash"));
        assert_eq!(snapshot.indexed_attachments.len(), 2);
    }

    #[tokio::test]
    async fn desktop_only_relation_remains_live_without_a_physical_path() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open test database");
        create_live_set_tables(&pool).await;
        sqlx::query(
            "CREATE TABLE attachments (hash TEXT PRIMARY KEY, internal_path TEXT NOT NULL);
             INSERT INTO agents VALUES ('agent', 'agent', NULL);
             INSERT INTO topics VALUES ('agent', 'agent', 'topic', NULL);
             INSERT INTO messages VALUES ('agent', 'agent', 'topic', 'message', NULL);
             INSERT INTO attachments VALUES ('desktop-hash', '');
             INSERT INTO message_attachments
                (owner_type, owner_id, topic_id, msg_id, hash, display_name, src, status)
             VALUES
                ('agent', 'agent', 'topic', 'message', 'desktop-hash', 'desktop.pdf', NULL, 'desktop_only');",
        )
        .execute(&pool)
        .await
        .expect("create desktop-only fixture");

        let mut tx = pool.begin().await.expect("begin snapshot");
        let snapshot = load_attachment_gc_snapshot(&mut tx)
            .await
            .expect("desktop-only attachment is a logical live reference");
        assert!(snapshot.live_hashes.contains("desktop-hash"));
        tx.commit().await.expect("finish read snapshot");
        let relation: (String, Option<String>) = sqlx::query_as(
            "SELECT status, src FROM message_attachments WHERE hash = 'desktop-hash'",
        )
        .fetch_one(&pool)
        .await
        .expect("read preserved relation");
        assert_eq!(relation, ("desktop_only".into(), None));
    }

    #[tokio::test]
    async fn gc_snapshot_marks_unreferenced_index_as_orphan() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open test database");
        create_live_set_tables(&pool).await;
        sqlx::query(
            "CREATE TABLE attachments (hash TEXT PRIMARY KEY, internal_path TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .expect("create attachments");
        let temp = tempfile::tempdir().expect("physical attachment directory");
        let candidate = temp.path().join("orphan.bin");
        std::fs::write(&candidate, b"preserve online").expect("physical candidate");
        sqlx::query("INSERT INTO attachments VALUES ('orphan-hash', ?)")
            .bind(candidate.to_string_lossy().as_ref())
            .execute(&pool)
            .await
            .expect("insert orphan index");

        let mut tx = pool.begin().await.expect("begin snapshot");
        let snapshot = load_attachment_gc_snapshot(&mut tx)
            .await
            .expect("read GC snapshot");
        assert!(!snapshot.live_hashes.contains("orphan-hash"));
        assert_eq!(snapshot.indexed_attachments.len(), 1);
        tx.commit().await.expect("finish read snapshot");
        let index_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM attachments WHERE hash = 'orphan-hash'")
                .fetch_one(&pool)
                .await
                .expect("read preserved index");
        assert_eq!(index_count, 1);
        assert_eq!(
            std::fs::read(&candidate).expect("physical candidate remains"),
            b"preserve online"
        );
    }
}
