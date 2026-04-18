use crate::vcp_modules::agent_service::{read_agent_config_internal, AgentConfigState};
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::context_assembler_utils::assemble_history_for_vcp;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::message_service;
use crate::vcp_modules::vcp_client::{perform_vcp_request, ActiveRequests, VcpRequestPayload};
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, State};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentChatPayload {
    pub agent_id: String,
    pub topic_id: String,
    pub user_message: ChatMessage,
    pub vcp_url: String,
    pub vcp_api_key: String,
    pub thinking_message_id: String,
}

#[tauri::command]
pub async fn handle_agent_chat_message(
    app_handle: AppHandle,
    agent_state: State<'_, AgentConfigState>,
    db_state: State<'_, DbState>,
    active_requests: State<'_, ActiveRequests>,
    payload: AgentChatPayload,
) -> Result<Value, String> {
    let agent_id = payload.agent_id;
    let topic_id = payload.topic_id;
    let user_message = payload.user_message;
    let thinking_id = payload.thinking_message_id;

    // 1. 读取 Agent 配置
    let agent_config =
        read_agent_config_internal(&app_handle, &agent_state, &agent_id, Some(true)).await?;

    // 2. 将用户消息追加到数据库
    message_service::append_single_message(
        app_handle.clone(),
        &db_state.pool,
        &agent_id,
        "agent",
        topic_id.clone(),
        user_message.clone(),
    )
    .await?;

    // 3. 加载完整历史记录用于上下文组装
    let history = message_service::load_chat_history_internal(
        &app_handle,
        &agent_id,
        "agent",
        &topic_id,
        None, // 加载全部（或按需限制）
        None,
    )
    .await?;

    // 4. 使用公共工具组装上下文
    let mut messages = assemble_history_for_vcp(&history);

    // 5. 注入 System Prompt
    if !agent_config.system_prompt.is_empty() {
        let system_content = agent_config
            .system_prompt
            .replace("{{AgentName}}", &agent_config.name);
        messages.insert(
            0,
            json!({
                "role": "system",
                "content": system_content
            }),
        );
    }

    // 6. 构造 VCP 请求载荷
    let model_config = json!({
        "model": agent_config.model,
        "temperature": agent_config.temperature,
        "max_tokens": agent_config.max_output_tokens,
        "contextTokenLimit": agent_config.context_token_limit,
        "stream": true
    });

    let request_payload = VcpRequestPayload {
        vcp_url: payload.vcp_url,
        vcp_api_key: payload.vcp_api_key,
        messages,
        model_config,
        message_id: thinking_id.clone(),
        context: Some(json!({
            "agentId": agent_id,
            "topicId": topic_id
        })),
        stream_channel: Some("vcp-stream".to_string()),
    };

    // 7. 发起请求
    perform_vcp_request(&app_handle, active_requests.0.clone(), request_payload).await?;

    Ok(json!({ "status": "sent", "messageId": thinking_id }))
}
