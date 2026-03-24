// AppSettingsManager: 处理应用全局配置的核心模块
// 源 JS 逻辑参考: ../VCPChat/modules/utils/appSettingsManager.js
// 职责: 管理 settings.json 及其备份，实现原子写入、数据验证、多重恢复机制与并发控制。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::sync::Mutex;
use tokio::time::sleep;

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

    #[serde(rename = "userAvatarUrl")]
    pub user_avatar_url: Option<String>,
    #[serde(rename = "userAvatarCalculatedColor")]
    pub user_avatar_calculated_color: Option<String>,
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

    /// 捕获所有未定义的字段，确保 settings.json 的完整性
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
    /// 执行业务逻辑验证
    /// 对齐 JS: SettingsValidator.validate
    pub fn validate(&mut self) {
        if self.sidebar_width < 100 || self.sidebar_width > 800 {
            self.sidebar_width = 260;
        }
    }
}

pub struct AppSettingsState {
    pub cache: Arc<Mutex<Option<AppSettings>>>,
    pub cache_timestamp: Arc<Mutex<u64>>,
    pub lock: Arc<Mutex<()>>,
}

impl AppSettingsState {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(None)),
            cache_timestamp: Arc::new(Mutex::new(0)),
            lock: Arc::new(Mutex::new(())),
        }
    }
}

fn get_settings_path<R: Runtime>(app_handle: &AppHandle<R>) -> Result<PathBuf, String> {
    let mut path = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    path.push("settings.json");
    Ok(path)
}

/// 指数退避重试读取文件
async fn retry_read_to_string(path: &Path) -> Result<String, String> {
    let delays = [50, 100, 200];

    for &delay in delays.iter() {
        match fs::read_to_string(path) {
            Ok(content) => return Ok(content),
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    sleep(Duration::from_millis(delay)).await;
                    continue;
                }
                return Err(e.to_string());
            }
        }
    }
    fs::read_to_string(path).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn read_app_settings<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, AppSettingsState>,
) -> Result<AppSettings, String> {
    let path = get_settings_path(&app_handle)?;

    // 1. 缓存检查
    if let Ok(metadata) = fs::metadata(&path) {
        let mtime = metadata
            .modified()
            .map(|t| t.duration_since(UNIX_EPOCH).unwrap().as_millis() as u64)
            .unwrap_or(0);

        let cache_ts = *state.cache_timestamp.lock().await;
        if mtime <= cache_ts {
            if let Some(cached) = &*state.cache.lock().await {
                return Ok(cached.clone());
            }
        }
    }

    // 2. 文件不存在处理
    if !path.exists() {
        let default_settings = AppSettings {
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
            user_avatar_url: None,
            user_avatar_calculated_color: None,
            current_theme_mode: None,
            theme_last_updated: None,
            flowlock_continue_delay: 5,
            sync_server_ip: "".to_string(),
            sync_server_port: 5974,
            sync_token: "".to_string(),
            topic_summary_model: Some("gemini-2.5-flash".to_string()),
            topic_summary_model_temperature: Some(0.7),
            extra: serde_json::Value::Object(serde_json::Map::new()),
        };
        return Ok(default_settings);
    }

    // 3. 读取并解析 (带重试)
    let content = retry_read_to_string(&path).await?;
    let settings_res: Result<AppSettings, serde_json::Error> = serde_json::from_str(&content);

    let mut settings = match settings_res {
        Ok(s) => s,
        Err(e) => {
            // [恢复逻辑] 尝试从备份恢复
            let mut backup_recovered = None;
            let backup_path = path.with_extension("json.backup");
            if backup_path.exists() {
                if let Ok(backup_content) = fs::read_to_string(&backup_path) {
                    if let Ok(backup_settings) =
                        serde_json::from_str::<AppSettings>(&backup_content)
                    {
                        backup_recovered = Some(backup_settings);
                    }
                }
            }

            match backup_recovered {
                Some(bs) => bs,
                None => return Err(e.to_string()),
            }
        }
    };

    // 动态注入用户真实头像路径
    // 注意：在 VCP Mobile 架构中，桌面端的 AppData/UserData 映射为手机端的 app_config_dir/data
    let config_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    let user_data_dir = config_dir.join("data");

    // 检查常见的头像格式 (下划线格式)
    let extensions = ["png", "jpg", "jpeg", "webp"];
    settings.user_avatar_url = None;
    for ext in extensions {
        let avatar_path = user_data_dir.join(format!("user_avatar.{}", ext));
        if avatar_path.exists() {
            // 转换为正斜杠路径，方便前端处理
            settings.user_avatar_url = Some(avatar_path.to_string_lossy().replace("\\", "/"));
            break;
        }
    }

    // 4. 更新缓存
    let mtime = fs::metadata(&path)
        .and_then(|m| m.modified())
        .map(|t| t.duration_since(UNIX_EPOCH).unwrap().as_millis() as u64)
        .unwrap_or_else(|_| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64
        });

    *state.cache.lock().await = Some(settings.clone());
    *state.cache_timestamp.lock().await = mtime;

    Ok(settings)
}

#[tauri::command]
pub async fn write_app_settings<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, AppSettingsState>,
    mut settings: AppSettings,
) -> Result<bool, String> {
    // 获取全局设置锁
    let _lock = state.lock.lock().await;

    // 数据验证
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
    let path = get_settings_path(app_handle)?;
    let temp_path = path.with_extension("json.tmp");
    let backup_path = path.with_extension("json.backup");

    // 确保目录存在
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    // 1. 写入临时文件
    let content = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(&temp_path, &content).map_err(|e| e.to_string())?;

    // 2. 验证
    let _: AppSettings = serde_json::from_str(&content)
        .map_err(|e| format!("Temp file validation failed: {}", e))?;

    // 3. 备份
    if path.exists() {
        fs::copy(&path, &backup_path).map_err(|e| e.to_string())?;
    }

    // 4. 原子替换
    fs::rename(&temp_path, &path).map_err(|e| e.to_string())?;

    // 5. 更新缓存
    let mtime = fs::metadata(&path)
        .and_then(|m| m.modified())
        .map(|t| t.duration_since(UNIX_EPOCH).unwrap().as_millis() as u64)
        .unwrap_or_else(|_| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64
        });

    *state.cache.lock().await = Some(settings.clone());
    *state.cache_timestamp.lock().await = mtime;

    Ok(true)
}
