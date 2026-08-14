use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime, State};
use tauri_plugin_vcp_mobile::cli::{
    close_cli_pty_inner, open_cli_pty_inner, read_cli_pty_inner, resize_cli_pty_inner,
    write_cli_pty_inner, CloseCliPtyRequest, CloseCliPtyResponse, OpenCliPtyRequest,
    OpenCliPtyResponse, ReadCliPtyRequest, ReadCliPtyResponse, ResizeCliPtyRequest,
    ResizeCliPtyResponse, WriteCliPtyRequest, WriteCliPtyResponse,
};

use super::runtime::MobileCliRuntimeState;

const MAX_PTY_READ_BYTES: u32 = 64 * 1024;
const MAX_PTY_WRITE_BYTES: usize = 16 * 1024;
const MAX_PTY_WAIT_MS: u32 = 1_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct OpenVcpMobileCliTerminalRequest {
    pub operation_id: String,
    pub cwd: String,
    pub rows: u16,
    pub cols: u16,
}

fn validate_identity(
    operation_id: &str,
    session_id: Option<&str>,
    generation: u64,
) -> Result<(), String> {
    if operation_id.is_empty() || operation_id.len() > 256 || operation_id.contains('\0') {
        return Err("invalid terminal operation_id".to_string());
    }
    if let Some(session_id) = session_id {
        if !session_id.starts_with("pty-") || session_id.len() > 256 || session_id.contains('\0') {
            return Err("invalid terminal session_id".to_string());
        }
        if generation == 0 {
            return Err("terminal session_generation must be positive".to_string());
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn open_vcp_mobile_cli_terminal<R: Runtime>(
    app: AppHandle<R>,
    runtime: State<'_, MobileCliRuntimeState>,
    request: OpenVcpMobileCliTerminalRequest,
) -> Result<OpenCliPtyResponse, String> {
    validate_identity(&request.operation_id, None, 0)?;
    if !(1..=1_000).contains(&request.rows) || !(1..=1_000).contains(&request.cols) {
        return Err("terminal dimensions are outside the supported range".to_string());
    }
    let (runtime_generation, rootfs_path) = runtime
        .terminal_runtime(&app, &request.operation_id)
        .await?;
    let response = open_cli_pty_inner(
        &app,
        &OpenCliPtyRequest {
            operation_id: request.operation_id.clone(),
            runtime_generation,
            rootfs_path,
            cwd: request.cwd,
            rows: request.rows,
            cols: request.cols,
        },
    )
    .await?;
    if response.operation_id != request.operation_id
        || response.runtime_generation != runtime_generation
        || response.session_generation == 0
    {
        return Err("terminal open response identity mismatch".to_string());
    }
    let replay = STANDARD
        .decode(&response.replay_base64)
        .map_err(|_| "terminal open returned invalid replay base64".to_string())?;
    if replay.len() > 128 * 1024 {
        return Err("terminal replay exceeds its bounded scrollback".to_string());
    }
    Ok(response)
}

#[tauri::command]
pub async fn read_vcp_mobile_cli_terminal<R: Runtime>(
    app: AppHandle<R>,
    request: ReadCliPtyRequest,
) -> Result<ReadCliPtyResponse, String> {
    validate_identity(
        &request.operation_id,
        Some(&request.session_id),
        request.session_generation,
    )?;
    if request.max_bytes == 0
        || request.max_bytes > MAX_PTY_READ_BYTES
        || request.wait_ms > MAX_PTY_WAIT_MS
    {
        return Err("terminal read budget is outside the supported range".to_string());
    }
    let response = read_cli_pty_inner(&app, &request).await?;
    if response.operation_id != request.operation_id
        || response.session_id != request.session_id
        || response.session_generation != request.session_generation
        || response.cursor < request.cursor
    {
        return Err("terminal read response identity mismatch".to_string());
    }
    let decoded = STANDARD
        .decode(&response.data_base64)
        .map_err(|_| "terminal read returned invalid base64".to_string())?;
    if decoded.len() > request.max_bytes as usize
        || response.cursor != request.cursor.saturating_add(decoded.len() as u64)
    {
        return Err("terminal read returned an invalid bounded cursor".to_string());
    }
    Ok(response)
}

#[tauri::command]
pub async fn write_vcp_mobile_cli_terminal<R: Runtime>(
    app: AppHandle<R>,
    request: WriteCliPtyRequest,
) -> Result<WriteCliPtyResponse, String> {
    validate_identity(
        &request.operation_id,
        Some(&request.session_id),
        request.session_generation,
    )?;
    let decoded = STANDARD
        .decode(&request.data_base64)
        .map_err(|_| "terminal write data is not valid base64".to_string())?;
    if decoded.is_empty() || decoded.len() > MAX_PTY_WRITE_BYTES {
        return Err("terminal write is outside the supported range".to_string());
    }
    let response = write_cli_pty_inner(&app, &request).await?;
    if response.operation_id != request.operation_id
        || response.session_id != request.session_id
        || response.session_generation != request.session_generation
        || response.written_bytes as usize != decoded.len()
    {
        return Err("terminal write response identity mismatch".to_string());
    }
    Ok(response)
}

#[tauri::command]
pub async fn resize_vcp_mobile_cli_terminal<R: Runtime>(
    app: AppHandle<R>,
    request: ResizeCliPtyRequest,
) -> Result<ResizeCliPtyResponse, String> {
    validate_identity(
        &request.operation_id,
        Some(&request.session_id),
        request.session_generation,
    )?;
    if !(1..=1_000).contains(&request.rows) || !(1..=1_000).contains(&request.cols) {
        return Err("terminal dimensions are outside the supported range".to_string());
    }
    let response = resize_cli_pty_inner(&app, &request).await?;
    if response.operation_id != request.operation_id
        || response.session_id != request.session_id
        || response.session_generation != request.session_generation
        || response.rows != request.rows
        || response.cols != request.cols
    {
        return Err("terminal resize response identity mismatch".to_string());
    }
    Ok(response)
}

#[tauri::command]
pub async fn close_vcp_mobile_cli_terminal<R: Runtime>(
    app: AppHandle<R>,
    request: CloseCliPtyRequest,
) -> Result<CloseCliPtyResponse, String> {
    validate_identity(
        &request.operation_id,
        Some(&request.session_id),
        request.session_generation,
    )?;
    let response = close_cli_pty_inner(&app, &request).await?;
    if response.operation_id != request.operation_id
        || response.session_id != request.session_id
        || response.session_generation != request.session_generation
        || !response.closed
    {
        return Err("terminal close response identity mismatch".to_string());
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_identity_and_budgets_fail_closed() {
        assert!(validate_identity("open-1", None, 0).is_ok());
        assert!(validate_identity("read-1", Some("pty-session"), 1).is_ok());
        assert!(validate_identity("read-1", Some("job-session"), 1).is_err());
        assert!(validate_identity("read-1", Some("pty-session"), 0).is_err());
        assert!(STANDARD.decode("not base64!").is_err());
        assert_eq!(MAX_PTY_READ_BYTES, 65_536);
        assert_eq!(MAX_PTY_WRITE_BYTES, 16_384);
    }
}
