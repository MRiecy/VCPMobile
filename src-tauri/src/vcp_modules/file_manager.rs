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
    pub extracted_text: Option<String>,
    pub thumbnail_path: Option<String>,
}

/// 内部辅助函数：生成图片缩略图
fn generate_thumbnail(original_path: &std::path::Path, hash: &str) -> Option<String> {
    let mut thumb_path = original_path.parent()?.to_path_buf();
    thumb_path.push("thumbnails");
    
    if !thumb_path.exists() {
        let _ = fs::create_dir_all(&thumb_path);
    }
    
    let thumb_file_path = thumb_path.join(format!("{}_thumb.webp", hash));
    
    // 如果缩略图已存在，直接返回
    if thumb_file_path.exists() {
        return Some(format!("file://{}", thumb_file_path.to_string_lossy()));
    }

    // 生成缩略图 (限制在 200px 左右)
    if let Ok(img) = image::open(original_path) {
        let thumbnail = img.thumbnail(200, 200);
        if thumbnail.save(&thumb_file_path).is_ok() {
            return Some(format!("file://{}", thumb_file_path.to_string_lossy()));
        }
    }
    None
}

/// 内部辅助函数：校验路径安全性，防止路径遍历攻击
fn ensure_safe_path(app_handle: &AppHandle, path: &std::path::Path) -> Result<(), String> {
    let config_dir = app_handle.path().app_config_dir().map_err(|e| e.to_string())?;
    // 允许访问 App 配置目录及其子目录
    if path.starts_with(&config_dir) {
        Ok(())
    } else {
        Err("非法路径访问：禁止访问应用数据目录以外的文件".to_string())
    }
}

/// 内部辅助函数：获取当前平台下的真实路径 (用于历史记录自动纠错)
pub fn resolve_attachment_path(
    app_handle: &AppHandle,
    hash: &str,
    original_name: &str,
) -> Option<String> {
    let mut attachments_dir = app_handle.path().app_config_dir().ok()?;
    attachments_dir.push("data");
    attachments_dir.push("attachments");

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

/// 内部辅助函数：根据 MIME 类型或扩展名提取文本内容
fn try_extract_text(path: &std::path::Path, mime_type: &str) -> Option<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    // 对齐桌面端：支持常见文本和代码格式
    if mime_type.starts_with("text/")
        || mime_type == "application/json"
        || mime_type == "application/javascript"
        || mime_type == "application/x-javascript"
        || matches!(
            ext.as_str(),
            "md" | "txt"
                | "json"
                | "js"
                | "ts"
                | "rs"
                | "py"
                | "c"
                | "cpp"
                | "h"
                | "hpp"
                | "css"
                | "html"
        )
    {
        return fs::read_to_string(path).ok();
    }
    None
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
    // 0. OOM 防御：限制 store_file 只能处理 20MB 以下的文件
    if file_bytes.len() > 20 * 1024 * 1024 {
        return Err("文件过大，请使用文件选取器 (pick_and_store_attachment) 以流式上传 (Limit: 20MB)".to_string());
    }

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

    // 提取文本内容 (如果适用)
    let extracted_text = try_extract_text(&internal_file_path, &mime_type);
    
    // 生成缩略图 (如果适用)
    let thumbnail_path = if mime_type.starts_with("image/") {
        generate_thumbnail(&internal_file_path, &hash)
    } else {
        None
    };

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
        extracted_text,
        thumbnail_path,
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
    let mut buffer = [0u8; 128 * 1024]; // 提升至 128KB 以减少大文件的系统调用开销

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

    // 提取文本内容 (如果适用)
    let extracted_text = try_extract_text(&internal_file_path, &mime_type);

    // 生成缩略图 (如果适用)
    let thumbnail_path = if mime_type.starts_with("image/") {
        generate_thumbnail(&internal_file_path, &hash)
    } else {
        None
    };

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
        extracted_text,
        thumbnail_path,
    }))
}

/// 根据附件哈希获取当前平台的物理路径 (用于路径重定心)
#[tauri::command]
pub async fn get_attachment_real_path(
    app_handle: AppHandle,
    hash: String,
    original_name: String,
) -> Result<String, String> {
    let mut attachments_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    attachments_dir.push("data");
    attachments_dir.push("attachments");

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

/// 唤起系统默认应用打开文件
#[tauri::command]
pub async fn open_file(app_handle: AppHandle, path: String) -> Result<(), String> {
    let clean_path = path.replace("file://", "");
    let path_buf = std::path::PathBuf::from(&clean_path);
    
    // 安全校验：禁止打开系统敏感路径
    if let Err(e) = ensure_safe_path(&app_handle, &path_buf) {
        return Err(e);
    }

    // 使用 tauri-plugin-opener 的原生能力
    use tauri_plugin_opener::OpenerExt;
    app_handle.opener().open_path(clean_path, Option::<String>::None).map_err(|e| e.to_string())
}

/// 读取本地文件并转换为 Base64 字符串 (用于多模态 Payload)
#[tauri::command]
pub async fn read_local_file_base64(app_handle: AppHandle, path: String) -> Result<String, String> {
    let clean_path = path.replace("file://", "");
    let path_buf = std::path::PathBuf::from(&clean_path);

    if !path_buf.exists() {
        return Err(format!("File not found: {}", clean_path));
    }

    // 安全校验
    if let Err(e) = ensure_safe_path(&app_handle, &path_buf) {
        return Err(e);
    }

    // OOM 防御：禁止读取超过 50MB 的文件到内存进行 Base64 转换
    let metadata = std::fs::metadata(&path_buf).map_err(|e| e.to_string())?;
    if metadata.len() > 50 * 1024 * 1024 {
        return Err("文件过大，无法进行多模态转换 (Limit: 50MB)".to_string());
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
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        _ => "application/octet-stream", // Fallback
    };

    Ok(format!("data:{};base64,{}", mime_type, base64_str))
}

/// 清理孤儿附件 (无任何历史记录引用的文件)
/// 算法：扫描所有 history.json，收集所有 hash，对比索引并删除未引用的物理文件
#[tauri::command]
pub async fn cleanup_orphaned_attachments(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
) -> Result<String, String> {
    let mut attachments_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    let config_root = attachments_dir.clone();
    attachments_dir.push("data");
    attachments_dir.push("attachments");

    if !attachments_dir.exists() {
        return Ok("没有附件需要清理".to_string());
    }

    // 1. 获取影子数据库中记录的所有哈希
    let all_indexed_hashes: Vec<(String, String)> =
        sqlx::query_as("SELECT hash, local_path FROM attachment_index")
            .fetch_all(&db_state.pool)
            .await
            .map_err(|e| e.to_string())?;

    if all_indexed_hashes.is_empty() {
        return Ok("索引库为空，无需清理".to_string());
    }

    // 2. 深度扫描 topics 目录下的所有 history.json 提取正在使用的 hash
    let mut topics_dir = config_root.clone();
    topics_dir.push("UserData"); // 适配桌面端 UserData 目录
    
    let mut used_hashes = std::collections::HashSet::new();
    
    // 使用简单的递归目录遍历
    fn scan_dir(dir: &std::path::Path, used: &mut std::collections::HashSet<String>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    scan_dir(&path, used);
                } else if path.file_name().map_or(false, |n| n == "history.json") {
                    if let Ok(content) = fs::read_to_string(&path) {
                        // 使用正则提取所有 "hash": "..." 字段，避免昂贵的 JSON 反序列化
                        let re = regex::Regex::new(r#""hash"\s*:\s*"([a-f0-9]{64})""#).unwrap();
                        for cap in re.captures_iter(&content) {
                            used.insert(cap[1].to_string());
                        }
                    }
                }
            }
        }
    }
    
    scan_dir(&topics_dir, &mut used_hashes);

    // 3. 找出未引用的哈希并删除
    let mut deleted_count = 0;
    let mut freed_size = 0u64;

    for (hash, local_path) in all_indexed_hashes {
        if !used_hashes.contains(&hash) {
            let path = std::path::Path::new(&local_path);
            if path.exists() {
                if let Ok(meta) = fs::metadata(path) {
                    freed_size += meta.len();
                }
                let _ = fs::remove_file(path);
                
                // 同时删除可能的缩略图
                if let Some(parent) = path.parent() {
                    let thumb_path = parent.join("thumbnails").join(format!("{}_thumb.webp", hash));
                    if thumb_path.exists() {
                        let _ = fs::remove_file(thumb_path);
                    }
                }
                
                deleted_count += 1;
            }
            
            // 从索引库中移除
            let _ = sqlx::query("DELETE FROM attachment_index WHERE hash = ?")
                .bind(&hash)
                .execute(&db_state.pool)
                .await;
        }
    }

    Ok(format!("清理完成：删除了 {} 个孤儿附件，释放了 {:.2} MB 空间", 
        deleted_count, (freed_size as f64) / 1024.0 / 1024.0))
}
