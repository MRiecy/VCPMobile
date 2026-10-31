// maintenance_manager.rs - 负责系统维护、垃圾回收与缓存清理的核心模块
// 职责: 聚合所有低频但关键的系统维护任务，对齐前端 MaintenanceSection 领域。

use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::file_manager::{
    delete_attachment_physical, get_attachments_root_dir, get_cache_root_dir,
    get_multimodal_cache_dir, get_thumbnails_root_dir,
};
use crate::vcp_modules::infra::utils::{is_valid_cas_hash, now_secs, YieldCounter};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, State};

struct AttachmentGcSnapshot {
    live_hashes: HashSet<String>,
    indexed_attachments: HashMap<String, String>,
}

const LAST_AUTOMATIC_MAINTENANCE_KEY: &str = "last_automatic_maintenance";
const ATTACHMENT_GC_KEY: &str = "attachment_gc_candidates_v1";
const ATTACHMENT_SCAN_INTERVAL: i64 = 3 * 24 * 60 * 60;

async fn last_automatic_maintenance(pool: &sqlx::SqlitePool) -> Result<i64, String> {
    let value: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
        .bind(LAST_AUTOMATIC_MAINTENANCE_KEY)
        .fetch_optional(pool)
        .await
        .map_err(|error| format!("读取自动维护时间失败: {error}"))?;
    value.map_or(Ok(0), |value| {
        value
            .parse::<i64>()
            .ok()
            .filter(|time| *time >= 0)
            .ok_or_else(|| "自动维护时间格式无效".to_string())
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
    pub reclaimed: usize,
    pub ghost_files: usize,
}

// SQLite CROSS JOIN fixes loop order. For a pending plan the first step must
// seek idx_message_attachments_hash, then probe just those messages and topics.
const LIVE_ATTACHMENT_HASHES: &str = "SELECT DISTINCT ma.hash FROM message_attachments ma \
             CROSS JOIN messages m ON ma.owner_type = m.owner_type AND ma.owner_id = m.owner_id \
                AND ma.topic_id = m.topic_id AND ma.msg_id = m.msg_id \
             CROSS JOIN topics t ON m.owner_type = t.owner_type AND m.owner_id = t.owner_id \
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
               )";

/// Read live messages, recoverable share drafts and the CAS index at one revision.
async fn load_attachment_gc_snapshot(
    tx: &mut sqlx::SqliteConnection,
    candidate_hashes: Option<&str>,
) -> Result<AttachmentGcSnapshot, String> {
    // The cold-start variant probes the hash indexes; an optional OR predicate
    // in the full-scan SQL would still make SQLite visit unrelated references.
    let index_sql = "SELECT hash, internal_path FROM attachments";
    let (references, indexed) = match candidate_hashes {
        Some(hashes) => (
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "{LIVE_ATTACHMENT_HASHES} AND ma.hash IN (SELECT value FROM json_each(?))"
            )))
            .bind(hashes),
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "{index_sql} WHERE hash IN (SELECT value FROM json_each(?))"
            )))
            .bind(hashes),
        ),
        None => (
            sqlx::query_scalar(LIVE_ATTACHMENT_HASHES),
            sqlx::query_as(index_sql),
        ),
    };
    let mut live_hashes: HashSet<String> = references
        .fetch_all(&mut *tx)
        .await
        .map_err(|error| format!("读取附件 GC 有效引用失败: {error}"))?
        .into_iter()
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

    let indexed_attachments = indexed
        .fetch_all(&mut *tx)
        .await
        .map_err(|error| format!("读取附件 GC 索引失败: {error}"))?
        .into_iter()
        .collect();

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

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum GcDirectory {
    Attachments,
    Thumbnails,
    Multimodal,
}

struct GcRoots {
    attachments: PathBuf,
    thumbnails: PathBuf,
    multimodal: PathBuf,
}

impl GcRoots {
    fn for_app<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<Self, String> {
        Ok(Self {
            attachments: get_attachments_root_dir(app)?,
            thumbnails: get_thumbnails_root_dir(app)?,
            multimodal: get_multimodal_cache_dir(app)?,
        })
    }

    fn path(&self, directory: GcDirectory) -> &Path {
        match directory {
            GcDirectory::Attachments => &self.attachments,
            GcDirectory::Thumbnails => &self.thumbnails,
            GcDirectory::Multimodal => &self.multimodal,
        }
    }
}

const GC_DIRECTORIES: [GcDirectory; 3] = [
    GcDirectory::Attachments,
    GcDirectory::Thumbnails,
    GcDirectory::Multimodal,
];

async fn directory_exists(root: &Path) -> Result<bool, String> {
    match tokio::fs::symlink_metadata(root).await {
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => Err(format!("附件维护根不是普通目录: {}", root.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("无法检查附件目录 {}: {error}", root.display())),
    }
}

#[derive(Debug, Serialize, Deserialize)]
enum GcCandidate {
    Indexed(String),
    File(GcDirectory, String),
}

enum ManagedFile<'a> {
    Temporary,
    Cas(&'a str),
}

fn managed_file(directory: GcDirectory, name: &str) -> Option<ManagedFile<'_>> {
    if Path::new(name).file_name()?.to_str()? != name || name.contains(['/', '\\']) {
        return None;
    }
    if is_managed_attachment_temp(name) {
        return Some(ManagedFile::Temporary);
    }
    // native_media writes <CAS>.<hyphenated UUID>.tmp beside the final JSON.
    if matches!(directory, GcDirectory::Multimodal) {
        if let Some((hash, id)) = name.strip_suffix(".tmp").and_then(|stem| stem.split_once('.')) {
            if is_valid_cas_hash(hash) && id.len() == 36 && uuid::Uuid::parse_str(id).is_ok() {
                return Some(ManagedFile::Temporary);
            }
        }
    }
    let hash = match directory {
        GcDirectory::Attachments => Path::new(name).file_stem()?.to_str(),
        GcDirectory::Thumbnails => name
            .strip_suffix("_thumb.webp")
            .or_else(|| name.strip_suffix("_thumb.jpg"))
            .or_else(|| name.strip_suffix("_thumb.png")),
        GcDirectory::Multimodal => name.strip_suffix(".json"),
    }?;
    is_valid_cas_hash(hash).then_some(ManagedFile::Cas(hash))
}

async fn scan_attachment_gc(db: &DbState, roots: &GcRoots) -> Result<(), String> {
    let mut tx = db.pool.begin().await.map_err(|e| e.to_string())?;
    let snapshot = load_attachment_gc_snapshot(&mut tx, None).await?;
    tx.commit().await.map_err(|e| e.to_string())?;

    let mut candidates: Vec<GcCandidate> = snapshot
        .indexed_attachments
        .keys()
        .filter(|hash| is_valid_cas_hash(hash) && !snapshot.live_hashes.contains(*hash))
        .cloned()
        .map(GcCandidate::Indexed)
        .collect();
    for directory in GC_DIRECTORIES {
        let root = roots.path(directory);
        if !directory_exists(root).await? {
            continue;
        }
        let mut entries = tokio::fs::read_dir(root).await.map_err(|e| e.to_string())?;
        while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            match managed_file(directory, name) {
                None => continue,
                Some(ManagedFile::Cas(hash)) if snapshot.live_hashes.contains(hash) => continue,
                _ => {}
            }
            if entry
                .file_type()
                .await
                .map_err(|e| e.to_string())?
                .is_file()
            {
                candidates.push(GcCandidate::File(directory, name.to_owned()));
            }
        }
    }
    // A directory or database error leaves the previous plan and deadline intact.
    let value = serde_json::to_string(&candidates).map_err(|e| e.to_string())?;
    let mut tx = db
        .write_transaction("maintenance.attachment-gc-scan")
        .await?;
    sqlx::query(
        "INSERT INTO settings (key,value,updated_at) VALUES (?,?,?) \
        ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
    )
    .bind(ATTACHMENT_GC_KEY)
    .bind(value)
    .bind(now_secs())
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    tx.commit().await
}

fn seconds_until_attachment_scan(last: Option<i64>, now: i64) -> i64 {
    last.filter(|&stamp| (1..=now).contains(&stamp))
        .map_or(0, |stamp| (ATTACHMENT_SCAN_INTERVAL - (now - stamp)).max(0))
}

async fn next_attachment_scan<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<i64, String> {
    let db = app.state::<DbState>();
    let last: Option<i64> = sqlx::query_scalar("SELECT updated_at FROM settings WHERE key = ?")
        .bind(ATTACHMENT_GC_KEY)
        .fetch_optional(&db.pool)
        .await
        .map_err(|e| e.to_string())?;
    let remaining = seconds_until_attachment_scan(last, now_secs());
    if remaining > 0 {
        return Ok(remaining);
    }
    scan_attachment_gc(&db, &GcRoots::for_app(app)?).await?;
    Ok(ATTACHMENT_SCAN_INTERVAL)
}

async fn delete_candidate_file(root: &Path, name: &str) -> std::io::Result<bool> {
    let path = root.join(name);
    match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) if metadata.is_file() => {
            tokio::fs::remove_file(path).await?;
            Ok(true)
        }
        Ok(_) => Err(std::io::Error::other("拒绝删除非普通附件文件")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

// The cold-start owner runs before upload intake. Keep the write transaction
// through revalidation so a draft/message cannot acquire ownership mid-delete.
async fn consume_attachment_gc(
    db: &DbState,
    roots: &GcRoots,
) -> Result<AttachmentGcReport, String> {
    let mut tx = db.write_transaction("maintenance.attachment-gc").await?;
    let raw: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key=?")
        .bind(ATTACHMENT_GC_KEY)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    let candidates: Vec<GcCandidate> = serde_json::from_str(raw.as_deref().unwrap_or("[]"))
        .map_err(|e| format!("附件清理清单无效: {e}"))?;
    if candidates.is_empty() {
        return Ok(Default::default());
    }
    let mut hashes = HashSet::new();
    for candidate in &candidates {
        match candidate {
            GcCandidate::Indexed(hash) if is_valid_cas_hash(hash) => {
                hashes.insert(hash.as_str());
            }
            GcCandidate::File(directory, name) => match managed_file(*directory, name) {
                Some(ManagedFile::Cas(hash)) => {
                    hashes.insert(hash);
                }
                Some(ManagedFile::Temporary) => {}
                None => return Err(format!("附件清理文件名无效: {name}")),
            },
            _ => return Err("附件清理哈希无效".into()),
        }
    }
    let hashes = serde_json::to_string(&hashes).map_err(|e| e.to_string())?;
    let mut current = load_attachment_gc_snapshot(&mut tx, Some(&hashes)).await?;
    for directory in GC_DIRECTORIES {
        directory_exists(roots.path(directory)).await?;
    }
    let mut report = AttachmentGcReport::default();
    let mut failed = Vec::new();
    let mut failed_indexes = HashSet::new();
    for candidate in candidates {
        match &candidate {
            GcCandidate::Indexed(hash) => {
                if current.live_hashes.contains(hash) {
                    continue;
                }
                let Some(path) = current.indexed_attachments.get(hash) else {
                    continue;
                };
                if let Err(e) = delete_attachment_physical(
                    &roots.attachments,
                    &roots.thumbnails,
                    &roots.multimodal,
                    hash,
                    path,
                )
                .await
                {
                    log::warn!("[Maintenance] Retaining indexed attachment {hash}: {e}");
                    failed_indexes.insert(hash.clone());
                    failed.push(candidate);
                    continue;
                }
                sqlx::query("DELETE FROM attachments WHERE hash=?")
                    .bind(hash)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| e.to_string())?;
                current.indexed_attachments.remove(hash);
                report.reclaimed += 1;
            }
            GcCandidate::File(directory, name) => {
                if let Some(ManagedFile::Cas(hash)) = managed_file(*directory, name) {
                    // An indexed deletion can fail after partially unlinking its
                    // files. Keep its named siblings for the same cold-start retry.
                    if failed_indexes.contains(hash) {
                        failed.push(candidate);
                        continue;
                    }
                    if current.live_hashes.contains(hash)
                        || current.indexed_attachments.contains_key(hash)
                    {
                        continue;
                    }
                }
                match delete_candidate_file(roots.path(*directory), name).await {
                    Ok(removed) => report.ghost_files += usize::from(removed),
                    Err(error) => {
                        log::warn!("[Maintenance] Retaining attachment file {name}: {error}");
                        failed.push(candidate);
                    }
                }
            }
        }
    }
    sqlx::query("UPDATE settings SET value=? WHERE key=?")
        .bind(serde_json::to_string(&failed).map_err(|e| e.to_string())?)
        .bind(ATTACHMENT_GC_KEY)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await?;
    Ok(report)
}

/// Cold start only consumes the previous online scan; a new install has no plan.
pub async fn reclaim_orphaned_attachments<R: tauri::Runtime>(
    app: &AppHandle<R>,
    db: &DbState,
) -> Result<AttachmentGcReport, String> {
    let pending: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM settings WHERE key=? AND value!='[]')")
            .bind(ATTACHMENT_GC_KEY)
            .fetch_one(&db.pool)
            .await
            .map_err(|e| e.to_string())?;
    if !pending {
        return Ok(Default::default());
    }
    consume_attachment_gc(db, &GcRoots::for_app(app)?).await
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
    let gc_app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let delay = match next_attachment_scan(&gc_app).await {
                Ok(delay) => delay,
                Err(error) => {
                    log::warn!("[Maintenance] Attachment candidate scan failed: {error}");
                    ATTACHMENT_SCAN_INTERVAL
                }
            };
            tokio::time::sleep(std::time::Duration::from_secs(delay as u64)).await;
        }
    });
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

    if now.saturating_sub(last_maintenance) > three_days_secs {
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
        scan_attachment_gc, consume_attachment_gc, DbState, GcRoots, GC_DIRECTORIES, ATTACHMENT_GC_KEY,
    };

    #[test]
    fn attachment_gc_deadline_handles_first_boot_interval_and_clock_rollback() {
        use super::{seconds_until_attachment_scan as delay, ATTACHMENT_SCAN_INTERVAL as interval};
        assert_eq!(delay(None, 100), 0);
        assert_eq!(delay(Some(100), 100), interval);
        assert_eq!(delay(Some(100), 99 + interval), 1);
        assert_eq!(delay(Some(100), 100 + interval), 0);
        assert_eq!(delay(Some(i64::MAX), 100), 0);
    }

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
        for value in ["-1", "-9223372036854775808", "not-a-time", "9223372036854775808"] {
            sqlx::query("UPDATE settings SET value=? WHERE key='last_automatic_maintenance'")
                .bind(value).execute(&db.pool).await.unwrap();
            assert!(last_automatic_maintenance(&db.pool).await.is_err(), "{value}");
            let stored: String = sqlx::query_scalar("SELECT value FROM settings WHERE key='last_automatic_maintenance'")
                .fetch_one(&db.pool).await.unwrap();
            assert_eq!(stored, value, "invalid completion evidence must not become a zero/default schedule");
        }
        sqlx::query("UPDATE settings SET value=? WHERE key='last_automatic_maintenance'")
            .bind(i64::MAX.to_string()).execute(&db.pool).await.unwrap();
        assert_eq!(last_automatic_maintenance(&db.pool).await.unwrap(), i64::MAX);
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
    async fn cold_gc_revalidates_the_durable_plan_and_retries_file_failures() {
        async fn read_plan(db: &DbState) -> (String, i64) {
            sqlx::query_as("SELECT value,updated_at FROM settings WHERE key=?")
                .bind(ATTACHMENT_GC_KEY).fetch_one(&db.pool).await.unwrap()
        }
        let temp = tempfile::tempdir().unwrap();
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(temp.path().join("gc.sqlite")).create_if_missing(true);
        let connect = || sqlx::sqlite::SqlitePoolOptions::new().max_connections(1)
            .connect_with(options.clone());
        let pool = connect().await.unwrap();
        create_live_set_tables(&pool).await;
        sqlx::raw_sql("CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT NOT NULL,updated_at BIGINT NOT NULL);
            CREATE TABLE attachments(hash TEXT PRIMARY KEY,internal_path TEXT NOT NULL);
            INSERT INTO agents VALUES ('agent','owner',NULL);
            INSERT INTO topics VALUES ('agent','owner','topic',NULL);
            INSERT INTO messages VALUES ('agent','owner','topic','message',NULL);")
            .execute(&pool).await.unwrap();
        let db = DbState::new(pool, Default::default());
        let roots = GcRoots {
            attachments: temp.path().join("original"),
            thumbnails: temp.path().join("thumb"),
            multimodal: temp.path().join("multimodal"),
        };
        for dir in GC_DIRECTORIES {
            std::fs::create_dir(roots.path(dir)).unwrap();
        }
        let from_message = "a".repeat(64);
        let from_share = "b".repeat(64);
        let orphan = "c".repeat(64);
        let ghost = "d".repeat(64);
        let indexed_later = "e".repeat(64);
        let not_scanned = roots.attachments.join(format!("{}.bin", "f".repeat(64)));
        for hash in [&from_message, &from_share, &orphan, &ghost, &indexed_later] {
            let filename = format!("{hash}.bin");
            std::fs::write(roots.attachments.join(&filename), b"original").unwrap();
            std::fs::write(roots.thumbnails.join(format!("{hash}_thumb.jpg")), b"thumb").unwrap();
            std::fs::write(roots.multimodal.join(format!("{hash}.json")), b"json").unwrap();
        }
        for hash in [&from_message, &orphan] {
            sqlx::query("INSERT INTO attachments VALUES (?,?)")
                .bind(hash)
                .bind(format!("/old/sandbox/{hash}.bin"))
                .execute(&db.pool)
                .await
                .unwrap();
        }
        let stale_temp = roots
            .attachments
            .join(format!(".ingest-{}.tmp", uuid::Uuid::new_v4()));
        std::fs::write(&stale_temp, b"temp").unwrap();
        let extra_original = roots.attachments.join(format!("{orphan}.jpg"));
        std::fs::write(&extra_original, b"older extension").unwrap();
        let media_temp = format!("{from_message}.{}.tmp", uuid::Uuid::new_v4());
        let lookalike = format!("{from_message}.unmanaged.tmp");
        std::fs::write(roots.multimodal.join(&media_temp), b"unfinished").unwrap();
        std::fs::write(roots.attachments.join(&media_temp), b"not a managed temp here").unwrap();
        std::fs::write(roots.multimodal.join(&lookalike), b"unknown file").unwrap();
        scan_attachment_gc(&db, &roots).await.unwrap();
        assert!(stale_temp.exists());
        let planned = read_plan(&db).await;
        assert!(planned.1 > 0);
        // Fail after the first directory has already been scanned.
        let saved = temp.path().join("saved-thumbnails");
        std::fs::rename(&roots.thumbnails, &saved).unwrap();
        std::fs::write(&roots.thumbnails, b"not a directory").unwrap();
        assert!(scan_attachment_gc(&db, &roots).await.is_err());
        assert_eq!(read_plan(&db).await, planned);
        std::fs::remove_file(&roots.thumbnails).unwrap();
        std::fs::rename(saved, &roots.thumbnails).unwrap();

        sqlx::query(
            "INSERT INTO message_attachments(owner_type,owner_id,topic_id,msg_id,hash)
            VALUES ('agent','owner','topic','message',?)",
        )
        .bind(&from_message)
        .execute(&db.pool)
        .await
        .unwrap();
        let share = serde_json::json!({"generation":1, "deliveryId":"d", "state":"draft",
            "items":[{"itemId":"x", "fileName":"x.bin", "sha256":from_share}]});
        sqlx::query("INSERT INTO current_share VALUES (1,?)")
            .bind(share.to_string())
            .execute(&db.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO attachments VALUES (?,?)")
            .bind(&indexed_later)
            .bind(format!("{indexed_later}.bin"))
            .execute(&db.pool)
            .await
            .unwrap();
        std::fs::write(&not_scanned, b"arrived after scan").unwrap();
        // A candidate replaced with a directory must survive and stay on the retry list.
        std::fs::remove_file(&stale_temp).unwrap();
        std::fs::create_dir(&stale_temp).unwrap();
        let invalid_thumbnail = roots.thumbnails.join(format!("{orphan}_thumb.png"));
        std::fs::create_dir(&invalid_thumbnail).unwrap();
        db.pool.close().await;
        let db = DbState::new(connect().await.unwrap(), Default::default());
        let report = consume_attachment_gc(&db, &roots).await.unwrap();
        assert_eq!(report.reclaimed, 0);
        assert_eq!(report.ghost_files, 4);
        assert!(!roots.multimodal.join(&media_temp).exists());
        assert!(roots.attachments.join(&media_temp).exists());
        assert!(roots.multimodal.join(&lookalike).exists());
        assert!(not_scanned.exists() && stale_temp.is_dir());
        for hash in [&from_message, &from_share, &indexed_later, &orphan] {
            assert!(roots.attachments.join(format!("{hash}.bin")).exists());
            assert!(roots.thumbnails.join(format!("{hash}_thumb.jpg")).exists());
            assert!(roots.multimodal.join(format!("{hash}.json")).exists());
        }
        assert!(!roots.attachments.join(format!("{ghost}.bin")).exists());
        assert!(!roots.thumbnails.join(format!("{ghost}_thumb.jpg")).exists());
        assert!(!roots.multimodal.join(format!("{ghost}.json")).exists());
        let remaining: Vec<String> =
            sqlx::query_scalar("SELECT hash FROM attachments ORDER BY hash")
                .fetch_all(&db.pool)
                .await
                .unwrap();
        assert_eq!(remaining, [&from_message, &orphan, &indexed_later].map(String::as_str));
        let retry = read_plan(&db).await;
        assert!(retry.0.contains(".ingest-"));
        assert_eq!(retry.1, planned.1);
        std::fs::remove_dir(&stale_temp).unwrap();
        std::fs::remove_dir(&invalid_thumbnail).unwrap();
        let retried = consume_attachment_gc(&db, &roots).await.unwrap();
        assert_eq!((retried.reclaimed, retried.ghost_files), (1, 1));
        assert!(!roots.attachments.join(format!("{orphan}.bin")).exists());
        assert!(!roots.thumbnails.join(format!("{orphan}_thumb.jpg")).exists());
        assert!(!roots.multimodal.join(format!("{orphan}.json")).exists());
        assert!(!extra_original.exists());
        let orphan_index: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM attachments WHERE hash=?")
            .bind(&orphan).fetch_one(&db.pool).await.unwrap();
        assert_eq!(orphan_index, 0);
        assert_eq!(read_plan(&db).await, ("[]".into(), planned.1));

        let outside_name = format!("{}.bin", "9".repeat(64));
        let outside = temp.path().join(&outside_name);
        std::fs::write(&outside, b"outside CAS").unwrap();
        let bad_plan = serde_json::json!([{"File":["Attachments", format!("../{outside_name}")]}]);
        sqlx::query("UPDATE settings SET value=? WHERE key=?")
            .bind(bad_plan.to_string()).bind(ATTACHMENT_GC_KEY).execute(&db.pool).await.unwrap();
        assert!(consume_attachment_gc(&db, &roots).await.is_err());
        assert!(outside.is_file());
        assert_eq!(read_plan(&db).await.0, bad_plan.to_string());
        db.pool.close().await;
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
        let snapshot = load_attachment_gc_snapshot(&mut tx, None)
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
        assert!(load_attachment_gc_snapshot(&mut tx, None)
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
        let error = load_attachment_gc_snapshot(&mut tx, None)
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
        let snapshot = load_attachment_gc_snapshot(&mut tx, None)
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
        let snapshot = load_attachment_gc_snapshot(&mut tx, None)
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
        let snapshot = load_attachment_gc_snapshot(&mut tx, None)
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
