//! 任务调度中心远端服务：`/admin_api/task-assistant/*` 的认证代理。
//!
//! 职责边界：认证、超时、响应大小上限、503（插件未加载）语义化、
//! taskId URL 编码（后端 createTaskId 可含中文）、全局开关的 read-modify-write。
//! 配置/状态 JSON 以 Value 透传（契约解释权在前端 Store 的归一化层）。

use crate::vcp_modules::infra::admin_api;
use crate::vcp_modules::settings_manager::{read_settings, SettingsState};
use reqwest::{Method, RequestBuilder, StatusCode};
use serde_json::Value;
use std::time::Duration;
use tauri::{AppHandle, Runtime, State};

/// 常规读/写总超时。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// 手动触发：后端派发会同步等待 Agent 响应（wakeUpAgent 上限 180s），留足余量。
const TRIGGER_TIMEOUT: Duration = Duration::from_secs(200);
/// 响应体上限：config 含 tasks + history(≤200) + 模板提示词，4 MiB 绰绰有余。
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

fn build_url(settings: &crate::vcp_modules::settings_manager::Settings, suffix: &[&str]) -> Result<String, String> {
    Ok(admin_api::admin_url(settings, suffix)?.to_string())
}

/// 为 taskId 做 URL 路径段编码（后端 id 形如 `task_巡航_xxx`，可含中文/空格）。
fn task_url(
    settings: &crate::vcp_modules::settings_manager::Settings,
    task_id: &str,
) -> Result<String, String> {
    let trimmed = task_id.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return Err("taskId 为空或含控制字符".to_string());
    }
    build_url(settings, &["task-assistant", "tasks", trimmed])
}

async fn send_json(request: RequestBuilder, timeout: Duration) -> Result<Value, String> {
    let resp = request
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| format!("任务调度请求失败: {e}"))?;

    let status = resp.status();
    if let Some(len) = resp.content_length() {
        if len > MAX_RESPONSE_BYTES {
            return Err(format!("任务调度响应体异常庞大（{len} 字节），已拒绝读取"));
        }
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取任务调度响应失败: {e}"))?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(format!(
            "任务调度响应体超过上限（{} 字节），已拒绝解析",
            bytes.len()
        ));
    }

    let body: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "服务器返回了不符合契约的 JSON".to_string())?;

    if status == StatusCode::SERVICE_UNAVAILABLE {
        let detail = body
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or("VCPTaskAssistant 插件未加载");
        return Err(format!("PLUGIN_UNAVAILABLE:{detail}"));
    }
    if !status.is_success() {
        let detail = body
            .get("error")
            .and_then(|e| e.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                "管理员凭据校验失败，请检查 设置 中的管理员账号与密码".to_string()
            }
            _ => format!("任务调度失败: {detail}"),
        });
    }
    Ok(body)
}

async fn settings_of<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
) -> Result<crate::vcp_modules::settings_manager::Settings, String> {
    let settings = read_settings(app_handle, settings_state).await?;
    admin_api::ensure_admin_config(&settings, "任务调度中心")?;
    Ok(settings)
}

/// 拉取完整配置（tasks + availableTaskTypes + taskTemplates + history）。
#[tauri::command]
pub async fn task_get_config<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let url = build_url(&settings, &["task-assistant", "config"])?;
    send_json(
        admin_api::client_get(&settings, &url)?,
        DEFAULT_TIMEOUT,
    )
    .await
}

/// 拉取轻量状态（globalEnabled / activeTimerCount / tasks.runtime / 最近 20 条 history）。
#[tauri::command]
pub async fn task_get_status<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let url = build_url(&settings, &["task-assistant", "status"])?;
    send_json(
        admin_api::client_get(&settings, &url)?,
        DEFAULT_TIMEOUT,
    )
    .await
}

/// 手动触发任务（忽略调度时间）。长耗时操作：后端同步等待 Agent 响应。
#[tauri::command]
pub async fn task_trigger<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    task_id: String,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let url = build_url(&settings, &["task-assistant", "trigger"])?;
    let trimmed = task_id.trim().to_string();
    if trimmed.is_empty() {
        return Err("taskId 不能为空".to_string());
    }
    send_json(
        admin_api::client_post_json(&settings, &url, &serde_json::json!({ "taskId": trimmed }))?,
        TRIGGER_TIMEOUT,
    )
    .await
}

/// 启用/禁用单个任务（细粒度 PATCH，避免全量覆盖保存）。
#[tauri::command]
pub async fn task_set_enabled<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    task_id: String,
    enabled: bool,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let url = task_url(&settings, &task_id)?;
    send_json(
        admin_api::client_request(&settings, Method::PATCH, &url)?
            .json(&serde_json::json!({ "enabled": enabled })),
        DEFAULT_TIMEOUT,
    )
    .await
}

/// 全局调度开关。
///
/// 安全注意：后端 `POST /task-assistant/config` 是**全量覆盖**语义——
/// 请求体不含 tasks 时会把任务列表清空。因此这里必须 read-modify-write：
/// 先 GET 完整 config，仅改写 globalEnabled 后整体回写。
#[tauri::command]
pub async fn task_set_global_enabled<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    enabled: bool,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let config_url = build_url(&settings, &["task-assistant", "config"])?;

    let mut config = send_json(
        admin_api::client_get(&settings, &config_url)?,
        DEFAULT_TIMEOUT,
    )
    .await?;

    let payload = config
        .get_mut("config")
        .and_then(|c| c.as_object_mut())
        .ok_or_else(|| "服务器返回的 config 结构异常，已中止写入".to_string())?;
    payload.insert("globalEnabled".to_string(), Value::Bool(enabled));

    send_json(
        admin_api::client_post_json(&settings, &config_url, &config)?,
        DEFAULT_TIMEOUT,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcp_modules::settings_manager::Settings;

    fn settings() -> Settings {
        Settings {
            vcp_server_url: "http://localhost:8080/v1/chat/completions".to_string(),
            admin_username: "admin".to_string(),
            admin_password: "secret".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn task_url_percent_encodes_chinese_id() {
        let url = task_url(&settings(), "task_forum_晨间巡航_123").unwrap();
        assert!(url.starts_with("http://localhost:8080/admin_api/task-assistant/tasks/"));
        assert!(!url.contains('晨'), "taskId 必须百分号编码: {url}");
        assert!(url.contains("%E6%99%A8"));
    }

    #[test]
    fn task_url_rejects_empty_and_control_chars() {
        assert!(task_url(&settings(), "").is_err());
        assert!(task_url(&settings(), "  ").is_err());
        assert!(task_url(&settings(), "abc\u{0007}").is_err());
    }

    #[test]
    fn task_url_preserves_safe_id() {
        let url = task_url(&settings(), "task_custom_prompt_demo_1724000000000").unwrap();
        assert!(url.ends_with("/tasks/task_custom_prompt_demo_1724000000000"));
    }
}
