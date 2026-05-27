use crate::vcp_modules::agent_service::{read_agent_config_internal, AgentConfigState};
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::context_assembler_utils::assemble_history_for_vcp;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::message_service;
use crate::vcp_modules::vcp_client::{
    perform_vcp_request, ActiveRequests, StreamEvent, VcpRequestPayload,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{ipc::Channel, AppHandle, State};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentChatPayload {
    pub agent_id: String,
    pub topic_id: String,
    pub user_message: ChatMessage,
    pub vcp_url: String,
    pub vcp_api_key: String,
}

#[tauri::command]
pub async fn handle_agent_chat_message(
    app_handle: AppHandle,
    agent_state: State<'_, AgentConfigState>,
    db_state: State<'_, DbState>,
    active_requests: State<'_, ActiveRequests>,
    payload: AgentChatPayload,
    stream_channel: Channel<crate::vcp_modules::vcp_client::StreamEvent>,
) -> Result<Value, String> {
    internal_process_agent_chat_message(
        app_handle,
        agent_state,
        db_state,
        active_requests,
        payload,
        stream_channel,
        true, // append_user_msg
    )
    .await
}

pub async fn internal_process_agent_chat_message(
    app_handle: AppHandle,
    agent_state: State<'_, AgentConfigState>,
    db_state: State<'_, DbState>,
    active_requests: State<'_, ActiveRequests>,
    payload: AgentChatPayload,
    stream_channel: Channel<crate::vcp_modules::vcp_client::StreamEvent>,
    append_user_msg: bool,
) -> Result<Value, String> {
    let agent_id = payload.agent_id;
    let topic_id = payload.topic_id;
    let user_message = payload.user_message;

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let thinking_id = format!("msg_{}_{}", agent_id, timestamp);

    // 1. 读取 Agent 配置
    let agent_config =
        read_agent_config_internal(&app_handle, &agent_state, &agent_id, Some(true)).await?;

    // 2. 只有在需要时才将用户消息追加到数据库 (重新生成时设为 false)
    if append_user_msg {
        message_service::append_single_message(
            app_handle.clone(),
            &db_state.pool,
            &agent_id,
            "agent",
            topic_id.clone(),
            user_message.clone(),
        )
        .await?;
    }

    // 3. 加载完整历史记录用于上下文组装
    let history = message_service::load_chat_history_internal(
        &app_handle,
        &agent_id,
        "agent",
        &topic_id,
        None, // 加载全部（或按需限制）
        None,
        true,
    )
    .await?;

    // 4. 使用公共工具组装上下文
    let mut messages = assemble_history_for_vcp(&history);

    // 5. 注入 System Prompt (优先使用移动端专用提示词)
    let effective_prompt = if !agent_config.mobile_system_prompt.is_empty() {
        &agent_config.mobile_system_prompt
    } else {
        &agent_config.system_prompt
    };
    if !effective_prompt.is_empty() {
        let system_content = effective_prompt.replace("{{AgentName}}", &agent_config.name);
        messages.insert(
            0,
            json!({
                "role": "system",
                "content": system_content
            }),
        );
    }

    // 6. 构造 VCP 请求载荷
    let mut model_config = json!({
        "model": agent_config.model,
        "max_tokens": agent_config.max_output_tokens,
        "contextTokenLimit": agent_config.context_token_limit,
        "stream": true
    });
    if agent_config.use_temperature {
        model_config["temperature"] = json!(agent_config.temperature);
    }

    let context = Some(json!({
        "agentId": agent_id,
        "topicId": topic_id,
        "agentName": agent_config.name
    }));

    let request_payload = VcpRequestPayload {
        vcp_url: payload.vcp_url,
        vcp_api_key: payload.vcp_api_key,
        messages,
        model_config,
        message_id: thinking_id.clone(),
        context: context.clone(),
    };

    // 在发起 VCP 请求前，向前端发射 thinking 事件以初始化气泡
    let _ = stream_channel.send(StreamEvent::thinking(thinking_id.clone(), context));

    // 7. 启动前台服务保活
    if let Err(e) =
        tauri_plugin_vcp_mobile::stream::start_stream_service_inner(&app_handle, &agent_config.name)
    {
        println!(
            "[AgentChatAppService] Failed to start streaming service: {}",
            e
        );
    }

    // 8. 发起请求
    let result = perform_vcp_request(
        &app_handle,
        active_requests.0.clone(),
        request_payload,
        Some(stream_channel.clone()),
    )
    .await;

    // 9. 停止前台服务
    if let Err(e) =
        tauri_plugin_vcp_mobile::stream::stop_stream_service_inner(&app_handle, &agent_config.name)
    {
        println!(
            "[AgentChatAppService] Failed to stop streaming service: {}",
            e
        );
    }

    // 8. 流式结束后（含中断），将最终内容预渲染并入库
    match result {
        Ok((res, is_aborted)) => {
            if let Some(full_content) = res["fullContent"].as_str() {
                let finish_reason = if is_aborted {
                    Some("cancelled_by_user".to_string())
                } else {
                    res["finishReason"].as_str().map(|s| s.to_string())
                };

                let final_msg = ChatMessage {
                    id: thinking_id.clone(),
                    role: "assistant".to_string(),
                    name: Some(agent_config.name.clone()),
                    content: full_content.to_string(),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64,
                    is_thinking: Some(false),
                    agent_id: Some(agent_id.clone()),
                    group_id: None,
                    topic_id: Some(topic_id.clone()),
                    is_group_message: Some(false),
                    finish_reason: finish_reason.clone(),
                    attachments: None,
                    blocks: None,
                    shell: None,
                    content_hash: None,
                };

                let patch_result = message_service::patch_single_message(
                    app_handle.clone(),
                    &db_state.pool,
                    &agent_id,
                    "agent",
                    topic_id.clone(),
                    final_msg,
                    false,
                )
                .await;
                let end_blocks = match &patch_result {
                    Ok(blocks) => Some(blocks.clone()),
                    Err(e) => {
                        println!(
                            "[AgentChatAppService] Failed to patch final message after stream: {}",
                            e
                        );
                        None
                    }
                };
                let _ = stream_channel.send(StreamEvent::end(
                    thinking_id.clone(),
                    Some(json!({
                        "agentId": agent_id,
                        "topicId": topic_id,
                        "agentName": agent_config.name
                    })),
                    finish_reason,
                    end_blocks,
                ));
            }
        }
        Err(e) => {
            println!("[AgentChatAppService] perform_vcp_request failed: {}", e);
        }
    }

    Ok(json!({ "status": "sent", "messageId": thinking_id }))
}
