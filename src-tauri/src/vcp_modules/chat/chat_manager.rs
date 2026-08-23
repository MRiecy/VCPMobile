use crate::vcp_modules::content_parser::ContentBlock;
use crate::vcp_modules::message_service;
use crate::vcp_modules::sync_service::{SyncCommand, SyncState};
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
use serde::{Deserialize, Serialize};
use tauri::Manager;

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Attachment {
    #[serde(default)]
    pub r#type: String,
    /// 物理存储路径：真理之源。用于后续超栈文件追踪，或跨端同步时的原始路径参考
    #[serde(default)]
    pub src: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(
        rename = "attachmentOrder",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub attachment_order: Option<i32>,

    // 平铺数据库中的核心附件字段
    #[serde(rename = "internalPath", default)]
    pub internal_path: String,
    #[serde(rename = "extractedText", skip_serializing_if = "Option::is_none")]
    pub extracted_text: Option<String>,
    #[serde(rename = "imageFrames", skip_serializing_if = "Option::is_none")]
    pub image_frames: Option<Vec<String>>,
    #[serde(rename = "thumbnailPath", skip_serializing_if = "Option::is_none")]
    pub thumbnail_path: Option<String>,
    #[serde(rename = "createdAt", skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ChatMessage {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    #[serde(alias = "senderName")]
    pub name: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "String::is_empty")]
    pub content: String,
    #[serde(default)]
    pub timestamp: u64,
    #[serde(rename = "updatedAt", skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "isThinking")]
    #[serde(default)]
    pub is_thinking: Option<bool>,

    #[serde(rename = "agentId", skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(rename = "groupId", skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(rename = "topicId", skip_serializing_if = "Option::is_none")]
    pub topic_id: Option<String>,
    #[serde(rename = "isGroupMessage", skip_serializing_if = "Option::is_none")]
    pub is_group_message: Option<bool>,
    #[serde(rename = "finishReason", skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<Attachment>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocks: Option<serde_json::Value>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<crate::vcp_modules::pre_renderer::MessageShell>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HistoryChunk {
    pub message: ChatMessage,
    pub index: usize,
    pub is_last: bool,
}

// --- 历史记录存取逻辑 ---

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn load_chat_history_streamed(
    app_handle: tauri::AppHandle,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    limit: Option<usize>,
    offset: Option<usize>,
    before_timestamp: Option<i64>,
    before_message_id: Option<String>,
    on_message: tauri::ipc::Channel<HistoryChunk>,
) -> Result<usize, String> {
    let messages = crate::vcp_modules::message_service::load_chat_history_internal(
        &app_handle,
        &owner_id,
        &owner_type,
        &topic_id,
        limit,
        offset,
        before_timestamp,
        before_message_id.as_deref(),
        false,
        false, // include_extracted_text: 前端列表加载不需要大体积的提取文本内容
    )
    .await?;

    let total = messages.len();
    for (index, message) in messages.into_iter().enumerate() {
        let is_last = index == total.saturating_sub(1);
        let _ = on_message.send(HistoryChunk {
            message,
            index,
            is_last,
        });
    }
    Ok(total)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn load_chat_history(
    app_handle: tauri::AppHandle,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    limit: Option<usize>,
    offset: Option<usize>,
    before_timestamp: Option<i64>,
    before_message_id: Option<String>,
) -> Result<Vec<ChatMessage>, String> {
    crate::vcp_modules::message_service::load_chat_history_internal(
        &app_handle,
        &owner_id,
        &owner_type,
        &topic_id,
        limit,
        offset,
        before_timestamp,
        before_message_id.as_deref(),
        false,
        false,
    )
    .await
}

/// 锚点加载：以指定消息为中心加载前后上下文窗口（全局搜索跳转定位用）。
/// 返回按 (timestamp, msg_id) 升序排列的消息列表。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn load_chat_history_around(
    app_handle: tauri::AppHandle,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    anchor_msg_id: String,
    before_n: Option<usize>,
    after_n: Option<usize>,
) -> Result<Vec<ChatMessage>, String> {
    crate::vcp_modules::message_service::load_chat_history_around_internal(
        &app_handle,
        &owner_id,
        &owner_type,
        &topic_id,
        &anchor_msg_id,
        before_n.unwrap_or(10).min(50),
        after_n.unwrap_or(10).min(50),
    )
    .await
}

#[tauri::command]
pub async fn append_single_message(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, crate::vcp_modules::db_manager::DbState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    message: ChatMessage,
) -> Result<Vec<ContentBlock>, String> {
    message_service::append_single_message(
        app_handle,
        &db_state.pool,
        &owner_id,
        &owner_type,
        topic_id,
        message,
    )
    .await
}

#[tauri::command]
pub async fn patch_single_message(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, crate::vcp_modules::db_manager::DbState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    message: ChatMessage,
) -> Result<Vec<ContentBlock>, String> {
    message_service::patch_single_message(
        app_handle,
        &db_state.pool,
        &owner_id,
        &owner_type,
        topic_id,
        message,
        false,
    )
    .await
}

#[tauri::command]
pub async fn delete_messages(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, crate::vcp_modules::db_manager::DbState>,
    active_requests: tauri::State<'_, crate::vcp_modules::vcp_client::ActiveRequests>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    msg_ids: Vec<String>,
) -> Result<(), String> {
    let key = TopicKey::new(owner_type, owner_id, &topic_id);
    let result = message_service::delete_messages(&db_state.pool, &key, msg_ids, None).await?;
    for msg_id in &result.active_ids {
        if let Err(error) = active_requests.cancel(&MessageKey::new(key.clone(), msg_id)) {
            log::warn!("Failed to cancel deleted generation {}: {}", msg_id, error);
        }
    }
    notify_message_deletions(&app_handle, &key, &result);
    Ok(())
}

pub fn notify_message_deletions<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    topic: &TopicKey,
    result: &message_service::MessageDeletionResult,
) {
    if let Some(sync_state) = app_handle.try_state::<SyncState>() {
        for message_id in &result.deleted_ids {
            let _ = sync_state.ws_sender.send(SyncCommand::NotifyMessageDelete {
                key: MessageKey::new(topic.clone(), message_id),
                deleted_at: result.deleted_at,
            });
        }
    }
}

#[tauri::command]
pub async fn truncate_history_after_timestamp(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, crate::vcp_modules::db_manager::DbState>,
    active_requests: tauri::State<'_, crate::vcp_modules::vcp_client::ActiveRequests>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    timestamp: i64,
) -> Result<(), String> {
    let key = TopicKey::new(owner_type, owner_id, &topic_id);
    let deletion = message_service::truncate_history_after_timestamp(
        app_handle.clone(),
        &db_state.pool,
        &key,
        timestamp,
    )
    .await?;
    for msg_id in &deletion.active_ids {
        if let Err(error) = active_requests.cancel(&MessageKey::new(key.clone(), msg_id)) {
            log::warn!(
                "Failed to cancel truncated generation {}: {}",
                msg_id,
                error
            );
        }
    }
    notify_message_deletions(&app_handle, &key, &deletion);
    Ok(())
}

// --- 增量同步逻辑 (Delta Sync) (Moved to sync_manager.rs) ---
