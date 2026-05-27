// group_chat_application_service.rs: 编排群聊工作流
// 职责: 1. 读取配置 2. 保存消息 3. 决策发言者 4. 组装上下文 5. 执行 AI 调用 6. 发射事件

use crate::vcp_modules::agent_service::{read_agent_config, AgentConfigState};
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::context_assembler_utils::assemble_history_for_vcp;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_context_assembler::assemble_group_context;
use crate::vcp_modules::group_service::{read_group_config, GroupManagerState};
use crate::vcp_modules::group_speaking_policy::determine_naturerandom_speakers;
use crate::vcp_modules::message_service;
use crate::vcp_modules::vcp_client::{
    perform_vcp_request, ActiveRequests, CancelledGroupTurns, StreamEvent, VcpRequestPayload,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};
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

    println!(
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
        if let Ok(cfg) = read_agent_config(
            app_handle.clone(),
            agent_state.clone(),
            member_id.clone(),
            Some(false),
        )
        .await
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

    // 为了给 AI 决策提供上下文，我们只读取最新的 20 条（或按需分配）
    let recent_history_for_decision = message_service::load_chat_history_internal(
        &app_handle,
        &group_id,
        "group",
        &topic_id,
        Some(8), // 限制上下文长度
        None,
        true,
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
    } else {
        println!(
            "[GroupChatAppService] Mode {} not implemented, ignoring.",
            group_config.mode
        );
        return Ok(json!({"status": "no_ai_response"}));
    };

    if speakers.is_empty() {
        return Ok(json!({"status": "no_ai_response"}));
    }

    // 提前加载全量历史记录作为接力上下文的基础
    let mut full_history_for_context = message_service::load_chat_history_internal(
        &app_handle,
        &group_id,
        "group",
        &topic_id,
        None, // 加载全部用于 VCP 上下文
        None,
        true,
        true, // include_extracted_text: 组装群聊上下文发送给 VCP 时需要包含附件提取文本内容
    )
    .await?;

    // 5. 串行异步调度 (约束：群聊内部必须串行)
    let mut final_new_msgs = Vec::new();

    for speaker in speakers {
        // 检查全局中断令牌：如果话题已被标记为取消，立即停止接力赛
        if cancelled_turns.0.contains(&topic_id) {
            println!(
                "[GroupChatAppService] Group turn for topic {} was cancelled. Breaking loop.",
                topic_id
            );
            break;
        }

        let app_handle = app_handle.clone();
        let db_pool = db_state.pool.clone();
        let active_requests_map = active_requests.0.clone();
        let group_id = group_id.clone();
        let topic_id = topic_id.clone();
        let vcp_url = vcp_url.clone();
        let vcp_api_key = vcp_api_key.clone();

        let group_config_inner = group_config.clone();
        let active_member_configs_inner = active_member_configs.clone();

        let agent_id = speaker.id.clone();
        let agent_name = speaker.name.clone();
        let message_id = format!(
            "msg_group_{}_{}_{}",
            user_message.id,
            agent_id,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis()
        );

        // 组装上下文
        let system_prompt =
            assemble_group_context(&speaker, &group_config_inner, &active_member_configs_inner)
                .await;

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
            "stream": true
        });
        if speaker.use_temperature {
            model_config["temperature"] = json!(speaker.temperature);
        }

        let mut messages = assemble_history_for_vcp(&full_history_for_context);
        if let Some(invite_prompt) = &group_config_inner.invite_prompt {
            let processed_invite = invite_prompt.replace("{{VCPChatAgentName}}", &agent_name);
            messages.push(json!({
                "role": "user",
                "content": processed_invite
            }));
        }
        messages.insert(0, json!({"role": "system", "content": system_prompt}));

        let context = Some(json!({
            "groupId": group_id,
            "topicId": topic_id,
            "agentId": agent_id,
            "isGroupMessage": true,
            "agentName": agent_name
        }));

        let request_payload = VcpRequestPayload {
            vcp_url,
            vcp_api_key,
            messages,
            model_config,
            message_id: message_id.clone(),
            context: context.clone(),
        };

        // 发射 thinking 事件，让前端为当前接力的 Agent 创建思考占位消息
        if let Some(chan) = &stream_channel {
            let _ = chan.send(StreamEvent::thinking(message_id.clone(), context));
        }

        // 启动前台服务保活
        if let Err(e) =
            tauri_plugin_vcp_mobile::stream::start_stream_service_inner(&app_handle, &agent_name)
        {
            println!(
                "[GroupChatAppService] Failed to start streaming service: {}",
                e
            );
        }

        // 执行请求 (串行等待)
        let res_result = perform_vcp_request(
            &app_handle,
            active_requests_map,
            request_payload,
            stream_channel.clone(),
        )
        .await;

        // 停止前台服务
        if let Err(e) =
            tauri_plugin_vcp_mobile::stream::stop_stream_service_inner(&app_handle, &agent_name)
        {
            println!(
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

                let ai_msg = ChatMessage {
                    id: message_id,
                    role: "assistant".to_string(),
                    name: Some(agent_name),
                    content: full_content.to_string(),
                    timestamp: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64,
                    is_thinking: Some(false),
                    agent_id: Some(agent_id.clone()),
                    group_id: Some(group_id.clone()),
                    topic_id: Some(topic_id.clone()),
                    is_group_message: Some(true),
                    finish_reason,
                    attachments: None,
                    blocks: None,
                    shell: None,
                    content_hash: None,
                };

                // 立即进行一次断点存盘 (针对单个 Agent)
                let append_result = message_service::append_single_message(
                    app_handle.clone(),
                    &db_pool,
                    &group_id,
                    "group",
                    topic_id.clone(),
                    ai_msg.clone(),
                )
                .await;
                let end_blocks = match &append_result {
                    Ok(blocks) => Some(blocks.clone()),
                    Err(e) => {
                        eprintln!(
                            "[GroupChatAppService] Failed to append final message: {}",
                            e
                        );
                        None
                    }
                };
                if let Some(chan) = &stream_channel {
                    let _ = chan.send(StreamEvent::end(
                        ai_msg.id.clone(),
                        Some(json!({
                            "groupId": group_id,
                            "topicId": topic_id,
                            "agentId": agent_id,
                            "isGroupMessage": true,
                            "agentName": ai_msg.name.clone()
                        })),
                        ai_msg.finish_reason.clone(),
                        end_blocks,
                    ));
                }

                // 关键优化：将新生成的回复追加到内存上下文，提供给接力赛的下一个 Agent
                full_history_for_context.push(ai_msg.clone());

                final_new_msgs.push(ai_msg);
            }
        } else if let Err(e) = res_result {
            eprintln!(
                "[GroupChatAppService] Error during agent {} response: {}",
                agent_id, e
            );
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
        true, // append_user_msg
    )
    .await
}
