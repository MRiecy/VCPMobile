// maintenance_manager.rs - 负责系统维护、垃圾回收与缓存清理的核心模块
// 职责: 聚合所有低频但关键的系统维护任务，对齐前端 MaintenanceSection 领域。

use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::infra::utils::{is_valid_cas_hash, now_secs, YieldCounter};
use crate::vcp_modules::settings_manager::{read_settings, update_settings, SettingsState};
use std::collections::HashSet;
use tauri::{AppHandle, Manager, State};

struct AttachmentGcSnapshot {
    live_hashes: HashSet<String>,
    indexed_attachments: Vec<(String, String)>,
}

struct AttachmentGcPlan {
    live_count: usize,
    orphaned_indexed_count: usize,
}

/// Load every input used by online attachment diagnostics from one SQLite snapshot.
/// Callers must not mutate logical tombstones unless this succeeds.
async fn load_attachment_gc_snapshot(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<AttachmentGcSnapshot, String> {
    let live_hashes = sqlx::query_as::<_, (String,)>(
        "SELECT DISTINCT ma.hash FROM message_attachments ma \
             INNER JOIN messages m ON ma.owner_type = m.owner_type AND ma.owner_id = m.owner_id \
                AND ma.topic_id = m.topic_id AND ma.msg_id = m.msg_id \
             INNER JOIN topics t ON m.owner_type = t.owner_type AND m.owner_id = t.owner_id \
                AND m.topic_id = t.topic_id \
             LEFT JOIN agents a ON a.owner_type = 'agent' AND t.owner_id = a.agent_id AND t.owner_type = 'agent' \
             LEFT JOIN groups g ON g.owner_type = 'group' AND t.owner_id = g.group_id AND t.owner_type = 'group' \
             WHERE m.deleted_at IS NULL \
               AND ma.deleted_at IS NULL \
               AND t.deleted_at IS NULL \
               AND (t.owner_type != 'agent' OR a.deleted_at IS NULL) \
               AND (t.owner_type != 'group' OR g.deleted_at IS NULL)",
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

async fn apply_attachment_gc_index_mutations(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    _orphaned_indexed: &[(String, String)],
) -> Result<(), String> {
    sqlx::query(
        "UPDATE message_attachments \
         SET display_name = '[附件已删除]', src = NULL, status = 'removed' \
         WHERE deleted_at IS NOT NULL \
            OR (owner_type, owner_id, topic_id, msg_id) IN (\
                SELECT owner_type, owner_id, topic_id, msg_id \
                FROM messages WHERE deleted_at IS NOT NULL\
            ) \
            OR (owner_type, owner_id, topic_id) IN (\
                SELECT owner_type, owner_id, topic_id FROM topics \
                WHERE deleted_at IS NOT NULL \
                   OR owner_type = 'agent' AND owner_id IN (\
                       SELECT agent_id FROM agents WHERE owner_type = 'agent' AND deleted_at IS NOT NULL\
                   ) \
                   OR owner_type = 'group' AND owner_id IN (\
                       SELECT group_id FROM groups WHERE owner_type = 'group' AND deleted_at IS NOT NULL\
                   ) \
            )",
    )
    .execute(&mut **tx)
    .await
    .map_err(|e| format!("更新附件墓碑失败: {e}"))?;

    // 在线维护只允许逻辑墓碑更新。附件索引与物理文件必须保留，直到后续
    // owner + quarantine/grace 协议能把新引用与 unlink 线性化。
    Ok(())
}

async fn prepare_attachment_gc(pool: &sqlx::SqlitePool) -> Result<AttachmentGcPlan, String> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|error| format!("开启附件 GC 快照失败: {error}"))?;
    let snapshot = load_attachment_gc_snapshot(&mut tx).await?;
    let orphaned_indexed: Vec<(String, String)> = snapshot
        .indexed_attachments
        .into_iter()
        .filter(|(hash, _)| !snapshot.live_hashes.contains(hash))
        .collect();

    // Snapshot and logical tombstone updates share one transaction. If another connection
    // commits after the read snapshot, SQLite rejects the stale read-to-write upgrade.
    // Physical files and attachment indices are deliberately outside this online action.
    apply_attachment_gc_index_mutations(&mut tx, &orphaned_indexed).await?;
    tx.commit()
        .await
        .map_err(|error| format!("提交附件 GC 索引事务失败: {error}"))?;

    Ok(AttachmentGcPlan {
        live_count: snapshot.live_hashes.len(),
        orphaned_indexed_count: orphaned_indexed.len(),
    })
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

/// 1. 清理 WebView 缓存 (Level 1)
/// 调用 Tauri v2 原生接口清除浏览数据 (HTTP Cache, Images, etc.)，并物理抹除磁盘 HTTP Cache。
/// 提示：此操作仅处理网络与媒体层静态资源，V8 code_cache 字节码由 Level 3 重建管理。
#[tauri::command]
pub async fn clear_webview_cache(app: AppHandle) -> Result<String, String> {
    let mut cleared_details = String::new();
    let mut freed_size = 0u64;

    // 1. 调用内置接口清除 WebView 的内存和浏览状态数据
    if let Some(webview) = app.get_webview_window("main") {
        webview
            .clear_all_browsing_data()
            .map_err(|e| format!("WebView 缓存清理失败: {}", e))?;
        cleared_details.push_str("标准浏览数据已清除；");
    } else {
        cleared_details.push_str("未找到主窗口，跳过标准清理；");
    }

    // 2. 物理清除 HTTP 缓存
    if let Ok(cache_dir) = app.path().app_cache_dir() {
        let http_cache_dir = cache_dir.join("WebView").join("Default").join("HTTP Cache");
        if http_cache_dir.exists() {
            // 在物理删除前先统计大小
            freed_size = calculate_dir_size(&http_cache_dir).await;

            if tokio::fs::remove_dir_all(&http_cache_dir).await.is_ok() {
                cleared_details.push_str("物理 HTTP Cache 已抹除；");
            } else {
                freed_size = 0;
                cleared_details.push_str("部分 HTTP 物理缓存被占用，已标记失效；");
            }
        }
    }

    let freed_size_mb = (freed_size as f64) / 1024.0 / 1024.0;
    Ok(format!(
        "WebView 缓存清理成功 ({})，释放空间: {:.2} MB",
        cleared_details.trim_end_matches('；'),
        freed_size_mb
    ))
}

/// 2. 检查孤儿附件 (Level 2)
/// 在线阶段只统计候选并更新已删除消息的逻辑墓碑，不删除索引或物理文件。
#[tauri::command]
pub async fn cleanup_orphaned_attachments(
    _app_handle: AppHandle,
    db_state: State<'_, DbState>,
) -> Result<String, String> {
    // A complete snapshot is still required for diagnostics and logical tombstones. Online
    // physical deletion is intentionally disabled: a reference may be added after any DB
    // commit, so unlinking without a shared attachment owner can delete newly-live CAS data.
    let plan = prepare_attachment_gc(&db_state.pool).await?;
    Ok(format!(
        "已完成附件引用检查：有效引用 {} 个，孤儿候选 {} 个；在线物理回收已延期，未删除附件索引、主文件、缩略图或多模态缓存",
        plan.live_count, plan.orphaned_indexed_count
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

/// 2.5 检查单个孤儿附件 (供前端取消暂存时调用)
/// 当前仅确认引用状态；物理回收等待 owner + quarantine/grace 协议。
#[tauri::command]
pub async fn cleanup_single_orphaned_attachment(
    _app_handle: AppHandle,
    db_state: State<'_, DbState>,
    hash: String,
) -> Result<String, String> {
    if !is_valid_cas_hash(&hash) {
        return Err("附件哈希格式无效".to_string());
    }
    // 1. 查 message_attachments 确定该 hash 是否被有效历史消息引用
    let is_used: bool = sqlx::query_scalar::<_, i32>(
        "SELECT EXISTS(\
         SELECT 1 FROM message_attachments ma \
         INNER JOIN messages m ON ma.owner_type = m.owner_type AND ma.owner_id = m.owner_id \
            AND ma.topic_id = m.topic_id AND ma.msg_id = m.msg_id \
         WHERE ma.hash = ? AND m.deleted_at IS NULL)",
    )
    .bind(&hash)
    .fetch_one(&db_state.pool)
    .await
    .map_err(|e| e.to_string())?
        != 0;

    if is_used {
        return Ok("附件已被其他消息引用，跳过清理".to_string());
    }

    Ok("附件当前未被有效消息引用；在线物理回收已延期，文件与索引保持不变".to_string())
}

/// 3. 初始化自动维护逻辑 (在 App 启动时调用)
///    如果距离上次清理超过 3 天，则自动触发一次 WebView 缓存清理
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

    // 从 extra 中提取上次清理时间
    let last_clear = settings
        .extra
        .get("lastWebviewCacheClear")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let now = now_secs();

    let three_days_secs = 3 * 24 * 60 * 60;

    if now - last_clear > three_days_secs {
        log::info!("[Maintenance] Triggering scheduled maintenance (WebView & SQLite)...");

        // 1. WebView 清理
        if let Some(webview) = app.get_webview_window("main") {
            let _ = webview.clear_all_browsing_data();
        }

        // 2. SQLite 物理空间回收与查询规划器优化
        let db_state = app.state::<DbState>();
        let _ = db_state.run_incremental_vacuum_optimize(100).await;

        // 3. 自动清除已删除消息的多余附件关联 (防线二：自动维护自愈)
        let _ = sqlx::query(
            "DELETE FROM message_attachments WHERE (owner_type, owner_id, topic_id, msg_id) IN (\
             SELECT ma.owner_type, ma.owner_id, ma.topic_id, ma.msg_id FROM message_attachments ma \
             INNER JOIN messages m ON ma.owner_type = m.owner_type AND ma.owner_id = m.owner_id \
                AND ma.topic_id = m.topic_id AND ma.msg_id = m.msg_id \
             WHERE m.deleted_at IS NOT NULL)",
        )
        .execute(&db_state.pool)
        .await;

        // 更新时间戳
        let updates = serde_json::json!({
            "lastWebviewCacheClear": now
        });
        let _ = update_settings(app.clone(), settings_state, updates).await;
        log::info!("[Maintenance] Scheduled maintenance complete.");
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_attachment_gc_index_mutations, load_attachment_gc_snapshot, prepare_attachment_gc,
    };

    async fn create_live_set_tables(pool: &sqlx::SqlitePool) {
        sqlx::query(
            "CREATE TABLE message_attachments (
                topic_id TEXT NOT NULL,
                msg_id TEXT NOT NULL,
                hash TEXT NOT NULL,
                deleted_at INTEGER,
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
                topic_id TEXT PRIMARY KEY,
                owner_id TEXT NOT NULL,
                owner_type TEXT NOT NULL,
                deleted_at INTEGER
             )",
        )
        .execute(pool)
        .await
        .expect("create topics");
        sqlx::query("CREATE TABLE agents (agent_id TEXT PRIMARY KEY, deleted_at INTEGER)")
            .execute(pool)
            .await
            .expect("create agents");
        sqlx::query("CREATE TABLE groups (group_id TEXT PRIMARY KEY, deleted_at INTEGER)")
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
        let error = prepare_attachment_gc(&pool)
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
        sqlx::query("INSERT INTO agents VALUES ('agent-live', NULL), ('agent-deleted', 7)")
            .execute(&pool)
            .await
            .expect("insert owners");
        sqlx::query(
            "INSERT INTO topics VALUES
                ('topic-live', 'agent-live', 'agent', NULL),
                ('topic-deleted', 'agent-deleted', 'agent', NULL)",
        )
        .execute(&pool)
        .await
        .expect("insert topics");
        sqlx::query(
            "INSERT INTO messages VALUES
                ('topic-live', 'message-live', NULL),
                ('topic-deleted', 'message-deleted', NULL)",
        )
        .execute(&pool)
        .await
        .expect("insert messages");
        sqlx::query(
            "INSERT INTO message_attachments (topic_id, msg_id, hash, deleted_at) VALUES
                ('topic-live', 'message-live', 'live-hash', NULL),
                ('topic-deleted', 'message-deleted', 'deleted-hash', NULL)",
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
             INSERT INTO agents VALUES ('agent', NULL);
             INSERT INTO topics VALUES ('topic', 'agent', 'agent', NULL);
             INSERT INTO messages VALUES ('topic', 'message', NULL);
             INSERT INTO attachments VALUES ('desktop-hash', '');
             INSERT INTO message_attachments
                (topic_id, msg_id, hash, deleted_at, display_name, src, status)
             VALUES
                ('topic', 'message', 'desktop-hash', NULL, 'desktop.pdf', NULL, 'desktop_only');",
        )
        .execute(&pool)
        .await
        .expect("create desktop-only fixture");

        let plan = prepare_attachment_gc(&pool)
            .await
            .expect("desktop-only attachment is a logical live reference");
        assert_eq!(plan.live_count, 1);
        assert_eq!(plan.orphaned_indexed_count, 0);
        let relation: (String, Option<String>) = sqlx::query_as(
            "SELECT status, src FROM message_attachments WHERE hash = 'desktop-hash'",
        )
        .fetch_one(&pool)
        .await
        .expect("read preserved relation");
        assert_eq!(relation, ("desktop_only".into(), None));
    }

    #[tokio::test]
    async fn online_gc_keeps_orphan_index_and_physical_candidate() {
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

        let plan = prepare_attachment_gc(&pool).await.expect("online GC plan");

        assert_eq!(plan.orphaned_indexed_count, 1);
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

    #[tokio::test]
    async fn concurrent_reference_prevents_stale_gc_plan_from_committing() {
        let temp = tempfile::tempdir().expect("temp database directory");
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(temp.path().join("gc.sqlite"))
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_millis(100));
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .expect("open WAL test database");
        create_live_set_tables(&pool).await;
        sqlx::query(
            "CREATE TABLE attachments (hash TEXT PRIMARY KEY, internal_path TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .expect("create attachments");
        sqlx::query("INSERT INTO agents VALUES ('agent', NULL)")
            .execute(&pool)
            .await
            .expect("insert owner");
        sqlx::query("INSERT INTO topics VALUES ('topic', 'agent', 'agent', NULL)")
            .execute(&pool)
            .await
            .expect("insert topic");
        sqlx::query("INSERT INTO messages VALUES ('topic', 'message', NULL)")
            .execute(&pool)
            .await
            .expect("insert message");
        sqlx::query("INSERT INTO attachments VALUES ('raced-hash', '/raced')")
            .execute(&pool)
            .await
            .expect("insert attachment index");

        let mut stale_tx = pool.begin().await.expect("begin stale GC snapshot");
        let snapshot = load_attachment_gc_snapshot(&mut stale_tx)
            .await
            .expect("read stale GC snapshot");
        let orphaned: Vec<_> = snapshot
            .indexed_attachments
            .into_iter()
            .filter(|(hash, _)| !snapshot.live_hashes.contains(hash))
            .collect();
        assert_eq!(orphaned.len(), 1);

        // A second WAL connection makes the previously orphaned hash live after the
        // first connection has established its read snapshot.
        sqlx::query(
            "INSERT INTO message_attachments (topic_id, msg_id, hash, deleted_at)
             VALUES ('topic', 'message', 'raced-hash', NULL)",
        )
        .execute(&pool)
        .await
        .expect("publish concurrent attachment reference");

        assert!(
            apply_attachment_gc_index_mutations(&mut stale_tx, &orphaned)
                .await
                .is_err(),
            "SQLite must reject a stale read snapshot upgrading to destructive write"
        );
        stale_tx.rollback().await.ok();

        let index_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM attachments WHERE hash = 'raced-hash'")
                .fetch_one(&pool)
                .await
                .expect("read attachment index");
        let reference_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM message_attachments WHERE hash = 'raced-hash'",
        )
        .fetch_one(&pool)
        .await
        .expect("read attachment reference");
        assert_eq!(index_count, 1);
        assert_eq!(reference_count, 1);
    }
}
