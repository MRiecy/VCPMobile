use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::settings_manager::{read_settings, SettingsState};
use crate::vcp_modules::vcp_client::{
    freeze_chat_connection, resolve_model_discovery_endpoint, ChatRequestPurpose,
    MODEL_DISCOVERY_UNAVAILABLE,
};
use futures_util::future::join_all;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::sync::{Mutex, RwLock};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default)]
    pub object: String,
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub owned_by: String,
}

pub struct ModelManagerState {
    pub cached_models: Arc<RwLock<Vec<ModelInfo>>>,
    pub http_client: Client,
    active_batch_task: Arc<Mutex<Option<OwnedBatchTask>>>,
    batch_lifecycle: Mutex<()>,
    refresh_lifecycle: Mutex<()>,
    next_batch_owner: AtomicU64,
}

struct OwnedBatchTask {
    owner_id: u64,
    handle: tokio::task::JoinHandle<()>,
}

fn is_current_batch_owner(active_owner: Option<u64>, owner_id: u64) -> bool {
    active_owner == Some(owner_id)
}

impl ModelManagerState {
    pub fn new() -> Self {
        let http_client = Client::builder()
            .pool_max_idle_per_host(10)
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            cached_models: Arc::new(RwLock::new(Vec::new())),
            http_client,
            active_batch_task: Arc::new(Mutex::new(None)),
            batch_lifecycle: Mutex::new(()),
            refresh_lifecycle: Mutex::new(()),
            next_batch_owner: AtomicU64::new(0),
        }
    }
}

#[tauri::command]
pub async fn get_cached_models<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, ModelManagerState>,
) -> Result<Vec<ModelInfo>, String> {
    // 1. 优先尝试内存缓存
    let mem_cached = state.cached_models.read().await.clone();
    if !mem_cached.is_empty() {
        return Ok(mem_cached);
    }

    // 2. 内存缺失时尝试从数据库 (settings 表) 读取
    let db_state = app.state::<DbState>();
    let pool = &db_state.pool;

    let row = sqlx::query("SELECT value FROM settings WHERE key = 'cached_models'")
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;

    if let Some(r) = row {
        use sqlx::Row;
        let json_str: String = r.get("value");
        if let Ok(models) = serde_json::from_str::<Vec<ModelInfo>>(&json_str) {
            // 回写到内存防止下次重复读取 DB
            *state.cached_models.write().await = models.clone();
            return Ok(models);
        }
    }

    Ok(Vec::new())
}

#[tauri::command]
pub async fn refresh_models<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, ModelManagerState>,
    settings_state: State<'_, SettingsState>,
) -> Result<Vec<ModelInfo>, String> {
    let _refresh_guard = state.refresh_lifecycle.lock().await;
    let settings = read_settings(app.clone(), settings_state).await?;
    let models_url = resolve_model_discovery_endpoint(
        &settings.vcp_server_url,
        settings.chat_endpoint_mode,
    )?
    .ok_or_else(|| {
        format!(
            "{MODEL_DISCOVERY_UNAVAILABLE}: 原始 URL 无法安全推导 /v1/models；主聊天仍可直接使用该地址"
        )
    })?;
    let vcp_api_key = settings.vcp_api_key;

    let client = state.http_client.clone();

    let res = client
        .get(&models_url)
        .header("Authorization", format!("Bearer {}", vcp_api_key))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("网络请求失败: {}", e))?;

    if res.status().is_success() {
        let json_res: Value = res
            .json()
            .await
            .map_err(|e| format!("JSON解析失败: {}", e))?;
        if let Some(data) = json_res.get("data").and_then(|d| d.as_array()) {
            let models: Vec<ModelInfo> = data
                .iter()
                .filter_map(|m| serde_json::from_value(m.clone()).ok())
                .collect();

            // 1. 更新内存缓存
            *state.cached_models.write().await = models.clone();

            // 2. 持久化到数据库 (settings 表)
            let db_state = app.state::<DbState>();
            let json_str = serde_json::to_string(&models).unwrap_or_default();
            let now = crate::vcp_modules::infra::utils::now_millis();

            let persist_result = async {
                let mut tx = db_state.write_transaction("model.cache").await?;
                sqlx::query("INSERT INTO settings (key, value, updated_at) VALUES ('cached_models', ?, ?)
                             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at")
                    .bind(json_str)
                    .bind(now)
                    .execute(&mut *tx)
                    .await
                    .map_err(|error| error.to_string())?;
                tx.commit().await.map_err(|error| error.to_string())?;
                Ok::<(), String>(())
            }
            .await;
            if let Err(error) = persist_result {
                log::warn!("[ModelManager] Failed to persist model cache: {error}");
            }

            Ok(models)
        } else {
            Err("Unexpected response format".to_string())
        }
    } else {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        Err(format!("获取模型失败 ({}): {}", status.as_u16(), text))
    }
}

#[tauri::command]
pub async fn get_hot_models<R: Runtime>(
    app: AppHandle<R>,
    _state: State<'_, ModelManagerState>,
    limit: usize,
) -> Result<Vec<String>, String> {
    let db_state = app.state::<DbState>();
    let pool = &db_state.pool;

    let rows =
        sqlx::query("SELECT model_id FROM model_usage_stats ORDER BY usage_count DESC LIMIT ?")
            .bind(limit as i64)
            .fetch_all(pool)
            .await
            .map_err(|e| e.to_string())?;

    let mut models = Vec::new();
    for row in rows {
        use sqlx::Row;
        models.push(row.get("model_id"));
    }

    Ok(models)
}

#[tauri::command]
pub async fn get_favorite_models<R: Runtime>(
    app: AppHandle<R>,
    _state: State<'_, ModelManagerState>,
) -> Result<Vec<String>, String> {
    let db_state = app.state::<DbState>();
    let pool = &db_state.pool;

    let rows = sqlx::query("SELECT model_id FROM model_favorites ORDER BY created_at DESC")
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

    let mut models = Vec::new();
    for row in rows {
        use sqlx::Row;
        models.push(row.get("model_id"));
    }

    Ok(models)
}

#[tauri::command]
pub async fn toggle_favorite_model<R: Runtime>(
    app: AppHandle<R>,
    _state: State<'_, ModelManagerState>,
    model_id: String,
) -> Result<bool, String> {
    let db_state = app.state::<DbState>();
    let mut tx = db_state.write_transaction("model.favorite").await?;

    let row = sqlx::query("SELECT model_id FROM model_favorites WHERE model_id = ?")
        .bind(&model_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    let favorited = if row.is_some() {
        sqlx::query("DELETE FROM model_favorites WHERE model_id = ?")
            .bind(&model_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        false
    } else {
        let now = crate::vcp_modules::infra::utils::now_millis();

        sqlx::query("INSERT INTO model_favorites (model_id, created_at) VALUES (?, ?)")
            .bind(&model_id)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        true
    };

    tx.commit().await.map_err(|e| e.to_string())?;

    Ok(favorited)
}

#[tauri::command]
pub async fn record_model_usage<R: Runtime>(
    app: AppHandle<R>,
    _state: State<'_, ModelManagerState>,
    model_id: String,
) -> Result<(), String> {
    let db_state = app.state::<DbState>();

    let now = crate::vcp_modules::infra::utils::now_millis();

    let mut tx = db_state.write_transaction("model.usage").await?;
    sqlx::query(
        "INSERT INTO model_usage_stats (model_id, usage_count, updated_at) 
         VALUES (?, 1, ?) 
         ON CONFLICT(model_id) DO UPDATE SET usage_count = usage_count + 1, updated_at = excluded.updated_at"
    )
    .bind(&model_id)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;

    Ok(())
}

pub async fn perform_single_test_internal(
    client: &Client,
    endpoint_url: &str,
    vcp_api_key: &str,
    model_id: &str,
) -> Result<(), String> {
    let payload = serde_json::json!({
        "model": model_id,
        "messages": [{"role": "user", "content": "ping"}],
        "max_tokens": 12
    });

    let res = client
        .post(endpoint_url)
        .header("Authorization", format!("Bearer {}", vcp_api_key))
        .header("Content-Type", "application/json")
        .json(&payload)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("连接失败: {}", e))?;

    if res.status().is_success() {
        Ok(())
    } else {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        let err_msg = if let Ok(val) = serde_json::from_str::<Value>(&text) {
            val.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
                .unwrap_or(text)
        } else {
            text
        };
        Err(format!("HTTP {}: {}", status.as_u16(), err_msg))
    }
}

#[tauri::command]
pub async fn test_model_connectivity<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, ModelManagerState>,
    settings_state: State<'_, SettingsState>,
    model_id: String,
) -> Result<u64, String> {
    let settings = read_settings(app, settings_state).await?;
    let connection = freeze_chat_connection(&settings, ChatRequestPurpose::Interactive)?;

    let start = std::time::Instant::now();
    perform_single_test_internal(
        &state.http_client,
        &connection.endpoint_url,
        &connection.api_key,
        &model_id,
    )
    .await?;
    let duration = start.elapsed().as_millis() as u64;
    Ok(duration)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTestProgress {
    pub model_id: String,
    pub status: String, // "testing", "success", "failed", "completed"
    pub latency: Option<u64>,
    pub error: Option<String>,
}

#[tauri::command]
pub async fn start_batch_model_test<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, ModelManagerState>,
    settings_state: State<'_, SettingsState>,
    model_ids: Vec<String>,
    progress_channel: Channel<ModelTestProgress>,
) -> Result<(), String> {
    if model_ids.len() > 200 {
        return Err("单次最多测试 200 个模型".to_string());
    }
    let _lifecycle_guard = state.batch_lifecycle.lock().await;

    // 1. 在主线程同步读取一次设置，规避生命周期逃逸问题，同时实现零冗余开销
    let settings = read_settings(app, settings_state).await?;
    let connection = freeze_chat_connection(&settings, ChatRequestPurpose::Interactive)?;
    let endpoint_url = connection.endpoint_url;
    let vcp_api_key = connection.api_key;

    // 2. 物理级硬性中止上一次的批量测试任务，从源头防止网络泄漏
    {
        let mut active_task = state.active_batch_task.lock().await;
        if let Some(task) = active_task.take() {
            log::info!("[ModelManager] Aborting previous active batch model test task...");
            task.handle.abort();
            drop(active_task);
            if let Err(error) = task.handle.await {
                if !error.is_cancelled() {
                    log::warn!("[ModelManager] Previous batch task join failed: {}", error);
                }
            }
        }
    }

    let http_client = state.http_client.clone();
    let active_batch_task = state.active_batch_task.clone();
    let owner_id = state.next_batch_owner.fetch_add(1, Ordering::SeqCst) + 1;
    let (start_tx, start_rx) = tokio::sync::oneshot::channel::<()>();

    // 3. 启动全新的后台异步任务管理队列（将所有拥有权 String 和 bool 安全 move 进闭包，生命周期自动延长为 'static）
    let handle = tokio::spawn(async move {
        if start_rx.await.is_err() {
            return;
        }
        log::info!(
            "[ModelManager] Starting new batch model test for {} models...",
            model_ids.len()
        );

        let chunks: Vec<Vec<String>> = model_ids.chunks(5).map(|chunk| chunk.to_vec()).collect();

        for chunk in chunks {
            // 通知前端这批 5 个模型进入测试状态
            for model_id in &chunk {
                let _ = progress_channel.send(ModelTestProgress {
                    model_id: model_id.clone(),
                    status: "testing".to_string(),
                    latency: None,
                    error: None,
                });
            }

            // 并发执行当前分块的 5 个请求
            let mut futures = Vec::new();
            for model_id in chunk {
                let client_inner = http_client.clone();
                let endpoint_url_inner = endpoint_url.clone();
                let vcp_api_key_inner = vcp_api_key.clone();
                let channel_inner = progress_channel.clone(); // 克隆通道克隆体，用于实时单任务回传
                futures.push(async move {
                    let start = std::time::Instant::now();
                    let res = perform_single_test_internal(
                        &client_inner,
                        &endpoint_url_inner,
                        &vcp_api_key_inner,
                        &model_id,
                    )
                    .await;
                    let latency = start.elapsed().as_millis() as u64;

                    // 🌟 物理测试完成瞬间立刻回传进度，不再等待同批次其他任务，实现真正的实时渲染
                    match res {
                        Ok(_) => {
                            let _ = channel_inner.send(ModelTestProgress {
                                model_id,
                                status: "success".to_string(),
                                latency: Some(latency),
                                error: None,
                            });
                        }
                        Err(err_msg) => {
                            let _ = channel_inner.send(ModelTestProgress {
                                model_id,
                                status: "failed".to_string(),
                                latency: None,
                                error: Some(err_msg),
                            });
                        }
                    }
                });
            }

            // 等待当前组的所有 5 个任务物理结束，才进入下一批（保障控流阈值）
            let _ = join_all(futures).await;
        }

        // 4. 只有仍持有 owner 的任务可以发送 completed 并清理自己的句柄。
        let mut active_task = active_batch_task.lock().await;
        let active_owner = active_task.as_ref().map(|task| task.owner_id);
        if is_current_batch_owner(active_owner, owner_id) {
            let _ = progress_channel.send(ModelTestProgress {
                model_id: "".to_string(),
                status: "completed".to_string(),
                latency: None,
                error: None,
            });
            active_task.take();
            log::info!(
                "[ModelManager] Batch model test owner {} completed successfully.",
                owner_id
            );
        }
    });

    // 保存当前任务句柄以支持随时 Abort
    *state.active_batch_task.lock().await = Some(OwnedBatchTask { owner_id, handle });
    let _ = start_tx.send(());

    Ok(())
}

#[tauri::command]
pub async fn stop_all_model_tests(state: State<'_, ModelManagerState>) -> Result<(), String> {
    let _lifecycle_guard = state.batch_lifecycle.lock().await;
    let mut active_task = state.active_batch_task.lock().await;
    if let Some(task) = active_task.take() {
        log::info!(
            "[ModelManager] Stop command received. Aborting backend model test task physically."
        );
        task.handle.abort();
        drop(active_task);
        if let Err(error) = task.handle.await {
            if !error.is_cancelled() {
                return Err(format!("等待模型测试任务停止失败: {}", error));
            }
        }
    }
    Ok(())
}

// 初始化加载
pub async fn init_model_manager<R: Runtime>(_app: &AppHandle<R>, _state: &ModelManagerState) {
    // 数据库架构下无需在启动时将全量收藏与使用数据加载至内存
}

#[cfg(test)]
mod task_owner_tests {
    use super::is_current_batch_owner;

    #[test]
    fn old_batch_finalizer_cannot_clear_new_owner() {
        assert!(is_current_batch_owner(Some(2), 2));
        assert!(!is_current_batch_owner(Some(2), 1));
        assert!(!is_current_batch_owner(None, 1));
    }
}
