use crate::vcp_modules::sync_protocol::SyncCommand;
use crate::vcp_modules::sync_service::SyncState;
use crate::vcp_modules::sync_session::run_sync_session;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, State};
use tokio::sync::mpsc;

#[tauri::command]
pub async fn get_sync_status(state: State<'_, SyncState>) -> Result<String, String> {
    Ok(state.connection_status.read().await.clone())
}

#[tauri::command]
pub async fn start_manual_sync(
    handle: AppHandle,
    state: State<'_, SyncState>,
) -> Result<(), String> {
    if state
        .is_syncing
        .swap(true, Ordering::SeqCst)
    {
        return Err("同步已在进行中".to_string());
    }

    let (tx, rx) = mpsc::unbounded_channel::<SyncCommand>();

    let app_handle = handle.clone();
    let connection_status = state.connection_status.clone();
    let is_syncing = state.is_syncing.clone();

    let tx_cmd = tx.clone();
    tauri::async_runtime::spawn(async move {
        run_sync_session(app_handle, tx, rx, connection_status).await;
        is_syncing.store(false, Ordering::SeqCst);
    });

    tx_cmd
        .send(SyncCommand::StartManualSync)
        .map_err(|e| e.to_string())
}

#[derive(Debug, serde::Serialize)]
pub struct SyncLogFileInfo {
    pub filename: String,
    pub created_at: u64,
    pub size_bytes: u64,
}

#[tauri::command]
pub async fn list_sync_log_files(app: AppHandle) -> Result<Vec<SyncLogFileInfo>, String> {
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|e| e.to_string())?
        .join("sync_logs");
    if !log_dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    let mut read_dir = tokio::fs::read_dir(&log_dir)
        .await
        .map_err(|e| e.to_string())?;
    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let metadata = entry.metadata().await.map_err(|e| e.to_string())?;
        if metadata.is_file() {
            let filename = entry.file_name().to_string_lossy().to_string();
            let created_at = metadata
                .created()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            entries.push(SyncLogFileInfo {
                filename,
                created_at,
                size_bytes: metadata.len(),
            });
        }
    }

    entries.sort_by_key(|b| std::cmp::Reverse(b.created_at));
    Ok(entries)
}

#[tauri::command]
pub async fn get_sync_session_log_path(
    state: State<'_, SyncState>,
) -> Result<Option<String>, String> {
    let guard = state.current_log_path.read().await;
    Ok(guard.clone())
}

#[tauri::command]
pub async fn read_sync_log_file(app: AppHandle, filename: String) -> Result<String, String> {
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|e| e.to_string())?
        .join("sync_logs");
    let file_path = log_dir.join(&filename);

    let canonical_dir = log_dir.canonicalize().map_err(|e| e.to_string())?;
    let canonical_file = file_path.canonicalize().map_err(|e| e.to_string())?;
    if !canonical_file.starts_with(&canonical_dir) {
        return Err("Invalid file path".to_string());
    }

    let content = tokio::fs::read_to_string(&canonical_file)
        .await
        .map_err(|e| e.to_string())?;
    Ok(content)
}

#[tauri::command]
pub async fn clear_old_sync_logs(app: AppHandle, keep_days: u32) -> Result<u32, String> {
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|e| e.to_string())?
        .join("sync_logs");
    if !log_dir.exists() {
        return Ok(0);
    }

    let cutoff =
        std::time::SystemTime::now() - std::time::Duration::from_secs(keep_days as u64 * 86400);
    let mut removed = 0u32;

    let mut read_dir = tokio::fs::read_dir(&log_dir)
        .await
        .map_err(|e| e.to_string())?;
    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let metadata = entry.metadata().await.map_err(|e| e.to_string())?;
        if metadata.is_file() {
            let modified = metadata
                .modified()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            if modified < cutoff {
                let _ = tokio::fs::remove_file(entry.path()).await;
                removed += 1;
            }
        }
    }

    Ok(removed)
}
