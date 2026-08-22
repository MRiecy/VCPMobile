// group_chat_application_service.rs: 编排群聊工作流
// 职责: 1. 读取配置 2. 保存消息 3. 决策发言者 4. 组装上下文 5. 执行 AI 调用 6. 发射事件

use crate::vcp_modules::agent_service::{read_agent_config_internal, AgentConfigState};
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_context_assembler::assemble_group_context;
use crate::vcp_modules::group_service::{read_group_config, GroupManagerState};
use crate::vcp_modules::group_speaking_policy::determine_naturerandom_speakers;
use crate::vcp_modules::message_service;
use crate::vcp_modules::vcp_client::{
    perform_vcp_request_registered, ActiveRequestLease, ActiveRequests, CancelledGroupTurns,
    StreamEvent, VcpRequestPayload,
};
use serde::Deserialize;
use serde_json::{json, Value};

use tauri::{ipc::Channel, AppHandle, Emitter, State};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupChatPayload {
    pub group_id: String,
    pub topic_id: String,
    pub user_message: ChatMessage,
    pub vcp_url: String,
    pub vcp_api_key: String,
}

pub struct GroupChatParams {
    pub group_id: String,
    pub topic_id: String,
    pub user_message: ChatMessage,
    pub vcp_url: String,
    pub vcp_api_key: String,
    pub stream_channel: Option<Channel<crate::vcp_modules::vcp_client::StreamEvent>>,
}

/// 单个发言者回合的共享上下文（引用借用，避免每棒重复 clone 配置）
struct GroupSpeakerTurnParams<'a> {
    app_handle: &'a AppHandle,
    db_pool: &'a sqlx::Pool<sqlx::Sqlite>,
    active_requests: &'a ActiveRequests,
    group_config: &'a crate::vcp_modules::group_types::GroupConfig,
    active_member_configs: &'a [crate::vcp_modules::agent_types::AgentConfig],
    group_id: &'a str,
    topic_id: &'a str,
    vcp_url: &'a str,
    vcp_api_key: &'a str,
    stream_channel: &'a Option<Channel<crate::vcp_modules::vcp_client::StreamEvent>>,
}

/// 执行群聊中单个发言者的完整回合：
/// 组装上下文 → 模型路由 → 注册租约 → begin_stream_message → thinking 事件 →
/// VCP 请求 → finalize 落盘与事件 / 失败时补发 error 终结事件。
///
/// 返回生成的助手消息（`None` 表示该棒失败且已通知前端）。
/// 结构化失败（`Err`）表示回合无法合法开始（DB/上下文装配错误等）。
async fn run_group_speaker_turn(
    params: &GroupSpeakerTurnParams<'_>,
    speaker: &crate::vcp_modules::agent_types::AgentConfig,
    full_history_for_context: &mut Vec<ChatMessage>,
) -> Result<Option<ChatMessage>, String> {
    let app_handle = params.app_handle;
    let db_pool = params.db_pool;
    let group_config_inner = params.group_config;
    let active_member_configs_inner = params.active_member_configs;
    let group_id = params.group_id;
    let topic_id = params.topic_id;
    let vcp_url = params.vcp_url;
    let vcp_api_key = params.vcp_api_key;
    let stream_channel = params.stream_channel;

    let agent_id = speaker.id.clone();
    let agent_name = speaker.name.clone();
    let message_id = format!("msg_group_{}", uuid::Uuid::new_v4());

    // 组装上下文
    let base_system_prompt =
        assemble_group_context(speaker, group_config_inner, active_member_configs_inner).await;

    // 动态路由决策：是否使用群组统一模型
    let model_to_use = if group_config_inner.use_unified_model {
        if let Some(ref unified) = group_config_inner.unified_model {
            if !unified.is_empty() {
                unified.clone()
            } else {
                speaker.model.clone()
            }
        } else {
            speaker.model.clone()
        }
    } else {
        speaker.model.clone()
    };

    // 构造请求载荷
    let mut model_config = json!({
        "model": model_to_use,
        "max_tokens": speaker.max_output_tokens,
        "contextTokenLimit": speaker.context_token_limit,
        "stream": speaker.stream_output
    });
    if speaker.use_temperature {
        model_config["temperature"] = json!(speaker.temperature);
    }

    // 组装上下文，委派上下文级联装配外观中枢，完成微观编织与宏观 Tavern 规则流水线拦截
    let invite_prompt_processed = group_config_inner
        .invite_prompt
        .as_ref()
        .map(|ip| ip.replace("{{VCPChatAgentName}}", &agent_name));

    let messages = crate::vcp_modules::context_assembler::orchestrate_chat_context(
        db_pool,
        full_history_for_context,
        topic_id,
        &agent_name,
        "group",
        base_system_prompt,
        invite_prompt_processed,
    )
    .await?;

    let context = Some(json!({
        "groupId": group_id,
        "topicId": topic_id,
        "agentId": agent_id,
        "isGroupMessage": true,
        "agentName": agent_name
    }));

    let request_payload = VcpRequestPayload {
        vcp_url: vcp_url.to_string(),
        vcp_api_key: vcp_api_key.to_string(),
        messages: messages.clone(),
        model_config: model_config.clone(),
        message_id: message_id.clone(),
        context: context.clone(),
        transport_request_id: None,
    };

    let (_request_lease, cancellation_token) =
        ActiveRequestLease::try_acquire(params.active_requests.0.clone(), message_id.clone())?;
    message_service::begin_stream_message(
        db_pool,
        group_id,
        "group",
        topic_id,
        &message_id,
        Some(&agent_id),
        Some(&agent_name),
    )
    .await?;

    if let Err(error) =
        tauri_plugin_vcp_mobile::stream::start_stream_service_inner(app_handle, &agent_name)
    {
        log::warn!("[GroupChatAppService] Failed to start streaming service: {error}");
    }

    // 发射 thinking 事件，让前端为当前接力的 Agent 创建思考占位消息
    if let Some(chan) = stream_channel {
        let _ = chan.send(StreamEvent::thinking(message_id.clone(), context.clone()));
    }

    let res_result = perform_vcp_request_registered(
        app_handle,
        request_payload,
        stream_channel.clone(),
        cancellation_token,
    )
    .await;

    // 停止前台服务
    if let Err(e) =
        tauri_plugin_vcp_mobile::stream::stop_stream_service_inner(app_handle, &agent_name)
    {
        log::warn!(
            "[GroupChatAppService] Failed to stop streaming service: {}",
            e
        );
    }

    if let Ok((res, is_aborted)) = res_result {
        if let Some(full_content) = res["fullContent"].as_str() {
            let finish_reason = if is_aborted {
                Some("cancelled_by_user".to_string())
            } else {
                res["finishReason"].as_str().map(|s| s.to_string())
            };

            // 1. 委托流终结器落盘与发射事件
            message_service::finalize_stream_message(
                app_handle.clone(),
                db_pool,
                group_id,
                "group",
                topic_id.to_string(),
                message_id.clone(),
                full_content.to_string(),
                is_aborted,
                finish_reason.clone(),
                stream_channel.clone(),
                Some(agent_id.clone()),
            )
            .await?;

            // 2. 将此棒生成的回复追加到内存上下文中，提供给接力赛的下一个 Agent
            let final_ts = crate::vcp_modules::infra::utils::now_millis() as u64;

            let ai_msg = ChatMessage {
                id: message_id,
                role: "assistant".to_string(),
                name: Some(agent_name),
                content: full_content.to_string(),
                timestamp: final_ts,
                updated_at: None,
                is_thinking: Some(false),
                agent_id: Some(agent_id.clone()),
                group_id: Some(group_id.to_string()),
                topic_id: Some(topic_id.to_string()),
                is_group_message: Some(true),
                finish_reason,
                attachments: None,
                blocks: None,
                shell: None,
                content_hash: None,
            };
            full_history_for_context.push(ai_msg.clone());
            return Ok(Some(ai_msg));
        }
        Ok(None)
    } else if let Err(e) = res_result {
        log::error!(
            "[GroupChatAppService] Error during agent {} response: {}",
            agent_id,
            e
        );
        // 与 agent/assistant 路径对齐：接力失败必须给前端终结事件，
        // 否则该 Agent 的 thinking 气泡永久悬挂，用户看到"无反应"
        if let Some(chan) = stream_channel {
            let _ = chan.send(StreamEvent::error(
                message_id.clone(),
                context.clone(),
                e.clone(),
            ));
        }
        Ok(None)
    } else {
        Ok(None)
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn internal_process_group_chat_message(
    app_handle: AppHandle,
    group_state: State<'_, GroupManagerState>,
    agent_state: State<'_, AgentConfigState>,
    db_state: State<'_, DbState>,
    active_requests: State<'_, ActiveRequests>,
    cancelled_turns: State<'_, CancelledGroupTurns>,
    params: GroupChatParams,
    append_user_msg: bool,
) -> Result<Value, String> {
    let stream_channel = params.stream_channel;
    let group_id = params.group_id;
    let topic_id = params.topic_id;
    let user_message = params.user_message;
    let vcp_url = params.vcp_url;
    let vcp_api_key = params.vcp_api_key;

    log::info!(
        "[GroupChatAppService] process_group_chat_message invoked for group: {}",
        group_id
    );

    // 0. 重置该话题的中断标记 (确保开启新回合)
    cancelled_turns.0.remove(&topic_id);

    // 1. 加载群组配置
    let group_config =
        read_group_config(app_handle.clone(), group_state.clone(), group_id.clone()).await?;

    // 2. 加载成员配置
    let mut active_member_configs = Vec::new();
    for member_id in &group_config.members {
        if let Ok(cfg) =
            read_agent_config_internal(&app_handle, &agent_state, member_id, Some(false)).await
        {
            active_member_configs.push(cfg);
        }
    }

    // 3. 异步追加用户消息 (重新生成时设为 false)
    if append_user_msg {
        message_service::append_single_message(
            app_handle.clone(),
            &db_state.pool,
            &group_id,
            "group",
            topic_id.clone(),
            user_message.clone(),
        )
        .await?;
    }

    // 为了给 AI 决策提供上下文，我们只轻量读取最新的 8 条纯文本和附件（不加载任何 UI 渲染数据）
    let recent_history_for_decision = message_service::load_chat_text_history_for_context(
        &app_handle,
        &topic_id,
        Some(8), // 限制上下文长度
        None,
        false, // include_extracted_text: 决策发言者不需要大体积的提取文本内容
    )
    .await?;

    // 4. 决策引擎：谁该说话？
    let speakers = if group_config.mode == "sequential" {
        active_member_configs.clone()
    } else if group_config.mode == "naturerandom" {
        determine_naturerandom_speakers(
            &active_member_configs,
            &recent_history_for_decision,
            &group_config,
            &user_message,
        )
    } else if group_config.mode == "invite_only" {
        // 邀请发言：用户消息不触发自动回复，发言由 invite_group_member_to_speak 显式驱动
        return Ok(json!({
            "status": "no_ai_response",
            "reason": "invite_only",
        }));
    } else {
        log::warn!(
            "[GroupChatAppService] Mode {} not implemented, ignoring.",
            group_config.mode
        );
        return Ok(json!({
            "status": "no_ai_response",
            "reason": "mode_not_implemented",
            "mode": group_config.mode,
        }));
    };

    if speakers.is_empty() {
        return Ok(json!({
            "status": "no_ai_response",
            "reason": "no_speakers",
        }));
    }

    // 提前加载轻量级全量纯文本和附件历史记录作为接力上下文的基础 (从底层隔离 UI 渲染反序列化和 Shell 计算)
    let mut full_history_for_context = message_service::load_chat_text_history_for_context(
        &app_handle,
        &topic_id,
        None, // 加载全部用于 VCP 上下文
        None,
        true, // include_extracted_text: 组装群聊上下文发送给 VCP 时需要包含附件提取文本内容
    )
    .await?;

    // 5. 串行异步调度 (约束：群聊内部必须串行)
    let mut final_new_msgs = Vec::new();

    for speaker in speakers {
        // 检查全局中断令牌：如果话题已被标记为取消，立即停止接力赛
        if cancelled_turns.0.contains(&topic_id) {
            log::info!(
                "[GroupChatAppService] Group turn for topic {} was cancelled. Breaking loop.",
                topic_id
            );
            break;
        }

        let turn_params = GroupSpeakerTurnParams {
            app_handle: &app_handle,
            db_pool: &db_state.pool,
            active_requests: &active_requests,
            group_config: &group_config,
            active_member_configs: &active_member_configs,
            group_id: &group_id,
            topic_id: &topic_id,
            vcp_url: &vcp_url,
            vcp_api_key: &vcp_api_key,
            stream_channel: &stream_channel,
        };

        if let Some(msg) =
            run_group_speaker_turn(&turn_params, &speaker, &mut full_history_for_context).await?
        {
            final_new_msgs.push(msg);
        }
    }

    // 6. 统一收集结果并最终发射信号
    let agent_ids: Vec<String> = final_new_msgs
        .iter()
        .filter_map(|m| m.agent_id.clone())
        .collect();

    // 确保无论如何都发射“回合结束”信号给前端
    let _ = app_handle.emit(
        "vcp-group-turn-finished",
        json!({
            "groupId": group_id,
            "topic_id": topic_id,
            "agentIds": agent_ids
        }),
    );

    // 回合结束，清理中断标记
    cancelled_turns.0.remove(&topic_id);

    Ok(json!({"status": "completed"}))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn handle_group_chat_message(
    app_handle: AppHandle,
    group_state: State<'_, GroupManagerState>,
    agent_state: State<'_, AgentConfigState>,
    db_state: State<'_, DbState>,
    active_requests: State<'_, ActiveRequests>,
    cancelled_turns: State<'_, CancelledGroupTurns>,
    payload: GroupChatPayload,
    stream_channel: Channel<crate::vcp_modules::vcp_client::StreamEvent>,
) -> Result<Value, String> {
    log::info!(
        "[GroupChatAppService] handle_group_chat_message invoked for group: {}",
        payload.group_id
    );

    internal_process_group_chat_message(
        app_handle,
        group_state,
        agent_state,
        db_state,
        active_requests,
        cancelled_turns,
        GroupChatParams {
            group_id: payload.group_id,
            topic_id: payload.topic_id,
            user_message: payload.user_message,
            vcp_url: payload.vcp_url,
            vcp_api_key: payload.vcp_api_key,
            stream_channel: Some(stream_channel),
        },
        false, // append_user_msg
    )
    .await
}

// ==========================================================================
// 邀请发言（invite_only 模式）
// ==========================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupInvitePayload {
    pub group_id: String,
    pub topic_id: String,
    pub agent_id: String,
    pub vcp_url: String,
    pub vcp_api_key: String,
}

/// 邀请指定群成员单人发言。
///
/// 与桌面端 `handleInviteAgentToSpeak` 对齐：invite_only 模式下 AI 不自动响应，
/// 发言只由本命令显式触发。invitePrompt 已在 `run_group_speaker_turn` 的
/// 上下文装配中织入（`{{VCPChatAgentName}}` 占位符替换）。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn invite_group_member_to_speak(
    app_handle: AppHandle,
    group_state: State<'_, GroupManagerState>,
    agent_state: State<'_, AgentConfigState>,
    db_state: State<'_, DbState>,
    active_requests: State<'_, ActiveRequests>,
    cancelled_turns: State<'_, CancelledGroupTurns>,
    payload: GroupInvitePayload,
    stream_channel: Channel<crate::vcp_modules::vcp_client::StreamEvent>,
) -> Result<Value, String> {
    log::info!(
        "[GroupChatAppService] invite_group_member_to_speak: group={}, topic={}, agent={}",
        payload.group_id,
        payload.topic_id,
        payload.agent_id
    );

    // 开启新回合前清理中断标记
    cancelled_turns.0.remove(&payload.topic_id);

    let group_config = read_group_config(
        app_handle.clone(),
        group_state.clone(),
        payload.group_id.clone(),
    )
    .await?;

    // 被邀成员必须属于该群，拒绝任何群外 agentId
    if !group_config.members.contains(&payload.agent_id) {
        return Err(format!(
            "成员 {} 不属于群组 {}",
            payload.agent_id, payload.group_id
        ));
    }

    // 被邀成员配置加载失败必须显式报错，不得静默跳过
    let speaker =
        read_agent_config_internal(&app_handle, &agent_state, &payload.agent_id, Some(false))
            .await
            .map_err(|e| format!("加载被邀请成员 {} 的配置失败: {e}", payload.agent_id))?;

    // 全部成员配置供群上下文组装使用
    let mut active_member_configs = Vec::new();
    for member_id in &group_config.members {
        if let Ok(cfg) =
            read_agent_config_internal(&app_handle, &agent_state, member_id, Some(false)).await
        {
            active_member_configs.push(cfg);
        }
    }

    // 轻量全量历史（含附件提取文本），与接力链路一致
    let mut full_history_for_context = message_service::load_chat_text_history_for_context(
        &app_handle,
        &payload.topic_id,
        None,
        None,
        true,
    )
    .await?;

    let stream_channel = Some(stream_channel);
    let turn_params = GroupSpeakerTurnParams {
        app_handle: &app_handle,
        db_pool: &db_state.pool,
        active_requests: &active_requests,
        group_config: &group_config,
        active_member_configs: &active_member_configs,
        group_id: &payload.group_id,
        topic_id: &payload.topic_id,
        vcp_url: &payload.vcp_url,
        vcp_api_key: &payload.vcp_api_key,
        stream_channel: &stream_channel,
    };

    let produced =
        run_group_speaker_turn(&turn_params, &speaker, &mut full_history_for_context).await?;

    let agent_ids: Vec<String> = produced.iter().filter_map(|m| m.agent_id.clone()).collect();
    let _ = app_handle.emit(
        "vcp-group-turn-finished",
        json!({
            "groupId": payload.group_id,
            "topic_id": payload.topic_id,
            "agentIds": agent_ids
        }),
    );

    cancelled_turns.0.remove(&payload.topic_id);
    Ok(json!({"status": "completed"}))
}
