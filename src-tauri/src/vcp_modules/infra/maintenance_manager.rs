// maintenance_manager.rs - 负责系统维护、垃圾回收与缓存清理的核心模块
// 职责: 聚合所有低频但关键的系统维护任务，对齐前端 MaintenanceSection 领域。

use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::file_manager::{
    delete_attachment_physical, get_attachments_root_dir, get_multimodal_cache_dir,
    get_thumbnails_root_dir,
};
use crate::vcp_modules::infra::utils::{is_valid_cas_hash, now_secs, YieldCounter};
use crate::vcp_modules::settings_manager::{read_settings, update_settings, SettingsState};
use tauri::{AppHandle, Manager, State};

/// 辅助函数：异步深度遍历计算目录大小（带协作式 CPU 挂起出让，每 200 个文件出让一次时间片）
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

/// 1. 清理 WebView 缓存 (Level 1)
/// 调用 Tauri v2 原生接口清除浏览数据 (HTTP Cache, Images, etc.)，并物理抹除磁盘 HTTP Cache。
/// 提示：此操作仅处理网络与媒体层静态资源，V8 code_cache 字节码由 Level 3 重建管理。
#[tauri::command]
pub async fn clear_webview_cache(app: AppHandle) -> Result<String, String> {
    let mut cleared_details = String::new();
    let mut freed_size = 0u64;

    // 1. 调用内置接口清除 WebView 的内存和浏览状态数据
    if let Some(webview) = app.get_webview_window("main") {
        webview
            .clear_all_browsing_data()
            .map_err(|e| format!("WebView 缓存清理失败: {}", e))?;
        cleared_details.push_str("标准浏览数据已清除；");
    } else {
        cleared_details.push_str("未找到主窗口，跳过标准清理；");
    }

    // 2. 物理清除 HTTP 缓存
    if let Ok(cache_dir) = app.path().app_cache_dir() {
        let http_cache_dir = cache_dir.join("WebView").join("Default").join("HTTP Cache");
        if http_cache_dir.exists() {
            // 在物理删除前先统计大小
            freed_size = calculate_dir_size(&http_cache_dir).await;

            if tokio::fs::remove_dir_all(&http_cache_dir).await.is_ok() {
                cleared_details.push_str("物理 HTTP Cache 已抹除；");
            } else {
                freed_size = 0;
                cleared_details.push_str("部分 HTTP 物理缓存被占用，已标记失效；");
            }
        }
    }

    let freed_size_mb = (freed_size as f64) / 1024.0 / 1024.0;
    Ok(format!(
        "WebView 缓存清理成功 ({})，释放空间: {:.2} MB",
        cleared_details.trim_end_matches('；'),
        freed_size_mb
    ))
}

/// 2. 清理孤儿附件 (Level 2)
/// 深度扫描并删除未被引用的孤立附件与缩略图。
/// 第一阶段：基于数据库比对，清除消息已被删除的孤立附件；
/// 第二阶段：物理磁盘双向校验扫盲，清除在库里完全无登记记录的无主“幽灵文件”及无主缩略图。
#[tauri::command]
pub async fn cleanup_orphaned_attachments(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
) -> Result<String, String> {
    let attachments_dir = get_attachments_root_dir(&app_handle)?;

    // ==========================================
    // 🌟 第一阶段：基于数据库与消息引用的孤儿清理 🌟
    // ==========================================
    let mut deleted_count = 0;
    let mut freed_size = 0u64;

    // 1.5 将已逻辑删除或所属消息已删除的 message_attachments 记录清空为无害的墓碑态 (清空敏感正文、src路径等)，但保留主键条目
    let _ = sqlx::query(
        "UPDATE message_attachments \
         SET display_name = '[附件已删除]', src = NULL, status = 'removed' \
         WHERE deleted_at IS NOT NULL \
            OR (topic_id, msg_id) IN (\
                SELECT topic_id, msg_id FROM messages WHERE deleted_at IS NOT NULL\
            )",
    )
    .execute(&db_state.pool)
    .await;

    // 2. 查 message_attachments 确定哪些 hash 正在被有效消息引用 (防线四：GC 强校验)
    let used_hashes: std::collections::HashSet<String> = sqlx::query_as::<_, (String,)>(
        "SELECT DISTINCT ma.hash FROM message_attachments ma \
             INNER JOIN messages m ON ma.topic_id = m.topic_id AND ma.msg_id = m.msg_id \
             WHERE m.deleted_at IS NULL AND ma.deleted_at IS NULL",
    )
    .fetch_all(&db_state.pool)
    .await
    .map_err(|e| e.to_string())?
    .into_iter()
    .map(|(h,)| h)
    .collect();

    // 3. 获取数据库中记录的所有附件哈希与物理路径
    let all_indexed_hashes: Vec<(String, String)> =
        sqlx::query_as("SELECT hash, internal_path FROM attachments")
            .fetch_all(&db_state.pool)
            .await
            .unwrap_or_default();

    for (hash, local_path) in all_indexed_hashes {
        if !used_hashes.contains(&hash) {
            let path = std::path::Path::new(&local_path);
            if path.exists() {
                if let Ok(meta) = tokio::fs::metadata(path).await {
                    freed_size += meta.len();
                }
                let _ = delete_attachment_physical(&app_handle, &hash, &local_path).await;

                deleted_count += 1;
            }

            // 从数据库中移除
            let _ = sqlx::query("DELETE FROM attachments WHERE hash = ?")
                .bind(&hash)
                .execute(&db_state.pool)
                .await;
        }
    }

    // ==========================================
    // 🌟 第二阶段：双向磁盘“幽灵文件”扫盲清扫 🌟
    // ==========================================
    let mut ghost_deleted_count = 0;
    let mut ghost_freed_size = 0u64;

    // 1. 获取最新在库的所有有效附件 hash
    let db_hashes_rows: Vec<(String,)> = sqlx::query_as("SELECT hash FROM attachments")
        .fetch_all(&db_state.pool)
        .await
        .unwrap_or_default();
    let current_in_db_hashes: std::collections::HashSet<String> =
        db_hashes_rows.into_iter().map(|(h,)| h).collect();

    // 2. 双向物理校验：清理无主物理附件
    let mut file_yield = YieldCounter::new(200);
    if attachments_dir.exists() {
        if let Ok(mut entries) = tokio::fs::read_dir(&attachments_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                // 协作式 CPU 出让挂起：使用公共 YieldCounter
                file_yield.tick().await;

                let path = entry.path();
                if path.is_file() {
                    let file_name = entry.file_name().to_string_lossy().to_string();
                    let hash = file_name
                        .split('.')
                        .next()
                        .unwrap_or(&file_name)
                        .to_string();

                    // 64位十六进制哈希强校验，保障删除安全
                    if is_valid_cas_hash(&hash) && !current_in_db_hashes.contains(&hash) {
                        if let Ok(meta) = tokio::fs::metadata(&path).await {
                            ghost_freed_size += meta.len();
                        }
                        if tokio::fs::remove_file(&path).await.is_ok() {
                            ghost_deleted_count += 1;
                            log::info!(
                                "[Maintenance] GC swept ghost attachment file: {}",
                                file_name
                            );
                        }
                    }
                }
            }
        }
    }

    // 3. 双向物理校验：清理无主物理缩略图
    let mut thumb_yield = YieldCounter::new(200);
    if let Ok(thumbnails_dir) = get_thumbnails_root_dir(&app_handle) {
        if thumbnails_dir.exists() {
            if let Ok(mut entries) = tokio::fs::read_dir(&thumbnails_dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    // 协作式 CPU 出让挂起：使用公共 YieldCounter
                    thumb_yield.tick().await;

                    let path = entry.path();
                    if path.is_file() {
                        let file_name = entry.file_name().to_string_lossy().to_string();
                        // 提取缩略图对应的哈希值 (hash_thumb.webp)
                        if file_name.ends_with("_thumb.webp") && file_name.len() == 75 {
                            let hash = file_name[..64].to_string();
                            if !current_in_db_hashes.contains(&hash) {
                                if let Ok(meta) = tokio::fs::metadata(&path).await {
                                    ghost_freed_size += meta.len();
                                }
                                if tokio::fs::remove_file(&path).await.is_ok() {
                                    ghost_deleted_count += 1;
                                    log::info!(
                                        "[Maintenance] GC swept ghost thumbnail file: {}",
                                        file_name
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 4. 双向物理校验：清理无主多模态缓存 (.json 文件)
    let mut cache_yield = YieldCounter::new(200);
    if let Ok(cache_dir) = get_multimodal_cache_dir(&app_handle) {
        if cache_dir.exists() {
            if let Ok(mut entries) = tokio::fs::read_dir(&cache_dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    cache_yield.tick().await;
                    let path = entry.path();
                    if path.is_file() {
                        let file_name = entry.file_name().to_string_lossy().to_string();
                        if file_name.ends_with(".json") && file_name.len() == 69 {
                            let hash = file_name[..64].to_string();
                            if !current_in_db_hashes.contains(&hash) {
                                if let Ok(meta) = tokio::fs::metadata(&path).await {
                                    ghost_freed_size += meta.len();
                                }
                                if tokio::fs::remove_file(&path).await.is_ok() {
                                    ghost_deleted_count += 1;
                                    log::info!(
                                        "[Maintenance] GC swept ghost multimodal cache file: {}",
                                        file_name
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let total_deleted = deleted_count + ghost_deleted_count;
    let total_freed_mb = ((freed_size + ghost_freed_size) as f64) / 1024.0 / 1024.0;

    Ok(format!(
        "清理完成：共删除 {} 个孤立文件 (常规: {} 个，幽灵: {} 个)，释放空间: {:.2} MB",
        total_deleted, deleted_count, ghost_deleted_count, total_freed_mb
    ))
}

/// 3. 重建系统缓存与性能物理整理 (Level 3)
/// 物理抹除 V8 code_cache 字节码编译缓存并运行 SQLite 碎片整理，坚决不清理同步日志诊断数据。
#[tauri::command]
pub async fn reconstruct_system_cache(
    app: AppHandle,
    db_state: State<'_, DbState>,
) -> Result<String, String> {
    let mut cleared_details = String::new();

    // 1. 强力物理抹除 V8 code_cache
    if let Ok(cache_dir) = app.path().app_cache_dir() {
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

    // 2. SQLite 物理空间碎片真空整理与查询规划器优化
    // 分批回收 500 个 Page，避免造成单次大 Vacuum 导致长时间的 I/O 阻塞与锁竞争
    let _ = db_state.run_incremental_vacuum_optimize(500).await;
    cleared_details.push_str("SQLite 空间碎片整理与索引规划器重构已执行；");

    Ok(format!(
        "系统缓存重建与数据库真空物理收缩完成 ({})",
        cleared_details.trim_end_matches('；')
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

    if let Some((path_str, _)) = row {
        // 调用统一的物理删除原语，连同缩略图一并抹除
        let _ = delete_attachment_physical(&app_handle, &hash, &path_str).await;

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

    let now = now_secs();

    let three_days_secs = 3 * 24 * 60 * 60;

    if now - last_clear > three_days_secs {
        log::info!("[Maintenance] Triggering scheduled maintenance (WebView & SQLite)...");

        // 1. WebView 清理
        if let Some(webview) = app.get_webview_window("main") {
            let _ = webview.clear_all_browsing_data();
        }

        // 2. SQLite 物理空间回收与查询规划器优化
        let db_state = app.state::<DbState>();
        let _ = db_state.run_incremental_vacuum_optimize(100).await;

        // 3. 自动清除已删除消息的多余附件关联 (防线二：自动维护自愈)
        let _ = sqlx::query(
            "DELETE FROM message_attachments WHERE (topic_id, msg_id) IN (\
             SELECT ma.topic_id, ma.msg_id FROM message_attachments ma \
             INNER JOIN messages m ON ma.topic_id = m.topic_id AND ma.msg_id = m.msg_id \
             WHERE m.deleted_at IS NOT NULL)",
        )
        .execute(&db_state.pool)
        .await;

        // 更新时间戳
        let updates = serde_json::json!({
            "lastWebviewCacheClear": now
        });
        let _ = update_settings(app.clone(), settings_state, updates).await;
        log::info!("[Maintenance] Scheduled maintenance complete.");
    }
}
