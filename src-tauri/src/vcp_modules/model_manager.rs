use crate::vcp_modules::app_settings_manager::{read_app_settings, AppSettingsState};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
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

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[allow(dead_code)]
pub struct ModelUsageStats {
    pub usage: HashMap<String, u32>,
}

pub struct ModelManagerState {
    pub cached_models: Arc<RwLock<Vec<ModelInfo>>>,
    pub favorites: Arc<RwLock<Vec<String>>>,
    pub usage_stats: Arc<RwLock<HashMap<String, u32>>>,
    pub is_dirty: Arc<RwLock<bool>>,
}

impl ModelManagerState {
    pub fn new() -> Self {
        Self {
            cached_models: Arc::new(RwLock::new(Vec::new())),
            favorites: Arc::new(RwLock::new(Vec::new())),
            usage_stats: Arc::new(RwLock::new(HashMap::new())),
            is_dirty: Arc::new(RwLock::new(false)),
        }
    }
}

async fn get_app_data_path<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("AppData"))
}

async fn load_favorites<R: Runtime>(app: &AppHandle<R>) -> Vec<String> {
    let path = get_app_data_path(app).await.join("model_favorites.json");
    if let Ok(content) = tokio::fs::read_to_string(&path).await {
        if let Ok(favs) = serde_json::from_str::<Vec<String>>(&content) {
            return favs;
        }
    }
    Vec::new()
}

async fn save_favorites<R: Runtime>(
    app: &AppHandle<R>,
    favorites: &[String],
) -> Result<(), String> {
    let path = get_app_data_path(app).await.join("model_favorites.json");
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let content = serde_json::to_string_pretty(favorites).map_err(|e| e.to_string())?;
    tokio::fs::write(path, content)
        .await
        .map_err(|e| e.to_string())
}

async fn load_usage_stats<R: Runtime>(app: &AppHandle<R>) -> HashMap<String, u32> {
    let path = get_app_data_path(app).await.join("model_usage_stats.json");
    if let Ok(content) = tokio::fs::read_to_string(&path).await {
        if let Ok(stats) = serde_json::from_str::<HashMap<String, u32>>(&content) {
            return stats;
        }
    }
    HashMap::new()
}

async fn save_usage_stats<R: Runtime>(
    app: &AppHandle<R>,
    stats: &HashMap<String, u32>,
) -> Result<(), String> {
    let path = get_app_data_path(app).await.join("model_usage_stats.json");
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let content = serde_json::to_string_pretty(stats).map_err(|e| e.to_string())?;
    tokio::fs::write(path, content)
        .await
        .map_err(|e| e.to_string())
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
pub async fn get_hot_models(
    state: State<'_, ModelManagerState>,
    limit: usize,
) -> Result<Vec<String>, String> {
    let stats = state.usage_stats.read().await;
    let mut entries: Vec<(&String, &u32)> = stats.iter().collect();
    entries.sort_by(|a, b| b.1.cmp(a.1));
    Ok(entries
        .into_iter()
        .take(limit)
        .map(|(k, _)| k.clone())
        .collect())
}

#[tauri::command]
pub async fn get_favorite_models<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, ModelManagerState>,
) -> Result<Vec<String>, String> {
    let mut favs = state.favorites.write().await;
    if favs.is_empty() {
        *favs = load_favorites(&app).await;
    }
    Ok(favs.clone())
}

#[tauri::command]
pub async fn toggle_favorite_model<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, ModelManagerState>,
    model_id: String,
) -> Result<bool, String> {
    let mut favs = state.favorites.write().await;
    if favs.is_empty() {
        *favs = load_favorites(&app).await;
    }

    let favorited;
    if let Some(pos) = favs.iter().position(|id| id == &model_id) {
        favs.remove(pos);
        favorited = false;
    } else {
        favs.push(model_id);
        favorited = true;
    }

    save_favorites(&app, &favs).await?;
    Ok(favorited)
}

#[tauri::command]
pub async fn record_model_usage<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, ModelManagerState>,
    model_id: String,
) -> Result<(), String> {
    let mut stats = state.usage_stats.write().await;
    if stats.is_empty() {
        *stats = load_usage_stats(&app).await;
    }

    let count = stats.entry(model_id).or_insert(0);
    *count += 1;

    let mut dirty = state.is_dirty.write().await;
    if !*dirty {
        *dirty = true;
        let app_clone = app.clone();
        let stats_clone = stats.clone();
        let dirty_clone = state.is_dirty.clone();

        // 简单的异步延迟落盘逻辑
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let mut d = dirty_clone.write().await;
            if *d {
                if let Err(e) = save_usage_stats(&app_clone, &stats_clone).await {
                    eprintln!("[ModelManager] Failed to save usage stats: {}", e);
                }
                *d = false;
            }
        });
    }

    Ok(())
}

// 初始化加载
pub async fn init_model_manager<R: Runtime>(app: &AppHandle<R>, state: &ModelManagerState) {
    let favs = load_favorites(app).await;
    *state.favorites.write().await = favs;

    let stats = load_usage_stats(app).await;
    *state.usage_stats.write().await = stats;
}
