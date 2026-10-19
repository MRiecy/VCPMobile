use crate::vcp_modules::agent_service::{read_agent_config_internal, AgentConfigState};
use crate::vcp_modules::chat::voice_service::VoiceReplyPolicy;
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::message_service;
use crate::vcp_modules::settings_manager::{read_config, SettingsState};
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
use crate::vcp_modules::vcp_client::{
    freeze_chat_connection, message_transport_request_id, perform_vcp_request_registered,
    ActiveRequestLease, ActiveRequests, ChatConnectionSnapshot, ChatRequestPurpose, StreamEvent,
    VcpRequestPayload,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{ipc::Channel, AppHandle, State};
use tauri_plugin_vcp_mobile::stream::{ForegroundLease, ForegroundTask};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentChatPayload {
    pub agent_id: String,
    pub topic_id: String,
    pub user_message: ChatMessage,
}

/// Register the real request before preparation awaits, without creating a
/// pending database message until configuration and context are ready.
pub(crate) struct PreparedAgentRequest {
    key: MessageKey,
    lease: ActiveRequestLease,
    cancellation_token: CancellationToken,
}

impl PreparedAgentRequest {
    pub(crate) fn acquire(
        app: &AppHandle,
        requests: &ActiveRequests,
        payload: &AgentChatPayload,
        channel: Option<Channel<StreamEvent>>,
        cancellation_token: CancellationToken,
    ) -> Result<Self, String> {
        let key = MessageKey::new(
            TopicKey::new("agent", &payload.agent_id, &payload.topic_id),
            format!("msg_{}", uuid::Uuid::new_v4()),
        );
        let (lease, cancellation_token) =
            ActiveRequestLease::try_acquire_with_cancellation(
                requests.0.clone(), key.clone(), cancellation_token,
            )?;
        lease.set_origin_user_message(&payload.user_message.id)?;
        lease.bind_output(app, channel)?;
        Ok(Self { key, lease, cancellation_token })
    }
}

#[tauri::command]
pub async fn handle_agent_chat_message(
    app_handle: AppHandle,
    agent_state: State<'_, AgentConfigState>,
    db_state: State<'_, DbState>,
    active_requests: State<'_, ActiveRequests>,
    settings_state: State<'_, SettingsState>,
    payload: AgentChatPayload,
    stream_channel: Channel<crate::vcp_modules::vcp_client::StreamEvent>,
) -> Result<Value, String> {
    let request = PreparedAgentRequest::acquire(
        &app_handle,
        &active_requests,
        &payload,
        Some(stream_channel.clone()),
        CancellationToken::new(),
    )?;
    let settings = read_config(app_handle.clone(), settings_state).await?;
    let connection = freeze_chat_connection(&settings, ChatRequestPurpose::Interactive)?;
    process_prepared_agent_chat_message(
        app_handle,
        agent_state,
        db_state,
        request,
        payload,
        connection,
        Some(stream_channel),
        false, // append_user_msg
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn internal_process_agent_chat_message(
    app_handle: AppHandle,
    agent_state: State<'_, AgentConfigState>,
    db_state: State<'_, DbState>,
    active_requests: State<'_, ActiveRequests>,
    payload: AgentChatPayload,
    connection: ChatConnectionSnapshot,
    stream_channel: Channel<crate::vcp_modules::vcp_client::StreamEvent>,
    append_user_msg: bool,
) -> Result<Value, String> {
    let request = PreparedAgentRequest::acquire(
        &app_handle,
        &active_requests,
        &payload,
        Some(stream_channel.clone()),
        CancellationToken::new(),
    )?;
    process_prepared_agent_chat_message(
        app_handle,
        agent_state,
        db_state,
        request,
        payload,
        connection,
        Some(stream_channel),
        append_user_msg,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn process_prepared_agent_chat_message(
    app_handle: AppHandle,
    agent_state: State<'_, AgentConfigState>,
    db_state: State<'_, DbState>,
    request: PreparedAgentRequest,
    payload: AgentChatPayload,
    connection: ChatConnectionSnapshot,
    stream_channel: Option<Channel<StreamEvent>>,
    append_user_msg: bool,
    reply_policy: Option<Arc<VoiceReplyPolicy>>,
) -> Result<Value, String> {
    let agent_id = payload.agent_id;
    let topic_id = payload.topic_id;
    let topic_key = TopicKey::new("agent", &agent_id, &topic_id);
    let user_message = payload.user_message;

    let PreparedAgentRequest { key: request_key, lease: _request_lease, cancellation_token } = request;
    let thinking_id = request_key.msg_id.clone();

    if cancellation_token.is_cancelled() {
        return Ok(json!({ "status": "cancelled", "messageId": thinking_id }));
    }

    // 1. 读取 Agent 配置
    let agent_config =
        read_agent_config_internal(&app_handle, &agent_state, &agent_id, Some(true)).await?;

    // 2. 只有在需要时才将用户消息追加到数据库 (重新生成时设为 false)
    if append_user_msg {
        if cancellation_token.is_cancelled() {
            return Ok(json!({ "status": "cancelled", "messageId": thinking_id }));
        }
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
        reply_policy.as_deref(),
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

    if cancellation_token.is_cancelled() {
        return Ok(json!({ "status": "cancelled", "messageId": thinking_id }));
    }
    message_service::begin_stream_message(
        &db_state,
        &request_key,
        Some(&agent_id),
        Some(&agent_config.name),
    )
    .await?;

    // A separate lease per operation also releases on cancellation of this future.
    // Acquisition reports native failure through the shared notification channel.
    let _foreground_lease =
        ForegroundLease::acquire(&app_handle, ForegroundTask::Stream, &agent_config.name).ok();

    // 在发起 VCP 请求前，向前端发射 thinking 事件以初始化气泡
    crate::vcp_modules::vcp_client::deliver_request_event(
        &app_handle,
        &request_key,
        stream_channel.clone(),
        StreamEvent::thinking(thinking_id.clone(), context.clone()),
    );

    let request_payload = VcpRequestPayload {
        vcp_url: connection.endpoint_url,
        vcp_api_key: connection.api_key,
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
        stream_channel.clone(),
        cancellation_token,
    )
    .await;

    // Keep the native lease through the durable finalizer (including errors).
    // 8. 流式结束后（含中断），将最终内容委派统一的 Finalizer 进行存盘与事件分发
    match result {
        Ok(response) => {
            if message_service::finalize_stream_message(
                app_handle.clone(),
                &db_state.pool,
                &request_key,
                response.full_content,
                response.is_aborted,
                response.finish_reason,
                stream_channel,
                Some(agent_id.clone()),
            )
            .await
            .is_err()
            {
                return Ok(json!({ "status": "recovery_pending", "messageId": thinking_id }));
            }
        }
        Err(failure) => {
            log::error!(
                "[AgentChatAppService] perform_vcp_request failed: {}",
                failure
            );
            let stop_pending = failure.is_stop_pending();
            let recovery_pending = failure.is_recovery_pending();
            let (error, partial_content) = failure.into_parts();
            if stop_pending {
                crate::vcp_modules::vcp_client::deliver_request_event(
                    &app_handle,
                    &request_key,
                    stream_channel,
                    StreamEvent {
                        r#type: "stopping".to_string(),
                        message_id: thinking_id.clone(),
                        context,
                        content: partial_content,
                        ..Default::default()
                    },
                );
                return Ok(json!({ "status": "stopping", "messageId": thinking_id }));
            }
            if recovery_pending {
                let notice = format!("后台生成连接中断，稍后返回前台可继续接管：{error}");
                let mut event = StreamEvent::error(thinking_id.clone(), context, notice.clone());
                event.content = partial_content;
                crate::vcp_modules::vcp_client::deliver_request_event(
                    &app_handle,
                    &request_key,
                    stream_channel,
                    event,
                );
                return Ok(json!({ "status": "recovery_pending", "messageId": thinking_id }));
            }
            if crate::vcp_modules::vcp_client::finalize_stream_error(
                &app_handle,
                &db_state.pool,
                &request_key,
                partial_content.unwrap_or_default(),
                error,
                stream_channel,
            )
            .await
            .is_err()
            {
                return Ok(json!({ "status": "recovery_pending", "messageId": thinking_id }));
            }
        }
    }

    Ok(json!({ "status": "sent", "messageId": thinking_id }))
}
