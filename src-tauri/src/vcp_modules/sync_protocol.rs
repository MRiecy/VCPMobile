use serde_json::json;
use tauri::{AppHandle, Emitter, Runtime};

pub enum SyncCommand {
    NotifyLocalChange {
        id: String,
        data_type: crate::vcp_modules::sync_types::SyncDataType,
        hash: String,
        ts: i64,
    },
    StartTopicMetadata,
    StartTopicValidation,
    StartMessages,
    Finalize,
    NotifyDelete {
        data_type: crate::vcp_modules::sync_types::SyncDataType,
        id: String,
    },
    StartManualSync,
}

pub enum PipelineCommand {
    StartTopicMetadata,
    StartTopicValidation,
    StartMessages,
    Finalize,
}

pub fn emit_sync_log<R: Runtime>(app_handle: &AppHandle<R>, level: &str, message: &str) {
    let _ = app_handle.emit(
        "vcp-log",
        json!({
            "id": format!("{}_{}", level, chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)),
            "level": level,
            "category": "sync",
            "message": message,
        }),
    );
}
