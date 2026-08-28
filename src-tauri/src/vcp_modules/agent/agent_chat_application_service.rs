use crate::vcp_modules::agent_service::{read_agent_config_internal, AgentConfigState};
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::message_service;
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
use crate::vcp_modules::vcp_client::{
    message_transport_request_id, perform_vcp_request, perform_vcp_request_registered,
    ActiveRequestLease, ActiveRequests, StreamEvent, VcpRequestPayload,
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
        false, // append_user_msg
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
    let topic_key = TopicKey::new("agent", &agent_id, &topic_id);
    let user_message = payload.user_message;

    let thinking_id = format!("msg_{}", uuid::Uuid::new_v4());

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

    // 3. 加载轻量级纯文本和附件历史记录用于大模型上下文组装 (从底层隔离 UI 渲染反序列化和 Shell 计算)
    let history = message_service::load_chat_text_history_for_context(
        &app_handle,
        &topic_key,
        None,
        None,
        true, // include_extracted_text: 组装上下文发送给 VCP 时需要包含附件提取文本内容
    )
    .await?;

    // 4. 委派上下文级联装配外观中枢，完成微观编织与宏观 Tavern 规则流水线拦截
    let effective_prompt = if !agent_config.mobile_system_prompt.is_empty() {
        agent_config.mobile_system_prompt.clone()
    } else {
        agent_config.system_prompt.clone()
    };

    let messages = crate::vcp_modules::context_assembler::orchestrate_chat_context(
        &db_state.pool,
        &history,
        &topic_key,
        &agent_config.name,
        "agent",
        effective_prompt,
        None,
    )
    .await?;

    // 6. 构造 VCP 请求载荷
    let mut model_config = json!({
        "model": agent_config.model,
        "max_tokens": agent_config.max_output_tokens,
        "contextTokenLimit": agent_config.context_token_limit,
        "stream": agent_config.stream_output
    });
    if agent_config.use_temperature {
        model_config["temperature"] = json!(agent_config.temperature);
    }

    let context = Some(json!({
        "agentId": agent_id,
        "ownerId": agent_id,
        "ownerType": "agent",
        "topicId": topic_id,
        "agentName": agent_config.name
    }));

    let request_key = MessageKey::new(topic_key.clone(), thinking_id.clone());
    let (_request_lease, cancellation_token) =
        ActiveRequestLease::try_acquire(active_requests.0.clone(), request_key.clone())?;
    message_service::begin_stream_message(
        &db_state.pool,
        &request_key,
        Some(&agent_id),
        Some(&agent_config.name),
    )
    .await?;

    // All fallible preparation and owner creation are complete before acquiring foreground
    // service state, so every later model/coordinator outcome reaches the paired stop below.
    if let Err(error) =
        tauri_plugin_vcp_mobile::stream::start_stream_service_inner(&app_handle, &agent_config.name)
    {
        log::warn!("[AgentChatAppService] Failed to start streaming service: {error}");
    }

    // 在发起 VCP 请求前，向前端发射 thinking 事件以初始化气泡
    let _ = stream_channel.send(StreamEvent::thinking(thinking_id.clone(), context.clone()));

    let request_payload = VcpRequestPayload {
        vcp_url: payload.vcp_url,
        vcp_api_key: payload.vcp_api_key,
        messages,
        model_config,
        message_id: thinking_id.clone(),
        context: context.clone(),
        transport_request_id: Some(message_transport_request_id(&request_key)),
    };

    // 8. 发起请求
    let result = perform_vcp_request_registered(
        &app_handle,
        request_payload,
        Some(stream_channel.clone()),
        cancellation_token,
    )
    .await;

    // 9. 停止前台服务
    if let Err(e) =
        tauri_plugin_vcp_mobile::stream::stop_stream_service_inner(&app_handle, &agent_config.name)
    {
        log::warn!(
            "[AgentChatAppService] Failed to stop streaming service: {}",
            e
        );
    }

    // 8. 流式结束后（含中断），将最终内容委派统一的 Finalizer 进行存盘与事件分发
    match result {
        Ok((res, is_aborted)) => {
            if let Some(full_content) = res["fullContent"].as_str() {
                let finish_reason = if is_aborted {
                    Some("cancelled_by_user".to_string())
                } else {
                    res["finishReason"].as_str().map(|s| s.to_string())
                };

                message_service::finalize_stream_message(
                    app_handle.clone(),
                    &db_state.pool,
                    &request_key,
                    full_content.to_string(),
                    is_aborted,
                    finish_reason,
                    Some(stream_channel),
                    Some(agent_id.clone()),
                )
                .await?;
            }
        }
        Err(failure) => {
            log::error!(
                "[AgentChatAppService] perform_vcp_request failed: {}",
                failure
            );
            let (error, partial_content) = failure.into_parts();
            let _ = crate::vcp_modules::vcp_client::finalize_stream_error(
                &app_handle,
                &db_state.pool,
                &request_key,
                partial_content.unwrap_or_default(),
                error,
                Some(stream_channel),
            )
            .await?;
        }
    }

    Ok(json!({ "status": "sent", "messageId": thinking_id }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)] // DORMANT ASSET: floating-assistant bridge is retained but not registered.
pub struct AssistantChatPayload {
    pub agent_id: String,
    pub temp_messages: Vec<crate::vcp_modules::chat::topic_service::TempMessage>,
    pub vcp_url: String,
    pub vcp_api_key: String,
}

#[tauri::command]
#[allow(dead_code)] // DORMANT ASSET: floating-assistant bridge is retained but not registered.
pub async fn handle_assistant_chat_stream(
    app_handle: AppHandle,
    agent_state: State<'_, AgentConfigState>,
    active_requests: State<'_, ActiveRequests>,
    payload: AssistantChatPayload,
    stream_channel: Channel<crate::vcp_modules::vcp_client::StreamEvent>,
) -> Result<Value, String> {
    let agent_id = payload.agent_id;
    let temp_messages = payload.temp_messages;

    let thinking_id = format!("msg_{}", uuid::Uuid::new_v4());

    // 1. 读取 Agent 配置
    let agent_config =
        read_agent_config_internal(&app_handle, &agent_state, &agent_id, Some(true)).await?;

    // 2. 启动前台服务保活
    if let Err(e) =
        tauri_plugin_vcp_mobile::stream::start_stream_service_inner(&app_handle, &agent_config.name)
    {
        log::warn!(
            "[AssistantChatAppService] Failed to start streaming service early: {}",
            e
        );
    }

    // 3. 构造请求消息数组 (注入 System Prompt)
    let mut messages: Vec<Value> = Vec::new();

    let effective_prompt = if !agent_config.mobile_system_prompt.is_empty() {
        agent_config.mobile_system_prompt.clone()
    } else {
        agent_config.system_prompt.clone()
    };

    messages.push(json!({
        "role": "system",
        "content": effective_prompt
    }));

    for temp_msg in temp_messages {
        messages.push(json!({
            "role": temp_msg.role,
            "content": temp_msg.content
        }));
    }

    // 4. 构造 VCP 请求载荷
    let mut model_config = json!({
        "model": agent_config.model,
        "max_tokens": agent_config.max_output_tokens,
        "contextTokenLimit": agent_config.context_token_limit,
        "stream": agent_config.stream_output
    });
    if agent_config.use_temperature {
        model_config["temperature"] = json!(agent_config.temperature);
    }

    let context = Some(json!({
        "agentId": agent_id,
        "ownerId": agent_id,
        "ownerType": "agent",
        "topicId": "assistant_chat",
        "agentName": agent_config.name
    }));

    let request_key = MessageKey::new(
        TopicKey::new("agent", &agent_id, "assistant_chat"),
        thinking_id.clone(),
    );
    let request_payload = VcpRequestPayload {
        vcp_url: payload.vcp_url,
        vcp_api_key: payload.vcp_api_key,
        messages,
        model_config,
        message_id: thinking_id.clone(),
        context: context.clone(),
        transport_request_id: Some(message_transport_request_id(&request_key)),
    };

    // 发送 thinking 事件通知前端初始化气泡
    let _ = stream_channel.send(StreamEvent::thinking(thinking_id.clone(), context.clone()));

    // 5. 发起流式请求 (直接调用 perform_vcp_request，不存入 DB)
    let result = perform_vcp_request(
        &app_handle,
        active_requests.0.clone(),
        request_key,
        request_payload,
        Some(stream_channel.clone()),
    )
    .await;

    // 6. 停止前台服务
    if let Err(e) =
        tauri_plugin_vcp_mobile::stream::stop_stream_service_inner(&app_handle, &agent_config.name)
    {
        log::warn!(
            "[AssistantChatAppService] Failed to stop streaming service: {}",
            e
        );
    }

    // 7. 处理请求结果并补发流终结事件
    let final_ts = crate::vcp_modules::infra::utils::now_millis() as u64;
    match result {
        Ok((res, is_aborted)) => {
            if res["fullContent"].is_string() {
                let finish_reason = if is_aborted {
                    Some("cancelled_by_user".to_string())
                } else {
                    res["finishReason"].as_str().map(|s| s.to_string())
                };

                // 发送 end 事件让前端知道传输完毕
                let _ = stream_channel.send(StreamEvent::end(
                    thinking_id.clone(),
                    context,
                    finish_reason,
                    res["fullContent"].as_str().map(str::to_string),
                    None,
                    Some(final_ts),
                ));
            }
        }
        Err(failure) => {
            log::error!(
                "[AssistantChatAppService] perform_vcp_request failed: {}",
                failure
            );
            let (error, partial_content) = failure.into_parts();
            let final_content = crate::vcp_modules::vcp_client::stream_error_content(
                partial_content.as_deref().unwrap_or_default(),
                &error,
            );
            let _ = stream_channel.send(StreamEvent::end(
                thinking_id.clone(),
                context,
                Some("error".to_string()),
                Some(final_content),
                None,
                Some(final_ts),
            ));
        }
    }

    Ok(json!({ "status": "sent", "messageId": thinking_id }))
}
