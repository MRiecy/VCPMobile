// group_chat_application_service.rs: 编排群聊工作流
// 职责: 1. 读取配置 2. 保存消息 3. 决策发言者 4. 组装上下文 5. 执行 AI 调用 6. 发射事件

use crate::vcp_modules::agent_service::{read_agent_config_internal, AgentConfigState};
#[cfg(target_os = "ios")]
use crate::vcp_modules::chat::ios_reply_task::ReplyTask;
use crate::vcp_modules::chat::voice_service::VoiceReplyPolicy;
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group::group_session::{
    GroupInputReservation, GroupReplyScope, GroupSession, ScopeCancellation,
};
use crate::vcp_modules::group_context_assembler::assemble_group_context;
use crate::vcp_modules::group_service::{read_group_config, GroupManagerState};
use crate::vcp_modules::group_speaking_policy::{
    append_jev_history, build_jev_request, determine_naturerandom_speakers,
    determine_sequential_speakers, fallback_jev_decision, parse_jev_decision,
    GroupMemberHistoryStats,
};
use crate::vcp_modules::group_types::GroupChatMode;
use crate::vcp_modules::infra::vcp_http_endpoint::ChatRequestPurpose;
use crate::vcp_modules::infra::vcp_ws_activity::ChatActivityGuard;
use crate::vcp_modules::message_service;
use crate::vcp_modules::settings_manager::{read_config, SettingsState};
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
use crate::vcp_modules::vcp_client::{
    freeze_chat_connection, message_transport_request_id, perform_vcp_request_registered,
    ActiveGroupTurnLease, ActiveGroupTurns, ActiveRequestLease, ActiveRequests,
    ChatConnectionSnapshot, StreamEvent, VcpRequestPayload, VcpResponse,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use tauri::{ipc::Channel, AppHandle, Manager, Runtime, State};
use tauri_plugin_vcp_mobile::stream::{ForegroundLease, ForegroundTask};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupChatPayload {
    pub group_id: String,
    pub topic_id: String,
    pub user_message: ChatMessage,
}

pub struct GroupChatParams {
    pub group_id: String,
    pub topic_id: String,
    pub user_message: ChatMessage,
    pub connection: ChatConnectionSnapshot,
    pub stream_channel: Option<Channel<crate::vcp_modules::vcp_client::StreamEvent>>,
}

/// 单个发言者回合的共享上下文（引用借用，避免每棒重复 clone 配置）
struct GroupSpeakerTurnParams<'a, R: Runtime> {
    app_handle: &'a AppHandle<R>,
    db_pool: &'a sqlx::Pool<sqlx::Sqlite>,
    active_requests: &'a ActiveRequests,
    group_turn: &'a ActiveGroupTurnLease,
    group_config: &'a crate::vcp_modules::group_types::GroupConfig,
    active_member_configs: &'a [crate::vcp_modules::agent_types::AgentConfig],
    group_id: &'a str,
    topic_id: &'a str,
    connection: &'a ChatConnectionSnapshot,
    stream_channel: &'a Option<Channel<crate::vcp_modules::vcp_client::StreamEvent>>,
    reply_policy: Option<&'a VoiceReplyPolicy>,
    reply_scope: Option<&'a Arc<GroupReplyScope>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GroupSpeakerOutcome {
    Settled,
    PreparationFailed,
    Cancelled,
    Stopping,
    RecoveryPending,
    Discarded,
}

impl GroupSpeakerOutcome {
    fn status(self) -> &'static str {
        match self {
            // A preparation failure is durably settled, but must not advance
            // the relay to another speaker with the same unusable attachment.
            Self::Settled | Self::PreparationFailed => "completed",
            Self::Cancelled => "cancelled",
            Self::Stopping => "stopping",
            Self::RecoveryPending => "recovery_pending",
            Self::Discarded => "discarded",
        }
    }
}

/// Automatic relay and explicit invitations share one turn owner and history.
/// Only a settled speaker may advance it. A durably saved attachment preparation
/// failure ends the turn; ordinary upstream errors retain their existing policy.
async fn run_group_speakers<R: Runtime>(
    params: &GroupSpeakerTurnParams<'_, R>,
    speakers: &[crate::vcp_modules::agent_types::AgentConfig],
    history: &mut Vec<ChatMessage>,
) -> Result<GroupSpeakerOutcome, String> {
    #[cfg(target_os = "ios")]
    let mut ios_reply = speakers.iter().any(|agent| agent.stream_output).then(|| {
        ReplyTask::begin(
            params.app_handle,
            &params.group_config.name,
            &speakers[0].name,
        )
    });
    for speaker in speakers {
        let outcome = run_group_speaker_turn(
            params,
            speaker,
            history,
            #[cfg(target_os = "ios")]
            ios_reply.as_ref(),
        )
        .await?;
        if outcome != GroupSpeakerOutcome::Settled {
            return Ok(outcome);
        }
    }
    #[cfg(target_os = "ios")]
    if let Some(task) = &mut ios_reply {
        task.success = true;
    }
    Ok(GroupSpeakerOutcome::Settled)
}

/// 执行群聊中单个发言者的完整回合：
/// 组装上下文 → 模型路由 → 注册租约 → begin_stream_message → thinking 事件 →
/// VCP 请求 → 由 durable finalizer 落盘并发送唯一终态事件。
///
/// 仅将已持久化的助手消息加入下一棒上下文，取消或已删除的任务不复活旧结果。
/// 结构化失败（`Err`）表示回合无法合法开始（DB/上下文装配错误等）。
async fn run_group_speaker_turn<R: Runtime>(
    params: &GroupSpeakerTurnParams<'_, R>,
    speaker: &crate::vcp_modules::agent_types::AgentConfig,
    full_history_for_context: &mut Vec<ChatMessage>,
    #[cfg(target_os = "ios")] ios_reply: Option<&ReplyTask>,
) -> Result<GroupSpeakerOutcome, String> {
    if params.group_turn.is_cancelled() {
        return Ok(GroupSpeakerOutcome::Cancelled);
    }
    let app_handle = params.app_handle;
    let db_pool = params.db_pool;
    let group_config_inner = params.group_config;
    let active_member_configs_inner = params.active_member_configs;
    let group_id = params.group_id;
    let topic_id = params.topic_id;
    let topic_key = TopicKey::new("group", group_id, topic_id);
    let connection = params.connection;
    let stream_channel = params.stream_channel;

    let agent_id = speaker.id.clone();
    let agent_name = speaker.name.clone();
    #[cfg(target_os = "ios")]
    if let Some(task) = ios_reply {
        task.update(&agent_name, 0);
    }
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

    let history_start = group_context_limit(group_config_inner)
        .map(|limit| full_history_for_context.len().saturating_sub(limit))
        .unwrap_or(0);
    let messages = crate::vcp_modules::context_assembler::orchestrate_chat_context(
        app_handle.state::<DbState>().inner(),
        &full_history_for_context[history_start..],
        &topic_key,
        &agent_name,
        base_system_prompt,
        invite_prompt_processed,
        params.reply_policy,
    )
    .await?;

    // Preparation may await IO; a stop received during it must not start the
    // next speaker or create another upstream request.
    if params.group_turn.is_cancelled()
        || params
            .reply_scope
            .is_some_and(|scope| scope.cancellation.is_cancelled())
    {
        return Ok(GroupSpeakerOutcome::Cancelled);
    }

    let context = Some(json!({
        "groupId": group_id,
        "ownerId": group_id,
        "ownerType": "group",
        "topicId": topic_id,
        "agentId": agent_id,
        "isGroupMessage": true,
        "agentName": agent_name
    }));

    let request_key = MessageKey::new(topic_key, message_id.clone());
    let request_payload = VcpRequestPayload {
        vcp_url: connection.endpoint_url.clone(),
        vcp_api_key: connection.api_key.clone(),
        messages,
        model_config,
        message_id: message_id.clone(),
        context: context.clone(),
        transport_request_id: Some(message_transport_request_id(&request_key)),
    };

    let scope_cancellation =
        ScopeCancellation::new(params.group_turn.child_token(), params.reply_scope);
    let (_request_lease, cancellation_token) = ActiveRequestLease::try_acquire_with_cancellation(
        params.active_requests.0.clone(),
        request_key.clone(),
        scope_cancellation.token.clone(),
    )?;
    if let Some(origin) = params
        .reply_scope
        .map(|scope| scope.origin.clone())
        .or_else(|| params.group_turn.origin_user_message())
    {
        _request_lease.set_origin_user_message(&origin)?;
    }
    let db_state = app_handle.state::<DbState>();
    message_service::begin_stream_message(
        &db_state,
        &request_key,
        Some(&agent_id),
        Some(&agent_name),
    )
    .await?;

    let _chat_activity = speaker
        .stream_output
        .then(|| ChatActivityGuard::acquire(app_handle));
    let _foreground_lease =
        ForegroundLease::acquire(app_handle, ForegroundTask::Stream, &agent_name).ok();

    // 发射 thinking 事件，让前端为当前接力的 Agent 创建思考占位消息
    crate::vcp_modules::vcp_client::deliver_request_event(
        app_handle,
        &request_key,
        stream_channel.clone(),
        StreamEvent::thinking(message_id.clone(), context.clone()),
    );

    // Once begin_stream_message has committed, even an already cancelled
    // speaker finishes through the durable finalizer rather than aborting DB IO.
    let res_result = if cancellation_token.is_cancelled() {
        Ok(VcpResponse::cancelled())
    } else {
        perform_vcp_request_registered(
            app_handle,
            request_payload,
            stream_channel.clone(),
            cancellation_token,
            #[cfg(target_os = "ios")]
            ios_reply,
        )
        .await
    };

    match res_result {
        Ok(response) => {
            // 1. 委托流终结器落盘与发射事件
            let finalized = message_service::finalize_stream_message(
                app_handle.clone(),
                db_pool,
                &request_key,
                response.full_content,
                response.is_aborted,
                response.finish_reason,
                stream_channel.clone(),
                Some(agent_id.clone()),
            )
            .await;
            let finalized = match finalized {
                Ok(Some(message)) => message,
                Ok(None) => return Ok(GroupSpeakerOutcome::Discarded),
                Err(_) => return Ok(GroupSpeakerOutcome::RecoveryPending),
            };

            // 2. 将此棒生成的回复追加到内存上下文中，提供给接力赛的下一个 Agent
            let ai_msg = ChatMessage {
                id: message_id,
                role: "assistant".to_string(),
                name: Some(agent_name),
                content: finalized.content,
                timestamp: finalized.timestamp,
                updated_at: None,
                is_thinking: Some(false),
                agent_id: Some(agent_id.clone()),
                group_id: Some(group_id.to_string()),
                topic_id: Some(topic_id.to_string()),
                is_group_message: Some(true),
                finish_reason: Some(finalized.finish_reason),
                attachments: None,
                blocks: None,
                shell: None,
                content_hash: None,
            };
            full_history_for_context.push(ai_msg);
            Ok(if response.is_aborted {
                GroupSpeakerOutcome::Cancelled
            } else {
                GroupSpeakerOutcome::Settled
            })
        }
        Err(failure) => {
            log::error!(
                "[GroupChatAppService] Error during agent {} response: {}",
                agent_id,
                failure
            );
            let stop_pending = failure.is_stop_pending();
            let recovery_pending = failure.is_recovery_pending();
            let preparation_failure = failure.is_preparation_failure();
            let (error, partial_content) = failure.into_parts();
            #[cfg(target_os = "ios")]
            let stream_failure =
                (speaker.stream_output && !preparation_failure).then(|| error.clone());
            if stop_pending {
                // A separately stopped speaker may still own an unconfirmed helper
                // request. Do not start another speaker until that request settles.
                params.group_turn.cancel();
                crate::vcp_modules::vcp_client::deliver_request_event(
                    app_handle,
                    &request_key,
                    stream_channel.clone(),
                    StreamEvent {
                        r#type: "stopping".to_string(),
                        message_id,
                        context,
                        content: partial_content,
                        ..Default::default()
                    },
                );
                return Ok(GroupSpeakerOutcome::Stopping);
            }
            if recovery_pending {
                let notice = format!("后台生成连接中断，稍后返回前台可继续接管：{error}");
                let mut event = StreamEvent::error(message_id, context, notice.clone());
                event.content = partial_content;
                crate::vcp_modules::vcp_client::deliver_request_event(
                    app_handle,
                    &request_key,
                    stream_channel.clone(),
                    event,
                );
                return Ok(GroupSpeakerOutcome::RecoveryPending);
            }
            let finalized = crate::vcp_modules::vcp_client::finalize_stream_error(
                app_handle,
                db_pool,
                &request_key,
                partial_content.unwrap_or_default(),
                error,
                stream_channel.clone(),
                Some(agent_id.clone()),
            )
            .await;
            #[cfg(target_os = "ios")]
            if let (Ok(Some(_)), Some(reason)) = (&finalized, stream_failure) {
                // Once this SSE failed, the group must not start another speaker.
                return Err(reason);
            }
            Ok(match finalized {
                Ok(Some(_)) if preparation_failure => GroupSpeakerOutcome::PreparationFailed,
                Ok(Some(_)) => GroupSpeakerOutcome::Settled,
                Ok(None) => GroupSpeakerOutcome::Discarded,
                Err(_) => GroupSpeakerOutcome::RecoveryPending,
            })
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn internal_process_group_chat_message(
    app_handle: AppHandle,
    group_state: State<'_, GroupManagerState>,
    agent_state: State<'_, AgentConfigState>,
    db_state: State<'_, DbState>,
    active_requests: State<'_, ActiveRequests>,
    active_group_turns: State<'_, ActiveGroupTurns>,
    params: GroupChatParams,
    append_user_msg: bool,
) -> Result<Value, String> {
    let topic_key = TopicKey::new("group", &params.group_id, &params.topic_id);
    if let Some(session) = active_group_turns.session(&topic_key) {
        let reservation = session.reserve()?;
        if append_user_msg {
            message_service::append_single_message(
                app_handle.clone(),
                &db_state.pool,
                &params.group_id,
                "group",
                params.topic_id.clone(),
                params.user_message.clone(),
            )
            .await?;
        }
        submit_group_session_input(
            &app_handle,
            reservation,
            &params.user_message,
            None,
            CancellationToken::new(),
        )
        .await?;
        return Ok(json!({"status":"accepted"}));
    }
    let group_turn = active_group_turns.try_acquire(topic_key)?;
    group_turn.set_origin_user_message(&params.user_message.id)?;
    group_turn.bind_optional_output(&app_handle, params.stream_channel.clone())?;
    process_prepared_group_chat_message(
        app_handle,
        group_state,
        agent_state,
        db_state,
        active_requests,
        group_turn,
        params,
        append_user_msg,
        None,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn process_prepared_group_chat_message(
    app_handle: AppHandle,
    group_state: State<'_, GroupManagerState>,
    agent_state: State<'_, AgentConfigState>,
    db_state: State<'_, DbState>,
    active_requests: State<'_, ActiveRequests>,
    group_turn: ActiveGroupTurnLease,
    params: GroupChatParams,
    append_user_msg: bool,
    reply_policy: Option<Arc<VoiceReplyPolicy>>,
    voice_cancellation: Option<CancellationToken>,
) -> Result<Value, String> {
    let stream_channel = params.stream_channel;
    let group_id = params.group_id;
    let topic_id = params.topic_id;
    let user_message = params.user_message;
    let connection = params.connection;
    let topic_key = TopicKey::new("group", &group_id, &topic_id);
    if group_turn.is_cancelled() {
        return Ok(json!({ "status": "cancelled" }));
    }

    log::info!(
        "[GroupChatAppService] process_group_chat_message invoked for group: {}",
        group_id
    );

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
        if group_turn.is_cancelled() {
            return Ok(json!({ "status": "cancelled" }));
        }
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

    if group_turn.is_cancelled() {
        return Ok(json!({ "status": "cancelled" }));
    }

    // 强类型模式在配置边界已经规范化，不再保留未知模式的静默分支。
    let speakers = match group_config.effective_mode() {
        GroupChatMode::Jev => {
            let session = group_turn.start_session("user_message", false)?;
            let reservation = session.reserve()?;
            let scope = submit_group_session_input(
                &app_handle,
                reservation,
                &user_message,
                reply_policy.clone(),
                voice_cancellation.unwrap_or_else(|| group_turn.child_token()),
            )
            .await?;
            spawn_jev_session(
                app_handle.clone(),
                group_turn,
                session,
                group_id,
                topic_id,
                connection,
            );
            if reply_policy.is_some() {
                return scope.wait().await;
            }
            return Ok(json!({"status":"accepted"}));
        }
        GroupChatMode::Sequential => {
            determine_sequential_speakers(&active_member_configs, &group_config)
        }
        GroupChatMode::NaturalRandom => {
            // 八条历史和当前用户消息，不加载 UI 渲染块或大体积提取文本。
            let recent_history_for_decision = message_service::load_chat_text_history_for_context(
                &app_handle,
                &topic_key,
                Some(9),
                None,
                false,
            )
            .await?;
            determine_naturerandom_speakers(
                &active_member_configs,
                &recent_history_for_decision,
                &group_config,
                &user_message,
            )
        }
        GroupChatMode::InviteOnly => {
            return Ok(json!({"status":"no_ai_response", "reason":"invite_only"}));
        }
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
        &topic_key,
        group_context_limit(&group_config),
        None,
        true, // include_extracted_text: 组装群聊上下文发送给 VCP 时需要包含附件提取文本内容
    )
    .await?;

    // 5. 串行异步调度 (约束：群聊内部必须串行)
    let static_scope = voice_cancellation.map(|cancellation| {
        GroupReplyScope::new(
            user_message.id.clone(),
            cancellation,
            reply_policy.clone(),
            || {},
            || {},
        )
    });
    let turn_params = GroupSpeakerTurnParams {
        app_handle: &app_handle,
        db_pool: &db_state.pool,
        active_requests: &active_requests,
        group_turn: &group_turn,
        group_config: &group_config,
        active_member_configs: &active_member_configs,
        group_id: &group_id,
        topic_id: &topic_id,
        connection: &connection,
        stream_channel: &stream_channel,
        reply_policy: reply_policy.as_deref(),
        reply_scope: static_scope.as_ref(),
    };
    let outcome =
        run_group_speakers(&turn_params, &speakers, &mut full_history_for_context).await?;

    // The turn lease publishes completion through its current output consumer,
    // including early returns and failures.
    Ok(json!({"status": outcome.status()}))
}

fn group_context_limit(config: &crate::vcp_modules::group_types::GroupConfig) -> Option<usize> {
    config
        .local_settings
        .enable_context_message_window
        .then_some(config.local_settings.context_message_window_size as usize)
}

async fn load_group_member_history_stats(
    pool: &sqlx::SqlitePool,
    key: &TopicKey,
) -> Result<BTreeMap<String, GroupMemberHistoryStats>, String> {
    // Count distance in the same stable, committed message order as model
    // history, before filtering to speakers. User/system messages count too.
    let rows = sqlx::query_as::<_, (String, i64, i64)>(
        "WITH ordered AS (
            SELECT m.agent_id, m.role,
                   ROW_NUMBER() OVER (ORDER BY m.timestamp DESC, m.msg_id DESC) - 1 AS distance
            FROM messages m
            WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ? AND m.deleted_at IS NULL
              AND NOT EXISTS (
                SELECT 1 FROM active_generations ag
                WHERE ag.owner_type = m.owner_type AND ag.owner_id = m.owner_id
                  AND ag.topic_id = m.topic_id AND ag.msg_id = m.msg_id
              )
         )
         SELECT agent_id, COUNT(*), MIN(distance)
         FROM ordered WHERE role = 'assistant' AND agent_id IS NOT NULL GROUP BY agent_id",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(rows
        .into_iter()
        .map(|(id, speech_count, last_speech_distance)| {
            (
                id,
                GroupMemberHistoryStats {
                    speech_count,
                    last_speech_distance,
                },
            )
        })
        .collect())
}

fn extract_group_mentions(content: &str, members: &[(String, String)]) -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();
    let mut candidates = members
        .iter()
        .filter_map(|(id, name)| {
            let name = name.trim().to_lowercase();
            (!name.is_empty() && names.insert(name.clone())).then_some((name, id.as_str()))
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Vec::new();
    }
    // Match the existing composer precedence: one longest name per occurrence,
    // then preserve appearance order and invite each member once.
    candidates.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
    let text = content.to_lowercase();
    let mut offset = 0;
    let mut seen = std::collections::BTreeSet::new();
    let mut mentions = Vec::new();
    while let Some(relative) = text[offset..].find('@') {
        let start = offset + relative + 1;
        if let Some((name, id)) = candidates
            .iter()
            .find(|(name, _)| text[start..].starts_with(name))
        {
            if seen.insert(*id) {
                mentions.push((*id).to_owned());
            }
            offset = start + name.len();
        } else {
            offset = start;
        }
    }
    mentions
}

pub(crate) async fn submit_group_session_input(
    app: &AppHandle,
    reservation: GroupInputReservation,
    message: &ChatMessage,
    policy: Option<Arc<VoiceReplyPolicy>>,
    cancellation: CancellationToken,
) -> Result<Arc<GroupReplyScope>, String> {
    let session = reservation.session.clone();
    let origin = message.id.clone();
    let turn_id = session.turn_id.clone();
    let feed_app = app.clone();
    let finish_origin = origin.clone();
    let finish_turn = turn_id.clone();
    let finish_cancel = cancellation.clone();
    let cancel_app = app.clone();
    let cancel_origin = origin.clone();
    let scope = GroupReplyScope::new(
        origin.clone(),
        cancellation,
        policy,
        move || {
            crate::vcp_modules::chat::voice_service::feed_chat_event(
                &feed_app,
                &StreamEvent {
                    r#type: "group_turn".into(),
                    context: Some(
                        json!({"turnId":finish_turn,"originUserMessageId":finish_origin,"ownerType":"group"}),
                    ),
                    chunk: Some(json!({"active":false})),
                    ..Default::default()
                },
            );
            if finish_cancel.is_cancelled() {
                let app = feed_app.clone();
                let origin = finish_origin.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = crate::vcp_modules::chat::voice_service::stop_chat_reply_by_origin(
                        &app, &origin,
                    )
                    .await;
                });
            }
        },
        move || {
            let app = cancel_app.clone();
            let origin = cancel_origin.clone();
            tauri::async_runtime::spawn(async move {
                let _ = crate::vcp_modules::chat::voice_service::stop_chat_reply_by_origin(
                    &app, &origin,
                )
                .await;
            });
        },
    );
    // The text is already durably stored by the normal typed/voice entry point.
    // Mentions only use actual members; no user-facing message is fabricated.
    let mut members = Vec::new();
    let key = app
        .state::<ActiveGroupTurns>()
        .topic_for_session(&session)
        .ok_or("群聊执行已结束")?;
    let config = read_group_config(
        app.clone(),
        app.state::<GroupManagerState>(),
        key.owner_id.clone(),
    )
    .await?;
    for id in &config.members {
        if let Ok(member) =
            read_agent_config_internal(app, &app.state::<AgentConfigState>(), id, Some(false)).await
        {
            members.push((id.clone(), member.name));
        }
    }
    let mentions = extract_group_mentions(&message.content, &members);
    crate::vcp_modules::chat::voice_service::feed_chat_event(
        app,
        &StreamEvent {
            r#type: "group_turn".into(),
            context: Some(
                json!({"turnId":turn_id,"originUserMessageId":origin,"ownerType":"group"}),
            ),
            chunk: Some(json!({"active":true})),
            ..Default::default()
        },
    );
    if let Err(error) = reservation.commit(scope.clone(), &mentions) {
        scope.finish();
        return Err(error);
    }
    let watched_scope = scope.clone();
    let watched_session = session.clone();
    tauri::async_runtime::spawn(async move {
        tokio::select! {
            _ = watched_scope.cancellation.cancelled() => watched_session.finish_cancelled_scope(&watched_scope),
            _ = watched_scope.wait() => {},
        }
    });
    app.state::<ActiveGroupTurns>().publish(&key);
    Ok(scope)
}

fn spawn_jev_session(
    app: AppHandle,
    turn: ActiveGroupTurnLease,
    session: Arc<GroupSession>,
    group_id: String,
    topic_id: String,
    connection: ChatConnectionSnapshot,
) {
    tauri::async_runtime::spawn(async move {
        let result =
            run_jev_session(&app, &turn, &session, &group_id, &topic_id, &connection).await;
        if let Err(error) = result {
            session.fail(error);
        }
        // The lease drop sends the sole inactive UI event after all request IO
        // has settled; per-origin scope completion is independent of the UI.
        drop(turn);
    });
}

async fn run_jev_session(
    app: &AppHandle,
    turn: &ActiveGroupTurnLease,
    session: &Arc<GroupSession>,
    group_id: &str,
    topic_id: &str,
    connection: &ChatConnectionSnapshot,
) -> Result<(), String> {
    let key = TopicKey::new("group", group_id, topic_id);
    let _foreground = ForegroundLease::acquire(app, ForegroundTask::Stream, "群聊裁决与回复").ok();
    #[cfg(target_os = "ios")]
    let mut ios_reply = None;
    loop {
        session.await_inputs().await?;
        if turn.is_cancelled() || session.latest_scope_cancelled() {
            session.close(Some("cancelled_by_user".into()));
            return Ok(());
        }
        let input_revision = session.snapshot()?.revision;
        let config = read_group_config(
            app.clone(),
            app.state::<GroupManagerState>(),
            group_id.into(),
        )
        .await?;
        if config.effective_mode() != GroupChatMode::Jev {
            if session.close_if_current(input_revision, "mode_changed")? {
                return Ok(());
            }
            continue;
        }
        let mut members = Vec::new();
        for id in &config.members {
            if let Ok(member) =
                read_agent_config_internal(app, &app.state::<AgentConfigState>(), id, Some(false))
                    .await
            {
                members.push(member);
            }
        }
        if members.is_empty() {
            if session.close_if_current(input_revision, "no_active_members")? {
                return Ok(());
            }
            continue;
        }
        if members.len() > 254 {
            return Err("JEV 群聊最多支持 254 位可用成员。".into());
        }
        #[cfg(target_os = "ios")]
        if ios_reply.is_none() && members.iter().any(|agent| agent.stream_output) {
            ios_reply = Some(ReplyTask::begin(app, &config.name, ""));
        }
        if let Some((id, scope)) = session.next_speaker(input_revision)? {
            app.state::<ActiveGroupTurns>().publish(&key);
            let Some(speaker) = members.iter().find(|member| member.id == id) else {
                session.speaker_finished(false, Ok(json!({"status":"completed"})))?;
                continue;
            };
            let mut history = message_service::load_chat_text_history_for_context(
                app,
                &key,
                group_context_limit(&config),
                None,
                true,
            )
            .await?;
            let db = app.state::<DbState>();
            let requests = app.state::<ActiveRequests>();
            let params = GroupSpeakerTurnParams {
                app_handle: app,
                db_pool: &db.pool,
                active_requests: &requests,
                group_turn: turn,
                group_config: &config,
                active_member_configs: &members,
                group_id,
                topic_id,
                connection,
                stream_channel: &None,
                reply_policy: scope.as_ref().and_then(|scope| scope.policy.as_deref()),
                reply_scope: scope.as_ref(),
            };
            let outcome = run_group_speaker_turn(
                &params,
                speaker,
                &mut history,
                #[cfg(target_os = "ios")]
                ios_reply.as_ref(),
            )
            .await?;
            session.speaker_finished(
                outcome == GroupSpeakerOutcome::Settled,
                Ok(json!({"status":outcome.status()})),
            )?;
            app.state::<ActiveGroupTurns>().publish(&key);
            match outcome {
                GroupSpeakerOutcome::Settled => continue,
                GroupSpeakerOutcome::Cancelled
                    if !turn.is_cancelled() && !session.latest_scope_cancelled() =>
                {
                    continue
                }
                other => {
                    session.close(Some(other.status().into()));
                    return Ok(());
                }
            }
        }
        let Some(runtime) = session.decision_context(input_revision)? else {
            continue;
        };
        app.state::<ActiveGroupTurns>().publish(&key);
        if runtime.rounds
            >= config
                .local_settings
                .mode_settings
                .jev
                .max_autonomous_rounds
        {
            session.decision_finished();
            if session.close_if_current(runtime.revision, "safety_limit")? {
                #[cfg(target_os = "ios")]
                if let Some(task) = ios_reply.as_mut() {
                    task.success = true;
                }
                return Ok(());
            }
            continue;
        }
        let db = app.state::<DbState>();
        #[cfg(target_os = "ios")]
        if let Some(task) = ios_reply.as_ref() {
            task.update("", 0);
        }
        let history_stats = load_group_member_history_stats(&db.pool, &key).await?;
        let settings = read_config(app.clone(), app.state::<SettingsState>()).await?;
        let mut request = build_jev_request(&members, &config, &runtime, &history_stats)?;
        let history_budget =
            crate::vcp_modules::group::jev_client::history_byte_budget(&settings.jev, &request)
                .map_err(|error| error.to_string())?;
        let history = message_service::load_chat_plain_text_history_for_context(
            &db.pool,
            &key,
            config.local_settings.mode_settings.jev.history_window as usize,
            history_budget,
        )
        .await?;
        let preparation_cancellation = runtime.cancellation.clone();
        let preparation = tokio::task::spawn_blocking(move || {
            append_jev_history(
                &mut request,
                history,
                history_budget,
                &preparation_cancellation,
            )?;
            Ok::<_, String>(request)
        });
        let prepared = tokio::select! {
            biased;
            _ = runtime.cancellation.cancelled() => None,
            result = preparation => Some(result),
        };
        if runtime.cancellation.is_cancelled() || !session.revision_is_current(runtime.revision) {
            session.decision_finished();
            continue;
        }
        let Some(prepared) = prepared else {
            session.decision_finished();
            continue;
        };
        let request =
            Arc::new(prepared.map_err(|error| format!("Jev 上下文装配任务失败：{error}"))??);
        let cancellation =
            ScopeCancellation::new(runtime.cancellation.clone(), runtime.scope.as_ref());
        let response = crate::vcp_modules::group::jev_client::decide(
            &settings.jev,
            request.clone(),
            cancellation.token.clone(),
        )
        .await;
        session.decision_finished();
        if turn.is_cancelled() {
            session.close(Some("cancelled_by_user".into()));
            return Ok(());
        }
        if !session.revision_is_current(runtime.revision) {
            continue;
        }
        if session.latest_scope_cancelled() {
            session.close(Some("cancelled_by_user".into()));
            return Ok(());
        }
        let (ids, reason) = match response {
            Ok(response) => parse_jev_decision(
                &response,
                &request,
                &members,
                &config,
                runtime.force_continue,
            )?,
            Err(error) if error.is_cancelled() => {
                session.close(Some("cancelled_by_user".into()));
                return Ok(());
            }
            Err(error) if error.is_retry_exhausted() => {
                log::warn!(
                    "[GroupChat] JEV retries exhausted, applying configured fallback: code={}, status={:?}",
                    error.code,
                    error.status
                );
                fallback_jev_decision(&members, &config)
            }
            Err(error) => {
                log::warn!(
                    "[GroupChat] JEV arbitration failed: code={}, status={:?}",
                    error.code,
                    error.status
                );
                return Err(error.to_string());
            }
        };
        let stop = reason.is_some();
        if session.accept_decision(runtime.revision, &ids, reason)? && stop {
            #[cfg(target_os = "ios")]
            if let Some(task) = ios_reply.as_mut() {
                task.success = true;
            }
            return Ok(());
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupSessionControlPayload {
    pub group_id: String,
    pub topic_id: String,
    pub action: String,
}

#[tauri::command]
pub async fn control_group_session(
    app_handle: AppHandle,
    payload: GroupSessionControlPayload,
    stream_channel: Channel<StreamEvent>,
) -> Result<Value, String> {
    if !matches!(payload.action.as_str(), "start" | "continue") {
        return Err("未知的群聊操作".into());
    }
    let config = read_group_config(
        app_handle.clone(),
        app_handle.state::<GroupManagerState>(),
        payload.group_id.clone(),
    )
    .await?;
    if config.effective_mode() != GroupChatMode::Jev {
        return Err("当前群组未启用 JEV 模式".into());
    }
    let key = TopicKey::new("group", &payload.group_id, &payload.topic_id);
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM topics WHERE owner_type='group' AND owner_id=? AND topic_id=? AND deleted_at IS NULL)")
        .bind(&payload.group_id).bind(&payload.topic_id).fetch_one(&app_handle.state::<DbState>().pool).await.map_err(|e| e.to_string())?;
    if !exists {
        return Err("群聊话题不存在".into());
    }
    let turn = app_handle
        .state::<ActiveGroupTurns>()
        .try_acquire(key.clone())?;
    if payload.action == "continue" {
        app_handle.state::<ActiveGroupTurns>().accept_continue(
            &key,
            config.local_settings.mode_settings.jev.continue_debounce_ms,
        )?;
    }
    let session = turn.start_session(
        if payload.action == "start" {
            "random_opening"
        } else {
            "continue"
        },
        payload.action == "continue",
    )?;
    if payload.action == "start" {
        let mut members = Vec::new();
        for id in &config.members {
            if read_agent_config_internal(
                &app_handle,
                &app_handle.state::<AgentConfigState>(),
                id,
                Some(false),
            )
            .await
            .is_ok()
            {
                members.push(id.clone());
            }
        }
        if members.is_empty() {
            return Err("群组没有可用的成员".into());
        }
        let index = rand::random::<usize>() % members.len();
        session.enqueue(&[members[index].clone()])?;
    }
    turn.bind_output(&app_handle, stream_channel)?;
    let settings = read_config(app_handle.clone(), app_handle.state::<SettingsState>()).await?;
    let connection = freeze_chat_connection(&settings, ChatRequestPurpose::Interactive)?;
    spawn_jev_session(
        app_handle,
        turn,
        session,
        payload.group_id,
        payload.topic_id,
        connection,
    );
    Ok(json!({"status":"accepted"}))
}

#[cfg(test)]
mod group_turn_tests {
    use super::*;
    use crate::vcp_modules::agent_types::AgentConfig;
    use crate::vcp_modules::group_types::GroupConfig;
    use crate::vcp_modules::infra::settings_manager::types::{JevConfig, JevProviderConfig};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn jev_mentions_choose_longest_names_and_preserve_text_order() {
        let members = vec![
            ("short".into(), "小明".into()),
            ("long".into(), "小明2".into()),
            ("alpha".into(), "Alpha".into()),
            ("alpha-beta".into(), "AlphaBeta".into()),
        ];
        assert_eq!(
            extract_group_mentions("@小明2，请回应 @ALPHABETA；再请 @小明。@小明2", &members),
            ["long", "alpha-beta", "short"]
        );
        assert_eq!(
            extract_group_mentions("@AlphaBeta @Alpha", &members),
            ["alpha-beta", "alpha"]
        );
    }

    #[test]
    fn jev_mentions_keep_first_duplicate_name_and_existing_halfwidth_syntax() {
        let members = vec![
            ("first".into(), "  Nova  ".into()),
            ("duplicate".into(), "NOVA".into()),
            ("blank".into(), "  ".into()),
            ("unicode".into(), "小助手🤖".into()),
        ];
        assert_eq!(
            extract_group_mentions("＠Nova @unknown @小助手🤖 @nova @NOVA", &members),
            ["unicode", "first"]
        );
        assert!(extract_group_mentions("普通正文 @", &members).is_empty());
    }

    #[test]
    fn blank_jev_navigator_uses_default_in_request_without_overwriting_saved_value() {
        let members = vec![AgentConfig {
            id: "a".into(),
            name: "A".into(),
            ..Default::default()
        }];
        let mut group = GroupConfig::default();
        let session = GroupSession::new(
            "navigator".into(),
            CancellationToken::new(),
            "user_message",
            false,
        );
        let runtime = session.decision_context(0).unwrap().unwrap();
        for saved in ["", " \t\n", "  Custom routing instructions  "] {
            group.local_settings.mode_settings.jev.navigator_prompt = saved.into();
            let request = build_jev_request(&members, &group, &runtime, &BTreeMap::new()).unwrap();
            assert!(request.validate().is_ok());
            let body = serde_json::to_value(request).unwrap();
            let expected = if saved.trim().is_empty() {
                crate::vcp_modules::group_types::JevModeSettings::default().navigator_prompt
            } else {
                saved.to_owned()
            };
            assert_eq!(
                body["questions"]["speaker_routing"]["instructions"],
                expected
            );
            assert_eq!(
                group.local_settings.mode_settings.jev.navigator_prompt,
                saved
            );
        }
    }

    #[tokio::test]
    async fn jev_member_stats_count_full_committed_history_in_stable_order() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../../../migrations/0100_baseline_v2.sql"))
            .execute(&pool)
            .await
            .unwrap();
        for (id, role, agent, timestamp, deleted) in [
            ("a-old", "assistant", Some("a"), 1, None),
            ("a-new", "assistant", Some("a"), 2, None),
            ("b-response", "assistant", Some("b"), 5, None),
            ("user-z", "user", None, 5, None),
            ("system", "system", None, 6, None),
            ("deleted", "assistant", Some("a"), 7, Some(8)),
            ("active", "assistant", Some("a"), 8, None),
        ] {
            sqlx::query(
                "INSERT INTO messages (owner_type,owner_id,topic_id,msg_id,role,agent_id,content,timestamp,created_at,updated_at,deleted_at)
                 VALUES ('group','group','topic',?,?,?,?,?,1,1,?)",
            )
            .bind(id)
            .bind(role)
            .bind(agent)
            .bind(id)
            .bind(timestamp)
            .bind(deleted)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query("INSERT INTO active_generations (owner_type,owner_id,topic_id,msg_id,created_at) VALUES ('group','group','topic','active',1)")
            .execute(&pool).await.unwrap();
        let stats =
            load_group_member_history_stats(&pool, &TopicKey::new("group", "group", "topic"))
                .await
                .unwrap();
        assert_eq!(stats["a"].speech_count, 2);
        assert_eq!(stats["a"].last_speech_distance, 3);
        assert_eq!(stats["b"].speech_count, 1);
        assert_eq!(stats["b"].last_speech_distance, 2);
        let group: GroupConfig = serde_json::from_value(json!({"id":"group"})).unwrap();
        let members: Vec<AgentConfig> = serde_json::from_value(json!([
            {"id":"a","name":"A"}, {"id":"never","name":"Never"}
        ]))
        .unwrap();
        let session = GroupSession::new(
            "stats".into(),
            CancellationToken::new(),
            "user_message",
            false,
        );
        let runtime = session.decision_context(0).unwrap().unwrap();
        // Even an empty/restricted text window must not erase full-history facts.
        let request = build_jev_request(&members, &group, &runtime, &stats).unwrap();
        assert_eq!(request.state["members"][0]["messagesSinceLastSpeech"], 3);
        assert_eq!(request.state["members"][0]["totalSpeechCount"], 2);
        assert!(request.state["members"][1]["messagesSinceLastSpeech"].is_null());
        assert_eq!(request.state["members"][1]["totalSpeechCount"], 0);
    }

    #[tokio::test]
    async fn jev_http_decision_selects_real_speaker_and_durably_commits_reply() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../../../migrations/0100_baseline_v2.sql"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!(
            "../../../migrations/0102_stream_stop_intent.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::raw_sql(
            "INSERT INTO agents (owner_type,agent_id,name,model,updated_at) VALUES ('agent','member','Member','test',1);
             INSERT INTO groups (owner_type,group_id,name,mode,updated_at) VALUES ('group','group','Group','sequential',1);
             INSERT INTO topics (owner_type,owner_id,topic_id,title,created_at,updated_at) VALUES ('group','group','topic','Topic',1,1);"
        ).execute(&pool).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for stage in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0u8; 4096];
                let header_end = loop {
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert_ne!(count, 0);
                    bytes.extend_from_slice(&buffer[..count]);
                    if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        break end + 4;
                    }
                };
                let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap();
                while bytes.len() < header_end + length {
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert_ne!(count, 0);
                    bytes.extend_from_slice(&buffer[..count]);
                }
                let body: Value =
                    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                let response = if stage == 0 {
                    assert!(headers.starts_with("POST /decision "));
                    assert_eq!(body["questions"]["speaker_routing"]["type"], "choice");
                    json!({"answers": {"speaker_routing": {"probabilities": {"agent_0":0.9,"end_conversation":0.1}}, "continue_discussion":{"noul":0.9}}})
                } else {
                    assert!(headers.starts_with("POST /chat "));
                    assert!(body["messages"].is_array());
                    json!({"choices":[{"message":{"content":"selected member reply"},"finish_reason":"stop"}]})
                }.to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).as_bytes()).await.unwrap();
            }
        });
        let group: GroupConfig =
            serde_json::from_value(json!({"id":"group","members":["member"],"chatMode":"jev"}))
                .unwrap();
        let members: Vec<AgentConfig> = serde_json::from_value(
            json!([{"id":"member","name":"Member","model":"test","streamOutput":false}]),
        )
        .unwrap();
        let session = GroupSession::new(
            "fixture".into(),
            CancellationToken::new(),
            "user_message",
            false,
        );
        let runtime = session.decision_context(0).unwrap().unwrap();
        let request =
            Arc::new(build_jev_request(&members, &group, &runtime, &BTreeMap::new()).unwrap());
        let config = JevConfig {
            enabled: true,
            typesafe: JevProviderConfig {
                api_url: format!("{endpoint}/decision"),
                api_key: "fixture-key".into(),
                ..Default::default()
            },
            max_retries: 0,
            ..Default::default()
        };
        let response = crate::vcp_modules::group::jev_client::decide(
            &config,
            request.clone(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let (ids, reason) =
            parse_jev_decision(&response, &request, &members, &group, false).unwrap();
        assert_eq!(ids, ["member"]);
        assert!(reason.is_none());
        let app = tauri::test::mock_app();
        app.manage(DbState::new(pool.clone(), std::path::PathBuf::new()));
        app.manage(ActiveRequests::default());
        app.manage(ActiveGroupTurns::default());
        let turns = app.state::<ActiveGroupTurns>();
        let turn = turns
            .try_acquire(TopicKey::new("group", "group", "topic"))
            .unwrap();
        let connection = ChatConnectionSnapshot {
            endpoint_url: format!("{endpoint}/chat"),
            api_key: String::new(),
        };
        let requests = app.state::<ActiveRequests>();
        let params = GroupSpeakerTurnParams {
            app_handle: app.handle(),
            db_pool: &pool,
            active_requests: &requests,
            group_turn: &turn,
            group_config: &group,
            active_member_configs: &members,
            group_id: "group",
            topic_id: "topic",
            connection: &connection,
            stream_channel: &None,
            reply_policy: None,
            reply_scope: None,
        };
        let mut history = Vec::new();
        let speaker = members.iter().find(|member| member.id == ids[0]).unwrap();
        assert_eq!(
            run_group_speaker_turn(
                &params,
                speaker,
                &mut history,
                #[cfg(target_os = "ios")]
                None,
            )
            .await
            .unwrap(),
            GroupSpeakerOutcome::Settled
        );
        let saved: (String, String) =
            sqlx::query_as("SELECT agent_id, content FROM messages WHERE role='assistant'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(saved, ("member".into(), "selected member reply".into()));
        assert_eq!(history.len(), 1);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn stopping_first_invited_speaker_does_not_start_the_second() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("test database");
        sqlx::raw_sql(include_str!("../../../migrations/0100_baseline_v2.sql"))
            .execute(&pool)
            .await
            .expect("baseline");
        sqlx::raw_sql(include_str!(
            "../../../migrations/0102_stream_stop_intent.sql"
        ))
        .execute(&pool)
        .await
        .expect("stream stop schema");
        sqlx::raw_sql(
            "INSERT INTO agents (owner_type, agent_id, name, model, updated_at)
             VALUES ('agent', 'first', 'First', 'test', 1),
                    ('agent', 'second', 'Second', 'test', 1);
             INSERT INTO groups (owner_type, group_id, name, mode, updated_at)
             VALUES ('group', 'group', 'Group', 'invite_only', 1);
             INSERT INTO topics (owner_type, owner_id, topic_id, title, created_at, updated_at)
             VALUES ('group', 'group', 'topic', 'Topic', 1, 1);",
        )
        .execute(&pool)
        .await
        .expect("group fixture");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("upstream listener");
        let endpoint = format!("http://{}", listener.local_addr().expect("listen address"));
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("first speaker request");
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = socket.read(&mut buffer).await.expect("request headers");
                assert_ne!(count, 0);
                request.extend_from_slice(&buffer[..count]);
            }
            socket.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"partial reply\"}}]}\n\n",
            ).await.expect("stream first speaker content");
            // Cancellation must close the first connection. A second invitation
            // would be an independent POST and must never arrive afterward.
            while socket.read(&mut buffer).await.unwrap_or(0) != 0 {}
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        });

        let app = tauri::test::mock_app();
        app.manage(DbState::new(pool.clone(), std::path::PathBuf::new()));
        app.manage(ActiveRequests::default());
        app.manage(ActiveGroupTurns::default());
        app.manage(crate::vcp_modules::infra::lifecycle_state::LifecycleState::new());
        let turns = app.state::<ActiveGroupTurns>();
        let group_turn = turns
            .try_acquire(TopicKey::new("group", "group", "topic"))
            .expect("one invitation turn");
        let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
        let channel = Channel::new(move |body| {
            let _ = sender.send(body.deserialize::<Value>()?);
            Ok(())
        });
        group_turn
            .bind_output(app.handle(), channel.clone())
            .expect("turn output");
        let speakers: Vec<AgentConfig> = serde_json::from_value(json!([
            {"id": "first", "name": "First", "model": "test"},
            {"id": "second", "name": "Second", "model": "test"}
        ]))
        .expect("speaker configurations");
        let group: GroupConfig = serde_json::from_value(json!({
            "id": "group", "members": ["first", "second"], "chatMode": "invite_only"
        }))
        .expect("group configuration");
        let connection = ChatConnectionSnapshot {
            endpoint_url: endpoint,
            api_key: String::new(),
        };
        let stream_channel = Some(channel);
        let requests = app.state::<ActiveRequests>();
        let params = GroupSpeakerTurnParams {
            app_handle: app.handle(),
            db_pool: &pool,
            active_requests: &requests,
            group_turn: &group_turn,
            group_config: &group,
            active_member_configs: &speakers,
            group_id: "group",
            topic_id: "topic",
            connection: &connection,
            stream_channel: &stream_channel,
            reply_policy: None,
            reply_scope: None,
        };
        let mut history = Vec::new();
        let outcome =
            {
                let request = run_group_speakers(&params, &speakers, &mut history);
                tokio::pin!(request);
                tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    tokio::select! {
                        outcome = &mut request => break outcome,
                        Some(event) = events.recv() => {
                            if event["type"] == "aurora"
                                && (event["aurora"]["chunk"].as_str() == Some("partial reply")
                                    || event["aurora"]["content"].as_str() == Some("partial reply"))
                            {
                                group_turn.cancel();
                            }
                        }
                    }
                }
            }).await.expect("stopped invitation finishes")
            };
        assert_eq!(
            outcome.expect("settle invitation"),
            GroupSpeakerOutcome::Cancelled
        );
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "partial reply");
        assert_eq!(
            history[0].finish_reason.as_deref(),
            Some("cancelled_by_user")
        );
        let persisted: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT agent_id, content, finish_reason FROM messages WHERE owner_type = 'group'",
        )
        .fetch_all(&pool)
        .await
        .expect("persisted replies");
        assert_eq!(
            persisted,
            vec![(
                "first".into(),
                "partial reply".into(),
                "cancelled_by_user".into()
            )]
        );
        assert!(server
            .await
            .expect("upstream completed without second request"));
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn handle_group_chat_message(
    app_handle: AppHandle,
    group_state: State<'_, GroupManagerState>,
    agent_state: State<'_, AgentConfigState>,
    db_state: State<'_, DbState>,
    active_requests: State<'_, ActiveRequests>,
    active_group_turns: State<'_, ActiveGroupTurns>,
    settings_state: State<'_, SettingsState>,
    payload: GroupChatPayload,
    stream_channel: Channel<crate::vcp_modules::vcp_client::StreamEvent>,
) -> Result<Value, String> {
    log::info!(
        "[GroupChatAppService] handle_group_chat_message invoked for group: {}",
        payload.group_id
    );

    let settings = read_config(app_handle.clone(), settings_state).await?;
    let connection = freeze_chat_connection(&settings, ChatRequestPurpose::Interactive)?;

    internal_process_group_chat_message(
        app_handle,
        group_state,
        agent_state,
        db_state,
        active_requests,
        active_group_turns,
        GroupChatParams {
            group_id: payload.group_id,
            topic_id: payload.topic_id,
            user_message: payload.user_message,
            connection,
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
    pub agent_ids: Vec<String>,
    #[serde(default)]
    pub origin_user_message_id: Option<String>,
}

/// 按邀请顺序让指定成员在同一群聊回合内发言。
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
    active_group_turns: State<'_, ActiveGroupTurns>,
    settings_state: State<'_, SettingsState>,
    payload: GroupInvitePayload,
    stream_channel: Channel<crate::vcp_modules::vcp_client::StreamEvent>,
) -> Result<Value, String> {
    log::info!(
        "[GroupChatAppService] invite_group_member_to_speak: group={}, topic={}, agents={:?}",
        payload.group_id,
        payload.topic_id,
        payload.agent_ids
    );
    let topic_key = TopicKey::new("group", &payload.group_id, &payload.topic_id);
    let group_config = read_group_config(
        app_handle.clone(),
        group_state.clone(),
        payload.group_id.clone(),
    )
    .await?;
    if group_config.effective_mode() == GroupChatMode::Jev {
        if payload.agent_ids.is_empty()
            || payload
                .agent_ids
                .iter()
                .any(|id| !group_config.members.contains(id))
        {
            return Err("请选择当前群组中的成员".into());
        }
        if let Some(session) = active_group_turns.session(&topic_key) {
            session.enqueue(&payload.agent_ids)?;
            active_group_turns.publish(&topic_key);
            return Ok(json!({"status":"accepted"}));
        }
        let turn = active_group_turns.try_acquire(topic_key.clone())?;
        let session = turn.start_session("manual_agent", false)?;
        session.enqueue(&payload.agent_ids)?;
        turn.bind_output(&app_handle, stream_channel)?;
        let settings = read_config(app_handle.clone(), settings_state).await?;
        let connection = freeze_chat_connection(&settings, ChatRequestPurpose::Interactive)?;
        spawn_jev_session(
            app_handle,
            turn,
            session,
            payload.group_id,
            payload.topic_id,
            connection,
        );
        return Ok(json!({"status":"accepted"}));
    }
    let group_turn = active_group_turns.try_acquire(topic_key.clone())?;
    if let Some(origin) = &payload.origin_user_message_id {
        group_turn.set_origin_user_message(origin)?;
    }
    group_turn.bind_output(&app_handle, stream_channel.clone())?;
    let settings = read_config(app_handle.clone(), settings_state).await?;
    let connection = freeze_chat_connection(&settings, ChatRequestPurpose::Interactive)?;

    let group_config = read_group_config(
        app_handle.clone(),
        group_state.clone(),
        payload.group_id.clone(),
    )
    .await?;

    if payload.agent_ids.is_empty() {
        return Err("请至少邀请一位群成员".to_string());
    }
    // Validate the complete invitation before any speaker issues an upstream
    // request. Repeated mentions of a member still invite that member once.
    let mut invited_ids = Vec::new();
    for agent_id in &payload.agent_ids {
        if !group_config.members.contains(agent_id) {
            return Err(format!("成员 {} 不属于群组 {}", agent_id, payload.group_id));
        }
        if !invited_ids.contains(agent_id) {
            invited_ids.push(agent_id.clone());
        }
    }

    let mut speakers = Vec::with_capacity(invited_ids.len());
    for agent_id in invited_ids {
        if group_turn.is_cancelled() {
            return Ok(json!({"status": "cancelled"}));
        }
        let speaker = read_agent_config_internal(&app_handle, &agent_state, &agent_id, Some(false))
            .await
            .map_err(|e| format!("加载被邀请成员 {} 的配置失败: {e}", agent_id))?;
        speakers.push(speaker);
    }

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
        &topic_key,
        group_context_limit(&group_config),
        None,
        true,
    )
    .await?;

    let stream_channel = Some(stream_channel);
    let turn_params = GroupSpeakerTurnParams {
        app_handle: &app_handle,
        db_pool: &db_state.pool,
        active_requests: &active_requests,
        group_turn: &group_turn,
        group_config: &group_config,
        active_member_configs: &active_member_configs,
        group_id: &payload.group_id,
        topic_id: &payload.topic_id,
        connection: &connection,
        stream_channel: &stream_channel,
        reply_policy: None,
        reply_scope: None,
    };

    let outcome =
        run_group_speakers(&turn_params, &speakers, &mut full_history_for_context).await?;

    Ok(json!({"status": outcome.status()}))
}
