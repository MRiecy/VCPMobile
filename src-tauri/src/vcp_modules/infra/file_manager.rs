use crate::vcp_modules::db_manager::DbState;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::sync::{Arc, OnceLock};
use tokio::io::AsyncReadExt;

use tauri::{AppHandle, Emitter, Manager, State};

const STORE_FILE_IPC_MAX_BYTES: usize = 2 * 1024 * 1024;
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

fn canonical_file_within_root(
    root: &std::path::Path,
    file: &std::path::Path,
    label: &str,
) -> Result<std::path::PathBuf, String> {
    let canonical_root = std::fs::canonicalize(root)
        .map_err(|error| format!("{} staging 根目录不可用: {}", label, error))?;
    let canonical_file = std::fs::canonicalize(file)
        .map_err(|error| format!("{} staging 文件不可用: {}", label, error))?;
    if !canonical_file.starts_with(&canonical_root) || !canonical_file.is_file() {
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

fn verify_small_existing_cas(
    path: &std::path::Path,
    expected_hash: &str,
    expected_size: u64,
) -> Result<(), String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("CAS 文件元数据读取失败: {}", error))?;
    if !metadata.is_file() || metadata.len() != expected_size {
        return Err("已存在的 CAS 文件大小不匹配，拒绝复用".to_string());
    }
    let bytes = fs::read(path).map_err(|error| format!("CAS 文件校验读取失败: {}", error))?;
    let actual_hash = crate::vcp_modules::infra::utils::calculate_sha256(&bytes);
    if actual_hash != expected_hash {
        return Err("已存在的 CAS 文件内容哈希不匹配，拒绝复用".to_string());
    }
    Ok(())
}

pub(crate) async fn verify_existing_cas(
    path: &std::path::Path,
    expected_hash: &str,
    expected_size: u64,
) -> Result<(), String> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|error| format!("CAS 文件元数据读取失败: {}", error))?;
    if !metadata.is_file() || metadata.len() != expected_size {
        return Err("已存在的 CAS 文件大小不匹配，拒绝复用".to_string());
    }
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| format!("CAS 文件校验打开失败: {}", error))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| format!("CAS 文件校验读取失败: {}", error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    if hex::encode(hasher.finalize()) != expected_hash {
        return Err("已存在的 CAS 文件内容哈希不匹配，拒绝复用".to_string());
    }
    Ok(())
}

/// =================================================================
/// vcp_modules/file_manager.rs - 附件物理存储与分片上传管理
/// =================================================================
/// 核心路径解析：获取基础数据存储根目录
/// Android: /storage/emulated/0/Android/data/<pkg>/files
pub fn get_data_root_dir<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> Result<std::path::PathBuf, String> {
    // document_dir 在 Android 上通常指向 .../files/documents
    let mut path = app_handle
        .path()
        .document_dir()
        .map_err(|e| format!("Failed to get document_dir: {}", e))?;
    path.pop(); // 弹出 documents，留下 .../files
    Ok(path)
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
    let mut path = get_data_root_dir(app_handle)?;
    path.push("thumbnails");
    Ok(path)
}

/// 核心路径解析：获取多模态抽取/转码持久化缓存目录
/// Android: /storage/emulated/0/Android/data/<pkg>/files/multimodal_cache
pub fn get_multimodal_cache_dir<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> Result<std::path::PathBuf, String> {
    let mut path = get_data_root_dir(app_handle)?;
    path.push("multimodal_cache");
    Ok(path)
}

/// 物理安全的文件重命名工具，能够跨越物理挂载分区 (EXDEV) 降级进行物理拷贝+删除
pub fn safe_rename<P: AsRef<std::path::Path>, Q: AsRef<std::path::Path>>(
    from: P,
    to: Q,
) -> std::io::Result<()> {
    let from = from.as_ref();
    let to = to.as_ref();

    if std::fs::rename(from, to).is_err() {
        // 跨分区时先复制到目标目录的唯一临时文件，再原子提交，禁止正式路径出现半文件。
        let parent = to.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "附件目标缺少父目录")
        })?;
        let temporary = parent.join(format!(".ingest-{}.tmp", uuid::Uuid::new_v4()));
        if let Err(error) = std::fs::copy(from, &temporary) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        if let Err(error) = std::fs::rename(&temporary, to) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        let _ = std::fs::remove_file(from);
    }
    Ok(())
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

/// 内部辅助函数：生成图片缩略图（短边 200px 自适应，已下沉到 Android Kotlin 侧，此处直接返回 None）
pub async fn generate_thumbnail<R: tauri::Runtime>(
    _app_handle: &tauri::AppHandle<R>,
    _original_path: &std::path::Path,
    _hash: &str,
) -> Option<String> {
    None
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

    let config_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    let canonical_config = std::fs::canonicalize(&config_dir).unwrap_or(config_dir);

    let cache_dir = app_handle
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?;
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

/// 内部辅助函数：获取当前平台下的真实路径 (用于历史记录自动纠错)
#[allow(dead_code)]
pub fn resolve_attachment_path(
    app_handle: &AppHandle,
    hash: &str,
    original_name: &str,
) -> Option<String> {
    if !crate::vcp_modules::infra::utils::is_valid_cas_hash(hash) {
        return None;
    }
    let attachments_dir = get_attachments_root_dir(app_handle).ok()?;

    let internal_file_name = safe_storage_extension(original_name)
        .map(|extension| format!("{}.{}", hash, extension))
        .unwrap_or_else(|| hash.to_string());

    let full_path = attachments_dir.join(internal_file_name);
    if full_path.exists() {
        Some(full_path.to_string_lossy().to_string())
    } else {
        None
    }
}

/// 内存映射读取文件，自动检测编码并转换为 UTF-8
/// 1. 优先 BOM 头检测（最可靠）
/// 2. 无 BOM 时使用 chardetng 统计检测（Firefox 同款）
use super::file_extractor::try_extract_text;

async fn commit_registered_attachment(
    pool: &sqlx::SqlitePool,
    hash: &str,
    mime_type: &str,
    size: u64,
    internal_path: &str,
    now: i64,
) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
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
         WHERE hash = ? AND status = 'desktop_only' AND deleted_at IS NULL
           AND EXISTS (
             SELECT 1 FROM messages m
             WHERE m.topic_id = message_attachments.topic_id
               AND m.msg_id = message_attachments.msg_id
               AND m.deleted_at IS NULL
           )
           AND EXISTS (
             SELECT 1 FROM topics t
             WHERE t.topic_id = message_attachments.topic_id
               AND t.deleted_at IS NULL
               AND (
                 (t.owner_type = 'agent' AND EXISTS (
                   SELECT 1 FROM agents a
                   WHERE a.agent_id = t.owner_id AND a.deleted_at IS NULL
                 ))
                 OR
                 (t.owner_type = 'group' AND EXISTS (
                   SELECT 1 FROM groups g
                   WHERE g.group_id = t.owner_id AND g.deleted_at IS NULL
                 ))
               )
           )",
    )
    .bind(format!("file://{internal_path}"))
    .bind(hash)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())
}

/// 将文件元数据注册到数据库并触发后处理 (缩略图、文本提取)
pub async fn register_attachment_internal<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    pool: &sqlx::SqlitePool,
    hash: String,
    original_name: String,
    mime_type: String,
    size: u64,
    internal_path: String,
) -> Result<AttachmentData, String> {
    let now = crate::vcp_modules::infra::utils::now_secs() as u64;

    // 1. 原子更新 CAS 索引，并把仍然存活的 desktop_only 关系提升为 ready。
    commit_registered_attachment(pool, &hash, &mime_type, size, &internal_path, now as i64).await?;

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
        if let Err(error) = sqlx::query(
            "UPDATE attachments 
             SET extracted_text = ?, thumbnail_path = ?, updated_at = ? 
             WHERE hash = ?",
        )
        .bind(&extracted_text)
        .bind(&thumbnail_path)
        .bind(now as i64)
        .bind(&hash)
        .execute(pool)
        .await
        {
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
        extracted_text: None, // 掐断大文本在前端的冗余中转传输，前端预览直接 fetch 物理路径
        thumbnail_path,
    })
}

/// 存储文件到中心化附件目录 (内容寻址存储)
///
/// 【适用场景】非 Android 端的前端小文件上传 (<2MB) 及录音片段、二维码等内存数据。
/// Android 端不走此函数：Android 通过原生插件 `pick_file` 在 Native 层完成文件拷贝与
/// 哈希计算后，直接调用 `register_local_file` 进行零拷贝注册。
///
/// IPC 仅承载小文件；更大文件必须走现有高速流式通道。
#[tauri::command]
pub async fn store_file(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    original_name: String,
    file_bytes: Vec<u8>,
    mime_type: String,
) -> Result<AttachmentData, String> {
    if file_bytes.len() > STORE_FILE_IPC_MAX_BYTES {
        return Err("文件过大，请使用高速链路上传 (Limit: 2MB)".to_string());
    }

    let attachments_dir = get_attachments_root_dir(&app_handle)?;
    tokio::fs::create_dir_all(&attachments_dir)
        .await
        .map_err(|e| e.to_string())?;

    let _permit = store_file_semaphore()
        .acquire_owned()
        .await
        .map_err(|_| "文件存储执行器已关闭".to_string())?;
    let size = file_bytes.len() as u64;
    let extension = safe_storage_extension(&original_name).map(str::to_owned);
    let (hash, internal_file_path) = tokio::task::spawn_blocking(move || {
        let hash = crate::vcp_modules::infra::utils::calculate_sha256(&file_bytes);
        let internal_file_name = extension
            .map(|extension| format!("{}.{}", hash, extension))
            .unwrap_or_else(|| hash.clone());
        let internal_file_path = attachments_dir.join(internal_file_name);
        if internal_file_path.exists() {
            verify_small_existing_cas(&internal_file_path, &hash, file_bytes.len() as u64)?;
        } else {
            let temporary_path =
                attachments_dir.join(format!(".ingest-{}-{}.tmp", hash, uuid::Uuid::new_v4()));
            if let Err(error) = fs::write(&temporary_path, &file_bytes) {
                let _ = fs::remove_file(&temporary_path);
                return Err(error.to_string());
            }
            if let Err(error) = fs::rename(&temporary_path, &internal_file_path) {
                let _ = fs::remove_file(&temporary_path);
                if !internal_file_path.exists() {
                    return Err(error.to_string());
                }
                verify_small_existing_cas(&internal_file_path, &hash, file_bytes.len() as u64)?;
            }
        }
        Ok::<_, String>((hash, internal_file_path))
    })
    .await
    .map_err(|error| format!("文件存储任务异常: {}", error))??;
    let internal_path_str = internal_file_path
        .to_str()
        .ok_or("无效的附件路径字符")?
        .to_string();

    // 3. 注册并返回元数据
    let refined_mime = get_refined_mime_type(&internal_file_path, &original_name, &mime_type);
    register_attachment_internal(
        &app_handle,
        &db_state.pool,
        hash,
        original_name,
        refined_mime,
        size,
        internal_path_str,
    )
    .await
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
) -> Result<AttachmentData, String> {
    let uploads_root = app_handle
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("uploads");
    tokio::fs::create_dir_all(&uploads_root)
        .await
        .map_err(|e| format!("无法创建上传 staging 目录: {}", e))?;
    let source_path =
        canonical_file_within_root(&uploads_root, std::path::Path::new(&local_path), "附件")?;
    let canonical_uploads_root = std::fs::canonicalize(&uploads_root)
        .map_err(|e| format!("上传 staging 根目录规范化失败: {}", e))?;
    if source_path.parent() != Some(canonical_uploads_root.as_path()) {
        return Err("附件必须是 uploads staging 根目录的直接文件".to_string());
    }

    let staged_thumbnail = match thumbnail_path.as_deref() {
        Some(path) => {
            let thumbnail_root = uploads_root.join("thumbnails");
            tokio::fs::create_dir_all(&thumbnail_root)
                .await
                .map_err(|e| format!("无法创建缩略图 staging 目录: {}", e))?;
            let staged =
                canonical_file_within_root(&thumbnail_root, std::path::Path::new(path), "缩略图")?;
            let canonical_thumbnail_root = std::fs::canonicalize(&thumbnail_root)
                .map_err(|e| format!("缩略图 staging 根目录规范化失败: {}", e))?;
            if staged.parent() != Some(canonical_thumbnail_root.as_path()) {
                return Err("缩略图必须是专用 staging 根目录的直接文件".to_string());
            }
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

    // 2. 异步读取元数据 (获取文件物理大小)
    let meta = tokio::fs::metadata(&source_path)
        .await
        .map_err(|e| format!("无法读取源文件元数据: {}", e))?;
    let size = meta.len();
    if size > STAGED_FILE_MAX_BYTES {
        return Err("staging 文件过大 (Limit: 512MB)".to_string());
    }

    // 3. Native 端给出的 hash 仅作一致性提示；特权边界始终流式重算。
    let mut file = tokio::fs::File::open(&source_path)
        .await
        .map_err(|e| format!("无法打开源文件: {}", e))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut hashed_bytes = 0u64;
    let mut last_emit_time = std::time::Instant::now();
    loop {
        let n = file
            .read(&mut buffer)
            .await
            .map_err(|e| format!("读取源文件失败: {}", e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        hashed_bytes += n as u64;
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
    let hash = hex::encode(hasher.finalize());
    verify_expected_hash(expected_hash.as_deref(), &hash)?;

    // 4. 计算目标路径
    let internal_file_name = safe_storage_extension(&original_name)
        .map(|extension| format!("{}.{}", hash, extension))
        .unwrap_or_else(|| hash.clone());

    let attachments_dir = get_attachments_root_dir(&app_handle)?;
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
        verify_existing_cas(&dest_path, &hash, size).await?;
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
            if let Err(error) = tokio::fs::copy(&source_path, &temporary_path).await {
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
                    verify_existing_cas(&dest_path, &hash, size).await?;
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
    let registration = register_attachment_internal(
        &app_handle,
        &db_state.pool,
        hash.clone(),
        original_name,
        refined_mime,
        size,
        dest_path_str,
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
            let thumbs_dir = get_thumbnails_root_dir(&app_handle)?;
            if !thumbs_dir.exists() {
                tokio::fs::create_dir_all(&thumbs_dir)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            let dest_thumb_path = thumbs_dir.join(format!("{}_thumb.webp", hash));
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
            sqlx::query("UPDATE attachments SET thumbnail_path = ?, updated_at = ? WHERE hash = ?")
                .bind(&dest_thumb_path_str)
                .bind(attachment_data.created_at as i64)
                .bind(&hash)
                .execute(&db_state.pool)
                .await
                .map_err(|error| format!("更新附件缩略图元数据失败: {}", error))?;
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

/// 移动端/桌面端原生文件选取与存储 (流式防 OOM 优化版)
#[tauri::command]
pub async fn get_attachment_real_path(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    hash: String,
    _original_name: String,
) -> Result<String, String> {
    if !crate::vcp_modules::infra::utils::is_valid_cas_hash(&hash) {
        return Err("非法的 Content-Addressable Storage (CAS) 哈希指纹格式".to_string());
    }
    let internal_path: Option<String> =
        sqlx::query_scalar("SELECT internal_path FROM attachments WHERE hash = ? LIMIT 1")
            .bind(&hash)
            .fetch_optional(&db_state.pool)
            .await
            .map_err(|e| format!("读取附件元数据失败: {}", e))?;
    let internal_path = internal_path.ok_or("本地附件库中未找到该文件")?;
    let clean_path = internal_path
        .strip_prefix("file://")
        .unwrap_or(&internal_path);
    let attachments_dir = get_attachments_root_dir(&app_handle)?;
    let canonical_path =
        canonical_file_within_root(&attachments_dir, std::path::Path::new(clean_path), "附件库")?;
    Ok(canonical_path.to_string_lossy().to_string())
}

/// 唤起系统默认应用打开文件或 URL
#[tauri::command]
pub async fn open_file(app_handle: AppHandle, path: String) -> Result<(), String> {
    let clean_path = path.replace("file://", "");

    // 网络 URL 直接打开，跳过本地路径安全校验
    if clean_path.starts_with("http://") || clean_path.starts_with("https://") {
        use tauri_plugin_opener::OpenerExt;
        return app_handle
            .opener()
            .open_url(clean_path, Option::<String>::None)
            .map_err(|e| e.to_string());
    }

    let path_buf = std::path::PathBuf::from(&clean_path);

    // 安全校验：禁止打开系统敏感路径
    ensure_safe_path(&app_handle, &path_buf)?;

    #[cfg(target_os = "android")]
    {
        return tauri_plugin_vcp_mobile::system::open_file_native(app_handle, clean_path);
    }

    // 使用 tauri-plugin-opener 的原生能力
    #[cfg(not(target_os = "android"))]
    {
        use tauri_plugin_opener::OpenerExt;
        app_handle
            .opener()
            .open_path(clean_path, Option::<String>::None)
            .map_err(|e| e.to_string())
    }
}

/// 清理上传缓存目录以及多媒体缓存碎片 (通常在启动时执行，清除上次闪退/强杀留下的僵尸文件)
pub fn clear_upload_cache(app_handle: &AppHandle) {
    if let Ok(cache_dir) = app_handle.path().app_cache_dir() {
        // 1. 清除上次上传未完成的分片临时目录
        let uploads_path = cache_dir.join("uploads");
        if uploads_path.exists() {
            let _ = fs::remove_dir_all(&uploads_path);
            let _ = fs::create_dir_all(&uploads_path);
            log::info!("[FileManager] Upload cache cleared.");
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

        // 3. 自动收敛多模态结果缓存 (300MB 阈值，收敛至 150MB)
        evict_multimodal_cache_if_needed(app_handle, 300 * 1024 * 1024, 150 * 1024 * 1024);
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
    pool: &sqlx::SqlitePool,
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
        let pool_c = pool.clone();
        let hash_c = hash.to_string();
        let text_c = text.clone();

        // 3. 异步持久化写入 SQLite，不阻塞当前的上下文加载请求
        tokio::spawn(async move {
            let _ = sqlx::query(
                "UPDATE attachments SET extracted_text = ?, updated_at = ? WHERE hash = ?",
            )
            .bind(&text_c)
            .bind(chrono::Utc::now().timestamp_millis())
            .bind(&hash_c)
            .execute(&pool_c)
            .await;
        });

        Some(text)
    } else {
        None
    }
}

// Online attachment GC is disabled. Keep this hardened sink dormant until a
// future owner + quarantine/grace workflow can safely reuse it.
#[allow(dead_code)]
fn invalid_delete_target(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message.into())
}

#[allow(dead_code)]
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

#[allow(dead_code)]
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

/// 强力物理删除指定的附件文件及其可能关联的原生硬解缩略图。
/// 当前在线维护不会调用此 sink；边界仍在这里收紧，防止未来复用越界删除。
#[allow(dead_code)]
pub async fn delete_attachment_physical<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    hash: &str,
    internal_path: &str,
) -> std::io::Result<()> {
    let path = std::path::Path::new(internal_path);
    let attachments_root = get_attachments_root_dir(app_handle).map_err(invalid_delete_target)?;
    let attachment_path = validated_attachment_file(&attachments_root, hash, path)?;

    // 统一处理缩略图定位与删除
    let thumbnails_root = get_thumbnails_root_dir(app_handle).map_err(invalid_delete_target)?;
    let thumb_name = format!("{}_thumb.webp", hash);
    let thumb_path = thumbnails_root.join(&thumb_name);
    let thumbnail_path = validated_direct_file(
        &thumbnails_root,
        &thumb_path,
        std::ffi::OsStr::new(&thumb_name),
    )?;

    // 统一处理多模态持久化缓存删除
    let cache_root = get_multimodal_cache_dir(app_handle).map_err(invalid_delete_target)?;
    let cache_name = format!("{}.json", hash);
    let cache_path = cache_root.join(&cache_name);
    let multimodal_path =
        validated_direct_file(&cache_root, &cache_path, std::ffi::OsStr::new(&cache_name))?;

    for validated in [attachment_path, thumbnail_path, multimodal_path]
        .into_iter()
        .flatten()
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
    use super::{
        canonical_file_within_root, commit_registered_attachment, safe_storage_extension,
        validated_attachment_file, validated_direct_file, verify_existing_cas,
        verify_expected_hash, verify_small_existing_cas,
    };
    use std::fs;

    #[test]
    fn storage_extension_keeps_common_suffixes_but_drops_path_payloads() {
        assert_eq!(safe_storage_extension("报告.PDF"), Some("PDF"));
        assert_eq!(safe_storage_extension("archive.tar.gz"), Some("gz"));
        assert_eq!(safe_storage_extension("payload.a/b"), None);
        assert_eq!(safe_storage_extension("payload.../../secret"), None);
        assert_eq!(safe_storage_extension("payload.very_long_extension"), None);
    }

    #[test]
    fn canonical_staging_gate_rejects_parent_and_symlink_escape() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("uploads");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&root).expect("root");
        fs::create_dir_all(&outside).expect("outside");
        let staged = root.join("ok.bin");
        let escaped = outside.join("secret.bin");
        fs::write(&staged, b"ok").expect("staged");
        fs::write(&escaped, b"secret").expect("escaped");

        assert!(canonical_file_within_root(&root, &staged, "test").is_ok());
        assert!(
            canonical_file_within_root(&root, &root.join("../outside/secret.bin"), "test").is_err()
        );

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&escaped, root.join("link.bin")).expect("symlink");
            assert!(canonical_file_within_root(&root, &root.join("link.bin"), "test").is_err());
        }
    }

    #[test]
    fn native_hash_is_only_a_hint_and_must_match_rust_recomputation() {
        let actual = "a".repeat(64);
        let forged = "b".repeat(64);
        assert!(verify_expected_hash(Some(&actual), &actual).is_ok());
        assert!(verify_expected_hash(Some(&forged), &actual).is_err());
        assert!(verify_expected_hash(None, &actual).is_ok());
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
    async fn existing_cas_file_must_match_size_and_hash_before_reuse() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("cas.bin");
        let bytes = b"complete";
        let hash = crate::vcp_modules::infra::utils::calculate_sha256(bytes);
        fs::write(&path, bytes).expect("complete cas");

        assert!(verify_small_existing_cas(&path, &hash, bytes.len() as u64).is_ok());
        assert!(verify_existing_cas(&path, &hash, bytes.len() as u64)
            .await
            .is_ok());

        fs::write(&path, b"corrupt!").expect("corrupt cas");
        assert!(verify_small_existing_cas(&path, &hash, bytes.len() as u64).is_err());
        assert!(verify_existing_cas(&path, &hash, bytes.len() as u64)
            .await
            .is_err());
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
                topic_id TEXT, msg_id TEXT, deleted_at INTEGER,
                PRIMARY KEY(topic_id, msg_id)
             );
             CREATE TABLE agents (agent_id TEXT PRIMARY KEY, deleted_at INTEGER);
             CREATE TABLE groups (group_id TEXT PRIMARY KEY, deleted_at INTEGER);
             CREATE TABLE topics (
                topic_id TEXT PRIMARY KEY, owner_id TEXT, owner_type TEXT, deleted_at INTEGER
             );
             CREATE TABLE message_attachments (
                topic_id TEXT, msg_id TEXT, hash TEXT, status TEXT, src TEXT, deleted_at INTEGER
             );
             INSERT INTO agents VALUES ('agent', NULL), ('deleted-agent', 9);
             INSERT INTO topics VALUES
                ('topic', 'agent', 'agent', NULL),
                ('deleted-topic', 'agent', 'agent', 9),
                ('deleted-owner-topic', 'deleted-agent', 'agent', NULL);
             INSERT INTO messages VALUES
                ('topic', 'live', NULL),
                ('topic', 'deleted-message', 9),
                ('topic', 'tombstoned-relation', NULL),
                ('deleted-topic', 'deleted-topic-message', NULL),
                ('deleted-owner-topic', 'deleted-owner-message', NULL);
             INSERT INTO message_attachments VALUES
                ('topic', 'live', 'hash', 'desktop_only', NULL, NULL),
                ('topic', 'deleted-message', 'hash', 'desktop_only', NULL, NULL),
                ('topic', 'tombstoned-relation', 'hash', 'removed', NULL, 9),
                ('deleted-topic', 'deleted-topic-message', 'hash', 'desktop_only', NULL, NULL),
                ('deleted-owner-topic', 'deleted-owner-message', 'hash', 'desktop_only', NULL, NULL);",
        )
        .execute(&pool)
        .await
        .expect("create fixture");

        commit_registered_attachment(&pool, "hash", "text/plain", 4, "/cas/hash", 10)
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
                ("tombstoned-relation".into(), "removed".into(), None),
            ]
        );
    }
}
