use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::message_application_service::append_single_message;
use crate::vcp_modules::path_topology_service::resolve_jsonl_path;
use tauri::AppHandle;
use std::fs;
use std::io::{BufRead, BufReader};

pub struct MessageSyncProjectionService;

impl MessageSyncProjectionService {
    /// Desktop -> Mobile: Import from history.json (Array) to history.jsonl
    pub async fn import_from_json(
        app_handle: &AppHandle,
        db_pool: &sqlx::Pool<sqlx::Sqlite>,
        item_id: &str,
        topic_id: &str,
        json_path: &std::path::Path,
    ) -> Result<(), String> {
        if !json_path.exists() {
            return Ok(());
        }

        let content = fs::read_to_string(json_path).map_err(|e| e.to_string())?;
        let history: Vec<ChatMessage> = serde_json::from_str(&content).map_err(|e| e.to_string())?;

        for msg in history {
            // Re-use append_single_message for idempotency and AOT compilation
            append_single_message(
                app_handle.clone(),
                db_pool,
                None, // Don't signal internal save back to watcher during sync
                item_id.to_string(),
                topic_id.to_string(),
                msg
            ).await?;
        }

        Ok(())
    }

    /// Mobile -> Desktop: Export from history.jsonl to history.json (Array)
    pub fn export_to_json(
        app_handle: &AppHandle,
        item_id: &str,
        topic_id: &str,
    ) -> Result<Vec<ChatMessage>, String> {
        let jsonl_path = resolve_jsonl_path(app_handle, item_id, topic_id);
        if !jsonl_path.exists() {
            return Ok(Vec::new());
        }

        let file = fs::File::open(jsonl_path).map_err(|e| e.to_string())?;
        let reader = BufReader::new(file);
        let mut history = Vec::new();

        for line in reader.lines() {
            let line = line.map_err(|e| e.to_string())?;
            if line.trim().is_empty() {
                continue;
            }
            let msg: ChatMessage = serde_json::from_str(&line).map_err(|e| e.to_string())?;
            history.push(msg);
        }

        Ok(history)
    }
}
