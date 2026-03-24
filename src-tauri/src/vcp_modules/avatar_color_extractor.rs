use image::{imageops::FilterType, GenericImageView};
use std::path::Path;

/// 提取图片的主色调（平均色）
/// 采用缩小图片到 50x50 后计算平均值的快速算法，避免移动端 OOM
pub fn extract_dominant_color(path: &str) -> Result<String, String> {
    // 尝试打开图片
    let img = image::open(Path::new(path))
        .map_err(|e| format!("Failed to open image at {}: {}", path, e))?;

    // 缩小到 50x50 以极大地加快处理速度，使用 Nearest 滤镜追求极致性能
    let thumbnail = img.resize_exact(50, 50, FilterType::Nearest);

    let mut r_total: u64 = 0;
    let mut g_total: u64 = 0;
    let mut b_total: u64 = 0;
    let mut count: u64 = 0;

    for (_x, _y, pixel) in thumbnail.pixels() {
        // 忽略完全透明的像素
        if pixel[3] == 0 {
            continue;
        }
        r_total += pixel[0] as u64;
        g_total += pixel[1] as u64;
        b_total += pixel[2] as u64;
        count += 1;
    }

    // 如果图片完全透明或没有有效像素，返回默认的灰色
    if count == 0 {
        return Ok("#808080".to_string());
    }

    let r = (r_total / count) as u8;
    let g = (g_total / count) as u8;
    let b = (b_total / count) as u8;

    // 格式化为 Hex 颜色字符串
    Ok(format!("#{:02x}{:02x}{:02x}", r, g, b))
}

/// Tauri IPC Command: 供前端调用提取头像颜色
/// 注意：前端传入的 path 必须是手机上的绝对物理路径，而不是 asset:// 协议路径
#[tauri::command]
pub async fn extract_avatar_color(path: String) -> Result<String, String> {
    // 在后台线程执行 CPU 密集型任务，防止阻塞 Tauri 主线程
    tauri::async_runtime::spawn_blocking(move || extract_dominant_color(&path))
        .await
        .map_err(|e| format!("Thread spawn error: {}", e))?
}
