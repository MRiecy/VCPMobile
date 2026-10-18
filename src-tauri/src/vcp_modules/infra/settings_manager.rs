// SettingsManager: 处理应用全局配置的核心模块
// 职责: 管理全局配置，实现基于 SQLite 的原子写入与并发控制。

use crate::vcp_modules::db_manager::DbState;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tauri::{AppHandle, Manager, Runtime, State};
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;

const MAX_CONFIG_BACKUP_BYTES: u64 = 1024 * 1024;
const BACKUP_STRING_KEYS: &[&str] = &[
    "userName",
    "adminUsername",
    "adminPassword",
    "vcpServerUrl",
    "vcpApiKey",
    "vcpLogUrl",
    "vcpLogKey",
    "syncHttpUrl",
    "syncServerUrl",
    "syncToken",
    "fileKey",
    "distributedWsUrl",
    "distributedVcpKey",
    "distributedDeviceName",
    "topicSummaryModel",
];

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedConfigBackup {
    settings: serde_json::Map<String, serde_json::Value>,
    preferences: serde_json::Map<String, serde_json::Value>,
    exported_at: String,
}

fn parse_backup_preferences(
    raw: Option<&serde_json::Value>,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let mut preferences = serde_json::Map::new();
    let Some(raw) = raw else {
        return Ok(preferences);
    };
    let source = raw
        .as_object()
        .ok_or_else(|| "备份字段 preferences 类型不正确".to_string())?;

    if let Some(theme) = source.get("theme") {
        let source = theme
            .as_object()
            .ok_or_else(|| "备份字段 preferences.theme 类型不正确".to_string())?;
        let mut theme = serde_json::Map::new();
        if let Some(key) = source.get("themeKey") {
            if !key.is_string() {
                return Err("备份字段 preferences.theme.themeKey 类型不正确".into());
            }
            // Availability belongs to the bundled frontend theme registry.
            theme.insert("themeKey".into(), key.clone());
        }
        if let Some(mode) = source.get("mode") {
            if !matches!(mode.as_str(), Some("light" | "dark" | "system")) {
                return Err("备份字段 preferences.theme.mode 类型不正确".into());
            }
            theme.insert("mode".into(), mode.clone());
        }
        if !theme.is_empty() {
            preferences.insert("theme".into(), theme.into());
        }
    }

    if let Some(chat) = source.get("chat") {
        let source = chat
            .as_object()
            .ok_or_else(|| "备份字段 preferences.chat 类型不正确".to_string())?;
        let mut chat = serde_json::Map::new();
        if let Some(mode) = source.get("presentationMode") {
            if !matches!(mode.as_str(), Some("bubble" | "panel" | "immersive")) {
                return Err("备份字段 preferences.chat.presentationMode 类型不正确".into());
            }
            chat.insert("presentationMode".into(), mode.clone());
        }
        for key in [
            "smoothStreamingEnabled",
            "messageHapticsEnabled",
            "collapseToolsWhileStreaming",
        ] {
            if let Some(value) = source.get(key) {
                if !value.is_boolean() {
                    return Err(format!("备份字段 preferences.chat.{key} 类型不正确"));
                }
                chat.insert(key.into(), value.clone());
            }
        }
        if !chat.is_empty() {
            preferences.insert("chat".into(), chat.into());
        }
    }
    Ok(preferences)
}

fn parse_backup_bytes(bytes: &[u8]) -> Result<ParsedConfigBackup, String> {
    if bytes.len() as u64 > MAX_CONFIG_BACKUP_BYTES {
        return Err("备份文件过大（超过 1 MiB），已拒绝导入".into());
    }
    let raw: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "备份文件不是合法的 JSON".to_string())?;
    if raw["app"] != "vcp-mobile" || raw["kind"] != "vcp-mobile-settings-backup" {
        return Err("这不是 VCP Mobile 的配置备份文件".into());
    }
    let version = raw["version"].as_u64();
    if !matches!(version, Some(1 | 2)) {
        return Err("不支持的备份版本".into());
    }
    let source = raw["settings"]
        .as_object()
        .ok_or_else(|| "备份文件缺少配置内容".to_string())?;
    let mut settings = serde_json::Map::new();
    for key in BACKUP_STRING_KEYS {
        if let Some(value) = source.get(*key).filter(|value| !value.is_null()) {
            if !value.is_string() {
                return Err(format!("备份字段 {key} 类型不正确"));
            }
            settings.insert((*key).to_string(), value.clone());
        }
    }
    if let Some(value) = source
        .get("distributedEnabled")
        .filter(|value| !value.is_null())
    {
        if !value.is_boolean() {
            return Err("备份字段 distributedEnabled 类型不正确".into());
        }
        settings.insert("distributedEnabled".into(), value.clone());
    }
    if let Some(value) = source
        .get("chatEndpointMode")
        .filter(|value| !value.is_null())
    {
        if !matches!(value.as_str(), Some("standard" | "vcpTools" | "raw")) {
            return Err("备份字段 chatEndpointMode 类型不正确".into());
        }
        settings.insert("chatEndpointMode".into(), value.clone());
    } else if let Some(value) = source.get("enableVcpToolInjection") {
        let enabled = value
            .as_bool()
            .ok_or_else(|| "备份字段 enableVcpToolInjection 类型不正确".to_string())?;
        settings.insert(
            "chatEndpointMode".into(),
            if enabled { "vcpTools" } else { "standard" }.into(),
        );
    }
    Ok(ParsedConfigBackup {
        settings,
        preferences: if version == Some(2) {
            parse_backup_preferences(raw.get("preferences"))?
        } else {
            serde_json::Map::new()
        },
        exported_at: raw["exportedAt"].as_str().unwrap_or_default().to_string(),
    })
}

/// Native selection hands over a temporary path; no external file bytes enter Vue.
#[tauri::command]
pub async fn parse_config_backup(path: String) -> Result<ParsedConfigBackup, String> {
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|error| error.to_string())?;
    let metadata = file.metadata().await.map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("请选择配置备份文件".into());
    }
    if metadata.len() > MAX_CONFIG_BACKUP_BYTES {
        return Err("备份文件过大（超过 1 MiB），已拒绝导入".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BACKUP_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| error.to_string())?;
    parse_backup_bytes(&bytes)
}

fn default_sync_log_level() -> String {
    "INFO".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum ChatEndpointMode {
    #[default]
    Standard,
    VcpTools,
    Raw,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default)]
    pub user_name: String,

    // 分布式设置
    #[serde(default)]
    pub distributed_enabled: bool,
    #[serde(default)]
    pub distributed_ws_url: String,
    #[serde(default)]
    pub distributed_vcp_key: String,
    #[serde(default)]
    pub distributed_device_name: String,

    // VCP 核心服务器
    #[serde(default)]
    pub vcp_server_url: String,
    #[serde(default)]
    pub chat_endpoint_mode: ChatEndpointMode,
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

    /// 同步时是否执行消息预渲染（默认 false，节省同步时间）
    #[serde(default)]
    pub sync_prerender_enabled: bool,

    #[serde(default)]
    pub enable_assistant: bool,

    #[serde(default)]
    pub assistant_agent_id: String,

    /// 仅保留此字段用于前端未来扩展的透参
    #[serde(flatten)]
    #[serde(default)]
    pub extra: serde_json::Value,
}

pub struct SettingsState {
    cache: Arc<Mutex<Option<Settings>>>,
    lock: Arc<Mutex<()>>,
    recovery_status: Arc<Mutex<SettingsRecoveryStatus>>,
    runtime_generation: Arc<AtomicU64>,
    runtime_reconcile_lock: Arc<Mutex<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuntimeSettingsChanges {
    vcp_log_changed: bool,
    distributed_reconcile_required: bool,
}

fn classify_runtime_settings_changes(
    old: Option<&Settings>,
    settings: &Settings,
) -> RuntimeSettingsChanges {
    let vcp_log_changed = old.map_or_else(
        || !settings.vcp_log_url.is_empty() || !settings.vcp_log_key.is_empty(),
        |old| old.vcp_log_url != settings.vcp_log_url || old.vcp_log_key != settings.vcp_log_key,
    );
    let distributed_reconcile_required = old.map_or(settings.distributed_enabled, |old| {
        let enabled_changed = old.distributed_enabled != settings.distributed_enabled;
        let params_changed = old.distributed_ws_url != settings.distributed_ws_url
            || old.distributed_vcp_key != settings.distributed_vcp_key
            || old.distributed_device_name != settings.distributed_device_name;
        enabled_changed || (params_changed && settings.distributed_enabled)
    });
    RuntimeSettingsChanges {
        vcp_log_changed,
        distributed_reconcile_required,
    }
}

#[derive(Debug, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SettingsRecoveryStatus {
    pub recovered_corrupt: bool,
    pub backup_key: Option<String>,
    pub message: Option<String>,
}

impl SettingsState {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(None)),
            lock: Arc::new(Mutex::new(())),
            recovery_status: Arc::new(Mutex::new(SettingsRecoveryStatus::default())),
            runtime_generation: Arc::new(AtomicU64::new(0)),
            runtime_reconcile_lock: Arc::new(Mutex::new(())),
        }
    }

    pub(crate) async fn lock_runtime_reconcile(&self) -> tokio::sync::OwnedMutexGuard<()> {
        self.runtime_reconcile_lock.clone().lock_owned().await
    }

    fn reserve_runtime_reconcile(&self) -> u64 {
        self.runtime_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn is_runtime_generation_current(&self, generation: u64) -> bool {
        self.runtime_generation.load(Ordering::SeqCst) == generation
    }
}

pub fn create_default_settings() -> Settings {
    Settings {
        user_name: "用户".to_string(),
        distributed_enabled: false,
        distributed_ws_url: "".to_string(),
        distributed_vcp_key: "".to_string(),
        distributed_device_name: "VCPMobile".to_string(),
        vcp_server_url: "".to_string(),
        chat_endpoint_mode: ChatEndpointMode::Standard,
        vcp_api_key: "".to_string(),
        vcp_log_url: "".to_string(),
        vcp_log_key: "".to_string(),
        sync_server_url: "".to_string(),
        sync_http_url: "".to_string(),
        sync_token: "".to_string(),
        admin_username: "".to_string(),
        admin_password: "".to_string(),
        file_key: "".to_string(),
        topic_summary_model: "gemini-3.1-flash-lite".to_string(),
        sync_log_level: "INFO".to_string(),
        sync_prerender_enabled: false,
        enable_assistant: false,
        assistant_agent_id: "".to_string(),
        agent_order: vec![],
        group_order: vec![],
        current_theme_mode: Some("dark".to_string()),
        extra: serde_json::Value::Object(serde_json::Map::new()),
    }
}

fn deserialize_persisted_settings(content: &str) -> Result<(Settings, bool), serde_json::Error> {
    let mut value: serde_json::Value = serde_json::from_str(content)?;
    let Some(settings) = value.as_object_mut() else {
        return serde_json::from_value(value).map(|settings| (settings, false));
    };

    let mut migrated = false;
    if !settings.contains_key("chatEndpointMode") {
        let endpoint_mode = match settings
            .get("enableVcpToolInjection")
            .and_then(serde_json::Value::as_bool)
        {
            Some(true) => ChatEndpointMode::VcpTools,
            Some(false) => ChatEndpointMode::Standard,
            // Mobile 旧版本始终请求 ChatVCP；缺少旧布尔字段时保持升级前行为。
            None => ChatEndpointMode::VcpTools,
        };
        settings.insert(
            "chatEndpointMode".to_string(),
            serde_json::to_value(endpoint_mode)?,
        );
        migrated = true;
    }

    // 旧开关只用于一次性迁移；新枚举存在时由新枚举取得唯一所有权。
    if settings.remove("enableVcpToolInjection").is_some() {
        migrated = true;
    }

    serde_json::from_value(value).map(|settings| (settings, migrated))
}

#[tauri::command]
pub async fn read_settings<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, SettingsState>,
) -> Result<Settings, String> {
    let _lock = state.lock.lock().await;
    read_settings_locked(&app_handle, &state).await
}

async fn read_settings_locked<R: Runtime>(
    app_handle: &AppHandle<R>,
    state: &SettingsState,
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
        match deserialize_persisted_settings(&content) {
            Ok((settings, migrated)) => {
                if migrated {
                    let migrated_content =
                        serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
                    let mut tx = db_state.write_transaction("settings.migration").await?;
                    sqlx::query(
                        "UPDATE settings SET value = ?, updated_at = ? WHERE key = 'global'",
                    )
                    .bind(migrated_content)
                    .bind(crate::vcp_modules::infra::utils::now_millis())
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| format!("持久化设置迁移失败: {e}"))?;
                    tx.commit()
                        .await
                        .map_err(|e| format!("提交设置迁移失败: {e}"))?;
                }
                settings
            }
            Err(parse_error) => {
                recover_corrupt_settings(
                    db_state.inner(),
                    state,
                    &content,
                    &parse_error.to_string(),
                )
                .await?
            }
        }
    } else {
        create_default_settings()
    };

    *state.cache.lock().await = Some(settings.clone());
    Ok(settings)
}

async fn recover_corrupt_settings(
    db_state: &DbState,
    state: &SettingsState,
    original: &str,
    parse_error: &str,
) -> Result<Settings, String> {
    let defaults = create_default_settings();
    let default_content =
        serde_json::to_string_pretty(&defaults).map_err(|error| error.to_string())?;
    let now = crate::vcp_modules::infra::utils::now_millis();
    let backup_key = format!("global_corrupt_backup_{}_{}", now, uuid::Uuid::new_v4());

    let mut transaction = db_state.write_transaction("settings.recovery").await?;
    sqlx::query("INSERT INTO settings (key, value, updated_at) VALUES (?, ?, ?)")
        .bind(&backup_key)
        .bind(original)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| format!("保存损坏设置原文失败: {error}"))?;
    sqlx::query("UPDATE settings SET value = ?, updated_at = ? WHERE key = 'global'")
        .bind(&default_content)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| format!("恢复默认设置失败: {error}"))?;
    transaction
        .commit()
        .await
        .map_err(|error| format!("提交设置恢复事务失败: {error}"))?;

    *state.recovery_status.lock().await = SettingsRecoveryStatus {
        recovered_corrupt: true,
        backup_key: Some(backup_key.clone()),
        message: Some(format!(
            "检测到损坏的全局设置，原文已保存为 {backup_key}，并恢复为默认值: {parse_error}"
        )),
    };
    log::error!(
        "[Settings] Corrupt global settings recovered; original preserved as {}: {}",
        backup_key,
        parse_error
    );
    Ok(defaults)
}

#[tauri::command]
pub async fn get_settings_recovery_status(
    state: State<'_, SettingsState>,
) -> Result<SettingsRecoveryStatus, String> {
    Ok(state.recovery_status.lock().await.clone())
}

#[tauri::command]
#[allow(dead_code)] // Dormant compatibility asset; intentionally absent from generate_handler!.
pub async fn write_settings<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, SettingsState>,
    settings: Settings,
) -> Result<bool, String> {
    let _runtime_guard = state.lock_runtime_reconcile().await;
    let _lock = state.lock.lock().await;
    let _ = read_settings_locked(&app_handle, &state).await?;
    internal_write_settings(&app_handle, &state, &settings).await
}

#[tauri::command]
pub async fn update_settings<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, SettingsState>,
    updates: serde_json::Value,
) -> Result<Settings, String> {
    let _runtime_guard = state.lock_runtime_reconcile().await;
    let _lock = state.lock.lock().await;

    let current = read_settings_locked(&app_handle, &state).await?;
    let mut current_val = serde_json::to_value(&current).map_err(|e| e.to_string())?;

    if let Some(obj) = updates.as_object() {
        if let Some(current_obj) = current_val.as_object_mut() {
            for (k, v) in obj {
                current_obj.insert(k.clone(), v.clone());
            }
            current_obj.remove("enableVcpToolInjection");
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

    let content = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    let now = crate::vcp_modules::infra::utils::now_millis();

    let mut tx = db_state.write_transaction("settings.save").await?;
    sqlx::query("INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('global', ?, ?)")
        .bind(&content)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;

    let runtime_changes = {
        let old_cache = state.cache.lock().await;
        classify_runtime_settings_changes(old_cache.as_ref(), settings)
    };

    *state.cache.lock().await = Some(settings.clone());

    // VCPLog/Info 与分布式节点共用一个 generation owner。任务执行时重新读取
    // 最新 Settings；旧 generation 只能退出，不能在新设置之后提交运行时副作用。
    if runtime_changes.vcp_log_changed || runtime_changes.distributed_reconcile_required {
        let generation = state.reserve_runtime_reconcile();
        let concrete_app = app_handle.state::<tauri::AppHandle>().inner().clone();
        tauri::async_runtime::spawn(async move {
            reconcile_current_runtime_settings(concrete_app, generation).await;
        });
    }

    Ok(true)
}

async fn reconcile_current_runtime_settings(app_handle: AppHandle, generation: u64) {
    let state = app_handle.state::<SettingsState>();
    let _runtime_guard = state.lock_runtime_reconcile().await;
    if !state.is_runtime_generation_current(generation) {
        log::debug!("[Settings] Skipping stale runtime reconciliation generation={generation}");
        return;
    }

    let settings = match read_settings(app_handle.clone(), app_handle.state()).await {
        Ok(settings) => settings,
        Err(error) => {
            log::error!("[Settings] Runtime reconciliation could not read settings: {error}");
            return;
        }
    };

    if let Err(error) = crate::vcp_modules::vcp_log_service::init_vcp_log_connection_internal(
        app_handle.clone(),
        settings.vcp_log_url.clone(),
        settings.vcp_log_key.clone(),
    )
    .await
    {
        log::error!("[Settings] VCPLog runtime reconciliation failed: {error}");
    }
    if let Err(error) = crate::vcp_modules::vcp_info_service::init_vcp_info_connection_internal(
        app_handle.clone(),
        settings.vcp_log_url,
        settings.vcp_log_key,
    )
    .await
    {
        log::error!("[Settings] VCPInfo runtime reconciliation failed: {error}");
    }

    crate::vcp_modules::infra::lifecycle_manager::reconcile_distributed_node(&app_handle, true)
        .await;
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

#[cfg(test)]
mod tests {
    use super::*;

    use sqlx::Row;

    #[tokio::test]
    async fn newer_runtime_generation_supersedes_waiting_reconciler() {
        let state = Arc::new(SettingsState::new());
        let runtime_guard = state.lock_runtime_reconcile().await;
        let stale_generation = state.reserve_runtime_reconcile();
        let task_state = state.clone();
        let stale_task = tokio::spawn(async move {
            let _guard = task_state.lock_runtime_reconcile().await;
            task_state.is_runtime_generation_current(stale_generation)
        });

        let current_generation = state.reserve_runtime_reconcile();
        drop(runtime_guard);

        assert!(!stale_task.await.expect("stale task joins"));
        assert!(state.is_runtime_generation_current(current_generation));
    }

    #[tokio::test]
    async fn corrupt_settings_are_backed_up_before_defaults_replace_global() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at BIGINT NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO settings (key, value, updated_at) VALUES ('global', ?, 1)")
            .bind("{broken-json")
            .execute(&pool)
            .await
            .unwrap();

        let state = SettingsState::new();
        let db_state = DbState::new(pool.clone(), std::path::PathBuf::new());
        let recovered = recover_corrupt_settings(&db_state, &state, "{broken-json", "syntax")
            .await
            .unwrap();
        assert_eq!(recovered.user_name, "用户");

        let rows = sqlx::query("SELECT key, value FROM settings ORDER BY key")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|row| {
            row.get::<String, _>("key")
                .starts_with("global_corrupt_backup_")
                && row.get::<String, _>("value") == "{broken-json"
        }));
        let global = rows
            .iter()
            .find(|row| row.get::<String, _>("key") == "global")
            .unwrap();
        assert!(serde_json::from_str::<Settings>(&global.get::<String, _>("value")).is_ok());

        let status = state.recovery_status.lock().await.clone();
        assert!(status.recovered_corrupt);
        assert!(status.backup_key.is_some());
    }
}
