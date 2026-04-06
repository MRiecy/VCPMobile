// settings_handlers.rs: 处理设置、主题相关的强类型指令
// 对齐原 settingsHandlers.js 逻辑，并增强移动端感知

use crate::vcp_modules::agent_service::{update_agent_config, AgentConfigState};
use crate::vcp_modules::app_settings_manager::{update_app_settings, AppSettingsState};
use serde::Deserialize;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, State};

#[derive(Debug, Deserialize)]
pub struct AvatarColorPayload {
    pub r#type: String, // "user" or "agent"
    pub id: Option<String>,
    pub color: String,
}

#[derive(Debug, Deserialize)]
pub struct UserAvatarPayload {
    pub buffer: Vec<u8>,
}

/// 保存头像颜色关联
/// 逻辑对齐: settingsHandlers.js -> save-avatar-color
/// 包含业务判断: 分辨是全局用户头像还是特定 Agent 头像
#[tauri::command]
pub async fn save_avatar_color(
    app_handle: AppHandle,
    agent_state: State<'_, AgentConfigState>,
    settings_state: State<'_, AppSettingsState>,
    payload: AvatarColorPayload,
) -> Result<bool, String> {
    if payload.r#type == "user" {
        let updates = serde_json::json!({
            "userAvatarCalculatedColor": payload.color
        });
        update_app_settings(app_handle, settings_state, updates).await?;
        Ok(true)
    } else if payload.r#type == "agent" {
        if let Some(agent_id) = payload.id {
            let updates = serde_json::json!({
                "avatarCalculatedColor": payload.color
            });
            update_agent_config(app_handle, agent_state, agent_id, updates).await?;
            Ok(true)
        } else {
            Err("Missing agent ID for color update".to_string())
        }
    } else {
        Err("Invalid type for avatar color".to_string())
    }
}

/// 保存全局用户头像
/// 逻辑对齐: settingsHandlers.js -> save-user-avatar (在 JS 中由 agentHandlers 处理)
#[tauri::command]
pub async fn save_user_avatar(
    app_handle: AppHandle,
    payload: UserAvatarPayload,
) -> Result<String, String> {
    let config_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    let avatar_path = config_dir.join("user_avatar.png");

    fs::write(&avatar_path, &payload.buffer).map_err(|e| e.to_string())?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    Ok(format!("{}?t={}", avatar_path.to_string_lossy(), timestamp))
}

/// 设置主题
/// 逻辑对齐: settingsHandlers.js -> set-theme
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

    // 注意: Tauri 的主题切换通常由系统或前端 CSS 处理，
    // 这里主要是同步持久化状态。

    Ok(true)
}

// --- 移动端感知指令 ---

/// 应用生命周期状态变更 (Active / Background)
#[tauri::command]
pub async fn notify_app_state(
    _app_handle: AppHandle,
    state: String, // "active", "background", "inactive"
) -> Result<(), String> {
    println!("[Rust] Mobile lifecycle state change: {}", state);
    // TODO: 可以在此处触发一些清理或优化逻辑
    Ok(())
}

/// 网络连接状态变更
#[tauri::command]
pub async fn notify_network_state(
    _app_handle: AppHandle,
    online: bool,
    r#type: String, // "wifi", "cellular", "none"
) -> Result<(), String> {
    println!(
        "[Rust] Network connection changed: online={}, type={}",
        online, r#type
    );
    Ok(())
}
