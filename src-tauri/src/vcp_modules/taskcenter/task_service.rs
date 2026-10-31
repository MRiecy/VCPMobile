//! 任务调度中心远端服务：`/admin_api/task-assistant/*` 的认证代理。
//!
//! 职责边界：认证、超时、响应大小上限、503（插件未加载）语义化、
//! taskId URL 编码（后端 createTaskId 可含中文）、全局开关的 read-modify-write。
//! 配置/状态 JSON 以 Value 透传（契约解释权在前端 Store 的归一化层）。

use crate::vcp_modules::infra::admin_api;
use crate::vcp_modules::settings_manager::{read_config, SettingsState};
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

fn build_url(
    settings: &crate::vcp_modules::settings_manager::Settings,
    suffix: &[&str],
) -> Result<String, String> {
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

    let body: Value =
        serde_json::from_slice(&bytes).map_err(|_| "服务器返回了不符合契约的 JSON".to_string())?;

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
) -> Result<std::sync::Arc<crate::vcp_modules::settings_manager::Settings>, String> {
    let settings = read_config(app_handle, settings_state).await?;
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
    send_json(admin_api::client_get(&settings, &url)?, DEFAULT_TIMEOUT).await
}

/// 拉取轻量状态（globalEnabled / activeTimerCount / tasks.runtime / 最近 20 条 history）。
#[tauri::command]
pub async fn task_get_status<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let url = build_url(&settings, &["task-assistant", "status"])?;
    send_json(admin_api::client_get(&settings, &url)?, DEFAULT_TIMEOUT).await
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
fn global_enabled_payload(response: Value, enabled: bool) -> Result<Value, String> {
    let mut config = response
        .get("config")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| "服务器返回的 config 结构异常，已中止写入".to_string())?;
    if !config.get("tasks").is_some_and(Value::is_array) {
        return Err("服务器返回的 tasks 结构异常，已中止写入".to_string());
    }
    config.insert("globalEnabled".to_string(), Value::Bool(enabled));
    Ok(Value::Object(config))
}

#[tauri::command]
pub async fn task_set_global_enabled<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    enabled: bool,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let config_url = build_url(&settings, &["task-assistant", "config"])?;

    let response = send_json(
        admin_api::client_get(&settings, &config_url)?,
        DEFAULT_TIMEOUT,
    )
    .await?;

    let config = global_enabled_payload(response, enabled)?;

    send_json(
        admin_api::client_post_json(&settings, &config_url, &config)?,
        DEFAULT_TIMEOUT,
    )
    .await
}

/// 新建任务（细粒度 POST；body 为完整 Task 契约，后端 sanitizeTaskInput 校验）。
#[tauri::command]
pub async fn task_create<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    task: Value,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let url = build_url(&settings, &["task-assistant", "tasks"])?;
    send_json(
        admin_api::client_post_json(&settings, &url, &task)?,
        DEFAULT_TIMEOUT,
    )
    .await
}

/// 更新任务（细粒度 PATCH，合并式；runtime/meta 由后端保留）。
#[tauri::command]
pub async fn task_update<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    task_id: String,
    task: Value,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let url = task_url(&settings, &task_id)?;
    send_json(
        admin_api::client_request(&settings, Method::PATCH, &url)?.json(&task),
        DEFAULT_TIMEOUT,
    )
    .await
}

/// 删除任务（细粒度 DELETE）。
#[tauri::command]
pub async fn task_delete<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    task_id: String,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let url = task_url(&settings, &task_id)?;
    send_json(
        admin_api::client_request(&settings, Method::DELETE, &url)?,
        DEFAULT_TIMEOUT,
    )
    .await
}

/// Agent 列表（目标选择器数据源）：GET /agent-assistant/config 的 agents[]。
/// 只回传选择器所需字段（chineseName/baseName/description/modelId），
/// 不透传 systemPrompt 等大字段。
#[tauri::command]
pub async fn task_agent_list<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let url = build_url(&settings, &["agent-assistant", "config"])?;
    let config = send_json(admin_api::client_get(&settings, &url)?, DEFAULT_TIMEOUT).await?;

    let agents = config
        .get("agents")
        .and_then(|a| a.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|agent| {
                    let chinese_name = agent.get("chineseName")?.as_str()?.trim().to_string();
                    if chinese_name.is_empty() {
                        return None;
                    }
                    Some(serde_json::json!({
                        "chineseName": chinese_name,
                        "baseName": agent.get("baseName").and_then(|v| v.as_str()).unwrap_or(""),
                        "description": agent.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                        "modelId": agent.get("modelId").and_then(|v| v.as_str()).unwrap_or(""),
                    }))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(Value::Array(agents))
}

/// 异步委托列表（运行中 + 最近记录）。
#[tauri::command]
pub async fn delegation_list<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let url = build_url(&settings, &["agent-assistant", "delegations"])?;
    send_json(admin_api::client_get(&settings, &url)?, DEFAULT_TIMEOUT).await
}

/// 请求取消异步委托。
#[tauri::command]
pub async fn delegation_cancel<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    delegation_id: String,
    reason: Option<String>,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let trimmed = delegation_id.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return Err("delegationId 为空或含控制字符".to_string());
    }
    let url = build_url(
        &settings,
        &["agent-assistant", "delegations", trimmed, "cancel"],
    )?;
    send_json(
        admin_api::client_post_json(
            &settings,
            &url,
            &serde_json::json!({ "reason": reason.unwrap_or_default() }),
        )?,
        DEFAULT_TIMEOUT,
    )
    .await
}

#[cfg(test)]
mod config_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn global_toggle_posts_only_the_preserved_inner_configuration() {
        let config = json!({
            "globalEnabled":false,"tasks":[{"id":"existing-task","enabled":false}],
            "settings":{"maxHistory":80},"extra":"keep"
        });
        let response =
            json!({"config":config,"availableTaskTypes":["custom_prompt"],"taskTemplates":{}});
        for enabled in [true, false] {
            let payload = global_enabled_payload(response.clone(), enabled).unwrap();
            let mut expected = config.clone();
            expected["globalEnabled"] = json!(enabled);
            assert_eq!(payload, expected);
            assert!(payload.get("config").is_none());
            assert!(payload.get("availableTaskTypes").is_none());
        }
        assert_eq!(
            global_enabled_payload(json!({"config":{"tasks":[]}}), true).unwrap(),
            json!({"tasks":[],"globalEnabled":true})
        );
    }

    #[test]
    fn damaged_task_lists_are_not_replaced_with_an_empty_default() {
        for response in [json!({}), json!({"config":false}), json!({"config":{}})] {
            assert!(global_enabled_payload(response, true).is_err());
        }
        for invalid in [Value::Null, json!(false), json!(0), json!(""), json!({})] {
            assert!(global_enabled_payload(json!({"config":{"tasks":invalid}}), false).is_err());
        }
    }
}
