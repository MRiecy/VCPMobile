// AppSettingsManager: 处理应用全局配置的核心模块
// 职责: 管理全局配置，实现基于 SQLite 的原子写入、数据验证与并发控制。

use crate::vcp_modules::db_manager::DbState;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::sync::Mutex;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppSettings {
    #[serde(rename = "sidebarWidth", default = "default_sidebar_width")]
    pub sidebar_width: i32,
    #[serde(
        rename = "notificationsSidebarWidth",
        default = "default_notifications_sidebar_width"
    )]
    pub notifications_sidebar_width: i32,
    #[serde(rename = "userName", default = "default_user_name")]
    pub user_name: String,
    #[serde(rename = "vcpServerUrl", default)]
    pub vcp_server_url: String,
    #[serde(rename = "vcpApiKey", default)]
    pub vcp_api_key: String,
    #[serde(rename = "vcpLogUrl", default)]
    pub vcp_log_url: String,
    #[serde(rename = "vcpLogKey", default)]
    pub vcp_log_key: String,
    #[serde(rename = "networkNotesPaths", default)]
    pub network_notes_paths: Vec<serde_json::Value>,
    #[serde(rename = "enableAgentBubbleTheme", default)]
    pub enable_agent_bubble_theme: bool,
    #[serde(rename = "enableSmoothStreaming", default)]
    pub enable_smooth_streaming: bool,
    #[serde(rename = "minChunkBufferSize", default = "default_one_i32")]
    pub min_chunk_buffer_size: i32,
    #[serde(
        rename = "smoothStreamIntervalMs",
        default = "default_smooth_stream_interval"
    )]
    pub smooth_stream_interval_ms: i32,
    #[serde(rename = "assistantAgent", default)]
    pub assistant_agent: String,
    #[serde(rename = "enableDistributedServer", default = "default_true")]
    pub enable_distributed_server: bool,
    #[serde(rename = "agentMusicControl", default)]
    pub agent_music_control: bool,
    #[serde(rename = "enableDistributedServerLogs", default)]
    pub enable_distributed_server_logs: bool,
    #[serde(rename = "enableVcpToolInjection", default)]
    pub enable_vcp_tool_injection: bool,

    #[serde(rename = "lastOpenItemId", default)]
    pub last_open_item_id: Option<String>,
    #[serde(rename = "lastOpenItemType", default)]
    pub last_open_item_type: Option<String>,
    #[serde(rename = "lastOpenTopicId", default)]
    pub last_open_topic_id: Option<String>,

    #[serde(rename = "combinedItemOrder", default)]
    pub combined_item_order: Vec<serde_json::Value>,
    #[serde(rename = "agentOrder", default)]
    pub agent_order: Vec<String>,

    #[serde(rename = "currentThemeMode")]
    pub current_theme_mode: Option<String>,
    #[serde(rename = "themeLastUpdated")]
    pub theme_last_updated: Option<i64>,
    #[serde(rename = "flowlockContinueDelay", default = "default_flowlock_delay")]
    pub flowlock_continue_delay: i32,

    #[serde(rename = "syncServerIp", default)]
    pub sync_server_ip: String,
    #[serde(rename = "syncServerPort", default = "default_sync_port")]
    pub sync_server_port: i32,
    #[serde(rename = "syncToken", default)]
    pub sync_token: String,

    #[serde(rename = "topicSummaryModel")]
    pub topic_summary_model: Option<String>,
    #[serde(rename = "topicSummaryModelTemperature")]
    pub topic_summary_model_temperature: Option<f32>,

    /// 捕获所有未定义的字段
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

fn default_sidebar_width() -> i32 {
    260
}
fn default_notifications_sidebar_width() -> i32 {
    300
}
fn default_user_name() -> String {
    "用户".to_string()
}
fn default_one_i32() -> i32 {
    1
}
fn default_smooth_stream_interval() -> i32 {
    25
}
fn default_true() -> bool {
    true
}
fn default_flowlock_delay() -> i32 {
    5
}
fn default_sync_port() -> i32 {
    5974
}

impl AppSettings {
    pub fn validate(&mut self) {
        if self.sidebar_width < 100 || self.sidebar_width > 800 {
            self.sidebar_width = 260;
        }
    }
}

pub struct AppSettingsState {
    pub cache: Arc<Mutex<Option<AppSettings>>>,
    pub lock: Arc<Mutex<()>>,
}

impl AppSettingsState {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(None)),
            lock: Arc::new(Mutex::new(())),
        }
    }
}

pub fn create_default_settings() -> AppSettings {
    AppSettings {
        sidebar_width: 260,
        notifications_sidebar_width: 300,
        user_name: "用户".to_string(),
        vcp_server_url: "".to_string(),
        vcp_api_key: "".to_string(),
        vcp_log_url: "".to_string(),
        vcp_log_key: "".to_string(),
        network_notes_paths: vec![],
        enable_agent_bubble_theme: false,
        enable_smooth_streaming: false,
        min_chunk_buffer_size: 1,
        smooth_stream_interval_ms: 25,
        assistant_agent: "".to_string(),
        enable_distributed_server: true,
        agent_music_control: false,
        enable_distributed_server_logs: false,
        enable_vcp_tool_injection: false,
        last_open_item_id: None,
        last_open_item_type: None,
        last_open_topic_id: None,
        combined_item_order: vec![],
        agent_order: vec![],
        current_theme_mode: None,
        theme_last_updated: None,
        flowlock_continue_delay: 5,
        sync_server_ip: "".to_string(),
        sync_server_port: 5974,
        sync_token: "".to_string(),
        topic_summary_model: Some("gemini-2.5-flash".to_string()),
        topic_summary_model_temperature: Some(0.7),
        extra: serde_json::Value::Object(serde_json::Map::new()),
    }
}

#[tauri::command]
pub async fn read_app_settings<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, AppSettingsState>,
) -> Result<AppSettings, String> {
    if let Some(cached) = &*state.cache.lock().await {
        return Ok(cached.clone());
    }

    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let row_res = sqlx::query("SELECT value FROM app_settings WHERE key = 'global'")
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
pub async fn write_app_settings<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, AppSettingsState>,
    mut settings: AppSettings,
) -> Result<bool, String> {
    let _lock = state.lock.lock().await;
    settings.validate();
    internal_write_app_settings(&app_handle, &state, &settings).await
}

#[tauri::command]
pub async fn update_app_settings<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, AppSettingsState>,
    updates: serde_json::Value,
) -> Result<AppSettings, String> {
    let _lock = state.lock.lock().await;

    let current = read_app_settings(app_handle.clone(), state.clone()).await?;
    let mut current_val = serde_json::to_value(&current).map_err(|e| e.to_string())?;

    if let Some(obj) = updates.as_object() {
        if let Some(current_obj) = current_val.as_object_mut() {
            for (k, v) in obj {
                current_obj.insert(k.clone(), v.clone());
            }
        }
    }

    let mut new_settings: AppSettings =
        serde_json::from_value(current_val).map_err(|e| e.to_string())?;
    new_settings.validate();

    internal_write_app_settings(&app_handle, &state, &new_settings).await?;

    Ok(new_settings)
}

async fn internal_write_app_settings<R: Runtime>(
    app_handle: &AppHandle<R>,
    state: &AppSettingsState,
    settings: &AppSettings,
) -> Result<bool, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let content = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    sqlx::query(
        "INSERT OR REPLACE INTO app_settings (key, value, updated_at) VALUES ('global', ?, ?)",
    )
    .bind(&content)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    *state.cache.lock().await = Some(settings.clone());
    Ok(true)
}

/// 设置主题
#[tauri::command]
pub async fn set_theme(
    app_handle: AppHandle,
    state: State<'_, AppSettingsState>,
    theme: String, // "light" or "dark"
) -> Result<bool, String> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let updates = serde_json::json!({
        "currentThemeMode": theme,
        "themeLastUpdated": timestamp
    });

    update_app_settings(app_handle, state, updates).await?;

    Ok(true)
}

/// 应用生命周期状态变更 (Active / Background)
#[tauri::command]
pub async fn notify_app_state(
    _app_handle: AppHandle,
    state: String, // "active", "background", "inactive"
) -> Result<(), String> {
    log::info!(
        "[AppSettingsManager] Mobile lifecycle state change: {}",
        state
    );
    Ok(())
}

/// 网络连接状态变更
#[tauri::command]
pub async fn notify_network_state(
    _app_handle: AppHandle,
    online: bool,
    r#type: String, // "wifi", "cellular", "none"
) -> Result<(), String> {
    log::info!(
        "[AppSettingsManager] Network connection changed: online={}, type={}",
        online,
        r#type
    );
    Ok(())
}
