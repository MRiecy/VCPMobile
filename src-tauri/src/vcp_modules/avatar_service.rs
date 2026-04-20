use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_service::{SyncCommand, SyncState};
use crate::vcp_modules::sync_types::SyncDataType;
use image::{imageops::FilterType, GenericImageView};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, Runtime};

/// Tauri IPC Command: 保存头像二进制数据到数据库
/// 前端裁剪后将 Blob/ArrayBuffer 传给 Rust
#[tauri::command]
pub async fn save_avatar_data<R: Runtime>(
    app_handle: AppHandle<R>,
    owner_type: String,
    owner_id: String,
    mime_type: String,
    image_data: Vec<u8>,
) -> Result<String, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    // 1. 计算 SHA-256 哈希作为唯一标识
    let mut hasher = Sha256::new();
    hasher.update(&image_data);
    let avatar_hash = format!("{:x}", hasher.finalize());

    // 2. 预计算主色调 (Dominant Color)
    // 直接从内存中的二进制数据提取，避免写入临时文件
    let dominant_color = extract_dominant_color_from_bytes(&image_data).ok();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    // 3. 写入 avatars 表 (原子化 Upsert)
    sqlx::query(
        "INSERT INTO avatars (owner_type, owner_id, avatar_hash, mime_type, image_data, dominant_color, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(owner_type, owner_id) DO UPDATE SET
            avatar_hash = excluded.avatar_hash,
            mime_type = excluded.mime_type,
            image_data = excluded.image_data,
            dominant_color = excluded.dominant_color,
            updated_at = excluded.updated_at"
    )
    .bind(&owner_type)
    .bind(&owner_id)
    .bind(&avatar_hash)
    .bind(&mime_type)
    .bind(&image_data)
    .bind(&dominant_color)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    log::info!(
        "[AvatarService] Saved avatar for {} {}: hash={}, color={:?}",
        owner_type,
        owner_id,
        avatar_hash,
        dominant_color
    );

    // 4. 通知同步中心：本地数据已变动
    if let Some(sync_state) = app_handle.try_state::<SyncState>() {
        let _ = sync_state.ws_sender.send(SyncCommand::NotifyLocalChange {
            id: format!("{}:{}", owner_type, owner_id),
            data_type: SyncDataType::Avatar,
            hash: avatar_hash.clone(),
            ts: now,
        });
    }

    Ok(avatar_hash)
}

#[derive(serde::Serialize)]
pub struct AvatarResult {
    pub mime_type: String,
    pub image_data: Vec<u8>,
    pub dominant_color: Option<String>,
    pub updated_at: i64,
}

/// Tauri IPC Command: 获取头像二进制数据
#[tauri::command]
pub async fn get_avatar<R: Runtime>(
    app_handle: AppHandle<R>,
    owner_type: String,
    owner_id: String,
) -> Result<Option<AvatarResult>, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let row_res = sqlx::query(
        "SELECT mime_type, image_data, dominant_color, updated_at FROM avatars WHERE owner_type = ? AND owner_id = ?"
    )
    .bind(&owner_type)
    .bind(&owner_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;

    if let Some(row) = row_res {
        use sqlx::Row;
        Ok(Some(AvatarResult {
            mime_type: row.get("mime_type"),
            image_data: row.get("image_data"),
            dominant_color: row.get("dominant_color"),
            updated_at: row.get("updated_at"),
        }))
    } else {
        Ok(None)
    }
}

/// 从字节数组中提取主色调 (公开供协议层兜底使用)
pub fn extract_dominant_color_from_bytes(data: &[u8]) -> Result<String, String> {
    let img = image::load_from_memory(data)
        .map_err(|e| format!("Failed to load image from memory: {}", e))?;

    // 缩小到 50x50 进行快速采样
    let thumbnail = img.resize_exact(50, 50, FilterType::Nearest);

    let mut r_total: u64 = 0;
    let mut g_total: u64 = 0;
    let mut b_total: u64 = 0;
    let mut count: u64 = 0;

    for (_x, _y, pixel) in thumbnail.pixels() {
        if pixel[3] == 0 {
            continue;
        } // 跳过透明像素
        r_total += pixel[0] as u64;
        g_total += pixel[1] as u64;
        b_total += pixel[2] as u64;
        count += 1;
    }

    if count == 0 {
        return Ok("#808080".to_string());
    }

    let r = (r_total / count) as u8;
    let g = (g_total / count) as u8;
    let b = (b_total / count) as u8;

    Ok(format!("#{:02x}{:02x}{:02x}", r, g, b))
}
