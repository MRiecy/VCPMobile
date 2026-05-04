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
    perform_vcp_request, ActiveRequests, CancelledGroupTurns, VcpRequestPayload,
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
    let current_history = message_service::load_chat_history_internal(
        &app_handle,
        &group_id,
        "group",
        &topic_id,
        Some(8), // 限制上下文长度
        None,
        true,
    )
    .await?;

    // 4. 决策引擎：谁该说话？
    let speakers = if group_config.mode == "sequential" {
        active_member_configs.clone()
    } else if group_config.mode == "naturerandom" {
        determine_naturerandom_speakers(
            &active_member_configs,
            &current_history,
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

        // 每次循环重新加载历史，以包含前一个 Agent 的回复
        let current_history_for_context = message_service::load_chat_history_internal(
            &app_handle,
            &group_id,
            "group",
            &topic_id,
            None,
            None,
            true,
        )
        .await?;

        let group_config_inner = group_config.clone();
        let active_member_configs_inner = active_member_configs.clone();

        let agent_id = speaker.id.clone();
        let agent_name = speaker.name.clone();
        let message_id = format!(
            "msg_{}_assistant_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
            agent_id
        );

        // 组装上下文
        let system_prompt =
            assemble_group_context(&speaker, &group_config_inner, &active_member_configs_inner)
                .await;

        // 构造请求载荷
        let model_config = json!({
            "model": speaker.model,
            "temperature": speaker.temperature,
            "stream": true
        });

        let mut messages = assemble_history_for_vcp(&current_history_for_context);
        messages.insert(0, json!({"role": "system", "content": system_prompt}));

        let request_payload = VcpRequestPayload {
            vcp_url,
            vcp_api_key,
            messages,
            model_config,
            message_id: message_id.clone(),
            context: Some(json!({
                "groupId": group_id,
                "topicId": topic_id,
                "agentId": agent_id,
                "isGroupMessage": true,
                "agentName": agent_name
            })),
        };

        // 执行请求 (串行等待)
        let res_result = perform_vcp_request(
            &app_handle,
            active_requests_map,
            request_payload,
            stream_channel.clone(),
        )
        .await;

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
                    agent_id: Some(agent_id),
                    group_id: Some(group_id.clone()),
                    topic_id: Some(topic_id.clone()),
                    is_group_message: Some(true),
                    finish_reason,
                    attachments: None,
                    blocks: None,
                };

                // 立即进行一次断点存盘 (针对单个 Agent)
                let _ = message_service::append_single_message(
                    app_handle.clone(),
                    &db_pool,
                    &group_id,
                    "group",
                    topic_id.clone(),
                    ai_msg.clone(),
                )
                .await;

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
