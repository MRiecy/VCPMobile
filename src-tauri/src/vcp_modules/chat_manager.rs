use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::message_service;
use crate::vcp_modules::topic_sync_service::{
    get_topic_delta_internal, get_topic_fingerprint_internal, TopicDelta, TopicFingerprint,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Attachment {
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub src: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub hash: Option<String>,
    #[serde(default)]
    #[serde(rename = "extractedText")]
    pub extracted_text: Option<String>,
    #[serde(default)]
    #[serde(rename = "thumbnailPath")]
    pub thumbnail_path: Option<String>,
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
    #[serde(alias = "thinking")]
    #[serde(default)]
    pub is_thinking: Option<bool>,
    #[serde(default)]
    pub attachments: Option<Vec<Attachment>>,
    /// 捕获所有其他未定义的字段
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

// --- 正则处理核心逻辑 (从 chatManager.js 权力下沉) ---

/// 对话深度计算逻辑 (对齐 JS 逻辑)
/// 在 VCP 中，从最新消息往回算
#[allow(dead_code)]
pub fn calculate_depth(history_len: usize, current_index: usize) -> i32 {
    if current_index >= history_len {
        return -1;
    }
    (history_len - 1 - current_index) as i32
}

#[tauri::command]
pub async fn process_regex_for_message(
    db_state: State<'_, DbState>,
    agent_id: String,
    content: String,
    scope: String,
    role: String,
    depth: i32,
) -> Result<String, String> {
    crate::vcp_modules::regex_service::apply_regex_rules(
        &db_state, &agent_id, &content, &scope, &role, depth,
    )
    .await
}

// --- 历史记录存取逻辑 ---

#[tauri::command]
pub async fn load_chat_history(
    app_handle: AppHandle,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<ChatMessage>, String> {
    message_service::load_chat_history_internal(
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
pub async fn save_chat_history(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    history: Vec<ChatMessage>,
) -> Result<(), String> {
    message_service::save_chat_history_internal(
        &app_handle,
        &db_state,
        &owner_id,
        &owner_type,
        &topic_id,
        &history,
    )
    .await
}

#[tauri::command]
pub async fn append_single_message(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
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
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
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
    )
    .await
}

#[tauri::command]
pub async fn delete_messages(
    db_state: State<'_, DbState>,
    topic_id: String,
    msg_ids: Vec<String>,
) -> Result<(), String> {
    message_service::delete_messages(&db_state.pool, &topic_id, msg_ids).await
}

#[tauri::command]
pub async fn truncate_history_after_timestamp(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
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

// --- 增量同步逻辑 (Delta Sync) ---

// --- 指纹与同步优化 ---

#[tauri::command]
pub async fn get_topic_fingerprint(
    app_handle: AppHandle,
    _owner_id: String,
    _owner_type: String,
    topic_id: String,
) -> Result<TopicFingerprint, String> {
    get_topic_fingerprint_internal(&app_handle, &topic_id).await
}

/// 对比内存中的历史记录与磁盘文件，计算增量更新 (Delta)
#[tauri::command]
pub async fn get_topic_delta(
    app_handle: AppHandle,
    _db_state: State<'_, DbState>,
    _owner_id: String,
    _owner_type: String,
    topic_id: String,
    current_history: Vec<ChatMessage>,
    fingerprint: Option<TopicFingerprint>,
) -> Result<TopicDelta, String> {
    get_topic_delta_internal(&app_handle, &topic_id, current_history, fingerprint).await
}
