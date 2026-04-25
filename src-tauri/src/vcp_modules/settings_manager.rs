// SettingsManager: 处理应用全局配置的核心模块
// 职责: 管理全局配置，实现基于 SQLite 的原子写入与并发控制。

use crate::vcp_modules::db_manager::DbState;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::sync::Mutex;

fn default_sync_log_level() -> String {
    "INFO".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default)]
    pub user_name: String,

    // VCP 核心服务器
    #[serde(default)]
    pub vcp_server_url: String,
    #[serde(default)]
    pub vcp_api_key: String,
    #[serde(default)]
    pub vcp_log_url: String,
    #[serde(default)]
    pub vcp_log_key: String,

    // VCP 数据同步连接
    #[serde(default)]
    pub sync_server_url: String, // WebSocket 服务 URL (ws://ip:port)
    #[serde(default)]
    pub sync_http_url: String, // HTTP API 服务 URL (http://ip:port)
    #[serde(default)]
    pub sync_token: String,

    // 管理接口鉴权 (用于表情包刷新等)
    #[serde(default)]
    pub admin_username: String,
    #[serde(default)]
    pub admin_password: String,

    // 表情包图床密钥
    #[serde(default)]
    pub file_key: String,

    // 话题总结配置
    #[serde(default)]
    pub topic_summary_model: String,

    // 同步日志配置
    #[serde(default = "default_sync_log_level")]
    pub sync_log_level: String,

    // 排序逻辑 (移动端分组)
    #[serde(default)]
    pub agent_order: Vec<String>,
    #[serde(default)]
    pub group_order: Vec<String>,

    #[serde(default)]
    pub current_theme_mode: Option<String>,

    /// 仅保留此字段用于前端未来扩展的透参
    #[serde(flatten)]
    #[serde(default)]
    pub extra: serde_json::Value,
}

pub struct SettingsState {
    pub cache: Arc<Mutex<Option<Settings>>>,
    pub lock: Arc<Mutex<()>>,
}

impl SettingsState {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(None)),
            lock: Arc::new(Mutex::new(())),
        }
    }
}

pub fn create_default_settings() -> Settings {
    Settings {
        user_name: "用户".to_string(),
        vcp_server_url: "".to_string(),
        vcp_api_key: "".to_string(),
        vcp_log_url: "".to_string(),
        vcp_log_key: "".to_string(),
        sync_server_url: "".to_string(),
        sync_http_url: "".to_string(),
        sync_token: "".to_string(),
        admin_username: "".to_string(),
        admin_password: "".to_string(),
        file_key: "".to_string(),
        topic_summary_model: "gemini-2.5-flash".to_string(),
        sync_log_level: "INFO".to_string(),
        agent_order: vec![],
        group_order: vec![],
        current_theme_mode: Some("dark".to_string()),
        extra: serde_json::Value::Object(serde_json::Map::new()),
    }
}

#[tauri::command]
pub async fn read_settings<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, SettingsState>,
) -> Result<Settings, String> {
    if let Some(cached) = &*state.cache.lock().await {
        return Ok(cached.clone());
    }

    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let row_res = sqlx::query("SELECT value FROM settings WHERE key = 'global'")
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;

    let settings = if let Some(row) = row_res {
        use sqlx::Row;
        let content: String = row.get("value");
        serde_json::from_str(&content).unwrap_or_else(|_| create_default_settings())
    } else {
        create_default_settings()
    };

    *state.cache.lock().await = Some(settings.clone());
    Ok(settings)
}

#[tauri::command]
pub async fn write_settings<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, SettingsState>,
    settings: Settings,
) -> Result<bool, String> {
    let _lock = state.lock.lock().await;
    internal_write_settings(&app_handle, &state, &settings).await
}

#[tauri::command]
pub async fn update_settings<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, SettingsState>,
    updates: serde_json::Value,
) -> Result<Settings, String> {
    let _lock = state.lock.lock().await;

    let current = read_settings(app_handle.clone(), state.clone()).await?;
    let mut current_val = serde_json::to_value(&current).map_err(|e| e.to_string())?;

    if let Some(obj) = updates.as_object() {
        if let Some(current_obj) = current_val.as_object_mut() {
            for (k, v) in obj {
                current_obj.insert(k.clone(), v.clone());
            }
        }
    }

    let new_settings: Settings = serde_json::from_value(current_val).map_err(|e| e.to_string())?;
    internal_write_settings(&app_handle, &state, &new_settings).await?;

    Ok(new_settings)
}

async fn internal_write_settings<R: Runtime>(
    app_handle: &AppHandle<R>,
    state: &SettingsState,
    settings: &Settings,
) -> Result<bool, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let content = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    sqlx::query("INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('global', ?, ?)")
        .bind(&content)
        .bind(now)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;

    *state.cache.lock().await = Some(settings.clone());

    // [强耦合联动] 只要配置写入成功，立即通知 VCP Log 服务更新连接状态 (自主维护连接)
    let h = app_handle.clone();
    let log_url = settings.vcp_log_url.clone();
    let log_key = settings.vcp_log_key.clone();
    tauri::async_runtime::spawn(async move {
        let _ = crate::vcp_modules::vcp_log_service::init_vcp_log_connection_internal(
            h, log_url, log_key,
        )
        .await;
    });

    Ok(true)
}

#[tauri::command]
pub async fn set_theme<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, SettingsState>,
    theme: String,
) -> Result<bool, String> {
    let updates = serde_json::json!({
        "currentThemeMode": theme
    });

    update_settings(app_handle, state, updates).await?;
    Ok(true)
}
