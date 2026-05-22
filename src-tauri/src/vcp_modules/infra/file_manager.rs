use crate::vcp_modules::db_manager::DbState;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};

use std::sync::Mutex;

/// =================================================================
/// vcp_modules/file_manager.rs - 附件物理存储与分片上传管理
/// =================================================================
/// 核心路径解析：获取附件存储根目录
/// Android: /storage/emulated/0/Android/data/<pkg>/files/attachments
/// Windows: %APPDATA%/<pkg>/data/attachments
pub fn get_attachments_root_dir<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> Result<std::path::PathBuf, String> {
    #[cfg(target_os = "android")]
    {
        // document_dir 在 Android 上通常指向 .../files/documents
        if let Ok(mut path) = app_handle.path().document_dir() {
            path.pop(); // 弹出 documents
            path.push("attachments");
            return Ok(path);
        }
    }

    // 桌面端或 Fallback: 使用内部配置目录下的 data/attachments
    let mut path = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| format!("Failed to get app_config_dir: {}", e))?;
    path.push("data");
    path.push("attachments");
    Ok(path)
}

/// 核心路径解析：获取缩略图存储根目录
pub fn get_thumbnails_root_dir<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> Result<std::path::PathBuf, String> {
    let mut path = get_attachments_root_dir(app_handle)?;
    path.pop(); // 弹出 attachments
    path.push("thumbnails");
    Ok(path)
}

/// 迁移逻辑：将旧的内部存储附件迁移到新的外部存储目录 (仅限 Android)
pub fn migrate_legacy_attachments(_app_handle: &AppHandle) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let app_handle = _app_handle;
        let mut old_dir = app_handle
            .path()
            .app_config_dir()
            .map_err(|e| e.to_string())?;
        old_dir.push("data");
        old_dir.push("attachments");

        let new_dir = get_attachments_root_dir(app_handle)?;

        if old_dir.exists() && old_dir != new_dir {
            if !new_dir.exists() {
                fs::create_dir_all(&new_dir).map_err(|e| e.to_string())?;
            }

            log::info!(
                "[FileManager] Migrating attachments from {:?} to {:?}",
                old_dir,
                new_dir
            );

            if let Ok(entries) = fs::read_dir(&old_dir) {
                for entry in entries.flatten() {
                    let old_path = entry.path();
                    if old_path.is_file() {
                        let file_name = old_path.file_name().unwrap();
                        let new_path = new_dir.join(file_name);
                        if !new_path.exists() {
                            let _ = fs::rename(&old_path, &new_path);
                        } else {
                            let _ = fs::remove_file(&old_path);
                        }
                    }
                }
            }

            // 迁移缩略图
            let mut old_thumb_dir = old_dir.clone();
            old_thumb_dir.pop();
            old_thumb_dir.push("thumbnails");

            let new_thumb_dir = get_thumbnails_root_dir(app_handle)?;
            if old_thumb_dir.exists() {
                if !new_thumb_dir.exists() {
                    let _ = fs::create_dir_all(&new_thumb_dir);
                }
                if let Ok(entries) = fs::read_dir(&old_thumb_dir) {
                    for entry in entries.flatten() {
                        let old_path = entry.path();
                        if old_path.is_file() {
                            let file_name = old_path.file_name().unwrap();
                            let new_path = new_thumb_dir.join(file_name);
                            if !new_path.exists() {
                                let _ = fs::rename(&old_path, &new_path);
                            } else {
                                let _ = fs::remove_file(&old_path);
                            }
                        }
                    }
                }
            }

            // 清理旧目录
            let _ = fs::remove_dir_all(&old_dir);
            let _ = fs::remove_dir_all(&old_thumb_dir);
        }
    }
    Ok(())
}

pub struct UploadSession {
    pub temp_path: std::path::PathBuf,
    pub original_name: String,
    pub mime_type: String,
    pub hasher: Mutex<Sha256>,
    pub current_size: Mutex<u64>,
}

pub struct UploadManagerState {
    // 正在进行中的分片上传任务
    pub sessions: DashMap<String, Arc<UploadSession>>,
}

impl UploadManagerState {
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
        }
    }
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
pub fn get_refined_mime_type(path: &std::path::Path, original_name: &str, initial_mime: &str) -> String {
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
            "js" | "mjs" | "bat" | "sh" | "py" | "java" | "c" | "cpp" | "h" | "hpp" | "cs"
            | "go" | "rb" | "php" | "swift" | "kt" | "kts" | "ts" | "tsx" | "jsx" | "vue"
            | "yml" | "yaml" | "toml" | "ini" | "log" | "sql" | "jsonc" | "rs" | "dart" | "lua"
            | "r" | "pl" | "ex" | "exs" | "zig" | "hs" | "scala" | "groovy" | "d" | "nim"
            | "cr" => return "text/plain".to_string(),
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
    let config_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;

    let cache_dir = app_handle
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?;

    // 允许访问 App 配置目录 (内部)、缓存目录 (临时)、附件目录 (可能在外部) 或 缩略图目录
    let attachments_dir = get_attachments_root_dir(app_handle)?;
    let thumbnails_dir = get_thumbnails_root_dir(app_handle)?;

    if path.starts_with(&config_dir)
        || path.starts_with(&cache_dir)
        || path.starts_with(&attachments_dir)
        || path.starts_with(&thumbnails_dir)
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
    let attachments_dir = get_attachments_root_dir(app_handle).ok()?;

    let ext = std::path::Path::new(original_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let internal_file_name = if ext.is_empty() {
        hash.to_string()
    } else {
        format!("{}.{}", hash, ext)
    };

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
fn read_text_with_mmap(path: &std::path::Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mmap = unsafe { memmap2::Mmap::map(&file).ok()? };

    // 1. BOM 检测
    if let Some((encoding, _bom_len)) = encoding_rs::Encoding::for_bom(&mmap) {
        let (text, _had_errors) = encoding.decode_with_bom_removal(&mmap);
        return Some(text.into_owned());
    }

    // 2. 统计检测（无 BOM）
    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(&mmap, true);
    let encoding = detector.guess(None, true);

    let (text, _had_errors) = encoding.decode_without_bom_handling(&mmap);
    Some(text.into_owned())
}

/// 内部辅助函数：根据 MIME 类型或扩展名提取文本内容
pub fn try_extract_text(path: &std::path::Path, mime_type: &str) -> Option<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    // 1. 识别是否属于“可直接读取”的文本/代码格式
    let is_text_type = mime_type.starts_with("text/")
        || mime_type == "application/json"
        || mime_type == "application/javascript"
        || mime_type == "application/x-javascript"
        || matches!(
            ext.as_str(),
            "md" | "txt"
                | "json"
                | "js"
                | "mjs"
                | "bat"
                | "sh"
                | "ts"
                | "tsx"
                | "jsx"
                | "vue"
                | "rs"
                | "py"
                | "java"
                | "c"
                | "cpp"
                | "h"
                | "hpp"
                | "cs"
                | "go"
                | "rb"
                | "php"
                | "swift"
                | "kt"
                | "css"
                | "html"
                | "xml"
                | "yaml"
                | "yml"
                | "toml"
                | "ini"
                | "sql"
                | "log"
                | "jsonc"
                | "dart"
                | "lua"
                | "r"
                | "pl"
                | "ex"
                | "exs"
                | "zig"
                | "hs"
                | "scala"
                | "groovy"
                | "d"
                | "nim"
                | "cr"
                | "csv"
        );

    if is_text_type {
        // 硬上限：防止极端巨型文件载入内存导致 OOM（50MB 为安全阈值）
        const MAX_FILE_SIZE_BYTES: u64 = 50 * 1024 * 1024;
        if let Ok(meta) = fs::metadata(path) {
            if meta.len() > MAX_FILE_SIZE_BYTES {
                return Some(format!(
                    "[文件过大（{:.2} MB），已跳过自动提取以保护内存]",
                    (meta.len() as f64) / 1024.0 / 1024.0
                ));
            }
        }

        // mmap + 自动编码检测 → UTF-8
        let text = read_text_with_mmap(path)?;

        // 按提取文本长度截断（对齐 2M 上下文模型，约 8-10M 字符）
        const MAX_TEXT_CHARS: usize = 10_000_000;
        if text.chars().count() > MAX_TEXT_CHARS {
            let truncated: String = text.chars().take(MAX_TEXT_CHARS).collect();
            return Some(format!("{}……（文本过长已截断）", truncated));
        }

        return Some(text);
    }

    // 3. 结构化文档 (PDF, Docx, etc.)
    // 后端目前不具备解析能力，直接返回 None，由前端 JIT 处理器或专门的插件负责处理
    None
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
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // 1. 更新数据库 (attachments)
    sqlx::query(
        "INSERT INTO attachments (hash, mime_type, size, internal_path, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(hash) DO UPDATE SET internal_path = excluded.internal_path",
    )
    .bind(&hash)
    .bind(&mime_type)
    .bind(size as i64)
    .bind(&internal_path)
    .bind(now as i64)
    .bind(now as i64)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    let internal_file_path = std::path::PathBuf::from(&internal_path);

    // 2. 提取文本内容 (如果适用)
    let extracted_text = try_extract_text(&internal_file_path, &mime_type);

    // 3. 生成缩略图 (如果适用，spawn_blocking 隔离 CPU 密集型操作)
    let thumbnail_path = if mime_type.starts_with("image/") {
        generate_thumbnail(app_handle, &internal_file_path, &hash).await
    } else {
        None
    };

    let ext = std::path::Path::new(&original_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let internal_file_name = if ext.is_empty() {
        hash.clone()
    } else {
        format!("{}.{}", hash, ext)
    };

    Ok(AttachmentData {
        id: format!("attachment_{}", hash),
        name: original_name,
        internal_file_name,
        internal_path,
        mime_type,
        size,
        hash,
        created_at: now,
        extracted_text,
        thumbnail_path,
    })
}

/// 存储文件到中心化附件目录 (内容寻址存储)
/// 这个方法依然保留，用于接收前端传来的极小内存数据（如录音片段或二维码）
#[tauri::command]
pub async fn store_file(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    original_name: String,
    file_bytes: Vec<u8>,
    mime_type: String,
) -> Result<AttachmentData, String> {
    // 0. 冗余兜底：前端已将 >2MB 文件分流至高速链路，此检查在正常情况下几乎不会触发。
    //    保留作为深层防御，防止未来前端逻辑变更、异常调用或 IPC 绕过导致 OOM。
    if file_bytes.len() > 100 * 1024 * 1024 {
        return Err("文件过大，请使用高速链路上传 (Limit: 100MB)".to_string());
    }

    // 1. 计算 SHA256 哈希值
    let mut hasher = Sha256::new();
    hasher.update(&file_bytes);
    let hash = hex::encode(hasher.finalize());

    let file_extension = std::path::Path::new(&original_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let internal_file_name = if file_extension.is_empty() {
        hash.clone()
    } else {
        format!("{}.{}", hash, file_extension)
    };

    let attachments_dir = get_attachments_root_dir(&app_handle)?;

    if !attachments_dir.exists() {
        fs::create_dir_all(&attachments_dir).map_err(|e| e.to_string())?;
    }

    let internal_file_path = attachments_dir.join(&internal_file_name);
    let internal_path_str = internal_file_path.to_str().unwrap().to_string();

    // 2. 写入物理文件 (如果哈希不存在)
    if !internal_file_path.exists() {
        fs::write(&internal_file_path, &file_bytes).map_err(|e| e.to_string())?;
    }

    // 3. 注册并返回元数据
    let refined_mime = get_refined_mime_type(&internal_file_path, &original_name, &mime_type);
    register_attachment_internal(
        &app_handle,
        &db_state.pool,
        hash,
        original_name,
        refined_mime,
        file_bytes.len() as u64,
        internal_path_str,
    )
    .await
}

// --- 分片上传系列指令 ---

#[tauri::command]
pub async fn init_chunked_upload(
    app_handle: AppHandle,
    state: State<'_, UploadManagerState>,
    original_name: String,
    mime_type: String,
) -> Result<String, String> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let mut temp_path = app_handle
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?;
    temp_path.push("uploads");
    if !temp_path.exists() {
        fs::create_dir_all(&temp_path).map_err(|e| e.to_string())?;
    }

    // 异步化僵尸文件清理流程，完全避免阻塞当前请求
    let temp_path_clone = temp_path.clone();
    let sessions_clone = state.sessions.clone();
    tokio::task::spawn(async move {
        const ORPHAN_TTL_SECS: u64 = 86400;
        let now = SystemTime::now();
        if let Ok(entries) = std::fs::read_dir(&temp_path_clone) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("tmp") {
                    let should_remove = if let Ok(metadata) = entry.metadata() {
                        if let Ok(modified) = metadata.modified() {
                            now.duration_since(modified)
                                .map(|d| d.as_secs() > ORPHAN_TTL_SECS)
                                .unwrap_or(false)
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    if should_remove {
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            sessions_clone.remove(stem);
                        }
                        let _ = std::fs::remove_file(&path);
                        log::info!("[FileManager] Removed orphan upload temp file: {:?}", path);
                    }
                }
            }
        }
    });

    temp_path.push(format!("{}.tmp", session_id));

    // 创建空文件
    fs::File::create(&temp_path).map_err(|e| e.to_string())?;

    let refined_mime = get_refined_mime_type(&temp_path, &original_name, &mime_type);
    let session = UploadSession {
        temp_path,
        original_name,
        mime_type: refined_mime,
        hasher: Mutex::new(Sha256::new()),
        current_size: Mutex::new(0),
    };

    state.sessions.insert(session_id.clone(), Arc::new(session));
    Ok(session_id)
}

#[tauri::command]
pub async fn append_chunk(
    state: State<'_, UploadManagerState>,
    session_id: String,
    chunk_bytes: Vec<u8>,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;

    let session = state.sessions.get(&session_id).ok_or("无效的上传会话")?;

    // 1. 同步更新哈希 (边搬砖边记账)
    {
        let mut hasher = session.hasher.lock().unwrap();
        hasher.update(&chunk_bytes);
    }

    // 2. 更新当前大小
    {
        let mut size = session.current_size.lock().unwrap();
        *size += chunk_bytes.len() as u64;
    }

    // 3. 异步写入磁盘
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&session.temp_path)
        .await
        .map_err(|e| format!("无法打开临时文件: {}", e))?;

    file.write_all(&chunk_bytes)
        .await
        .map_err(|e| format!("追加分片失败: {}", e))?;

    Ok(())
}

#[tauri::command]
pub async fn cancel_chunked_upload(
    _app_handle: AppHandle,
    state: State<'_, UploadManagerState>,
    session_id: String,
) -> Result<(), String> {
    if let Some((_, session)) = state.sessions.remove(&session_id) {
        if session.temp_path.exists() {
            let _ = fs::remove_file(&session.temp_path);
        }
        log::info!(
            "[FileManager] Cancelled and cleaned up upload session: {}",
            session_id
        );
    }
    Ok(())
}

#[tauri::command]
pub async fn finish_chunked_upload(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    state: State<'_, UploadManagerState>,
    session_id: String,
) -> Result<AttachmentData, String> {
    let (_, session) = state
        .sessions
        .remove(&session_id)
        .ok_or("上传会话已超时或不存在")?;

    // 1. 获取已经算好的哈希值 (0 内存读取开销！)
    let hasher = session.hasher.lock().unwrap().clone();
    let hash = hex::encode(hasher.finalize());
    let final_size = *session.current_size.lock().unwrap();

    // 2. 准备物理存储
    let ext = std::path::Path::new(&session.original_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let internal_file_name = if ext.is_empty() {
        hash.clone()
    } else {
        format!("{}.{}", hash, ext)
    };

    let attachments_dir = get_attachments_root_dir(&app_handle)?;
    if !attachments_dir.exists() {
        fs::create_dir_all(&attachments_dir).ok();
    }

    let internal_file_path = attachments_dir.join(&internal_file_name);
    let internal_path_str = internal_file_path.to_str().unwrap().to_string();

    // 3. 移动临时文件到正式目录 (Rename 是毫秒级的)
    fs::rename(&session.temp_path, &internal_file_path)
        .map_err(|e| format!("移动文件失败: {}", e))?;

    // 4. 对完整文件进行最终的 MIME 嗅探精修 (此时文件已完整，嗅探结果最准确)
    let final_refined_mime = get_refined_mime_type(&internal_file_path, &session.original_name, &session.mime_type);

    // 5. 复用统一的注册逻辑（入库、文本提取、缩略图生成）
    register_attachment_internal(
        &app_handle,
        &db_state.pool,
        hash,
        session.original_name.clone(),
        final_refined_mime,
        final_size,
        internal_path_str,
    )
    .await
}

/// 注册本地已有的文件（例如 Android Kotlin 端沙盒临时复制的大文件/硬解缩略图）
/// 彻底实现“前端零拷贝物理路径传输”
#[tauri::command]
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
    use tokio::io::AsyncReadExt;

    let source_path = std::path::PathBuf::from(&local_path);
    if !source_path.exists() {
        return Err(format!("本地源文件不存在: {}", local_path));
    }

    // 1. 安全性检查，防止路径遍历攻击
    ensure_safe_path(&app_handle, &source_path)?;

    // 2. 异步读取元数据 (获取文件物理大小)
    let meta = tokio::fs::metadata(&source_path)
        .await
        .map_err(|e| format!("无法读取源文件元数据: {}", e))?;
    let size = meta.len();

    // 3. 流式异步读取并计算 SHA-256 (若传入 expected_hash 则直接使用，免除二次哈希)
    let hash = match expected_hash {
        Some(h) => {
            log::info!("[FileManager] Reusing expected hash from native side: {}", h);
            h
        }
        None => {
            let mut file = tokio::fs::File::open(&source_path)
                .await
                .map_err(|e| format!("无法打开源文件: {}", e))?;
            
            let mut hasher = Sha256::new();
            let mut buffer = [0u8; 65536]; // 64KB 缓冲
            let mut hashed_bytes = 0u64;
            let mut last_emit_time = std::time::Instant::now();
            loop {
                let n = file.read(&mut buffer)
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
                        let pct = if size > 0 { (hashed_bytes as f64 / size as f64 * 100.0) as u32 } else { 0 };
                        let scaled_pct = 50 + (pct * 40 / 100); // 50% 到 90%
                        app_handle.emit("vcp-file-register-progress", serde_json::json!({
                            "progress": scaled_pct,
                            "stableId": sid,
                        })).ok();
                    }
                }
            }
            hex::encode(hasher.finalize())
        }
    };

    // 4. 计算目标路径
    let file_extension = std::path::Path::new(&original_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let internal_file_name = if file_extension.is_empty() {
        hash.clone()
    } else {
        format!("{}.{}", hash, file_extension)
    };

    let attachments_dir = get_attachments_root_dir(&app_handle)?;
    if !attachments_dir.exists() {
        tokio::fs::create_dir_all(&attachments_dir)
            .await
            .map_err(|e| e.to_string())?;
    }

    let dest_path = attachments_dir.join(&internal_file_name);
    let dest_path_str = dest_path.to_str().ok_or("无效的目标路径字符")?.to_string();

    // 如果目标文件已存在（内容寻址去重去冗余），则直接删除源临时文件
    if dest_path.exists() {
        let _ = tokio::fs::remove_file(&source_path).await;
        log::info!("[FileManager] Duplicated local file found. Removed source path: {}", local_path);
        if let Some(ref sid) = stable_id {
            app_handle.emit("vcp-file-register-progress", serde_json::json!({
                "progress": 99,
                "stableId": sid,
            })).ok();
        }
    } else {
        // 先尝试 rename 极速移动，失败时 fallback 复制 + 删除
        if let Some(ref sid) = stable_id {
            app_handle.emit("vcp-file-register-progress", serde_json::json!({
                "progress": 90,
                "stableId": sid,
            })).ok();
        }
        if tokio::fs::rename(&source_path, &dest_path).await.is_err() {
            tokio::fs::copy(&source_path, &dest_path)
                .await
                .map_err(|e| format!("复制文件到正式目录失败: {}", e))?;
            let _ = tokio::fs::remove_file(&source_path).await;
        }
        if let Some(ref sid) = stable_id {
            app_handle.emit("vcp-file-register-progress", serde_json::json!({
                "progress": 99,
                "stableId": sid,
            })).ok();
        }
    }

    // 5. 修正 MIME 类型
    let initial_mime = mime_type.unwrap_or_else(|| "application/octet-stream".to_string());
    let refined_mime = get_refined_mime_type(&dest_path, &original_name, &initial_mime);

    // 6. 调用统一的附件注册逻辑
    let mut attachment_data = register_attachment_internal(
        &app_handle,
        &db_state.pool,
        hash.clone(),
        original_name,
        refined_mime,
        size,
        dest_path_str,
    )
    .await?;

    // 7. 处理前端传入的已有缩略图 (如 Kotlin 侧硬件加速生成的缩略图)
    let mut final_thumbnail_path = attachment_data.thumbnail_path.clone();

    if let Some(ref tp) = thumbnail_path {
        let source_thumb = std::path::PathBuf::from(tp);
        if source_thumb.exists() {
            let thumbs_dir = get_thumbnails_root_dir(&app_handle)?;
            if !thumbs_dir.exists() {
                tokio::fs::create_dir_all(&thumbs_dir)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            let dest_thumb_path = thumbs_dir.join(format!("{}_thumb.webp", hash));
            let dest_thumb_path_str = dest_thumb_path.to_str().unwrap().to_string();

            if dest_thumb_path.exists() {
                let _ = tokio::fs::remove_file(&source_thumb).await;
            } else {
                if tokio::fs::rename(&source_thumb, &dest_thumb_path).await.is_err() {
                    if tokio::fs::copy(&source_thumb, &dest_thumb_path).await.is_ok() {
                        let _ = tokio::fs::remove_file(&source_thumb).await;
                    }
                }
            }

            // 更新 SQLite 中的 thumbnail_path，使其指向正式保存的缩略图
            sqlx::query(
                "UPDATE attachments SET thumbnail_path = ?, updated_at = ? WHERE hash = ?"
            )
            .bind(&dest_thumb_path_str)
            .bind(attachment_data.created_at as i64)
            .bind(&hash)
            .execute(&db_state.pool)
            .await
            .ok();

            final_thumbnail_path = Some(dest_thumb_path_str);
        }
    }

    attachment_data.thumbnail_path = final_thumbnail_path;
    Ok(attachment_data)
}

/// 移动端/桌面端原生文件选取与存储 (流式防 OOM 优化版)
#[tauri::command]
pub async fn get_attachment_real_path(
    app_handle: AppHandle,
    hash: String,
    original_name: String,
) -> Result<String, String> {
    let attachments_dir = get_attachments_root_dir(&app_handle)?;

    let file_extension = std::path::Path::new(&original_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let internal_file_name = if file_extension.is_empty() {
        hash
    } else {
        format!("{}.{}", hash, file_extension)
    };

    let full_path = attachments_dir.join(internal_file_name);
    if full_path.exists() {
        Ok(full_path.to_string_lossy().to_string())
    } else {
        Err("本地附件库中未找到该文件".to_string())
    }
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

    // 使用 tauri-plugin-opener 的原生能力
    use tauri_plugin_opener::OpenerExt;
    app_handle
        .opener()
        .open_path(clean_path, Option::<String>::None)
        .map_err(|e| e.to_string())
}

/// 清理上传缓存目录 (通常在启动时执行，清除上次闪退留下的僵尸文件)
pub fn clear_upload_cache(app_handle: &AppHandle) {
    if let Ok(mut temp_path) = app_handle.path().app_cache_dir() {
        temp_path.push("uploads");
        if temp_path.exists() {
            let _ = fs::remove_dir_all(&temp_path);
            let _ = fs::create_dir_all(&temp_path);
            println!("[FileManager] Upload cache cleared.");
        }
    }
}
