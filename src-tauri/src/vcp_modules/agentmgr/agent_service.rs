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
use crate::vcp_modules::settings_manager::{read_settings, Settings, SettingsState};
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
) -> Result<Settings, String> {
    let settings = read_settings(app_handle, settings_state).await?;
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

    if let Some(agents) = config.get("agents").and_then(|a| a.as_array()) {
        validate_agents(agents)?;
    }

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
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validate_agents_accepts_well_formed() {
        let agents = vec![
            json!({"chineseName": "小娜", "modelId": "gpt-4o"}),
            json!({"chineseName": "小冰", "modelId": "default"}),
        ];
        assert!(validate_agents(&agents).is_ok());
    }

    #[test]
    fn validate_agents_rejects_missing_chinese_name() {
        let agents = vec![json!({"modelId": "gpt-4o"})];
        let err = validate_agents(&agents).unwrap_err();
        assert!(err.contains("chineseName"));
    }

    #[test]
    fn validate_agents_rejects_blank_model_id() {
        let agents = vec![json!({"chineseName": "小娜", "modelId": "  "})];
        let err = validate_agents(&agents).unwrap_err();
        assert!(err.contains("小娜"));
        assert!(err.contains("modelId"));
    }

    #[test]
    fn validate_agents_rejects_duplicate_chinese_name_after_trim() {
        let agents = vec![
            json!({"chineseName": "小娜", "modelId": "a"}),
            json!({"chineseName": " 小娜 ", "modelId": "b"}),
        ];
        let err = validate_agents(&agents).unwrap_err();
        assert!(err.contains("重复"));
    }

    #[test]
    fn merge_top_level_preserves_unknown_keys_from_server() {
        let base = json!({"maxHistoryRounds": 7, "customFutureKey": {"x": 1}, "agents": []});
        let incoming = json!({"agents": [{"chineseName": "小娜"}], "maxHistoryRounds": 9});
        let merged = merge_top_level(&base, &incoming).unwrap();
        assert_eq!(merged["maxHistoryRounds"], json!(9));
        assert_eq!(merged["customFutureKey"], json!({"x": 1}));
        assert_eq!(merged["agents"], json!([{"chineseName": "小娜"}]));
    }

    #[test]
    fn merge_top_level_rejects_non_object_payloads() {
        assert!(merge_top_level(&json!({}), &json!([])).is_err());
        assert!(merge_top_level(&json!([]), &json!({})).is_err());
    }
}
