// maintenance_manager.rs - 负责系统维护、垃圾回收与缓存清理的核心模块
// 职责: 聚合所有低频但关键的系统维护任务，对齐前端 MaintenanceSection 领域。

use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::file_manager::{get_attachments_root_dir, get_thumbnails_root_dir};
use crate::vcp_modules::settings_manager::{read_settings, update_settings, SettingsState};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, State};

/// 1. 清理 WebView 缓存
/// 调用 Tauri v2 原生接口清除浏览数据 (HTTP Cache, Images, etc.)
#[tauri::command]
pub async fn clear_webview_cache(app: AppHandle) -> Result<String, String> {
    if let Some(webview) = app.get_webview_window("main") {
        webview
            .clear_all_browsing_data()
            .map_err(|e| format!("WebView 缓存清理失败: {}", e))?;
        Ok("WebView 缓存已成功清理".to_string())
    } else {
        Err("未找到主窗口，无法执行清理".to_string())
    }
}

/// 2. 清理孤儿附件 (从 file_manager.rs 迁移)
/// 深度扫描并删除未被引用的孤立附件与缩略图
#[tauri::command]
pub async fn cleanup_orphaned_attachments(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
) -> Result<String, String> {
    let attachments_dir = get_attachments_root_dir(&app_handle)?;

    if !attachments_dir.exists() {
        return Ok("没有附件需要清理".to_string());
    }

    // 1. 获取数据库中记录的所有哈希
    let all_indexed_hashes: Vec<(String, String)> =
        sqlx::query_as("SELECT hash, internal_path FROM attachments")
            .fetch_all(&db_state.pool)
            .await
            .map_err(|e| e.to_string())?;

    if all_indexed_hashes.is_empty() {
        return Ok("索引库为空，无需清理".to_string());
    }

    // 1.5 先清理已删除消息的 message_attachments 关联 (防线三：GC 阶段关联自愈)
    let _ = sqlx::query(
        "DELETE FROM message_attachments WHERE (topic_id, msg_id) IN (\
         SELECT ma.topic_id, ma.msg_id FROM message_attachments ma \
         INNER JOIN messages m ON ma.topic_id = m.topic_id AND ma.msg_id = m.msg_id \
         WHERE m.deleted_at IS NOT NULL)",
    )
    .execute(&db_state.pool)
    .await;

    // 2. 查 message_attachments 确定哪些 hash 正在被有效消息引用 (防线四：GC 强校验)
    let used_hashes: std::collections::HashSet<String> =
        sqlx::query_as::<_, (String,)>(
            "SELECT DISTINCT ma.hash FROM message_attachments ma \
             INNER JOIN messages m ON ma.topic_id = m.topic_id AND ma.msg_id = m.msg_id \
             WHERE m.deleted_at IS NULL"
        )
        .fetch_all(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|(h,)| h)
        .collect();

    // 3. 找出未引用的哈希并删除
    let mut deleted_count = 0;
    let mut freed_size = 0u64;

    for (hash, local_path) in all_indexed_hashes {
        if !used_hashes.contains(&hash) {
            let path = std::path::Path::new(&local_path);
            if path.exists() {
                if let Ok(meta) = tokio::fs::metadata(path).await {
                    freed_size += meta.len();
                }
                let _ = tokio::fs::remove_file(path).await;

                // 同时删除可能的缩略图
                let thumb_path = match get_thumbnails_root_dir(&app_handle) {
                    Ok(p) => p.join(format!("{}_thumb.webp", hash)),
                    Err(_) => path
                        .parent()
                        .unwrap()
                        .join("thumbnails")
                        .join(format!("{}_thumb.webp", hash)),
                };
                if thumb_path.exists() {
                    let _ = tokio::fs::remove_file(thumb_path).await;
                }

                deleted_count += 1;
            }

            // 从数据库中移除
            let _ = sqlx::query("DELETE FROM attachments WHERE hash = ?")
                .bind(&hash)
                .execute(&db_state.pool)
                .await;
        }
    }

    Ok(format!(
        "清理完成：删除了 {} 个孤儿附件，释放了 {:.2} MB 空间",
        deleted_count,
        (freed_size as f64) / 1024.0 / 1024.0
    ))
}

/// 2.5 精准清理单个孤儿附件 (供前端取消暂存时调用)
/// 检查特定 hash 是否被引用，若未引用则物理删除以防脏数据
#[tauri::command]
pub async fn cleanup_single_orphaned_attachment(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    hash: String,
) -> Result<String, String> {
    // 1. 查 message_attachments 确定该 hash 是否被有效历史消息引用
    let is_used: bool = sqlx::query_scalar::<_, i32>(
        "SELECT EXISTS(\
         SELECT 1 FROM message_attachments ma \
         INNER JOIN messages m ON ma.topic_id = m.topic_id AND ma.msg_id = m.msg_id \
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

    // 2. 获取记录的物理路径与创建时间
    let row: Option<(String, i64)> =
        sqlx::query_as("SELECT internal_path, created_at FROM attachments WHERE hash = ?")
            .bind(&hash)
            .fetch_optional(&db_state.pool)
            .await
            .map_err(|e| e.to_string())?;

    if let Some((path_str, created_at)) = row {
        // ⚡ 5分钟安全锁宽限期 (300秒)
        // 弹性判定：由于 created_at 部分来自秒，部分来自毫秒，统一换算为秒级进行对比
        let created_secs = if created_at > 10_000_000_000 {
            created_at / 1000 // 毫秒级转换为秒
        } else {
            created_at // 秒级
        };
        
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        if now_secs - created_secs < 300 {
            println!(
                "[Maintenance] Safe-Shield: Hash {} was created recently ({} secs ago). Skipping targeted GC to prevent race condition.",
                hash,
                now_secs - created_secs
            );
            return Ok("附件创建不久，处于安全锁宽限期，跳过物理清理以防时序冲突".to_string());
        }

        let path = std::path::Path::new(&path_str);
        if path.exists() {
            let _ = tokio::fs::remove_file(path).await;
        }

        // 删除可能的缩略图
        let thumb_path = match get_thumbnails_root_dir(&app_handle) {
            Ok(p) => p.join(format!("{}_thumb.webp", hash)),
            Err(_) => path
                .parent()
                .unwrap()
                .join("thumbnails")
                .join(format!("{}_thumb.webp", hash)),
        };
        if thumb_path.exists() {
            let _ = tokio::fs::remove_file(thumb_path).await;
        }

        // 从数据库中移除
        let _ = sqlx::query("DELETE FROM attachments WHERE hash = ?")
            .bind(&hash)
            .execute(&db_state.pool)
            .await;

        Ok("成功清理未引用的暂存附件".to_string())
    } else {
        Ok("数据库中未找到该附件记录".to_string())
    }
}

/// 3. 初始化自动维护逻辑 (在 App 启动时调用)
///    如果距离上次清理超过 3 天，则自动触发一次 WebView 缓存清理
pub async fn init_automatic_maintenance(app: AppHandle) {
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

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let three_days_secs = 3 * 24 * 60 * 60;

    if now - last_clear > three_days_secs {
        println!("[Maintenance] Triggering scheduled maintenance (WebView & SQLite)...");

        // 1. WebView 清理
        if let Some(webview) = app.get_webview_window("main") {
            let _ = webview.clear_all_browsing_data();
        }

        // 2. SQLite 物理空间回收 (增量 Vacuum)
        // 每次清理 100 个 Page，避免单次清理导致长时间 IO 阻塞
        let db_state = app.state::<DbState>();
        let _ = sqlx::query("PRAGMA incremental_vacuum(100)")
            .execute(&db_state.pool)
            .await;

        // 3. 自动清除已删除消息的多余附件关联 (防线二：自动维护自愈)
        let _ = sqlx::query(
            "DELETE FROM message_attachments WHERE (topic_id, msg_id) IN (\
             SELECT ma.topic_id, ma.msg_id FROM message_attachments ma \
             INNER JOIN messages m ON ma.topic_id = m.topic_id AND ma.msg_id = m.msg_id \
             WHERE m.deleted_at IS NOT NULL)",
        )
        .execute(&db_state.pool)
        .await;

        // 4. SQLite 查询规划器优化
        let _ = sqlx::query("PRAGMA optimize").execute(&db_state.pool).await;

        // 更新时间戳
        let updates = serde_json::json!({
            "lastWebviewCacheClear": now
        });
        let _ = update_settings(app.clone(), settings_state, updates).await;
        println!("[Maintenance] Scheduled maintenance complete.");
    }
}


