use crate::vcp_modules::message_service;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Attachment {
    pub r#type: String,
    #[serde(default)]
    pub src: String,
    pub name: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,

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
    pub content: String,
    #[serde(default)]
    pub timestamp: u64,
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

    #[serde(default)]
    pub attachments: Option<Vec<Attachment>>,
}

// --- 历史记录存取逻辑 ---

#[tauri::command]
pub async fn load_chat_history(
    app_handle: tauri::AppHandle,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<ChatMessage>, String> {
    crate::vcp_modules::message_service::load_chat_history_internal(
        &app_handle,
        &owner_id,
        &owner_type,
        &topic_id,
        limit,
        offset,
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
) -> Result<(), String> {
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
) -> Result<(), String> {
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
    db_state: tauri::State<'_, crate::vcp_modules::db_manager::DbState>,
    topic_id: String,
    msg_ids: Vec<String>,
) -> Result<(), String> {
    message_service::delete_messages(&db_state.pool, &topic_id, msg_ids).await
}

#[tauri::command]
pub async fn truncate_history_after_timestamp(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, crate::vcp_modules::db_manager::DbState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    timestamp: i64,
) -> Result<(), String> {
    message_service::truncate_history_after_timestamp(
        app_handle,
        &db_state.pool,
        &owner_id,
        &owner_type,
        &topic_id,
        timestamp,
    )
    .await
}

// --- 增量同步逻辑 (Delta Sync) (Moved to sync_manager.rs) ---
