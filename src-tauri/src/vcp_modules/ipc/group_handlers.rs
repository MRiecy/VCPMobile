// group_handlers.rs: 处理群组相关的 IPC 指令
// 职责: 1. 协调多 Agent 串行回复 2. 实现断点存盘 3. 触发前端实时同步

use crate::vcp_modules::agent_config_manager::{read_agent_config, AgentConfigState};
use crate::vcp_modules::chat_manager::{save_chat_history, ChatMessage};
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::file_watcher::WatcherState;
use crate::vcp_modules::group_manager::{read_group_config, GroupManagerState};
use crate::vcp_modules::group_orchestrator::{assemble_context, determine_naturerandom_speakers};
use crate::vcp_modules::vcp_client::{perform_vcp_request, ActiveRequests, VcpRequestPayload};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupChatPayload {
    pub group_id: String,
    pub topic_id: String,
    pub user_message: ChatMessage,
    pub vcp_url: String,
    pub vcp_api_key: String,
}

#[tauri::command]
pub async fn handle_group_chat_message(
    app_handle: AppHandle,
    group_state: State<'_, GroupManagerState>,
    agent_state: State<'_, AgentConfigState>,
    db_state: State<'_, DbState>,
    watcher_state: State<'_, WatcherState>,
    active_requests: State<'_, ActiveRequests>,
    payload: GroupChatPayload,
) -> Result<Value, String> {
    println!(
        "[GroupHandlers] handle_group_chat_message invoked for group: {}",
        payload.group_id
    );

    // 1. 加载群组配置
    let group_config = read_group_config(
        app_handle.clone(),
        group_state.clone(),
        payload.group_id.clone(),
    )
    .await?;

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

    // 3. 加载并更新历史记录 (存入用户消息)
    let history_command = crate::vcp_modules::chat_manager::load_chat_history(
        app_handle.clone(),
        payload.group_id.clone(),
        payload.topic_id.clone(),
        None,
        None,
    )
    .await?;

    let mut current_history = history_command;
    current_history.push(payload.user_message.clone());

    // 立即保存一次用户消息
    save_chat_history(
        app_handle.clone(),
        db_state.clone(),
        watcher_state.clone(),
        payload.group_id.clone(),
        payload.topic_id.clone(),
        current_history.clone(),
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
            &payload.user_message,
        )
    } else {
        println!(
            "[GroupHandlers] Mode {} not implemented, ignoring.",
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
        let app_handle = app_handle.clone();
        let db_pool = db_state.pool.clone();
        let watcher_state_ref = &*watcher_state;
        let active_requests_map = active_requests.0.clone();
        let group_id = payload.group_id.clone();
        let topic_id = payload.topic_id.clone();
        let vcp_url = payload.vcp_url.clone();
        let vcp_api_key = payload.vcp_api_key.clone();
        
        // 每次循环重新加载历史，以包含前一个 Agent 的回复
        let current_history_for_context = crate::vcp_modules::chat_manager::load_chat_history(
            app_handle.clone(),
            group_id.clone(),
            topic_id.clone(),
            None,
            None,
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
            assemble_context(&speaker, &group_config_inner, &active_member_configs_inner).await;

        // 构造请求载荷
        let mut model_config = speaker.extra.get("modelConfig").cloned().unwrap_or(json!({
            "model": speaker.model,
            "temperature": speaker.temperature,
            "stream": true
        }));

        if let Some(obj) = model_config.as_object_mut() {
            obj.insert("stream".to_string(), json!(true));
        }

        let mut messages = vec![json!({"role": "system", "content": system_prompt})];
        for msg in &current_history_for_context {
            messages.push(json!({
                "role": msg.role,
                "content": msg.content,
                "name": msg.name
            }));
        }

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
            stream_channel: None,
        };

        // 执行请求 (串行等待)
        let res_result = perform_vcp_request(&app_handle, active_requests_map, request_payload).await;

        if let Ok((res, _is_aborted)) = res_result {
            if let Some(full_content) = res["fullContent"].as_str() {
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
                    attachments: None,
                    extra: json!({
                        "agentId": agent_id,
                        "avatarUrl": speaker
                            .extra
                            .get("avatar")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    }),
                };

                // 立即进行一次断点存盘 (针对单个 Agent)
                let _ = crate::vcp_modules::chat_manager::append_single_message(
                    app_handle.clone(),
                    &db_pool,
                    Some(watcher_state_ref),
                    group_id.clone(),
                    topic_id.clone(),
                    ai_msg.clone(),
                )
                .await;

                final_new_msgs.push(ai_msg);
            }
        } else if let Err(e) = res_result {
            eprintln!(
                "[GroupHandlers] Error during agent {} response: {}",
                agent_id, e
            );
        }
    }

    // 6. 统一收集结果并最终发射信号
    let agent_ids: Vec<String> = final_new_msgs
        .iter()
        .filter_map(|m| {
            m.extra
                .get("agentId")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .collect();

    // 确保无论如何都发射“回合结束”信号给前端
    let _ = app_handle.emit(
        "vcp-group-turn-finished",
        json!({
            "groupId": payload.group_id,
            "topic_id": payload.topic_id,
            "agentIds": agent_ids
        }),
    );

    Ok(json!({"status": "completed"}))
}
