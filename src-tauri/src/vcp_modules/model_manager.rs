use crate::vcp_modules::app_settings_manager::{read_app_settings, AppSettingsState};
use crate::vcp_modules::db_manager::DbState;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::sync::RwLock;
use url::Url;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub owned_by: String,
}

pub struct ModelManagerState {
    pub cached_models: Arc<RwLock<Vec<ModelInfo>>>,
}

impl ModelManagerState {
    pub fn new() -> Self {
        Self {
            cached_models: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

#[tauri::command]
pub async fn get_cached_models(
    state: State<'_, ModelManagerState>,
) -> Result<Vec<ModelInfo>, String> {
    Ok(state.cached_models.read().await.clone())
}

#[tauri::command]
pub async fn refresh_models<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, ModelManagerState>,
    settings_state: State<'_, AppSettingsState>,
) -> Result<Vec<ModelInfo>, String> {
    let settings = read_app_settings(app.clone(), settings_state).await?;
    let vcp_url = settings.vcp_server_url;
    let vcp_api_key = settings.vcp_api_key;

    if vcp_url.is_empty() {
        return Err("VCP Server URL is not configured.".to_string());
    }

    let url_object = match Url::parse(&vcp_url) {
        Ok(url) => url,
        Err(e) => return Err(format!("URL 解析失败: {}", e)),
    };

    let port_str = match url_object.port() {
        Some(p) => format!(":{}", p),
        None => "".to_string(),
    };
    let host_with_port = format!("{}{}", url_object.host_str().unwrap_or(""), port_str);
    let base_url = format!("{}://{}", url_object.scheme(), host_with_port);

    let models_url = if base_url.ends_with('/') {
        format!("{}v1/models", base_url)
    } else {
        format!("{}/v1/models", base_url)
    };

    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    let res = client
        .get(&models_url)
        .header("Authorization", format!("Bearer {}", vcp_api_key))
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

            *state.cached_models.write().await = models.clone();
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
    let pool = &db_state.pool;

    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

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
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

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
    let pool = &db_state.pool;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    sqlx::query(
        "INSERT INTO model_usage_stats (model_id, usage_count, updated_at) 
         VALUES (?, 1, ?) 
         ON CONFLICT(model_id) DO UPDATE SET usage_count = usage_count + 1, updated_at = excluded.updated_at"
    )
    .bind(&model_id)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    Ok(())
}

// 初始化加载
pub async fn init_model_manager<R: Runtime>(_app: &AppHandle<R>, _state: &ModelManagerState) {
    // 数据库架构下无需在启动时将全量收藏与使用数据加载至内存
}
