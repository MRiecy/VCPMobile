//! Agent 管理远端服务：`/admin_api/agent-assistant/config` 与 `/admin_api/ai/models`
//! 的认证代理。
//!
//! 职责边界：认证、超时、响应大小上限、保存前的 read-modify-write 合并、
//! agents 写入前的防御性校验（chineseName/modelId 非空 + chineseName 唯一）。
//! 配置 JSON 以 Value 透传（契约解释权在前端 Store 的归一化层）。
//!
//! 上游事实（详见 plan/vcpmobile-more-tools-research/08 篇）：
//! - 后端 `POST /agent-assistant/config` 是**顶层浅合并**：未提交的顶层键保留，
//!   但 `agents` 数组一旦提交即整体替换，且无校验、无去重、无并发锁；
//! - 缺 chineseName/modelId 的条目保存成功但运行时被插件静默跳过；
//! - chineseName 重复时后写覆盖先写（运行时只剩一个）。

use crate::vcp_modules::infra::admin_api;
use crate::vcp_modules::settings_manager::{read_config, Settings, SettingsState};
use reqwest::{RequestBuilder, StatusCode};
use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;
use tauri::{AppHandle, Runtime, State};

/// 常规读/写总超时。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// 响应体上限：config 含 agents 的 systemPrompt 等长文本，4 MiB 绰绰有余。
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

fn build_url(settings: &Settings, suffix: &[&str]) -> Result<String, String> {
    Ok(admin_api::admin_url(settings, suffix)?.to_string())
}

async fn send_json(request: RequestBuilder, timeout: Duration) -> Result<Value, String> {
    let resp = request
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| format!("Agent 管理请求失败: {e}"))?;

    let status = resp.status();
    if let Some(len) = resp.content_length() {
        if len > MAX_RESPONSE_BYTES {
            return Err(format!(
                "Agent 管理响应体异常庞大（{len} 字节），已拒绝读取"
            ));
        }
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取 Agent 管理响应失败: {e}"))?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(format!(
            "Agent 管理响应体超过上限（{} 字节），已拒绝解析",
            bytes.len()
        ));
    }

    let body: Value =
        serde_json::from_slice(&bytes).map_err(|_| "服务器返回了不符合契约的 JSON".to_string())?;

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
            StatusCode::SERVICE_UNAVAILABLE => format!("PLUGIN_UNAVAILABLE:{detail}"),
            _ => format!("Agent 管理失败: {detail}"),
        });
    }
    Ok(body)
}

async fn settings_of<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
) -> Result<std::sync::Arc<Settings>, String> {
    let settings = read_config(app_handle, settings_state).await?;
    admin_api::ensure_admin_config(&settings, "Agent 管理")?;
    Ok(settings)
}

/// agents 写入前的防御性校验（后端无任何校验，这里守住底线）：
/// 每个条目 chineseName/modelId 必须非空；chineseName（trim 后）不得重复。
fn validate_agents(agents: &[Value]) -> Result<(), String> {
    let mut seen: HashSet<String> = HashSet::new();
    for (index, agent) in agents.iter().enumerate() {
        let label = format!("第 {} 个 Agent", index + 1);
        let chinese_name = agent
            .get("chineseName")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or("");
        if chinese_name.is_empty() {
            return Err(format!(
                "{label}缺少 chineseName（运行时被静默跳过），已中止保存"
            ));
        }
        let model_id = agent
            .get("modelId")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or("");
        if model_id.is_empty() {
            return Err(format!(
                "Agent「{chinese_name}」缺少 modelId（运行时被静默跳过），已中止保存"
            ));
        }
        if !seen.insert(chinese_name.to_string()) {
            return Err(format!(
                "chineseName「{chinese_name}」重复（运行时后写覆盖先写），已中止保存"
            ));
        }
    }
    Ok(())
}

/// 顶层浅合并：以服务端最新 config 为基，叠加上调用方提交的顶层键。
/// 与后端 POST 语义一致，但在写入前重新拉取可把并发竞态窗口缩到最小，
/// 并保证未知顶层键即使后端语义变化也能保留。
fn merge_top_level(base: &Value, incoming: &Value) -> Result<Value, String> {
    let mut merged = base
        .as_object()
        .cloned()
        .ok_or_else(|| "服务器返回的 config 结构异常，已中止写入".to_string())?;
    let incoming_obj = incoming
        .as_object()
        .ok_or_else(|| "提交的配置必须是 JSON 对象".to_string())?;
    if let Some(agents) = incoming_obj.get("agents") {
        let agents = agents.as_array().ok_or_else(|| "agents 必须是数组，已中止保存".to_string())?;
        validate_agents(agents)?;
    }
    for (key, value) in incoming_obj {
        merged.insert(key.clone(), value.clone());
    }
    Ok(Value::Object(merged))
}

/// 拉取完整 AgentAssistant 配置（原始 JSON，无 envelope）。
#[tauri::command]
pub async fn agentmgr_get_config<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let url = build_url(&settings, &["agent-assistant", "config"])?;
    send_json(admin_api::client_get(&settings, &url)?, DEFAULT_TIMEOUT).await
}

/// 保存配置（read-modify-write）：
/// 1. GET 服务端最新 config；2. 校验 agents；3. 顶层浅合并；4. POST 整体回写。
/// 入参 `config` 为调用方要提交的顶层键集合（通常含 agents + 编辑过的全局字段），
/// 未包含的顶层键以服务端现状为准保留。
#[tauri::command]
pub async fn agentmgr_save_config<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    config: Value,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let url = build_url(&settings, &["agent-assistant", "config"])?;

    let latest = send_json(admin_api::client_get(&settings, &url)?, DEFAULT_TIMEOUT).await?;
    let merged = merge_top_level(&latest, &config)?;

    send_json(
        admin_api::client_post_json(&settings, &url, &merged)?,
        DEFAULT_TIMEOUT,
    )
    .await
}

/// 可用模型列表（模型选择器数据源）：GET /admin_api/ai/models，
/// 投影为 id 字符串数组（OpenAI 格式 {data:[{id,...}]}，含语义路由虚拟模型）。
#[tauri::command]
pub async fn agentmgr_list_models<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let url = build_url(&settings, &["ai", "models"])?;
    let body = send_json(admin_api::client_get(&settings, &url)?, DEFAULT_TIMEOUT).await?;

    let ids = body
        .get("data")
        .and_then(|d| d.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|m| m.get("id").and_then(|v| v.as_str()))
                .map(|s| Value::String(s.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(Value::Array(ids))
}

#[cfg(test)]
mod config_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn agent_list_type_errors_cannot_replace_remote_agents() {
        let base = json!({"agents":[{"chineseName":"保留","modelId":"model"}],"maxHistoryRounds":7,"extra":"keep"});
        for invalid in [Value::Null, json!(false), json!(0), json!(""), json!({})] {
            assert!(merge_top_level(&base, &json!({"agents":invalid})).is_err());
        }
        assert!(merge_top_level(&base, &json!({"agents":[{"chineseName":"missing model"}]})).is_err());
        let updated = merge_top_level(&base, &json!({"maxHistoryRounds":12})).unwrap();
        assert_eq!(updated["agents"], base["agents"]);
        assert_eq!(updated["extra"], "keep");
        assert_eq!(updated["maxHistoryRounds"], 12);
        let cleared = merge_top_level(&base, &json!({"agents":[]})).unwrap();
        assert_eq!(cleared["agents"], json!([]), "an explicit empty list keeps its existing meaning");
        assert_eq!(cleared["extra"], "keep");
    }
}
