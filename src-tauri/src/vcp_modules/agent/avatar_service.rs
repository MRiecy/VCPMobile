use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_service::{SyncCommand, SyncState};
use crate::vcp_modules::sync_types::SyncDataType;
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
    // spawn_blocking 隔离 CPU 密集型图片解码与直方图计算
    let image_data_for_color = image_data.clone();
    let dominant_color = tokio::task::spawn_blocking(move || {
        extract_dominant_color_from_bytes(&image_data_for_color).ok()
    })
    .await
    .ok()
    .flatten();

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

/// Tauri IPC Command: 为已有头像计算并存储 dominant_color（处理存量数据）
#[tauri::command]
pub async fn compute_and_store_dominant_color(
    db_state: tauri::State<'_, DbState>,
    owner_type: String,
    owner_id: String,
) -> Result<String, String> {
    let pool = &db_state.pool;

    let row = sqlx::query("SELECT image_data FROM avatars WHERE owner_type = ? AND owner_id = ?")
        .bind(&owner_type)
        .bind(&owner_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;

    let image_data: Vec<u8> = match row {
        Some(r) => {
            use sqlx::Row;
            r.get("image_data")
        }
        None => return Err("Avatar not found".to_string()),
    };

    let color = extract_dominant_color_from_bytes(&image_data)?;

    sqlx::query("UPDATE avatars SET dominant_color = ? WHERE owner_type = ? AND owner_id = ?")
        .bind(&color)
        .bind(&owner_type)
        .bind(&owner_id)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;

    log::info!(
        "[AvatarService] Computed and stored dominant_color for {} {}: {}",
        owner_type,
        owner_id,
        color
    );

    Ok(color)
}

fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let r = r / 255.0;
    let g = g / 255.0;
    let b = b / 255.0;
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let df = mx - mn;

    let h = if df == 0.0 {
        0.0
    } else if mx == r {
        (60.0 * ((g - b) / df) + 360.0) % 360.0
    } else if mx == g {
        (60.0 * ((b - r) / df) + 120.0) % 360.0
    } else {
        (60.0 * ((r - g) / df) + 240.0) % 360.0
    };

    let s = if mx == 0.0 { 0.0 } else { df / mx };
    let v = mx;

    (h, s, v)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;

    let (r, g, b) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };

    (
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8,
    )
}

/// 从字节数组中提取主色调 (公开供协议层兜底使用)
/// 策略：自适应分辨率（FFmpeg内存解码降采样，大图限128x128，小图保持原样）+ 512-bin量化直方图峰值检测 + HSV饱和过滤
pub fn extract_dominant_color_from_bytes(data: &[u8]) -> Result<String, String> {
    // 1. 调用 FFmpeg 自适应降采样得到 Raw RGBA 字节流
    let rgba_data = crate::vcp_modules::media_processor::ffmpeg_cli::decode_avatar_to_rgba(data)
        .map_err(|e| format!("FFmpeg decode failed: {}", e))?;

    if rgba_data.is_empty() || rgba_data.len() % 4 != 0 {
        return Ok("#808080".to_string());
    }

    // 512-bin 直方图：每通道 3bit，bin 范围 0-7
    let mut histogram = [0u32; 512];
    let mut r_sums = [0u64; 512];
    let mut g_sums = [0u64; 512];
    let mut b_sums = [0u64; 512];

    // 兜底累加器：全局算术平均（用于所有 bin 都被过滤的极端回退）
    let mut r_total: u64 = 0;
    let mut g_total: u64 = 0;
    let mut b_total: u64 = 0;
    let mut total_count: u64 = 0;

    for i in (0..rgba_data.len()).step_by(4) {
        let r = rgba_data[i];
        let g = rgba_data[i + 1];
        let b = rgba_data[i + 2];
        let a = rgba_data[i + 3];

        if a == 0 {
            continue; // 跳过透明像素
        }

        // 全局累加（兜底回退用）
        r_total += r as u64;
        g_total += g as u64;
        b_total += b as u64;
        total_count += 1;

        // 直方图分 bin
        let bin = ((r / 32) as usize) * 64 + ((g / 32) as usize) * 8 + (b / 32) as usize;
        histogram[bin] += 1;
        r_sums[bin] += r as u64;
        g_sums[bin] += g as u64;
        b_sums[bin] += b as u64;
    }

    if total_count == 0 {
        return Ok("#808080".to_string());
    }

    // 找最高频的非背景 bin
    let mut best_bin: Option<usize> = None;
    let mut best_count = 0u32;

    for (bin, count) in histogram.iter().enumerate() {
        if *count == 0 {
            continue;
        }

        // 排除纯黑 bin (0,0,0)
        if bin == 0 {
            continue;
        }
        // 排除纯白 bin (7,7,7)
        if bin == 511 {
            continue;
        }

        // 排除近灰 bin：r_bin/g_bin/b_bin 接近说明是灰度系
        let r_bin = (bin / 64) as i16;
        let g_bin = ((bin % 64) / 8) as i16;
        let b_bin = (bin % 8) as i16;
        if (r_bin - g_bin).abs() <= 1 && (g_bin - b_bin).abs() <= 1 {
            continue;
        }

        if *count > best_count {
            best_count = *count;
            best_bin = Some(bin);
        }
    }

    // 计算最终颜色
    let (r, g, b) = if let Some(bin) = best_bin {
        // 在最佳 bin 内做 HSV 饱和过滤 + 亮度压制
        let mut vr_total: u64 = 0;
        let mut vg_total: u64 = 0;
        let mut vb_total: u64 = 0;
        let mut v_count: u64 = 0;

        for i in (0..rgba_data.len()).step_by(4) {
            let r = rgba_data[i];
            let g = rgba_data[i + 1];
            let b = rgba_data[i + 2];
            let a = rgba_data[i + 3];

            if a == 0 {
                continue;
            }

            let pixel_bin = ((r / 32) as usize) * 64 + ((g / 32) as usize) * 8 + (b / 32) as usize;
            if pixel_bin != bin {
                continue;
            }

            let (h, s, v) = rgb_to_hsv(r as f32, g as f32, b as f32);

            // 过滤低饱和和过亮像素
            if s < 0.15 || v > 0.88 {
                continue;
            }

            // 亮度压制 -5%，饱和度提升 +15%
            let v = (v * 0.95).min(1.0);
            let s = (s * 1.15).min(1.0);

            let (nr, ng, nb) = hsv_to_rgb(h, s, v);
            vr_total += nr as u64;
            vg_total += ng as u64;
            vb_total += nb as u64;
            v_count += 1;
        }

        if let Some(vr) = vr_total.checked_div(v_count) {
            (
                vr as u8,
                vg_total.checked_div(v_count).unwrap_or(0) as u8,
                vb_total.checked_div(v_count).unwrap_or(0) as u8,
            )
        } else {
            // bin 内全被过滤，回退到该 bin 原始平均
            let count = histogram[bin] as u64;
            (
                (r_sums[bin] / count) as u8,
                (g_sums[bin] / count) as u8,
                (b_sums[bin] / count) as u8,
            )
        }
    } else {
        // 回退到全局算术平均
        (
            (r_total / total_count) as u8,
            (g_total / total_count) as u8,
            (b_total / total_count) as u8,
        )
    };

    Ok(format!("#{:02x}{:02x}{:02x}", r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rgb_to_hsv_pure_colors() {
        // 纯红
        let (h, s, v) = rgb_to_hsv(255.0, 0.0, 0.0);
        assert!((h - 0.0).abs() < 1e-4);
        assert!((s - 1.0).abs() < 1e-4);
        assert!((v - 1.0).abs() < 1e-4);

        // 纯绿
        let (h, s, v) = rgb_to_hsv(0.0, 255.0, 0.0);
        assert!((h - 120.0).abs() < 1e-4);
        assert!((s - 1.0).abs() < 1e-4);
        assert!((v - 1.0).abs() < 1e-4);

        // 纯蓝
        let (h, s, v) = rgb_to_hsv(0.0, 0.0, 255.0);
        assert!((h - 240.0).abs() < 1e-4);
        assert!((s - 1.0).abs() < 1e-4);
        assert!((v - 1.0).abs() < 1e-4);
    }

    #[test]
    fn test_hsv_to_rgb_pure_colors() {
        // 纯红
        let (r, g, b) = hsv_to_rgb(0.0, 1.0, 1.0);
        assert_eq!(r, 255);
        assert_eq!(g, 0);
        assert_eq!(b, 0);

        // 纯绿
        let (r, g, b) = hsv_to_rgb(120.0, 1.0, 1.0);
        assert_eq!(r, 0);
        assert_eq!(g, 255);
        assert_eq!(b, 0);

        // 纯蓝
        let (r, g, b) = hsv_to_rgb(240.0, 1.0, 1.0);
        assert_eq!(r, 0);
        assert_eq!(g, 0);
        assert_eq!(b, 255);
    }

    #[test]
    fn test_rgb_hsv_roundtrip() {
        let colors = vec![
            (255.0, 255.0, 255.0), // 纯白
            (0.0, 0.0, 0.0),       // 纯黑
            (128.0, 128.0, 128.0), // 灰色
            (100.0, 150.0, 200.0), // 任意颜色
        ];

        for (r_in, g_in, b_in) in colors {
            let (h, s, v) = rgb_to_hsv(r_in, g_in, b_in);
            let (r_out, g_out, b_out) = hsv_to_rgb(h, s, v);

            // 允许有少量舍入误差
            assert!((r_in - r_out as f32).abs() <= 1.0);
            assert!((g_in - g_out as f32).abs() <= 1.0);
            assert!((b_in - b_out as f32).abs() <= 1.0);
        }
    }
}
