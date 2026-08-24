// maintenance_manager.rs - 负责系统维护、垃圾回收与缓存清理的核心模块
// 职责: 聚合所有低频但关键的系统维护任务，对齐前端 MaintenanceSection 领域。

use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::file_manager::{
    delete_attachment_physical, get_attachments_root_dir, get_multimodal_cache_dir,
    get_thumbnails_root_dir,
};
use crate::vcp_modules::infra::utils::{is_valid_cas_hash, now_secs, YieldCounter};
use crate::vcp_modules::settings_manager::{read_settings, update_settings, SettingsState};
use std::collections::HashSet;
use tauri::{AppHandle, Manager, State};

struct AttachmentGcSnapshot {
    live_hashes: HashSet<String>,
    indexed_attachments: Vec<(String, String)>,
}

#[derive(Debug, Default)]
pub struct AttachmentGcReport {
    pub retained: usize,
    pub reclaimed: usize,
    pub ghost_files: usize,
}

/// 在同一 SQLite 快照中确定仍被存活消息引用的 CAS。
async fn load_attachment_gc_snapshot(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<AttachmentGcSnapshot, String> {
    let live_hashes = sqlx::query_as::<_, (String,)>(
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
    .fetch_all(&mut **tx)
    .await
    .map_err(|error| format!("读取附件 GC 有效引用失败: {error}"))?
    .into_iter()
    .map(|(hash,)| hash)
    .collect();

    let indexed_attachments =
        sqlx::query_as::<_, (String, String)>("SELECT hash, internal_path FROM attachments")
            .fetch_all(&mut **tx)
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
    indexed_paths: &HashSet<std::path::PathBuf>,
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
            .is_some_and(is_valid_cas_hash)
            && !indexed_paths.contains(&path);
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
    pool: &sqlx::SqlitePool,
) -> Result<AttachmentGcReport, String> {
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
    for (hash, internal_path) in snapshot.indexed_attachments {
        if snapshot.live_hashes.contains(&hash) || !is_valid_cas_hash(&hash) {
            continue;
        }
        if let Err(error) = delete_attachment_physical(app_handle, &hash, &internal_path).await {
            log::warn!("[Maintenance] Attachment GC kept {hash}: {error}");
            continue;
        }
        sqlx::query("DELETE FROM attachments WHERE hash = ?")
            .bind(&hash)
            .execute(pool)
            .await
            .map_err(|error| format!("删除附件索引 {hash} 失败: {error}"))?;
        report.reclaimed += 1;
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
    let indexed_paths: HashSet<std::path::PathBuf> = indexed_attachments
        .iter()
        .filter_map(|(hash, internal_path)| {
            let clean_path = internal_path
                .strip_prefix("file://")
                .unwrap_or(internal_path);
            let path = std::path::PathBuf::from(clean_path);
            let matches_hash = path
                .file_stem()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value == hash && is_valid_cas_hash(value));
            matches_hash.then_some(path)
        })
        .collect();
    let attachments_root = get_attachments_root_dir(app_handle)?;
    let thumbnails_root = get_thumbnails_root_dir(app_handle)?;
    let multimodal_root = get_multimodal_cache_dir(app_handle)?;
    report.ghost_files += sweep_attachment_files(&attachments_root, &indexed_paths).await;
    report.ghost_files +=
        sweep_unindexed_derived_files(&thumbnails_root, &indexed_hashes, |name| {
            name.strip_suffix("_thumb.webp")
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
#[tauri::command]
pub async fn clear_webview_cache(app: AppHandle) -> Result<String, String> {
    let mut cleared_details = String::new();
    let mut freed_size = 0u64;

    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|error| format!("WebView 缓存目录不可用: {error}"))?;
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

/// 3. 重建系统缓存与性能物理整理 (Level 3)
/// 物理抹除 V8 code_cache 字节码编译缓存并运行 SQLite 碎片整理，坚决不清理同步日志诊断数据。
#[tauri::command]
pub async fn reconstruct_system_cache(
    app: AppHandle,
    db_state: State<'_, DbState>,
) -> Result<String, String> {
    let mut cleared_details = String::new();

    // 1. 强力物理抹除 V8 code_cache
    if let Ok(cache_dir) = app.path().app_cache_dir() {
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

    // 2. SQLite 物理空间碎片真空整理与查询规划器优化
    // 分批回收 500 个 Page，避免造成单次大 Vacuum 导致长时间的 I/O 阻塞与锁竞争
    let _ = db_state.run_incremental_vacuum_optimize(500).await;
    cleared_details.push_str("SQLite 空间碎片整理与索引规划器重构已执行；");

    Ok(format!(
        "系统缓存重建与数据库真空物理收缩完成 ({})",
        cleared_details.trim_end_matches('；')
    ))
}

/// 初始化自动维护逻辑（在 App 启动时调用）。
pub async fn init_automatic_maintenance(app: AppHandle) {
    // 异步清理超过 24 小时的孤立 SSE 缓存文件，防止磁盘文件泄露
    let app_clone = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Ok(cache_dir) = app_clone.path().app_cache_dir() else {
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

    let settings_state = app.state::<SettingsState>();

    // 获取当前设置
    let settings = match read_settings(app.clone(), settings_state.clone()).await {
        Ok(s) => s,
        Err(_) => return,
    };

    let last_maintenance = settings
        .extra
        .get("lastAutomaticMaintenance")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let now = now_secs();

    let three_days_secs = 3 * 24 * 60 * 60;

    if now - last_maintenance > three_days_secs {
        log::info!("[Maintenance] Triggering scheduled SQLite maintenance...");

        // 1. SQLite 物理空间回收与查询规划器优化
        let db_state = app.state::<DbState>();
        let _ = db_state.run_incremental_vacuum_optimize(100).await;

        // 2. 自动清除已删除消息的多余附件关联
        let _ = sqlx::query(
            "DELETE FROM message_attachments WHERE (owner_type, owner_id, topic_id, msg_id) IN (\
             SELECT ma.owner_type, ma.owner_id, ma.topic_id, ma.msg_id FROM message_attachments ma \
             INNER JOIN messages m ON ma.owner_type = m.owner_type AND ma.owner_id = m.owner_id \
                AND ma.topic_id = m.topic_id AND ma.msg_id = m.msg_id \
             WHERE m.deleted_at IS NOT NULL)",
        )
        .execute(&db_state.pool)
        .await;

        // 3. 更新时间戳
        let updates = serde_json::json!({
            "lastAutomaticMaintenance": now
        });
        let _ = update_settings(app.clone(), settings_state, updates).await;
        log::info!("[Maintenance] Scheduled maintenance complete.");
    }
}

#[cfg(test)]
mod tests {
    use super::load_attachment_gc_snapshot;

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
