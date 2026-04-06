use crate::vcp_modules::app_settings_manager::AppSettingsState;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::topic_service;
use tauri::State;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Topic {
    pub id: String,
    pub name: String,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    pub locked: bool,
    pub unread: bool,
    #[serde(rename = "unreadCount")]
    pub unread_count: i32,
    #[serde(rename = "msgCount")]
    pub msg_count: i32,
}

#[tauri::command]
pub async fn get_topics(
    db_state: tauri::State<'_, DbState>,
    owner_id: String,
    owner_type: String,
) -> Result<Vec<Topic>, String> {
    topic_service::get_topics(&db_state, &owner_id, &owner_type).await
}

#[tauri::command]
pub async fn create_topic(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    name: String,
) -> Result<Topic, String> {
    topic_service::create_topic(&app_handle, &db_state, &owner_id, &owner_type, &name).await
}

#[tauri::command]
pub async fn delete_topic(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
) -> Result<(), String> {
    topic_service::delete_topic(&app_handle, &db_state, &owner_id, &owner_type, &topic_id).await
}

#[tauri::command]
pub async fn update_topic_title(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    title: String,
) -> Result<(), String> {
    topic_service::update_topic_title(
        &app_handle,
        &db_state,
        &owner_id,
        &owner_type,
        &topic_id,
        &title,
    )
    .await
}

#[tauri::command]
pub async fn summarize_topic(
    app_handle: tauri::AppHandle,
    settings_state: State<'_, AppSettingsState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    agent_name: String,
) -> Result<String, String> {
    crate::vcp_modules::topic_summary_service::summarize_topic(
        app_handle,
        settings_state,
        owner_id,
        owner_type,
        topic_id,
        agent_name,
    )
    .await
}

#[tauri::command]
pub async fn toggle_topic_lock(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    locked: bool,
) -> Result<(), String> {
    topic_service::toggle_topic_lock(
        &app_handle,
        &db_state,
        &owner_id,
        &owner_type,
        &topic_id,
        locked,
    )
    .await
}

#[tauri::command]
pub async fn set_topic_unread(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    unread: bool,
) -> Result<(), String> {
    topic_service::set_topic_unread(
        &app_handle,
        &db_state,
        &owner_id,
        &owner_type,
        &topic_id,
        unread,
    )
    .await
}
