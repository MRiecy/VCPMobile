use crate::vcp_modules::app_settings_manager::AppSettingsState;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::topic_application_service;
pub use crate::vcp_modules::topic_repository_projection::Topic;
use crate::vcp_modules::topic_summary_service;
use tauri::{AppHandle, Runtime, State};

#[tauri::command]
pub async fn get_topics(
    db_state: tauri::State<'_, DbState>,
    item_id: String,
) -> Result<Vec<Topic>, String> {
    topic_application_service::get_topics(&db_state, &item_id).await
}

#[tauri::command]
pub async fn create_topic(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, DbState>,
    item_id: String,
    name: String,
) -> Result<Topic, String> {
    topic_application_service::create_topic(&app_handle, &db_state, &item_id, &name).await
}

#[tauri::command]
pub async fn delete_topic(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, DbState>,
    item_id: String,
    topic_id: String,
) -> Result<(), String> {
    topic_application_service::delete_topic(&app_handle, &db_state, &item_id, &topic_id).await
}

#[tauri::command]
pub async fn update_topic_title(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, DbState>,
    item_id: String,
    topic_id: String,
    title: String,
) -> Result<(), String> {
    topic_application_service::update_topic_title(
        &app_handle,
        &db_state,
        &item_id,
        &topic_id,
        &title,
    )
    .await
}

#[tauri::command]
pub async fn summarize_topic<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, AppSettingsState>,
    item_id: String,
    topic_id: String,
    agent_name: String,
) -> Result<String, String> {
    topic_summary_service::summarize_topic(
        app_handle,
        settings_state,
        item_id,
        topic_id,
        agent_name,
    )
    .await
}

#[tauri::command]
pub async fn toggle_topic_lock(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, DbState>,
    item_id: String,
    topic_id: String,
    locked: bool,
) -> Result<(), String> {
    topic_application_service::toggle_topic_lock(
        &app_handle,
        &db_state,
        &item_id,
        &topic_id,
        locked,
    )
    .await
}

#[tauri::command]
pub async fn set_topic_unread(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, DbState>,
    item_id: String,
    topic_id: String,
    unread: bool,
) -> Result<(), String> {
    topic_application_service::set_topic_unread(&app_handle, &db_state, &item_id, &topic_id, unread)
        .await
}
