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
use tauri::{AppHandle, Emitter, State};

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

    // 5. 串行异步流水线
    for speaker in speakers {
        let agent_id = speaker.id.clone();
        let agent_name = speaker.name.clone();
        let message_id = format!(
            "assistant_{}_{}",
            agent_id,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis()
        );

        // 组装上下文
        let system_prompt = assemble_context(&speaker, &group_config, &active_member_configs).await;

        // 构造请求载荷 (对齐 sendToVCP)
        let mut model_config = speaker.extra.get("modelConfig").cloned().unwrap_or(json!({
            "model": speaker.model,
            "temperature": speaker.temperature,
            "stream": true
        }));

        // 确保 stream 开启
        if let Some(obj) = model_config.as_object_mut() {
            obj.insert("stream".to_string(), json!(true));
        }

        let request_payload = VcpRequestPayload {
            vcp_url: payload.vcp_url.clone(),
            vcp_api_key: payload.vcp_api_key.clone(),
            messages: vec![json!({"role": "system", "content": system_prompt})], // 基础系统词由 orchestrator 处理
            model_config,
            message_id: message_id.clone(),
            context: Some(json!({
                "groupId": payload.group_id,
                "topicId": payload.topic_id,
                "agentId": agent_id,
                "isGroupMessage": true
            })),
            stream_channel: None, // 默认使用 vcp-stream
        };

        // 注入聊天历史作为上下文
        // (在实际 perform_vcp_request 中会再次处理，这里直接传给 payload.messages 的后续)
        let mut final_messages = request_payload.messages.clone();
        for msg in &current_history {
            final_messages.push(json!({
                "role": msg.role,
                "content": msg.content,
                "name": msg.name
            }));
        }

        let mut final_payload = request_payload;
        final_payload.messages = final_messages;

        // 执行请求并等待 (串行点)
        let mut loop_aborted = false;
        match perform_vcp_request(&app_handle, active_requests.0.clone(), final_payload).await {
            Ok((res, is_aborted)) => {
                if let Some(full_content) = res["fullContent"].as_str() {
                    // 无论是否中止，只要有内容就执行断点存盘 (解决半截消息丢失问题)
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
                        extra: json!({ "agentId": agent_id }),
                    };

                    current_history.push(ai_msg);

                    let _ = save_chat_history(
                        app_handle.clone(),
                        db_state.clone(),
                        watcher_state.clone(),
                        payload.group_id.clone(),
                        payload.topic_id.clone(),
                        current_history.clone(),
                    )
                    .await;

                    println!(
                        "[GroupHandlers] Breakpoint saved for agent: {}. Aborted: {}",
                        agent_id, is_aborted
                    );
                }

                if is_aborted {
                    loop_aborted = true;
                }
            }
            Err(e) => {
                eprintln!(
                    "[GroupHandlers] Error during agent {} response: {}",
                    agent_id, e
                );
                // 某个 Agent 出错，为了安全建议中断群组流水线，或者记录错误后继续
                // 这里选择记录并继续，除非是明确的中断
            }
        }

        if loop_aborted {
            println!("[GroupHandlers] Interrupt detected, breaking group sequence.");
            break;
        }
    }

    // 确保无论如何都发射“回合结束”信号给前端，解除输入框锁定 (状态解环)
    let _ = app_handle.emit(
        "vcp-group-turn-finished",
        json!({
            "groupId": payload.group_id,
            "topicId": payload.topic_id
        }),
    );

    Ok(json!({"status": "completed"}))
}
