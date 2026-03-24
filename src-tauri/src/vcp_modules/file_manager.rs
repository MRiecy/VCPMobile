use crate::vcp_modules::db_manager::DbState;
use base64::{engine::general_purpose, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

/// 附件元数据结构
/// 对齐 @/plans/Rust文件数据管理重构详细规划.md 中的 2.1 节
#[derive(Debug, Serialize, Deserialize)]
pub struct AttachmentData {
    pub id: String,
    pub name: String,
    pub internal_file_name: String,
    pub internal_path: String,
    pub mime_type: String,
    pub size: u64,
    pub hash: String,
    pub created_at: u64,
}

/// 存储文件到中心化附件目录 (内容寻址存储)
/// 这个方法依然保留，用于接收前端传来的小文件（如录音或直接剪贴板的图片）
#[tauri::command]
pub async fn store_file(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    original_name: String,
    file_bytes: Vec<u8>,
    mime_type: String,
) -> Result<AttachmentData, String> {
    // 1. 计算 SHA256 哈希值以确保唯一性
    let mut hasher = Sha256::new();
    hasher.update(&file_bytes);
    let hash = hex::encode(hasher.finalize());

    // 2. 准备内部文件名和路径
    let file_extension = std::path::Path::new(&original_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let internal_file_name = if file_extension.is_empty() {
        hash.clone()
    } else {
        format!("{}.{}", hash, file_extension)
    };

    let mut attachments_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    attachments_dir.push("data");
    attachments_dir.push("attachments");

    // 确保附件目录存在
    if !attachments_dir.exists() {
        fs::create_dir_all(&attachments_dir).map_err(|e| e.to_string())?;
    }

    let internal_file_path = attachments_dir.join(&internal_file_name);
    let internal_path_str = internal_file_path.to_str().unwrap().to_string();

    // 3. 检查影子数据库中是否已存在该哈希，或磁盘上是否已存在文件
    let existing: Option<(String,)> =
        sqlx::query_as("SELECT hash FROM attachment_index WHERE hash = ?")
            .bind(&hash)
            .fetch_optional(&db_state.pool)
            .await
            .map_err(|e| e.to_string())?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    if existing.is_none() || !internal_file_path.exists() {
        // 4. 写入物理文件
        fs::write(&internal_file_path, &file_bytes).map_err(|e| e.to_string())?;

        // 5. 更新影子数据库索引 (attachment_index)
        sqlx::query(
            "INSERT INTO attachment_index (hash, local_path, mime_type, size, created_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(hash) DO UPDATE SET local_path = excluded.local_path",
        )
        .bind(&hash)
        .bind(&internal_path_str)
        .bind(&mime_type)
        .bind(file_bytes.len() as i64)
        .bind(now as i64)
        .execute(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?;
    }

    // 6. 构造返回给前端的数据对象
    Ok(AttachmentData {
        id: format!("attachment_{}", hash),
        name: original_name,
        internal_file_name,
        internal_path: format!("file://{}", internal_path_str),
        mime_type,
        size: file_bytes.len() as u64,
        hash,
        created_at: now,
    })
}

/// 移动端/桌面端原生文件选取与存储 (流式防 OOM 优化版)
/// 触发原生选择器，通过分块读取计算哈希并拷贝文件，避免将整个大文件加载到内存
#[tauri::command]
pub async fn pick_and_store_attachment(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
) -> Result<Option<AttachmentData>, String> {
    // 1. 唤起原生文件选择器
    let (tx, rx) = tokio::sync::oneshot::channel();
    app_handle.dialog().file().pick_file(move |p| {
        let _ = tx.send(p);
    });

    let file_path = match rx.await.map_err(|e| e.to_string())? {
        Some(path) => path,
        None => return Ok(None), // 用户取消了选择
    };

    // 解析文件路径
    let path_buf = match file_path {
        tauri_plugin_dialog::FilePath::Path(p) => p,
        _ => return Err("暂不支持的文件路径类型".to_string()),
    };

    // 2. 提取文件名和推断 MIME 类型
    let original_name = path_buf
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown_file".to_string());

    let extension = path_buf
        .extension()
        .map(|e| e.to_string_lossy().to_string().to_lowercase())
        .unwrap_or_default();

    let mime_type = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "doc" | "docx" => "application/msword",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "mkv" => "video/x-matroska",
        "zip" => "application/zip",
        "rar" => "application/x-rar-compressed",
        "7z" => "application/x-7z-compressed",
        _ => "application/octet-stream",
    }
    .to_string();

    // 3. 获取文件大小并准备源文件流
    let file_size = fs::metadata(&path_buf)
        .map_err(|e| format!("无法获取文件信息: {}", e))?
        .len();

    let mut source_file =
        std::fs::File::open(&path_buf).map_err(|e| format!("无法打开源文件: {}", e))?;

    // 4. 流式计算 SHA256 哈希值 (防 OOM)
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192]; // 8KB buffer 读文件

    loop {
        use std::io::Read;
        let bytes_read = source_file
            .read(&mut buffer)
            .map_err(|e| format!("计算哈希失败: {}", e))?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    let hash = hex::encode(hasher.finalize());

    // 5. 准备内部文件名和目标路径
    let internal_file_name = if extension.is_empty() {
        hash.clone()
    } else {
        format!("{}.{}", hash, extension)
    };

    let mut attachments_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    attachments_dir.push("data");
    attachments_dir.push("attachments");

    if !attachments_dir.exists() {
        fs::create_dir_all(&attachments_dir).map_err(|e| e.to_string())?;
    }

    let internal_file_path = attachments_dir.join(&internal_file_name);
    let internal_path_str = internal_file_path.to_str().unwrap().to_string();

    // 6. 检查影子数据库中是否已存在该哈希，或磁盘上是否已存在文件
    let existing: Option<(String,)> =
        sqlx::query_as("SELECT hash FROM attachment_index WHERE hash = ?")
            .bind(&hash)
            .fetch_optional(&db_state.pool)
            .await
            .map_err(|e| e.to_string())?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    if existing.is_none() || !internal_file_path.exists() {
        // 如果文件不存在，进行拷贝。优先使用系统级 copy (底层高度优化)，回退使用流式复制
        if let Err(copy_err) = fs::copy(&path_buf, &internal_file_path) {
            eprintln!("[FileManager] 快速拷贝失败，回退为流式复制: {}", copy_err);

            // 重置源文件指针，准备流式复制
            use std::io::{Read, Seek, Write};
            source_file
                .seek(std::io::SeekFrom::Start(0))
                .map_err(|e| e.to_string())?;
            let mut target_file =
                std::fs::File::create(&internal_file_path).map_err(|e| e.to_string())?;

            loop {
                let bytes_read = source_file.read(&mut buffer).map_err(|e| e.to_string())?;
                if bytes_read == 0 {
                    break;
                }
                target_file
                    .write_all(&buffer[..bytes_read])
                    .map_err(|e| e.to_string())?;
            }
        }

        // 7. 更新影子数据库索引
        sqlx::query(
            "INSERT INTO attachment_index (hash, local_path, mime_type, size, created_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(hash) DO UPDATE SET local_path = excluded.local_path",
        )
        .bind(&hash)
        .bind(&internal_path_str)
        .bind(&mime_type)
        .bind(file_size as i64)
        .bind(now as i64)
        .execute(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?;
    }

    // 8. 返回前端数据
    Ok(Some(AttachmentData {
        id: format!("attachment_{}", hash),
        name: original_name,
        internal_file_name,
        internal_path: format!("file://{}", internal_path_str),
        mime_type,
        size: file_size,
        hash,
        created_at: now,
    }))
}

/// 读取本地图片并转换为 Base64 字符串 (绕过 WebView asset 协议限制)
#[tauri::command]
pub async fn read_local_image_base64(path: String) -> Result<String, String> {
    let clean_path = path.replace("file://", "");
    let path_buf = std::path::PathBuf::from(&clean_path);

    if !path_buf.exists() {
        return Err(format!("File not found: {}", clean_path));
    }

    let bytes = fs::read(&path_buf).map_err(|e| format!("Failed to read file: {}", e))?;
    let base64_str = general_purpose::STANDARD.encode(&bytes);

    let extension = path_buf
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let mime_type = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream", // Fallback
    };

    Ok(format!("data:{};base64,{}", mime_type, base64_str))
}
