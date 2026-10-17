// group_chat_application_service.rs: 编排群聊工作流
// 职责: 1. 读取配置 2. 保存消息 3. 决策发言者 4. 组装上下文 5. 执行 AI 调用 6. 发射事件

use crate::vcp_modules::agent_service::{read_agent_config_internal, AgentConfigState};
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_context_assembler::assemble_group_context;
use crate::vcp_modules::group_service::{read_group_config, GroupManagerState};
use crate::vcp_modules::group_speaking_policy::determine_naturerandom_speakers;
use crate::vcp_modules::message_service;
use crate::vcp_modules::settings_manager::{read_settings, SettingsState};
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
use crate::vcp_modules::vcp_client::{
    freeze_chat_connection, message_transport_request_id, perform_vcp_request_registered,
    ActiveGroupTurnLease, ActiveGroupTurns, ActiveRequestLease, ActiveRequests,
    ChatConnectionSnapshot, ChatRequestPurpose, StreamEvent, VcpRequestPayload,
};
use serde::Deserialize;
use serde_json::{json, Value};

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GroupSpeakerOutcome {
    Settled,
    Cancelled,
    Stopping,
    RecoveryPending,
    Discarded,
}

impl GroupSpeakerOutcome {
    fn status(self) -> &'static str {
        match self {
            Self::Settled => "completed",
            Self::Cancelled => "cancelled",
            Self::Stopping => "stopping",
            Self::RecoveryPending => "recovery_pending",
            Self::Discarded => "discarded",
        }
    }
}

/// Automatic relay and explicit invitations share one turn owner and history.
/// Only a settled speaker (including a durably saved error) may advance it.
async fn run_group_speakers<R: Runtime>(
    params: &GroupSpeakerTurnParams<'_, R>,
    speakers: &[crate::vcp_modules::agent_types::AgentConfig],
    history: &mut Vec<ChatMessage>,
) -> Result<GroupSpeakerOutcome, String> {
    for speaker in speakers {
        let outcome = run_group_speaker_turn(params, speaker, history).await?;
        if outcome != GroupSpeakerOutcome::Settled {
            return Ok(outcome);
        }
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
        &topic_key,
        &agent_name,
        "group",
        base_system_prompt,
        invite_prompt_processed,
    )
    .await?;

    // Preparation may await IO; a stop received during it must not start the
    // next speaker or create another upstream request.
    if params.group_turn.is_cancelled() {
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

    let (_request_lease, cancellation_token) = ActiveRequestLease::try_acquire_with_cancellation(
        params.active_requests.0.clone(),
        request_key.clone(),
        params.group_turn.child_token(),
    )?;
    let db_state = app_handle.state::<DbState>();
    message_service::begin_stream_message(
        &db_state,
        &request_key,
        Some(&agent_id),
        Some(&agent_name),
    )
    .await?;

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
        Ok((json!({ "fullContent": "" }), true))
    } else {
        perform_vcp_request_registered(
            app_handle,
            request_payload,
            stream_channel.clone(),
            cancellation_token,
        )
        .await
    };

    match res_result {
        Ok((res, is_aborted)) => {
            if let Some(full_content) = res["fullContent"].as_str() {
                let finish_reason = if is_aborted {
                    Some("cancelled_by_user".to_string())
                } else {
                    res["finishReason"].as_str().map(|s| s.to_string())
                };

                // 1. 委托流终结器落盘与发射事件
                let finalized = message_service::finalize_stream_message(
                    app_handle.clone(),
                    db_pool,
                    &request_key,
                    full_content.to_string(),
                    is_aborted,
                    finish_reason,
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
                return Ok(if is_aborted {
                    GroupSpeakerOutcome::Cancelled
                } else {
                    GroupSpeakerOutcome::Settled
                });
            }
            Err("群聊生成未返回可提交的正文".to_string())
        }
        Err(failure) => {
            log::error!(
                "[GroupChatAppService] Error during agent {} response: {}",
                agent_id,
                failure
            );
            let stop_pending = failure.is_stop_pending();
            let recovery_pending = failure.is_recovery_pending();
            let (error, partial_content) = failure.into_parts();
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
            )
            .await;
            Ok(match finalized {
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
    let stream_channel = params.stream_channel;
    let group_id = params.group_id;
    let topic_id = params.topic_id;
    let user_message = params.user_message;
    let connection = params.connection;
    let topic_key = TopicKey::new("group", &group_id, &topic_id);
    let group_turn = active_group_turns.try_acquire(topic_key.clone())?;
    if let Some(channel) = stream_channel.clone() {
        group_turn.bind_output(&app_handle, channel)?;
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
        &topic_key,
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
        &topic_key,
        None, // 加载全部用于 VCP 上下文
        None,
        true, // include_extracted_text: 组装群聊上下文发送给 VCP 时需要包含附件提取文本内容
    )
    .await?;

    // 5. 串行异步调度 (约束：群聊内部必须串行)
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
    };
    let outcome =
        run_group_speakers(&turn_params, &speakers, &mut full_history_for_context).await?;

    // The turn lease publishes completion through its current output consumer,
    // including early returns and failures.
    Ok(json!({"status": outcome.status()}))
}

#[cfg(test)]
mod group_turn_tests {
    use super::*;
    use crate::vcp_modules::agent_types::AgentConfig;
    use crate::vcp_modules::group_types::GroupConfig;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
            "id": "group", "members": ["first", "second"], "mode": "invite_only"
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

    let settings = read_settings(app_handle.clone(), settings_state).await?;
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
    let group_turn = active_group_turns.try_acquire(topic_key.clone())?;
    group_turn.bind_output(&app_handle, stream_channel.clone())?;
    let settings = read_settings(app_handle.clone(), settings_state).await?;
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
        group_turn: &group_turn,
        group_config: &group_config,
        active_member_configs: &active_member_configs,
        group_id: &payload.group_id,
        topic_id: &payload.topic_id,
        connection: &connection,
        stream_channel: &stream_channel,
    };

    let outcome =
        run_group_speakers(&turn_params, &speakers, &mut full_history_for_context).await?;

    Ok(json!({"status": outcome.status()}))
}
