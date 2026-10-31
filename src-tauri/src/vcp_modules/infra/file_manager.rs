use crate::vcp_modules::db_manager::DbState;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tokio::io::AsyncReadExt;

pub mod operations;
pub mod resources;
pub use operations::*;
pub use resources::*;

use tauri::{AppHandle, Emitter, Manager, State};

const STORE_FILE_IPC_MAX_BYTES: usize = 8 * 1024 * 1024;
const STAGED_FILE_MAX_BYTES: u64 = 512 * 1024 * 1024;

fn store_file_semaphore() -> Arc<tokio::sync::Semaphore> {
    static SEMAPHORE: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    SEMAPHORE
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2)))
        .clone()
}

pub(crate) fn safe_storage_extension(original_name: &str) -> Option<&str> {
    let extension = std::path::Path::new(original_name)
        .extension()
        .and_then(|extension| extension.to_str())?;
    if extension.is_empty()
        || extension.len() > 16
        || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        None
    } else {
        Some(extension)
    }
}

fn canonical_staged_file(
    root: &std::path::Path,
    file: &std::path::Path,
    label: &str,
) -> Result<std::path::PathBuf, String> {
    let canonical_root = std::fs::canonicalize(root)
        .map_err(|error| format!("{} staging 根目录不可用: {}", label, error))?;
    let canonical_file = std::fs::canonicalize(file)
        .map_err(|error| format!("{} staging 文件不可用: {}", label, error))?;
    if canonical_file.parent() != Some(canonical_root.as_path()) || !canonical_file.is_file() {
        return Err(format!("非法的 {} staging 文件路径", label));
    }
    Ok(canonical_file)
}

fn verify_expected_hash(expected_hash: Option<&str>, actual_hash: &str) -> Result<(), String> {
    if let Some(expected_hash) = expected_hash {
        if expected_hash != actual_hash {
            return Err("Native staging hash 与 Rust 重算结果不一致".to_string());
        }
    }
    Ok(())
}

fn verify_streamed_size(expected_size: u64, actual_size: u64) -> Result<(), String> {
    if actual_size != expected_size {
        return Err("staging 文件在哈希期间发生大小变化".to_string());
    }
    Ok(())
}

/// CAS 命中复用校验（瘦身版，只做元数据闸门）。
///
/// 为什么可以不做全量重哈希：正式路径的文件名即内容哈希（写入端先对新内容全量
/// 计算 SHA-256 再命名），且所有发布路径都是 temp+rename 原子提交，正式位置不可能
/// 存在半文件。「同尺寸不同内容」仅剩磁盘级损坏一种来源，而「能改写应用私有目录
/// 的攻击者」本就超出哈希校验能防的威胁模型。去掉重哈希后，重复附件命中不再产生
/// 第三次全量读盘（Kotlin 拷入一次 + Rust staging 重算一次已经足够）。
#[cfg(test)]
fn check_existing_cas_size(path: &std::path::Path, expected_size: u64) -> Result<(), String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("CAS 文件元数据读取失败: {}", error))?;
    if !metadata.is_file() || metadata.len() != expected_size {
        return Err("已存在的 CAS 文件大小不匹配，拒绝复用".to_string());
    }
    Ok(())
}

pub(crate) async fn check_existing_cas_size_async(
    path: &std::path::Path,
    expected_size: u64,
) -> Result<(), String> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|error| format!("CAS 文件元数据读取失败: {}", error))?;
    if !metadata.is_file() || metadata.len() != expected_size {
        return Err("已存在的 CAS 文件大小不匹配，拒绝复用".to_string());
    }
    Ok(())
}

/// =================================================================
/// vcp_modules/file_manager.rs - 附件物理存储与分片上传管理
/// =================================================================
/// SQLite 与应用支持文件的私有根目录；不包含 Android 的外部附件目录。
/// Android 保留 Tauri app_config_dir（activity.dataDir），iOS 使用 Application Support。
pub fn get_support_root_dir<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> Result<PathBuf, String> {
    #[cfg(target_os = "android")]
    {
        app_handle
            .path()
            .app_config_dir()
            .map_err(|e| e.to_string())
    }
    #[cfg(target_os = "ios")]
    {
        app_handle.path().app_data_dir().map_err(|e| e.to_string())
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        // 仅 host 开发/测试，保留现有配置目录，不建立桌面产品数据布局。
        app_handle
            .path()
            .app_config_dir()
            .map_err(|e| e.to_string())
    }
}

/// 核心路径解析：获取附件持久数据根目录。
/// Android 保留 external files；iOS 落在 Application Support，不能上溯 Documents。
pub fn get_data_root_dir<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> Result<std::path::PathBuf, String> {
    #[cfg(target_os = "android")]
    {
        let mut path = app_handle
            .path()
            .document_dir()
            .map_err(|e| format!("Failed to get document_dir: {}", e))?;
        path.pop(); // 保留现有 .../files/Documents 的父目录，避免迁移 Android 数据。
        Ok(path)
    }
    #[cfg(target_os = "ios")]
    {
        get_support_root_dir(app_handle)
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        // 仅保留原 host 开发数据位置；不能作为移动端目录实现复用。
        let mut path = app_handle
            .path()
            .document_dir()
            .map_err(|e| format!("Failed to get document_dir: {}", e))?;
        path.pop();
        Ok(path)
    }
}

/// 语音合成的持久缓存；随应用数据保留，不放入系统可回收的缓存目录。
pub fn get_tts_cache_root_dir<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> Result<PathBuf, String> {
    Ok(get_data_root_dir(app_handle)?.join("tts_cache"))
}

/// 可丢弃缓存的根目录。iOS 由 Tauri 解析为 Library/Caches/<实际 identifier>。
pub fn get_cache_root_dir<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> Result<PathBuf, String> {
    app_handle.path().app_cache_dir().map_err(|e| e.to_string())
}

/// 附件导入暂存区。原生复制、内存产物、登记和冷启动清理共用此路径。
/// 只存未提交的临时文件；CAS 成功登记后由导入 owner 释放，不能当作长期附件路径。
pub fn get_upload_staging_dir<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> Result<PathBuf, String> {
    Ok(get_cache_root_dir(app_handle)?.join("uploads"))
}

/// 核心路径解析：获取附件存储根目录
/// Android: /storage/emulated/0/Android/data/<pkg>/files/attachments
pub fn get_attachments_root_dir<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> Result<std::path::PathBuf, String> {
    let mut path = get_data_root_dir(app_handle)?;
    path.push("attachments");
    Ok(path)
}

/// 核心路径解析：获取缩略图存储根目录
/// Android: /storage/emulated/0/Android/data/<pkg>/files/thumbnails
pub fn get_thumbnails_root_dir<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> Result<std::path::PathBuf, String> {
    #[cfg(target_os = "ios")]
    let mut path = get_cache_root_dir(app_handle)?;
    #[cfg(not(target_os = "ios"))]
    let mut path = get_data_root_dir(app_handle)?;
    path.push("thumbnails");
    Ok(path)
}

/// 可重算的多模态抽取/转码缓存。Android/host 保留原位置；iOS 不写入持久支持目录。
pub fn get_multimodal_cache_dir<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> Result<std::path::PathBuf, String> {
    #[cfg(target_os = "ios")]
    let mut path = get_cache_root_dir(app_handle)?;
    #[cfg(not(target_os = "ios"))]
    let mut path = get_data_root_dir(app_handle)?;
    path.push("multimodal_cache");
    Ok(path)
}

/// 附件元数据结构
/// 对齐 @/plans/Rust文件数据管理重构详细规划.md 中的 2.1 节
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentData {
    pub id: String,
    pub name: String,
    pub internal_file_name: String,
    pub internal_path: String,
    #[serde(rename = "type")]
    pub mime_type: String, // 对应 JS 端的 type
    pub size: u64,
    pub hash: String,
    pub created_at: u64,
    pub extracted_text: Option<String>,
    pub thumbnail_path: Option<String>,
}

/// Rust-internal CAS fact used by model preprocessing and the local CLI projection owner.
/// The host path is never serialized into a model message, WebView response, or tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttachmentCasFile {
    pub path: PathBuf,
    pub mime_type: String,
    pub size_bytes: u64,
    pub sha256: String,
}

/// A catalog path identifies a CAS filename, never a container root. Rebuild
/// legacy absolute paths and relative names under this boot's storage root.
pub(crate) fn current_attachment_path(
    attachments_root: &Path,
    hash: &str,
    stored_path: &str,
) -> Result<Option<PathBuf>, String> {
    if !crate::vcp_modules::infra::utils::is_valid_cas_hash(hash) {
        return Err("invalid attachment CAS hash".to_string());
    }
    let stored_path = stored_path.strip_prefix("file://").unwrap_or(stored_path);
    if stored_path.is_empty() {
        return Ok(None);
    }
    let stored = Path::new(stored_path);
    let name = stored.file_name().ok_or("attachment CAS filename is missing")?;
    if stored.file_stem().and_then(|stem| stem.to_str()) != Some(hash) {
        return Err("attachment CAS filename does not match its hash".to_string());
    }
    Ok(Some(attachments_root.join(name)))
}

/// Shared with the blocking SQLite write worker. Missing bytes change readiness,
/// but must not erase the catalog's recoverable filename.
pub(crate) fn resolve_existing_attachment_path(
    attachments_root: &Path,
    hash: &str,
    stored_path: &str,
    expected_size: u64,
) -> Result<Option<PathBuf>, String> {
    let Some(candidate) = current_attachment_path(attachments_root, hash, stored_path)? else {
        return Ok(None);
    };
    match fs::symlink_metadata(&candidate) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot inspect attachment CAS file: {error}")),
        Ok(_) => {}
    }
    validate_attachment_cas_path(attachments_root, &candidate, hash, expected_size).map(Some)
}

fn current_thumbnail_path(root: &Path, hash: &str, stored: Option<&str>) -> Option<String> {
    if !crate::vcp_modules::infra::utils::is_valid_cas_hash(hash) {
        return None;
    }
    let mut names = Vec::new();
    if let Some(name) = stored.and_then(|path| Path::new(path.strip_prefix("file://").unwrap_or(path)).file_name()) {
        names.push(name.to_owned());
    }
    for extension in ["webp", "jpg", "png"] {
        names.push(std::ffi::OsString::from(format!("{hash}_thumb.{extension}")));
    }
    let stem = format!("{hash}_thumb");
    names.into_iter().find_map(|name| {
        let path = root.join(name);
        if path.file_stem().and_then(|value| value.to_str()) != Some(stem.as_str())
            || !matches!(path.extension().and_then(|extension| extension.to_str()), Some("webp" | "jpg" | "png")) {
            return None;
        }
        validated_direct_file(root, &path, path.file_name()?).ok().flatten()
            .map(|path| path.to_string_lossy().into_owned())
    })
}

struct AttachmentPathProjection {
    internal_path: String,
    src: String,
    status: &'static str,
    thumbnail_path: Option<String>,
}

fn project_attachment_paths(
    attachments_root: &Path,
    thumbnails_root: Option<&Path>,
    hash: &str,
    size: u64,
    stored_path: &str,
    thumbnail_path: Option<&str>,
) -> Result<AttachmentPathProjection, String> {
    let path = current_attachment_path(attachments_root, hash, stored_path)?;
    let existing = resolve_existing_attachment_path(attachments_root, hash, stored_path, size)?;
    let internal_path = path.map(|path| path.to_string_lossy().into_owned()).unwrap_or_default();
    let src = existing.as_ref().map(|path| format!("file://{}", path.to_string_lossy())).unwrap_or_default();
    Ok(AttachmentPathProjection {
        internal_path,
        src,
        status: if existing.is_some() { "ready" } else { "desktop_only" },
        // Derived cache failures cannot invalidate an otherwise valid original.
        thumbnail_path: thumbnails_root.and_then(|root| current_thumbnail_path(root, hash, thumbnail_path)),
    })
}

/// Refresh history, AI context and persisted share DTOs without changing their
/// message identity or synchronization fingerprint.
pub(crate) async fn refresh_attachment_paths<R: tauri::Runtime>(
    app: &AppHandle<R>,
    attachment: &mut crate::vcp_modules::chat_manager::Attachment,
) {
    let Some(hash) = attachment.hash.clone() else { return; };
    if attachment.internal_path.is_empty()
        && url::Url::parse(&attachment.src).is_ok_and(|url| matches!(url.scheme(), "http" | "https")) {
        return;
    }
    let unavailable = |attachment: &mut crate::vcp_modules::chat_manager::Attachment, error: &str| {
        log::warn!("Attachment {hash} local projection unavailable: {error}");
        attachment.src.clear();
        attachment.status = Some("desktop_only".to_string());
        attachment.thumbnail_path = None;
    };
    let attachments_root = match get_attachments_root_dir(app) {
        Ok(root) => root,
        Err(error) => { unavailable(attachment, &error); return; }
    };
    let thumbnails_root = get_thumbnails_root_dir(app).ok();
    let size = attachment.size;
    let stored = attachment.internal_path.clone();
    let thumbnail = attachment.thumbnail_path.clone();
    let lookup_hash = hash.clone();
    let projection = tokio::task::spawn_blocking(move || {
        let hash = lookup_hash;
        match project_attachment_paths(&attachments_root, thumbnails_root.as_deref(), &hash, size, &stored, thumbnail.as_deref()) {
            Ok(projection) => projection,
            Err(error) => {
                // A damaged/missing local attachment must not hide the rest of
                // the conversation. Native consumers still report the real error.
                log::warn!("Attachment {hash} is unavailable in the current container: {error}");
                AttachmentPathProjection {
                    internal_path: current_attachment_path(&attachments_root, &hash, &stored)
                        .ok().flatten().map(|path| path.to_string_lossy().into_owned()).unwrap_or_default(),
                    src: String::new(),
                    status: "desktop_only",
                    thumbnail_path: thumbnails_root.as_deref()
                        .and_then(|root| current_thumbnail_path(root, &hash, thumbnail.as_deref())),
                }
            }
        }
    }).await;
    let projection = match projection {
        Ok(projection) => projection,
        Err(error) => { unavailable(attachment, &error.to_string()); return; }
    };
    attachment.internal_path = projection.internal_path;
    attachment.src = projection.src;
    attachment.status = Some(projection.status.to_string());
    attachment.thumbnail_path = projection.thumbnail_path;
}

const ATTACHMENT_STORAGE_ROOTS_KEY: &str = "attachment_storage_roots";

/// Absolute CAS paths change when the OS relocates the app container. An old
/// database without a marker also needs one pass. File availability is still
/// checked when history, shares or native consumers resolve an attachment.
pub(crate) async fn reconcile_attachment_storage<R: tauri::Runtime>(
    app: &AppHandle<R>,
    db: &DbState,
) -> Result<(), String> {
    reconcile_attachment_storage_at(
        app,
        db,
        get_attachments_root_dir(app)?,
        get_thumbnails_root_dir(app)?,
    ).await
}

async fn reconcile_attachment_storage_at<R: tauri::Runtime>(
    app: &AppHandle<R>,
    db: &DbState,
    attachments_root: PathBuf,
    thumbnails_root: PathBuf,
) -> Result<(), String> {
    let roots = serde_json::to_string(&(&attachments_root, &thumbnails_root))
        .map_err(|error| error.to_string())?;
    let roots_match = || {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM settings WHERE key = ? AND value = ?)",
        )
        .bind(ATTACHMENT_STORAGE_ROOTS_KEY)
        .bind(&roots)
    };
    if roots_match().fetch_one(&db.pool).await.map_err(|e| e.to_string())? {
        return Ok(());
    }

    // Serialize root changes with concurrent imports and draft updates.
    let mut tx = db.write_transaction("file.attachment.relocate").await?;
    if roots_match().fetch_one(&mut *tx).await.map_err(|e| e.to_string())? {
        return tx.commit().await;
    }
    type Row = (String, i64, String, Option<String>);
    let rows: Vec<Row> = sqlx::query_as("SELECT hash, size, internal_path, thumbnail_path FROM attachments")
        .fetch_all(&mut *tx).await.map_err(|error| error.to_string())?;
    let projections = tokio::task::spawn_blocking(move || {
        rows.into_iter().map(|(hash, size, stored, thumbnail)| {
            let size = u64::try_from(size).map_err(|_| "attachment CAS size metadata is invalid".to_string())?;
            let projection = project_attachment_paths(&attachments_root, Some(&thumbnails_root), &hash, size, &stored, thumbnail.as_deref())?;
            Ok((hash, projection))
        }).collect::<Result<Vec<_>, String>>()
    }).await.map_err(|error| format!("attachment relocation task failed: {error}"))??;
    for (hash, projection) in projections {
        sqlx::query("UPDATE attachments SET internal_path = ?, thumbnail_path = ?
                     WHERE hash = ? AND (internal_path IS NOT ? OR thumbnail_path IS NOT ?)")
            .bind(&projection.internal_path).bind(&projection.thumbnail_path).bind(&hash)
            .bind(&projection.internal_path).bind(&projection.thumbnail_path)
            .execute(&mut *tx).await.map_err(|error| error.to_string())?;
        sqlx::query("UPDATE message_attachments SET src = ?, status = ?
                     WHERE hash = ? AND status IN ('ready', 'desktop_only')
                       AND (? <> '' OR (src NOT LIKE 'http://%' AND src NOT LIKE 'https://%'))
                       AND (src IS NOT ? OR status IS NOT ?)")
            .bind(&projection.src).bind(projection.status).bind(&hash)
            .bind(&projection.internal_path)
            .bind(&projection.src).bind(projection.status)
            .execute(&mut *tx).await.map_err(|error| error.to_string())?;
    }
    super::share_inbox::reconcile_current_share_paths(app, &mut tx).await?;
    sqlx::query("INSERT INTO settings (key, value, updated_at) VALUES (?, ?, ?)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at")
        .bind(ATTACHMENT_STORAGE_ROOTS_KEY)
        .bind(&roots)
        .bind(super::utils::now_secs())
        .execute(&mut *tx).await.map_err(|error| error.to_string())?;
    tx.commit().await
}

pub(crate) async fn resolve_attachment_cas_file<R: tauri::Runtime>(
    app_handle: &AppHandle<R>,
    pool: &sqlx::SqlitePool,
    hash: &str,
) -> Result<AttachmentCasFile, String> {
    if !crate::vcp_modules::infra::utils::is_valid_cas_hash(hash) {
        return Err("invalid attachment CAS hash".to_string());
    }
    let record = sqlx::query_as::<_, (String, i64, String)>(
        "SELECT mime_type, size, internal_path FROM attachments WHERE hash = ? LIMIT 1",
    )
    .bind(hash)
    .fetch_optional(pool)
    .await
    .map_err(|error| format!("cannot read attachment CAS metadata: {error}"))?
    .ok_or_else(|| "attachment is not present in the local CAS catalog".to_string())?;
    let size_bytes = u64::try_from(record.1)
        .map_err(|_| "attachment CAS size metadata is invalid".to_string())?;
    let mime_type = normalize_attachment_mime(&record.0)?;
    let attachments_root = get_attachments_root_dir(app_handle)?;
    let lookup_hash = hash.to_string();
    let path = tokio::task::spawn_blocking(move || {
        resolve_existing_attachment_path(&attachments_root, &lookup_hash, &record.2, size_bytes)?
            .ok_or_else(|| "attachment CAS file is not available in the current container".to_string())
    }).await.map_err(|error| format!("attachment CAS lookup task failed: {error}"))??;
    Ok(AttachmentCasFile {
        path,
        mime_type,
        size_bytes,
        sha256: hash.to_string(),
    })
}

fn normalize_attachment_mime(value: &str) -> Result<String, String> {
    let mime = value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let Some((kind, subtype)) = mime.split_once('/') else {
        return Err("attachment CAS MIME metadata is invalid".to_string());
    };
    let valid_token = |token: &str| {
        !token.is_empty()
            && token.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(
                        byte,
                        b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
                    )
            })
    };
    if mime.len() > 128 || !valid_token(kind) || !valid_token(subtype) {
        return Err("attachment CAS MIME metadata is invalid".to_string());
    }
    Ok(mime)
}

fn validate_attachment_cas_path(
    attachments_root: &Path,
    candidate: &Path,
    hash: &str,
    expected_size: u64,
) -> Result<PathBuf, String> {
    let source_metadata = fs::symlink_metadata(candidate)
        .map_err(|error| format!("cannot inspect attachment CAS file: {error}"))?;
    if !source_metadata.file_type().is_file() || source_metadata.len() != expected_size {
        return Err("attachment CAS file is not the expected real regular file".to_string());
    }
    let canonical_root = fs::canonicalize(attachments_root)
        .map_err(|error| format!("cannot resolve attachment CAS root: {error}"))?;
    let canonical = fs::canonicalize(candidate)
        .map_err(|error| format!("cannot resolve attachment CAS file: {error}"))?;
    if canonical.parent() != Some(canonical_root.as_path()) {
        return Err("attachment CAS file escaped its fixed root".to_string());
    }
    let stem = canonical
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "attachment CAS file name is invalid".to_string())?;
    if stem != hash {
        return Err("attachment CAS file name does not match its hash".to_string());
    }
    Ok(canonical)
}

/// 内部辅助函数：智能启发式检测文件是否可能为纯文本
/// 读取前 1024 字节，如果不包含 NULL 字节 (0x00)，则极大概率是文本或代码
fn is_likely_text_file(path: &std::path::Path) -> bool {
    use std::io::Read;
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };

    let mut buffer = [0u8; 1024];
    let n = match file.read(&mut buffer) {
        Ok(n) => n,
        Err(_) => return false,
    };

    if n == 0 {
        return false;
    }

    // 检查已读取的部分是否含有 NULL 字节
    for &b in &buffer[..n] {
        if b == 0 {
            return false;
        }
    }
    true
}

/// 内部辅助函数：精细化 MIME 类型判定 (对齐桌面端 fileManager.js)
/// 增加了魔数检测 (infer) 和 文本启发式检测 (no-NULL sniffing)
pub fn get_refined_mime_type(
    path: &std::path::Path,
    original_name: &str,
    initial_mime: &str,
) -> String {
    let ext = std::path::Path::new(original_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    // 1. 强制修正 MP3
    if ext == "mp3" {
        return "audio/mpeg".to_string();
    }

    // 2. 如果初始值无效，或者是一个通用后缀，则尝试根据扩展名路由
    let current_mime = initial_mime.to_string();

    if current_mime.is_empty() || current_mime == "application/octet-stream" {
        match ext.as_str() {
            "txt" => return "text/plain".to_string(),
            "json" => return "application/json".to_string(),
            "xml" => return "application/xml".to_string(),
            "csv" => return "text/csv".to_string(),
            "html" => return "text/html".to_string(),
            "css" => return "text/css".to_string(),
            "pdf" => return "application/pdf".to_string(),
            "doc" => return "application/msword".to_string(),
            "docx" => {
                return "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                    .to_string()
            }
            "xls" => return "application/vnd.ms-excel".to_string(),
            "xlsx" => {
                return "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                    .to_string()
            }
            "ppt" => return "application/vnd.ms-powerpoint".to_string(),
            "pptx" => {
                return "application/vnd.openxmlformats-officedocument.presentationml.presentation"
                    .to_string()
            }
            "jpg" | "jpeg" => return "image/jpeg".to_string(),
            "png" => return "image/png".to_string(),
            "gif" => return "image/gif".to_string(),
            "webp" => return "image/webp".to_string(),
            "svg" => return "image/svg+xml".to_string(),
            "bmp" => return "image/bmp".to_string(),
            "ico" => return "image/x-icon".to_string(),
            "tiff" | "tif" => return "image/tiff".to_string(),
            "heic" | "heif" => return "image/heic".to_string(),
            "avif" => return "image/avif".to_string(),
            "wav" => return "audio/wav".to_string(),
            "ogg" | "ogv" => return "audio/ogg".to_string(),
            "flac" => return "audio/flac".to_string(),
            "aac" => return "audio/aac".to_string(),
            "aiff" | "aif" => return "audio/aiff".to_string(),
            "m4a" => return "audio/mp4".to_string(),
            "opus" => return "audio/opus".to_string(),
            "amr" => return "audio/amr".to_string(),
            "mp4" | "m4v" => return "video/mp4".to_string(),
            "webm" => return "video/webm".to_string(),
            "mov" | "qt" => return "video/quicktime".to_string(),
            "avi" => return "video/x-msvideo".to_string(),
            "mkv" => return "video/x-matroska".to_string(),
            "wmv" => return "video/x-ms-wmv".to_string(),
            "flv" => return "video/x-flv".to_string(),
            "3gp" | "3g2" => return "video/3gpp".to_string(),
            "mts" | "m2ts" => return "video/mp2t".to_string(),
            // 所有代码/文本类文件统一为 text/plain 以触发提取逻辑
            _ if super::file_extractor::is_text_or_code_extension(&ext) => {
                return "text/plain".to_string();
            }
            _ => {
                // 3. 终极兜底：物理层嗅探
                if path.exists() {
                    // 3a. 魔数匹配 (用于识别被改了后缀的二进制文件)
                    if let Ok(Some(kind)) = infer::get_from_path(path) {
                        return kind.mime_type().to_string();
                    }

                    // 3b. 文本启发式 (用于识别未知的文本/代码格式，如 .pub, .env, .log)
                    if is_likely_text_file(path) {
                        return "text/plain".to_string();
                    }
                }
            }
        }
    }

    current_mime
}

/// 原件不变；使用同一原生派生服务生成 512px 列表图，记录真实编码后缀。
pub async fn generate_thumbnail<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    original_path: &std::path::Path,
    hash: &str,
) -> Option<String> {
    let directory = get_thumbnails_root_dir(app_handle).ok()?;
    tokio::fs::create_dir_all(&directory).await.ok()?;
    let output = directory.join(format!("{hash}_thumb"));
    let source = original_path.to_owned();
    let app = app_handle.clone();
    match tokio::task::spawn_blocking(move || {
        super::media_processor::image_extractor::derive_image(
            &app,
            &source,
            &output,
            512,
            512 * 512,
            1024 * 1024,
        )
    })
    .await
    {
        Ok(Ok(image)) => Some(image.path),
        Ok(Err(error)) => {
            log::warn!("附件缩略图生成失败: {error}");
            None
        }
        Err(error) => {
            log::warn!("附件缩略图任务异常: {error}");
            None
        }
    }
}

/// 内部辅助函数：校验路径安全性，防止路径遍历攻击
fn ensure_safe_path(app_handle: &AppHandle, path: &std::path::Path) -> Result<(), String> {
    // 物理展开目标路径的所有相对路径分量 (..)，杜绝字符级前缀欺骗的沙盒逃逸
    let canonical_path = if path.exists() {
        std::fs::canonicalize(path).map_err(|e| format!("路径规范化失败: {}", e))?
    } else {
        // 如果文件甚至不存在，安全起见直接阻断，因为在 register_local_file 中已校验 exists()，
        // open_file 也同样应阻断不存在的文件访问以防信息探测
        return Err("非法路径访问：目标文件不存在".to_string());
    };

    let config_dir = get_support_root_dir(app_handle)?;
    let canonical_config = std::fs::canonicalize(&config_dir).unwrap_or(config_dir);

    let cache_dir = get_cache_root_dir(app_handle)?;
    let canonical_cache = std::fs::canonicalize(&cache_dir).unwrap_or(cache_dir);

    // 允许访问 App 配置目录 (内部)、缓存目录 (临时)、附件目录 (可能在外部) 或 缩略图目录
    let attachments_dir = get_attachments_root_dir(app_handle)?;
    let canonical_attachments = std::fs::canonicalize(&attachments_dir).unwrap_or(attachments_dir);

    let thumbnails_dir = get_thumbnails_root_dir(app_handle)?;
    let canonical_thumbnails = std::fs::canonicalize(&thumbnails_dir).unwrap_or(thumbnails_dir);

    let multimodal_cache_dir = get_multimodal_cache_dir(app_handle)?;
    let canonical_multimodal_cache =
        std::fs::canonicalize(&multimodal_cache_dir).unwrap_or(multimodal_cache_dir);

    if canonical_path.starts_with(&canonical_config)
        || canonical_path.starts_with(&canonical_cache)
        || canonical_path.starts_with(&canonical_attachments)
        || canonical_path.starts_with(&canonical_thumbnails)
        || canonical_path.starts_with(&canonical_multimodal_cache)
    {
        Ok(())
    } else {
        Err(format!(
            "非法路径访问：禁止访问应用授权范围以外的文件 ({:?})",
            path
        ))
    }
}

/// 内存映射读取文件，自动检测编码并转换为 UTF-8
/// 1. 优先 BOM 头检测（最可靠）
/// 2. 无 BOM 时使用 chardetng 统计检测（Firefox 同款）
use super::file_extractor::try_extract_text;

async fn commit_registered_attachment(
    db_state: &DbState,
    hash: &str,
    mime_type: &str,
    size: u64,
    internal_path: &str,
    now: i64,
) -> Result<(), String> {
    let mut tx = db_state
        .write_transaction("file.attachment.register")
        .await?;
    sqlx::query(
        "INSERT INTO attachments (hash, mime_type, size, internal_path, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(hash) DO UPDATE SET
            mime_type = excluded.mime_type,
            size = excluded.size,
            internal_path = excluded.internal_path,
            updated_at = excluded.updated_at",
    )
    .bind(hash)
    .bind(mime_type)
    .bind(size as i64)
    .bind(internal_path)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query(
        "UPDATE message_attachments
         SET status = 'ready', src = ?
         WHERE hash = ? AND status = 'desktop_only'
           AND EXISTS (
             SELECT 1 FROM messages m
             WHERE m.owner_type = message_attachments.owner_type
               AND m.owner_id = message_attachments.owner_id
               AND m.topic_id = message_attachments.topic_id
               AND m.msg_id = message_attachments.msg_id
               AND m.deleted_at IS NULL
           )
           AND EXISTS (
             SELECT 1 FROM topics t
             WHERE t.owner_type = message_attachments.owner_type
               AND t.owner_id = message_attachments.owner_id
               AND t.topic_id = message_attachments.topic_id
               AND t.deleted_at IS NULL
               AND (
                 (t.owner_type = 'agent' AND EXISTS (
                   SELECT 1 FROM agents a
                   WHERE a.owner_type = 'agent' AND a.agent_id = t.owner_id AND a.deleted_at IS NULL
                 ))
                 OR
                 (t.owner_type = 'group' AND EXISTS (
                   SELECT 1 FROM groups g
                   WHERE g.owner_type = 'group' AND g.group_id = t.owner_id AND g.deleted_at IS NULL
                 ))
               )
           )",
    )
    .bind(format!("file://{internal_path}"))
    .bind(hash)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// 将文件元数据注册到数据库并触发后处理 (缩略图、文本提取)。
#[allow(clippy::too_many_arguments)]
async fn register_attachment_with_operation<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    pool: &sqlx::SqlitePool,
    hash: String,
    original_name: String,
    mime_type: String,
    size: u64,
    internal_path: String,
    operation: &operations::Operation,
) -> Result<AttachmentData, String> {
    let now = crate::vcp_modules::infra::utils::now_secs() as u64;
    let db_state = app_handle.state::<DbState>();
    let attachments_root = get_attachments_root_dir(app_handle)?;
    let registration_hash = hash.clone();
    let internal_path = tokio::task::spawn_blocking(move || {
        resolve_existing_attachment_path(&attachments_root, &registration_hash, &internal_path, size)?
            .map(|path| path.to_string_lossy().into_owned())
            .ok_or_else(|| "待注册附件不在当前 CAS 目录".to_string())
    }).await.map_err(|error| format!("附件注册路径检查失败: {error}"))??;

    if let Ok(existing) = resolve_attachment_cas_file(app_handle, pool, &hash).await {
        if existing.size_bytes != size {
            return Err(format!("附件 {hash} 的现有 CAS 大小与本次注册不一致"));
        }
        let existing_metadata: (Option<String>, Option<String>, i64) = sqlx::query_as(
            "SELECT extracted_text, thumbnail_path, created_at FROM attachments WHERE hash = ?",
        )
        .bind(&hash)
        .fetch_one(pool)
        .await
        .map_err(|error| format!("读取现有附件元数据失败: {error}"))?;
        let existing_path = existing.path.to_string_lossy().into_owned();
        let mut commit_guard = operation.status.lock().await;
        operation.check()?;
        commit_registered_attachment(
            &db_state,
            &hash,
            &existing.mime_type,
            existing.size_bytes,
            &existing_path,
            now as i64,
        )
        .await?;
        *commit_guard = operations::Status::Committed;
        drop(commit_guard);

        let candidate = std::path::Path::new(&internal_path);
        if candidate != existing.path && candidate.exists() {
            let attachments_root = get_attachments_root_dir(app_handle)?;
            if let Some(redundant) = validated_attachment_file(&attachments_root, &hash, candidate)
                .map_err(|error| error.to_string())?
            {
                if redundant != existing.path {
                    if let Err(error) = tokio::fs::remove_file(redundant).await {
                        log::warn!("清理重复附件副本失败: {error}");
                    }
                }
            }
        }

        let internal_file_name = existing
            .path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "现有附件文件名不是有效 UTF-8".to_string())?
            .to_string();
        let thumbnail_root = get_thumbnails_root_dir(app_handle).ok();
        let thumbnail_hash = hash.clone();
        let old_thumbnail = existing_metadata.1;
        let cached_thumbnail = tokio::task::spawn_blocking(move || {
            thumbnail_root.and_then(|root| current_thumbnail_path(&root, &thumbnail_hash, old_thumbnail.as_deref()))
        }).await.unwrap_or(None);
        let thumbnail_path = match cached_thumbnail {
            Some(path) => Some(path),
            None if existing.mime_type.starts_with("image/") => {
                generate_thumbnail(app_handle, &existing.path, &hash).await
            }
            _ => None,
        };
        if let Some(path) = &thumbnail_path {
            let update = async {
                let mut tx = db_state
                    .write_transaction("file.attachment.thumbnail")
                    .await?;
                sqlx::query("UPDATE attachments SET thumbnail_path = ? WHERE hash = ?")
                    .bind(path)
                    .bind(&hash)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| e.to_string())?;
                tx.commit().await.map_err(|e| e.to_string())
            }
            .await;
            if let Err(error) = update {
                log::warn!("附件缩略图登记失败: {error}");
            }
        }
        return Ok(AttachmentData {
            id: format!("attachment_{hash}"),
            name: original_name,
            internal_file_name,
            internal_path: existing_path,
            mime_type: existing.mime_type,
            size: existing.size_bytes,
            hash,
            created_at: u64::try_from(existing_metadata.2)
                .map_err(|_| "现有附件创建时间无效".to_string())?,
            extracted_text: None,
            thumbnail_path,
        });
    }

    // 取消与主记录提交只在这个短临界区裁定，派生处理不持有锁。
    let mut commit_guard = operation.status.lock().await;
    operation.check()?;
    // 1. 原子更新 CAS 索引，并把仍然存活的 desktop_only 关系提升为 ready。
    commit_registered_attachment(
        &db_state,
        &hash,
        &mime_type,
        size,
        &internal_path,
        now as i64,
    )
    .await?;
    *commit_guard = operations::Status::Committed;
    drop(commit_guard);

    let internal_file_path = std::path::PathBuf::from(&internal_path);

    // 2. 提取文本内容 (如果适用，使用 spawn_blocking 隔离 CPU 密集型操作以防阻塞 Tokio 异步线程)
    let path_c = internal_file_path.clone();
    let mime_c = mime_type.clone();
    let extracted_text =
        match tokio::task::spawn_blocking(move || try_extract_text(&path_c, &mime_c)).await {
            Ok(text) => text,
            Err(error) => {
                log::warn!("附件已注册，但文本提取任务异常: {}", error);
                None
            }
        };

    // 3. 生成缩略图 (如果适用，spawn_blocking 隔离 CPU 密集型操作)
    let thumbnail_path = if mime_type.starts_with("image/") {
        generate_thumbnail(app_handle, &internal_file_path, &hash).await
    } else {
        None
    };

    // 核心安全优化：在后端即时且闭环地将耗时提取出的重资产数据持久化写入数据库
    // 杜绝大文本数据在前端 WebView 绕一圈所导致的数据丢失或内存积压泄漏！
    if extracted_text.is_some() || thumbnail_path.is_some() {
        let derived_update = async {
            let mut tx = db_state
                .write_transaction("file.attachment.derived")
                .await?;
            sqlx::query(
                "UPDATE attachments
                 SET extracted_text = ?, thumbnail_path = ?, updated_at = ?
                 WHERE hash = ?",
            )
            .bind(&extracted_text)
            .bind(&thumbnail_path)
            .bind(now as i64)
            .bind(&hash)
            .execute(&mut *tx)
            .await
            .map_err(|error| error.to_string())?;
            tx.commit().await.map_err(|error| error.to_string())?;
            Ok::<(), String>(())
        }
        .await;
        if let Err(error) = derived_update {
            log::warn!(
                "附件主记录已提交，但派生文本/缩略图元数据更新失败: {}",
                error
            );
        }
    }

    let internal_file_name = safe_storage_extension(&original_name)
        .map(|extension| format!("{}.{}", hash, extension))
        .unwrap_or_else(|| hash.clone());

    Ok(AttachmentData {
        id: format!("attachment_{}", hash),
        name: original_name,
        internal_file_name,
        internal_path,
        mime_type,
        size,
        hash,
        created_at: now,
        extracted_text: None, // 正文留在 Rust，前端预览通过有界读取命令。
        thumbnail_path,
    })
}

/// 存储文件到中心化附件目录 (内容寻址存储)
///
/// 仅用于 WebView 已生成的头像、文本、录音等内存产物。
/// 外部文件由双端原生选择器交接路径，纯导出使用 store_temporary_export。
#[tauri::command]
pub async fn store_file(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    original_name: String,
    file_bytes: Vec<u8>,
    mime_type: String,
    operation_id: Option<String>,
) -> Result<AttachmentData, String> {
    if file_bytes.len() > STORE_FILE_IPC_MAX_BYTES {
        return Err("内存产物超过 8 MiB，请缩小输出；外部文件请使用系统选择器".to_string());
    }
    let (id, operation) = operations::acquire(operation_id)?;
    let _operation_lifetime = operations::FinishOnDrop(id.clone());
    let uploads = get_upload_staging_dir(&app_handle)?;
    tokio::fs::create_dir_all(&uploads)
        .await
        .map_err(|e| e.to_string())?;
    let path = uploads.join(format!("{id}-{}.memory", uuid::Uuid::new_v4()));
    let _permit = store_file_semaphore()
        .acquire_owned()
        .await
        .map_err(|_| "文件存储执行器已关闭".to_string())?;
    let result = async {
        operation.check()?;
        tokio::fs::write(&path, file_bytes)
            .await
            .map_err(|e| e.to_string())?;
        operation.check()?;
        register_staged_file(
            &app_handle,
            &db_state,
            path.to_string_lossy().into_owned(),
            original_name,
            Some(mime_type),
            None,
            Some(id.clone()),
            None,
            &operation,
        )
        .await
    }
    .await;
    let _ = tokio::fs::remove_file(&path).await;
    operations::finish(&id);
    result
}

/// 注册本地已有的文件（例如 Android Kotlin 端沙盒临时复制的大文件/硬解缩略图）
/// 彻底实现“前端零拷贝物理路径传输”
/// 注册本地已有文件到附件系统 (零拷贝移动)
///
/// 【适用场景】Android 端实际上传入口。原生插件 `pick_file` 已将文件从 Scoped Storage
/// 流式拷贝到 app_cache_dir 并完成 SHA-256 计算，本函数仅负责：
///   1. rename/move 到附件目录 (内容寻址去重)
///   2. 生成/复用缩略图
///   3. 提取文本内容 (如适用)
///   4. 写入 attachment_registry 数据库
/// 全程不加载文件内容到内存，实现真正的零拷贝。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn register_local_file(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    local_path: String,
    original_name: String,
    mime_type: Option<String>,
    thumbnail_path: Option<String>,
    stable_id: Option<String>,
    expected_hash: Option<String>,
    operation_id: Option<String>,
) -> Result<AttachmentData, String> {
    let (id, operation) = operations::acquire(operation_id)?;
    let _operation_lifetime = operations::FinishOnDrop(id.clone());
    let result = register_staged_file(
        &app_handle,
        &db_state,
        local_path.clone(),
        original_name,
        mime_type,
        thumbnail_path.clone(),
        stable_id,
        expected_hash,
        &operation,
    )
    .await;
    // 只删除已验证属于 staging 的本次文件，不触碰已发布的同 hash CAS。
    if let Ok(root) = get_upload_staging_dir(&app_handle) {
        if let Ok(path) = canonical_staged_file(&root, Path::new(&local_path), "附件") {
            let _ = tokio::fs::remove_file(path).await;
        }
        if let Some(path) = thumbnail_path {
            if let Ok(path) =
                canonical_staged_file(&root.join("thumbnails"), Path::new(&path), "缩略图")
            {
                let _ = tokio::fs::remove_file(path).await;
            }
        }
    }
    operations::finish(&id);
    result
}

#[allow(clippy::too_many_arguments)]
async fn register_staged_file(
    app_handle: &AppHandle,
    db_state: &DbState,
    local_path: String,
    original_name: String,
    mime_type: Option<String>,
    thumbnail_path: Option<String>,
    stable_id: Option<String>,
    expected_hash: Option<String>,
    operation: &operations::Operation,
) -> Result<AttachmentData, String> {
    operation.check()?;
    // 命令仍由中央 IPC 总闸 offload；哈希缓冲必须保持堆分配，避免再次内联进 future。
    let uploads_root = get_upload_staging_dir(app_handle)?;
    tokio::fs::create_dir_all(&uploads_root)
        .await
        .map_err(|e| format!("无法创建上传 staging 目录: {}", e))?;
    let source_path =
        canonical_staged_file(&uploads_root, std::path::Path::new(&local_path), "附件")?;

    let staged_thumbnail = match thumbnail_path.as_deref() {
        Some(path) => {
            let thumbnail_root = uploads_root.join("thumbnails");
            tokio::fs::create_dir_all(&thumbnail_root)
                .await
                .map_err(|e| format!("无法创建缩略图 staging 目录: {}", e))?;
            let staged =
                canonical_staged_file(&thumbnail_root, std::path::Path::new(path), "缩略图")?;
            Some(staged)
        }
        None => None,
    };

    // 1.5 强力防线：对外部注入的哈希指纹进行 Content-Addressable 强格式校验，阻断路径穿越与沙盒逃逸
    if let Some(ref eh) = expected_hash {
        if !crate::vcp_modules::infra::utils::is_valid_cas_hash(eh) {
            return Err("非法的 Content-Addressable Storage (CAS) 哈希指纹格式".to_string());
        }
    }

    // 2. 先打开受控 staging 文件，再从同一句柄读取元数据与内容。
    let mut file = tokio::fs::File::open(&source_path)
        .await
        .map_err(|e| format!("无法打开源文件: {}", e))?;
    let meta = file
        .metadata()
        .await
        .map_err(|e| format!("无法读取源文件元数据: {}", e))?;
    if !meta.is_file() {
        return Err("staging 路径不是普通文件".to_string());
    }
    let size = meta.len();
    if size > STAGED_FILE_MAX_BYTES {
        return Err("staging 文件过大 (Limit: 512MB)".to_string());
    }

    // 3. Native 端给出的 hash 仅作一致性提示；特权边界始终流式重算。
    let mut hasher = Sha256::new();
    // 缓冲区跨越 read().await；使用 Vec 让 64 KiB 数据驻留堆上，避免内联进 command future。
    let mut buffer = vec![0u8; 64 * 1024];
    let mut hashed_bytes = 0u64;
    let mut last_emit_time = std::time::Instant::now();
    loop {
        operation.check()?;
        let n = file
            .read(&mut buffer)
            .await
            .map_err(|e| format!("读取源文件失败: {}", e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        hashed_bytes = hashed_bytes
            .checked_add(n as u64)
            .ok_or_else(|| "staging 文件哈希字节数溢出".to_string())?;
        if hashed_bytes > size {
            return Err("staging 文件在哈希期间发生大小变化".to_string());
        }
        if let Some(ref sid) = stable_id {
            let now = std::time::Instant::now();
            if now.duration_since(last_emit_time).as_millis() > 200 {
                last_emit_time = now;
                let pct = if size > 0 {
                    (hashed_bytes as f64 / size as f64 * 100.0) as u32
                } else {
                    0
                };
                let scaled_pct = 50 + (pct * 40 / 100);
                app_handle
                    .emit(
                        "vcp-file-register-progress",
                        serde_json::json!({
                            "progress": scaled_pct,
                            "stableId": sid,
                        }),
                    )
                    .ok();
            }
        }
    }
    verify_streamed_size(size, hashed_bytes)?;
    drop(file);
    operation.check()?;
    let hash = crate::vcp_modules::infra::utils::finalize_sha256_hex(hasher);
    verify_expected_hash(expected_hash.as_deref(), &hash)?;

    // 4. 计算目标路径
    let internal_file_name = safe_storage_extension(&original_name)
        .map(|extension| format!("{}.{}", hash, extension))
        .unwrap_or_else(|| hash.clone());

    let attachments_dir = get_attachments_root_dir(app_handle)?;
    if !attachments_dir.exists() {
        tokio::fs::create_dir_all(&attachments_dir)
            .await
            .map_err(|e| e.to_string())?;
    }

    let dest_path = attachments_dir.join(&internal_file_name);
    let dest_path_str = dest_path.to_str().ok_or("无效的目标路径字符")?.to_string();

    enum Placement {
        Existing,
        Renamed,
        Copied,
    }
    let placement = if dest_path.exists() {
        check_existing_cas_size_async(&dest_path, size).await?;
        log::info!(
            "[FileManager] Duplicated local file found for staging path: {}",
            local_path
        );
        if let Some(ref sid) = stable_id {
            app_handle
                .emit(
                    "vcp-file-register-progress",
                    serde_json::json!({
                        "progress": 99,
                        "stableId": sid,
                    }),
                )
                .ok();
        }
        Placement::Existing
    } else {
        // 先尝试 rename 极速移动，失败时 fallback 复制 + 删除
        if let Some(ref sid) = stable_id {
            app_handle
                .emit(
                    "vcp-file-register-progress",
                    serde_json::json!({
                        "progress": 90,
                        "stableId": sid,
                    }),
                )
                .ok();
        }
        let placement = if tokio::fs::rename(&source_path, &dest_path).await.is_ok() {
            Placement::Renamed
        } else {
            let temporary_path =
                attachments_dir.join(format!(".ingest-{}-{}.tmp", hash, uuid::Uuid::new_v4()));
            if let Err(error) = copy_cancellable(&source_path, &temporary_path, operation).await {
                let _ = tokio::fs::remove_file(&temporary_path).await;
                return Err(format!("复制文件到正式目录失败: {}", error));
            }
            match tokio::fs::rename(&temporary_path, &dest_path).await {
                Ok(()) => Placement::Copied,
                Err(error) => {
                    let _ = tokio::fs::remove_file(&temporary_path).await;
                    if !dest_path.exists() {
                        return Err(format!("提交文件到正式目录失败: {}", error));
                    }
                    check_existing_cas_size_async(&dest_path, size).await?;
                    Placement::Existing
                }
            }
        };
        if let Some(ref sid) = stable_id {
            app_handle
                .emit(
                    "vcp-file-register-progress",
                    serde_json::json!({
                        "progress": 99,
                        "stableId": sid,
                    }),
                )
                .ok();
        }
        placement
    };

    // 5. 修正 MIME 类型
    let initial_mime = mime_type.unwrap_or_else(|| "application/octet-stream".to_string());
    let refined_mime = get_refined_mime_type(&dest_path, &original_name, &initial_mime);

    // 6. 调用统一的附件注册逻辑
    let registration = register_attachment_with_operation(
        app_handle,
        &db_state.pool,
        hash.clone(),
        original_name,
        refined_mime,
        size,
        dest_path_str,
        operation,
    )
    .await;
    let mut attachment_data = match registration {
        Ok(data) => data,
        Err(error) => {
            match placement {
                Placement::Renamed => {
                    log::warn!("附件数据库注册失败；完整 CAS 文件保留，等待重试或维护 GC");
                }
                // 完整 CAS 文件允许由维护 GC 清理；这里不删除，避免与同哈希并发注册竞争。
                Placement::Copied => {}
                Placement::Existing => {}
            }
            return Err(error);
        }
    };
    if matches!(placement, Placement::Existing | Placement::Copied) {
        if let Err(error) = tokio::fs::remove_file(&source_path).await {
            log::warn!("附件已注册，但清理 staging 文件失败: {}", error);
        }
    }

    // 7. 处理前端传入的已有缩略图 (如 Kotlin 侧硬件加速生成的缩略图)
    let mut final_thumbnail_path = attachment_data.thumbnail_path.clone();

    if let Some(source_thumb) = staged_thumbnail {
        let thumbnail_result: Result<String, String> = async {
            let thumbs_dir = get_thumbnails_root_dir(app_handle)?;
            if !thumbs_dir.exists() {
                tokio::fs::create_dir_all(&thumbs_dir)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            let kind = infer::get_from_path(&source_thumb)
                .map_err(|e| e.to_string())?
                .ok_or("无法识别缩略图编码")?;
            let extension = match kind.mime_type() {
                "image/jpeg" => "jpg",
                "image/png" => "png",
                "image/webp" => "webp",
                _ => return Err("缩略图编码不受支持".into()),
            };
            let dest_thumb_path = thumbs_dir.join(format!("{hash}_thumb.{extension}"));
            let dest_thumb_path_str = dest_thumb_path
                .to_str()
                .ok_or("无效的缩略图目标路径字符")?
                .to_string();

            if !dest_thumb_path.exists() {
                let temporary_path =
                    thumbs_dir.join(format!(".thumb-{}-{}.tmp", hash, uuid::Uuid::new_v4()));
                if let Err(error) = tokio::fs::copy(&source_thumb, &temporary_path).await {
                    let _ = tokio::fs::remove_file(&temporary_path).await;
                    return Err(format!("复制缩略图到正式目录失败: {}", error));
                }
                if let Err(error) = tokio::fs::rename(&temporary_path, &dest_thumb_path).await {
                    let _ = tokio::fs::remove_file(&temporary_path).await;
                    if !dest_thumb_path.exists() {
                        return Err(format!("提交缩略图到正式目录失败: {}", error));
                    }
                }
            }

            // 更新 SQLite 中的 thumbnail_path，使其指向正式保存的缩略图
            let mut tx = db_state
                .write_transaction("file.attachment.thumbnail")
                .await?;
            sqlx::query("UPDATE attachments SET thumbnail_path = ?, updated_at = ? WHERE hash = ?")
                .bind(&dest_thumb_path_str)
                .bind(attachment_data.created_at as i64)
                .bind(&hash)
                .execute(&mut *tx)
                .await
                .map_err(|error| format!("更新附件缩略图元数据失败: {}", error))?;
            tx.commit().await.map_err(|error| error.to_string())?;
            if let Err(error) = tokio::fs::remove_file(&source_thumb).await {
                log::warn!("附件缩略图已注册，但清理 staging 文件失败: {}", error);
            }
            Ok(dest_thumb_path_str)
        }
        .await;

        match thumbnail_result {
            Ok(path) => final_thumbnail_path = Some(path),
            Err(error) => {
                log::warn!(
                    "主附件已提交；外部派生缩略图处理失败，保留主附件成功结果: {}",
                    error
                );
            }
        }
    }

    attachment_data.thumbnail_path = final_thumbnail_path;
    Ok(attachment_data)
}

async fn copy_cancellable(
    source: &Path,
    target: &Path,
    operation: &operations::Operation,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let mut input = tokio::fs::File::open(source)
        .await
        .map_err(|e| e.to_string())?;
    let mut output = tokio::fs::File::create(target)
        .await
        .map_err(|e| e.to_string())?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut copied = 0u64;
    loop {
        operation.check()?;
        let size = input.read(&mut buffer).await.map_err(|e| e.to_string())?;
        if size == 0 {
            break;
        }
        copied = copied.checked_add(size as u64).ok_or("复制字节数溢出")?;
        if copied > STAGED_FILE_MAX_BYTES {
            return Err("文件超过 512 MiB".into());
        }
        output
            .write_all(&buffer[..size])
            .await
            .map_err(|e| e.to_string())?;
    }
    output.flush().await.map_err(|e| e.to_string())?;
    operation.check()
}

/// 分享 owner 已核对当前 generation；保留交接原件，独立副本进入普通 CAS 管线。
pub async fn import_shared_file(
    app: &AppHandle,
    pool: &sqlx::SqlitePool,
    source_path: &Path,
    original_name: String,
    mime_type: Option<String>,
    operation_id: String,
    expected_hash: Option<String>,
) -> Result<AttachmentData, String> {
    let _ = pool; // DbState 的统一写事务仍从 app owner 取得。
    let operation = operations::begin(&operation_id)?;
    let _operation_lifetime = operations::FinishOnDrop(operation_id.clone());
    let uploads = get_upload_staging_dir(app)?;
    tokio::fs::create_dir_all(&uploads)
        .await
        .map_err(|e| e.to_string())?;
    let staged = uploads.join(format!("{operation_id}-{}.shared", uuid::Uuid::new_v4()));
    let result = async {
        copy_cancellable(source_path, &staged, &operation).await?;
        register_staged_file(
            app,
            &app.state::<DbState>(),
            staged.to_string_lossy().into_owned(),
            original_name,
            mime_type,
            None,
            Some(operation_id.clone()),
            expected_hash,
            &operation,
        )
        .await
    }
    .await;
    let _ = tokio::fs::remove_file(staged).await;
    operations::finish(&operation_id);
    result
}

/// 取得已登记附件的当前物理路径。
#[tauri::command]
pub async fn get_attachment_real_path(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    hash: String,
    _original_name: String,
) -> Result<String, String> {
    resolve_attachment_cas_file(&app_handle, &db_state.pool, &hash)
        .await
        .map(|record| record.path.to_string_lossy().into_owned())
}

/// 系统预览/打开：结果由真实原生 completion 决定。
#[tauri::command]
pub async fn open_file(
    app_handle: AppHandle,
    path: String,
    file_name: Option<String>,
) -> Result<tauri_plugin_vcp_mobile::system::NativeFileActionResult, String> {
    let clean_path = path.strip_prefix("file://").unwrap_or(&path).to_string();

    // 网络 URL 直接打开，跳过本地路径安全校验
    if clean_path.starts_with("http://") || clean_path.starts_with("https://") {
        use tauri_plugin_opener::OpenerExt;
        app_handle
            .opener()
            .open_url(clean_path, Option::<String>::None)
            .map_err(|e| e.to_string())?;
        return Ok(tauri_plugin_vcp_mobile::system::NativeFileActionResult {
            outcome: "opened".into(),
        });
    }

    let path_buf = std::path::PathBuf::from(&clean_path);

    // 安全校验：禁止打开系统敏感路径
    ensure_safe_path(&app_handle, &path_buf)?;

    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        return tauri_plugin_vcp_mobile::system::open_file_native(
            app_handle, clean_path, None, None, file_name,
        )
        .await;
    }

    // 使用 tauri-plugin-opener 的原生能力
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = file_name;
        use tauri_plugin_opener::OpenerExt;
        app_handle
            .opener()
            .open_path(clean_path, Option::<String>::None)
            .map_err(|e| e.to_string())?;
        Ok(tauri_plugin_vcp_mobile::system::NativeFileActionResult {
            outcome: "opened".into(),
        })
    }
}

#[tauri::command]
pub async fn export_attachment_file(
    app_handle: AppHandle,
    path: String,
    file_name: Option<String>,
) -> Result<tauri_plugin_vcp_mobile::system::DownloadsSaveResult, String> {
    let path = PathBuf::from(path.strip_prefix("file://").unwrap_or(&path));
    ensure_safe_path(&app_handle, &path)?;
    let name = file_name
        .or_else(|| path.file_name().and_then(|n| n.to_str()).map(str::to_owned))
        .ok_or("文件名无效")?;
    let mime = get_refined_mime_type(&path, &name, "application/octet-stream");
    tauri_plugin_vcp_mobile::system::save_file_to_downloads(
        app_handle,
        path.to_string_lossy().into_owned(),
        name,
        Some(mime),
        STAGED_FILE_MAX_BYTES,
    )
    .await
}

/// 清理上传缓存目录以及多媒体缓存碎片 (通常在启动时执行，清除上次闪退/强杀留下的僵尸文件)
pub fn clear_upload_cache(app_handle: &AppHandle) {
    if let (Ok(cache_dir), Ok(uploads_path)) = (
        get_cache_root_dir(app_handle),
        get_upload_staging_dir(app_handle),
    ) {
        // 1. 清除上次上传未完成的分片临时目录
        if uploads_path.exists() {
            let _ = fs::remove_dir_all(&uploads_path);
            let _ = fs::create_dir_all(&uploads_path);
            log::info!("[FileManager] Upload cache cleared.");
        }

        // 这些目录只有当前进程的临时资源；Share Inbox 在 App Group，不在本清理范围。
        for name in ["image-work", "media-work", "temporary-exports", "rendered-input"] {
            let path = cache_dir.join(name);
            if path.is_dir() {
                let _ = fs::remove_dir_all(path);
            }
        }

        // 2. 🌟多媒体零拷贝转码垃圾碎片冷启动清理防线（Sweeper）🌟
        // 遍历整个 cache_dir，只要文件/目录名匹配 img_*.webp、aud_*.aac 或以 vid_ 开头的一律物理抹除，彻底防止碎屑泄露
        if cache_dir.exists() && cache_dir.is_dir() {
            if let Ok(entries) = fs::read_dir(&cache_dir) {
                let mut cleared_files = 0;
                let mut cleared_dirs = 0;
                for entry in entries.flatten() {
                    let path = entry.path();
                    let file_name = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
                    if (file_name.starts_with("img_") && file_name.ends_with(".webp"))
                        || (file_name.starts_with("aud_") && file_name.ends_with(".aac"))
                        || (file_name.starts_with("camera_") && file_name.ends_with(".jpg"))
                        || (file_name.starts_with("pick_") && file_name.ends_with("_temp"))
                    {
                        if fs::remove_file(&path).is_ok() {
                            cleared_files += 1;
                        }
                    } else if file_name.starts_with("vid_")
                        && path.is_dir()
                        && fs::remove_dir_all(&path).is_ok()
                    {
                        cleared_dirs += 1;
                    }
                }
                if cleared_files > 0 || cleared_dirs > 0 {
                    log::info!(
                        "[FileManager] Cold-boot GC: Swept {} media cache files and {} zombie video folders.",
                        cleared_files,
                        cleared_dirs
                    );
                }
            }
        }
    }
}

/// 限制并收敛多模态结果缓存目录的总大小 (LRU 思想，基于 mtime 淘汰最旧的 json 缓存)
/// 当总大小超过 max_size_bytes (e.g. 300MB) 时，自动淘汰最旧的缓存，直到大小收缩到 target_size_bytes (e.g. 150MB)
pub fn evict_multimodal_cache_if_needed<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    max_size_bytes: u64,
    target_size_bytes: u64,
) {
    let cache_dir = match get_multimodal_cache_dir(app_handle) {
        Ok(dir) => dir,
        Err(_) => return,
    };

    if !cache_dir.exists() || !cache_dir.is_dir() {
        return;
    }

    let entries = match fs::read_dir(&cache_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    struct CacheFile {
        path: std::path::PathBuf,
        size: u64,
        mtime: std::time::SystemTime,
    }

    let mut cache_files = Vec::new();
    let mut total_size = 0u64;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Ok(meta) = fs::metadata(&path) {
                let size = meta.len();
                let mtime = meta
                    .modified()
                    .unwrap_or_else(|_| std::time::SystemTime::now());
                total_size += size;
                cache_files.push(CacheFile { path, size, mtime });
            }
        }
    }

    if total_size <= max_size_bytes {
        return;
    }

    log::info!(
        "[FileManager] Multimodal cache size ({} MB) exceeds limit ({} MB). Starting eviction...",
        total_size / 1024 / 1024,
        max_size_bytes / 1024 / 1024
    );

    // 按照修改时间升序排列 (最旧的在前面)
    cache_files.sort_by_key(|f| f.mtime);

    let mut evicted_count = 0;
    let mut evicted_size = 0u64;

    for file in cache_files {
        if total_size - evicted_size <= target_size_bytes {
            break;
        }
        if fs::remove_file(&file.path).is_ok() {
            evicted_size += file.size;
            evicted_count += 1;
        }
    }

    log::info!(
        "[FileManager] Multimodal cache eviction complete. Evicted {} files, freed {:.2} MB. Current size: {:.2} MB.",
        evicted_count,
        evicted_size as f64 / 1024.0 / 1024.0,
        (total_size - evicted_size) as f64 / 1024.0 / 1024.0
    );
}

/// ⚡ 确保附件大文本已被安全提取。
/// 若数据库中缺失大文本，且手机本地物理文件真实存在，则在后台立即触发提取，并异步持久化自愈回库。
pub async fn ensure_extracted_text(
    db_state: &DbState,
    hash: &str,
    internal_path: &str,
    mime_type: &str,
) -> Option<String> {
    if internal_path.is_empty() {
        return None;
    }

    let path = std::path::Path::new(internal_path);
    if !path.exists() {
        return None;
    }

    // 1. 后缀名白名单过滤
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    let is_doc = super::file_extractor::is_extractable_extension(&ext);

    if !is_doc {
        return None;
    }

    log::debug!(
        "[FileManager] Self-Healing: Triggering real-time text extraction for hash={}",
        hash
    );

    // 2. 调起提取器进行自愈提取 (使用 spawn_blocking 隔离 CPU 密集型操作以防阻塞 Tokio 异步线程)
    let path_c = path.to_path_buf();
    let mime_c = mime_type.to_string();
    let text_opt = tokio::task::spawn_blocking(move || {
        super::file_extractor::try_extract_text(&path_c, &mime_c)
    })
    .await
    .ok()
    .flatten();

    if let Some(text) = text_opt {
        let db_c = db_state.clone();
        let hash_c = hash.to_string();
        let text_c = text.clone();

        // 3. 异步持久化写入 SQLite，不阻塞当前的上下文加载请求
        tokio::spawn(async move {
            let result = async {
                let mut tx = db_c.write_transaction("file.attachment.extract").await?;
                sqlx::query(
                    "UPDATE attachments SET extracted_text = ?, updated_at = ? WHERE hash = ?",
                )
                .bind(&text_c)
                .bind(chrono::Utc::now().timestamp_millis())
                .bind(&hash_c)
                .execute(&mut *tx)
                .await
                .map_err(|error| error.to_string())?;
                tx.commit().await.map_err(|error| error.to_string())?;
                Ok::<(), String>(())
            }
            .await;
            if let Err(error) = result {
                log::warn!("Failed to persist extracted attachment text: {error}");
            }
        });

        Some(text)
    } else {
        None
    }
}

fn invalid_delete_target(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message.into())
}

fn validated_direct_file(
    root: &std::path::Path,
    candidate: &std::path::Path,
    expected_name: &std::ffi::OsStr,
) -> std::io::Result<Option<std::path::PathBuf>> {
    let metadata = match std::fs::symlink_metadata(candidate) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(invalid_delete_target("拒绝删除非普通附件文件"));
    }

    let canonical_root = std::fs::canonicalize(root)?;
    let canonical_candidate = std::fs::canonicalize(candidate)?;
    if canonical_candidate.parent() != Some(canonical_root.as_path())
        || canonical_candidate.file_name() != Some(expected_name)
    {
        return Err(invalid_delete_target("附件删除目标越出固定存储目录"));
    }
    Ok(Some(canonical_candidate))
}

fn validated_attachment_file(
    attachments_root: &std::path::Path,
    hash: &str,
    internal_path: &std::path::Path,
) -> std::io::Result<Option<std::path::PathBuf>> {
    if !crate::vcp_modules::infra::utils::is_valid_cas_hash(hash) {
        return Err(invalid_delete_target("附件哈希格式无效"));
    }
    let file_name = internal_path
        .file_name()
        .ok_or_else(|| invalid_delete_target("附件删除目标缺少文件名"))?;
    let stem = internal_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| invalid_delete_target("附件删除目标文件名无效"))?;
    if stem != hash {
        return Err(invalid_delete_target("附件删除目标与内容哈希不匹配"));
    }
    validated_direct_file(attachments_root, internal_path, file_name)
}

/// 删除无引用 CAS 的主文件与派生缓存。空路径表示 metadata-only，无需主文件。
pub(crate) async fn delete_attachment_physical(
    attachments_root: &Path,
    thumbnails_root: &Path,
    cache_root: &Path,
    hash: &str,
    internal_path: &str,
) -> std::io::Result<()> {
    if !crate::vcp_modules::infra::utils::is_valid_cas_hash(hash) {
        return Err(invalid_delete_target("附件哈希格式无效"));
    }
    let attachment_path = match current_attachment_path(attachments_root, hash, internal_path)
        .map_err(invalid_delete_target)? {
        Some(path) => validated_attachment_file(attachments_root, hash, &path)?,
        None => None,
    };

    // 统一处理缩略图定位与删除
    let mut thumbnail_paths = Vec::new();
    for extension in ["webp", "jpg", "png"] {
        let name = format!("{hash}_thumb.{extension}");
        if let Some(path) = validated_direct_file(
            thumbnails_root,
            &thumbnails_root.join(&name),
            std::ffi::OsStr::new(&name),
        )? {
            thumbnail_paths.push(path);
        }
    }

    // 统一处理多模态持久化缓存删除
    let cache_name = format!("{}.json", hash);
    let cache_path = cache_root.join(&cache_name);
    let multimodal_path =
        validated_direct_file(cache_root, &cache_path, std::ffi::OsStr::new(&cache_name))?;

    for validated in [attachment_path, multimodal_path]
        .into_iter()
        .flatten()
        .chain(thumbnail_paths)
    {
        tokio::fs::remove_file(validated).await?;
    }
    Ok(())
}

/// 统一校验附件格式支持情况 (合并多模态媒体白名单与文本提取文档白名单)
#[tauri::command]
pub fn check_attachment_support(original_name: String) -> Result<bool, String> {
    let ext = std::path::Path::new(&original_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if crate::vcp_modules::infra::file_extractor::is_supported_attachment_extension(&ext) {
        Ok(true)
    } else {
        Err(format!(
            "系统不支持 .{} 格式附件。\n大媒体（图片/视频/音频）支持直读多模态；文档（pdf/docx/xlsx/pptx）及常见代码和文本支持内容提取注入上下文。",
            ext
        ))
    }
}

#[cfg(test)]
mod security_boundary_tests {
    #[cfg(unix)]
    use super::validated_direct_file;
    use super::{
        canonical_staged_file, commit_registered_attachment, validate_attachment_cas_path,
        validated_attachment_file,
    };
    use super::{check_existing_cas_size, check_existing_cas_size_async};
    use crate::vcp_modules::db_manager::DbState;
    use std::fs;

    #[tokio::test]
    async fn attachment_relocation_only_runs_for_new_roots_and_retries_a_failed_draft_repair() {
        let temp = tempfile::tempdir().unwrap();
        let attachments = temp.path().join("attachments");
        let thumbnails = temp.path().join("thumbnails");
        fs::create_dir(&attachments).unwrap();
        let hash = "b".repeat(64);
        let name = format!("{hash}.txt");
        fs::write(attachments.join(&name), b"data").unwrap();

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1).connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER);
             CREATE TABLE attachments (hash TEXT PRIMARY KEY, size INTEGER, internal_path TEXT, thumbnail_path TEXT);
             CREATE TABLE message_attachments (hash TEXT, src TEXT, status TEXT);
             CREATE TABLE current_share (singleton INTEGER PRIMARY KEY, record TEXT);",
        ).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO attachments VALUES (?, 4, ?, NULL)")
            .bind(&hash).bind(format!("/old/{name}")).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO message_attachments VALUES (?, '', 'desktop_only')")
            .bind(&hash).execute(&pool).await.unwrap();
        let db = DbState::new(pool, temp.path().join("test.db"));
        let app = tauri::test::mock_app();
        let reconcile = |files: &std::path::Path, thumbs: &std::path::Path| {
            super::reconcile_attachment_storage_at(app.handle(), &db, files.to_owned(), thumbs.to_owned())
        };
        let read = || sqlx::query_as::<_, (String, String, String)>(
            "SELECT internal_path, src, status FROM attachments JOIN message_attachments USING (hash)",
        ).fetch_one(&db.pool);

        reconcile(&attachments, &thumbnails).await.unwrap();
        let initial = read().await.unwrap();
        assert_eq!(initial.0, attachments.join(&name).to_string_lossy());
        assert!(initial.1.starts_with("file://"));
        assert_eq!(initial.2, "ready");

        // A matching marker does not read even the draft. A cache-only root
        // change does, and its failure must roll back both the catalog and marker.
        let new_thumbs = temp.path().join("new-cache").join("thumbnails");
        fs::remove_file(attachments.join(&name)).unwrap();
        sqlx::query("INSERT INTO current_share VALUES (1, 'invalid draft')")
            .execute(&db.pool).await.unwrap();
        reconcile(&attachments, &thumbnails).await.unwrap();
        assert!(reconcile(&attachments, &new_thumbs).await.is_err());
        assert_eq!(read().await.unwrap(), initial);
        sqlx::query("UPDATE current_share SET record = ?")
            .bind(r#"{"generation":3,"deliveryId":"keep","state":"draft","revision":5}"#)
            .execute(&db.pool).await.unwrap();
        reconcile(&attachments, &new_thumbs).await.unwrap();
        assert_eq!(read().await.unwrap(), (initial.0, String::new(), "desktop_only".into()));

        let new_files = temp.path().join("new-container").join("attachments");
        reconcile(&new_files, &new_thumbs).await.unwrap();
        assert_eq!(read().await.unwrap().0, new_files.join(&name).to_string_lossy());
    }

    #[test]
    fn canonical_staging_gate_accepts_only_direct_files_inside_its_owner_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("uploads");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&root).expect("root");
        fs::create_dir_all(&outside).expect("outside");
        let staged = root.join("ok.bin");
        let escaped = outside.join("secret.bin");
        fs::write(&staged, b"ok").expect("staged");
        fs::write(&escaped, b"secret").expect("escaped");

        assert!(canonical_staged_file(&root, &staged, "test").is_ok());
        assert!(canonical_staged_file(&root, &root.join("../outside/secret.bin"), "test").is_err());
        assert!(canonical_staged_file(&root, &root, "test").is_err());
        let thumbnail_root = root.join("thumbnails");
        fs::create_dir(&thumbnail_root).expect("thumbnail staging root");
        let thumbnail = thumbnail_root.join("image.webp");
        fs::write(&thumbnail, b"thumbnail").expect("staged thumbnail");
        assert!(canonical_staged_file(&root, &thumbnail, "test").is_err());
        assert!(canonical_staged_file(&thumbnail_root, &thumbnail, "test").is_ok());

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&escaped, root.join("link.bin")).expect("symlink");
            assert!(canonical_staged_file(&root, &root.join("link.bin"), "test").is_err());
        }
    }

    #[test]
    fn attachment_cas_resolution_requires_direct_hash_named_regular_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("attachments");
        fs::create_dir(&root).expect("attachments root");
        let hash = "a".repeat(64);
        let valid = root.join(format!("{hash}.png"));
        fs::write(&valid, b"image").expect("valid CAS file");
        assert_eq!(
            validate_attachment_cas_path(&root, &valid, &hash, 5).expect("valid CAS"),
            fs::canonicalize(&valid).expect("canonical CAS")
        );
        assert!(validate_attachment_cas_path(&root, &valid, &"b".repeat(64), 5).is_err());
        assert!(validate_attachment_cas_path(&root, &valid, &hash, 4).is_err());

        #[cfg(unix)]
        {
            let link = root.join(format!("{}.png", "c".repeat(64)));
            std::os::unix::fs::symlink(&valid, &link).expect("CAS symlink");
            assert!(validate_attachment_cas_path(&root, &link, &"c".repeat(64), 5).is_err());
        }
    }

    #[test]
    fn attachment_delete_target_requires_hash_named_direct_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("attachments");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&root).expect("attachment root");
        fs::create_dir_all(&outside).expect("outside root");
        let hash = "a".repeat(64);
        let valid = root.join(format!("{hash}.pdf"));
        let wrong_name = root.join(format!("{}.pdf", "b".repeat(64)));
        let escaped = outside.join(format!("{hash}.pdf"));
        fs::write(&valid, b"valid").expect("valid target");
        fs::write(&wrong_name, b"wrong").expect("wrong target");
        fs::write(&escaped, b"outside").expect("outside target");

        assert_eq!(
            validated_attachment_file(&root, &hash, &valid)
                .expect("valid target")
                .expect("present target"),
            fs::canonicalize(&valid).expect("canonical valid target")
        );
        assert!(validated_attachment_file(&root, "not-a-hash", &valid).is_err());
        assert!(validated_attachment_file(&root, &hash, &wrong_name).is_err());
        assert!(validated_attachment_file(&root, &hash, &escaped).is_err());
        assert!(escaped.exists());
    }

    #[cfg(unix)]
    #[test]
    fn attachment_delete_target_rejects_symlink_and_derived_escape() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("attachments");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&root).expect("attachment root");
        fs::create_dir_all(&outside).expect("outside root");
        let hash = "c".repeat(64);
        let outside_file = outside.join(format!("{hash}.bin"));
        let link = root.join(format!("{hash}.bin"));
        fs::write(&outside_file, b"outside").expect("outside target");
        symlink(&outside_file, &link).expect("attachment symlink");

        assert!(validated_attachment_file(&root, &hash, &link).is_err());
        assert!(validated_direct_file(
            &root,
            &outside_file,
            outside_file.file_name().expect("outside filename")
        )
        .is_err());
        assert_eq!(
            fs::read(&outside_file).expect("outside preserved"),
            b"outside"
        );
    }

    #[tokio::test]
    async fn existing_cas_file_is_reused_by_size_after_atomic_publish() {
        // 语义锚点：正式路径文件名即内容哈希（写入端全量算哈希后命名 + temp+rename
        // 原子提交），命中复用只做大小闸门；同尺寸不同内容属于已接受的残余风险
        // （仅剩磁盘级损坏一种来源），不再做全量重哈希。
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("cas.bin");
        fs::write(&path, b"complete").expect("complete cas");

        assert!(check_existing_cas_size(&path, 8).is_ok());
        assert!(check_existing_cas_size_async(&path, 8).await.is_ok());

        // 同尺寸内容差异：按设计接受（防截断，不防篡改）。
        fs::write(&path, b"corrupt!").expect("same-size corruption");
        assert!(check_existing_cas_size(&path, 8).is_ok());
        assert!(check_existing_cas_size_async(&path, 8).await.is_ok());

        // 截断/尺寸不符：拒绝复用。
        fs::write(&path, b"trunc").expect("truncated cas");
        assert!(check_existing_cas_size(&path, 8).is_err());
        assert!(check_existing_cas_size_async(&path, 8).await.is_err());
    }

    #[tokio::test]
    async fn registering_cas_promotes_only_live_desktop_only_relations() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open database");
        sqlx::query(
            "CREATE TABLE attachments (
                hash TEXT PRIMARY KEY, mime_type TEXT, size INTEGER, internal_path TEXT,
                created_at INTEGER, updated_at INTEGER
             );
             CREATE TABLE messages (
                owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT, deleted_at INTEGER,
                PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
             );
             CREATE TABLE agents (owner_type TEXT, agent_id TEXT PRIMARY KEY, deleted_at INTEGER);
             CREATE TABLE groups (owner_type TEXT, group_id TEXT PRIMARY KEY, deleted_at INTEGER);
             CREATE TABLE topics (
                owner_type TEXT, owner_id TEXT, topic_id TEXT, deleted_at INTEGER,
                PRIMARY KEY(owner_type, owner_id, topic_id)
             );
             CREATE TABLE message_attachments (
                owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT,
                hash TEXT, status TEXT, src TEXT
             );
             INSERT INTO agents VALUES
                ('agent', 'agent', NULL), ('agent', 'deleted-agent', 9);
             INSERT INTO topics VALUES
                ('agent', 'agent', 'topic', NULL),
                ('agent', 'agent', 'deleted-topic', 9),
                ('agent', 'deleted-agent', 'deleted-owner-topic', NULL);
             INSERT INTO messages VALUES
                ('agent', 'agent', 'topic', 'live', NULL),
                ('agent', 'agent', 'topic', 'deleted-message', 9),
                ('agent', 'agent', 'deleted-topic', 'deleted-topic-message', NULL),
                ('agent', 'deleted-agent', 'deleted-owner-topic', 'deleted-owner-message', NULL);
             INSERT INTO message_attachments VALUES
                ('agent', 'agent', 'topic', 'live', 'hash', 'desktop_only', NULL),
                ('agent', 'agent', 'topic', 'deleted-message', 'hash', 'desktop_only', NULL),
                ('agent', 'agent', 'deleted-topic', 'deleted-topic-message', 'hash', 'desktop_only', NULL),
                ('agent', 'deleted-agent', 'deleted-owner-topic', 'deleted-owner-message', 'hash', 'desktop_only', NULL);",
        )
        .execute(&pool)
        .await
        .expect("create fixture");

        let db_state = DbState::new(pool.clone(), std::path::PathBuf::new());
        commit_registered_attachment(&db_state, "hash", "text/plain", 4, "/cas/hash", 10)
            .await
            .expect("register attachment");

        let relations: Vec<(String, String, Option<String>)> =
            sqlx::query_as("SELECT msg_id, status, src FROM message_attachments ORDER BY msg_id")
                .fetch_all(&pool)
                .await
                .expect("read relations");
        assert_eq!(
            relations,
            vec![
                ("deleted-message".into(), "desktop_only".into(), None),
                ("deleted-owner-message".into(), "desktop_only".into(), None),
                ("deleted-topic-message".into(), "desktop_only".into(), None),
                (
                    "live".into(),
                    "ready".into(),
                    Some("file:///cas/hash".into())
                ),
            ]
        );
    }
}
