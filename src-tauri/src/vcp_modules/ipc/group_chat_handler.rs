// group_chat_handler.rs: 处理群组相关的 IPC 指令
// 职责: 1. 解析前端 Payload 2. 调用 Application Service 3. 返回结果给前端

use crate::vcp_modules::agent_service::AgentConfigState;
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::file_watcher::WatcherState;
use crate::vcp_modules::group_chat_application_service;
use crate::vcp_modules::group_service::GroupManagerState;
use crate::vcp_modules::vcp_client::ActiveRequests;
use serde::Deserialize;
use serde_json::Value;
use tauri::{AppHandle, State};

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
        "[GroupChatHandler] handle_group_chat_message invoked for group: {}",
        payload.group_id
    );

    group_chat_application_service::process_group_chat_message(
        app_handle,
        group_state,
        agent_state,
        db_state,
        watcher_state,
        active_requests,
        group_chat_application_service::GroupChatParams {
            group_id: payload.group_id,
            topic_id: payload.topic_id,
            user_message: payload.user_message,
            vcp_url: payload.vcp_url,
            vcp_api_key: payload.vcp_api_key,
        },
    )
    .await
}
