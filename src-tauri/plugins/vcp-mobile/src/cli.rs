//! Internal Android bridge for the VCPMobileCLI process host.
//!
//! These functions are intentionally not Tauri commands. The application-level
//! Rust runtime remains the sole job owner and calls this bridge directly.

#[cfg(target_os = "android")]
use crate::VcpMobileState;
use serde::{Deserialize, Serialize};
#[cfg(target_os = "android")]
use tauri::Manager;
use tauri::{AppHandle, Runtime};

#[cfg(not(any(target_os = "android", target_os = "ios")))]
const PROCESS_HOST_UNAVAILABLE: &str = "VCPMobileCLI ProcessHost is only available on Android";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrepareCliRuntimeRequest {
    pub operation_id: String,
    pub profile_id: String,
    pub runtime_generation: u64,
    pub rootfs_archive_bytes: u64,
    pub rootfs_archive_sha256: String,
    pub proot_bytes: u64,
    pub proot_sha256: String,
    pub proot_loader_bytes: u64,
    pub proot_loader_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrepareCliRuntimeResponse {
    pub operation_id: String,
    pub profile_id: String,
    pub runtime_generation: u64,
    pub archive_path: String,
    pub rootfs_parent_path: String,
    pub workspace_path: String,
    pub skills_path: String,
    pub output_path: String,
    pub projection_root_path: String,
    pub proot_path: String,
    pub proot_loader_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartCliProcessRequest {
    pub operation_id: String,
    pub job_id: String,
    pub attempt_id: String,
    pub runtime_generation: u64,
    pub command: String,
    pub rootfs_path: String,
    pub cwd: String,
    pub artifact_max_bytes: u64,
    pub background_lease: bool,
    pub timeout_ms: u64,
    pub display_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartCliProcessResponse {
    pub operation_id: String,
    pub job_id: String,
    pub attempt_id: String,
    pub runtime_generation: u64,
    pub pid: i32,
    pub pgid: i32,
    pub session_id: i32,
    pub start_time_ticks: u64,
    pub stdout_path: String,
    pub stderr_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InspectCliProcessRequest {
    pub operation_id: String,
    pub job_id: String,
    pub attempt_id: String,
    pub runtime_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CliProcessState {
    Running,
    Exited,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InspectCliProcessResponse {
    pub operation_id: String,
    pub job_id: String,
    pub attempt_id: String,
    pub runtime_generation: u64,
    pub state: CliProcessState,
    pub exit_code: Option<i32>,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub background_lease_lost: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CancelCliProcessRequest {
    pub operation_id: String,
    pub job_id: String,
    pub attempt_id: String,
    pub runtime_generation: u64,
    pub grace_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CancelCliProcessResponse {
    pub operation_id: String,
    pub job_id: String,
    pub attempt_id: String,
    pub runtime_generation: u64,
    pub found: bool,
    pub term_sent: bool,
    pub kill_sent: bool,
    pub group_gone: bool,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenCliPtyRequest {
    pub operation_id: String,
    pub runtime_generation: u64,
    pub rootfs_path: String,
    pub cwd: String,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenCliPtyResponse {
    pub operation_id: String,
    pub session_id: String,
    pub session_generation: u64,
    pub runtime_generation: u64,
    pub pid: i32,
    pub cwd: String,
    pub shell: String,
    pub state: String,
    pub exit_code: Option<i32>,
    pub cursor: u64,
    pub replay_base64: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadCliPtyRequest {
    pub operation_id: String,
    pub session_id: String,
    pub session_generation: u64,
    pub cursor: u64,
    pub max_bytes: u32,
    pub wait_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadCliPtyResponse {
    pub operation_id: String,
    pub session_id: String,
    pub session_generation: u64,
    pub cursor: u64,
    pub data_base64: String,
    pub timed_out: bool,
    pub eof: bool,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WriteCliPtyRequest {
    pub operation_id: String,
    pub session_id: String,
    pub session_generation: u64,
    pub data_base64: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WriteCliPtyResponse {
    pub operation_id: String,
    pub session_id: String,
    pub session_generation: u64,
    pub written_bytes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResizeCliPtyRequest {
    pub operation_id: String,
    pub session_id: String,
    pub session_generation: u64,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResizeCliPtyResponse {
    pub operation_id: String,
    pub session_id: String,
    pub session_generation: u64,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloseCliPtyRequest {
    pub operation_id: String,
    pub session_id: String,
    pub session_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloseCliPtyResponse {
    pub operation_id: String,
    pub session_id: String,
    pub session_generation: u64,
    pub closed: bool,
}

pub async fn prepare_cli_runtime_inner<R: Runtime>(
    app: &AppHandle<R>,
    request: &PrepareCliRuntimeRequest,
) -> Result<PrepareCliRuntimeResponse, String> {
    #[cfg(target_os = "android")]
    {
        let handle = app.state::<VcpMobileState<R>>().mobile_plugin_handle()?;
        return handle
            .run_mobile_plugin_async("prepareCliRuntime", request)
            .await
            .map_err(|error| format!("prepareCliRuntime failed: {error}"));
    }

    #[cfg(target_os = "ios")]
    {
        let _ = (app, request);
        crate::ios_unsupported("prepare_cli_runtime")
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = (app, request);
        Err(PROCESS_HOST_UNAVAILABLE.to_string())
    }
}

pub async fn start_cli_process_inner<R: Runtime>(
    app: &AppHandle<R>,
    request: &StartCliProcessRequest,
) -> Result<StartCliProcessResponse, String> {
    #[cfg(target_os = "android")]
    {
        let handle = app.state::<VcpMobileState<R>>().mobile_plugin_handle()?;
        return handle
            .run_mobile_plugin_async("startCliProcess", request)
            .await
            .map_err(|error| format!("startCliProcess failed: {error}"));
    }

    #[cfg(target_os = "ios")]
    {
        let _ = (app, request);
        crate::ios_unsupported("start_cli_process")
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = (app, request);
        Err(PROCESS_HOST_UNAVAILABLE.to_string())
    }
}

pub async fn inspect_cli_process_inner<R: Runtime>(
    app: &AppHandle<R>,
    request: &InspectCliProcessRequest,
) -> Result<InspectCliProcessResponse, String> {
    #[cfg(target_os = "android")]
    {
        let handle = app.state::<VcpMobileState<R>>().mobile_plugin_handle()?;
        return handle
            .run_mobile_plugin_async("inspectCliProcess", request)
            .await
            .map_err(|error| format!("inspectCliProcess failed: {error}"));
    }

    #[cfg(target_os = "ios")]
    {
        let _ = (app, request);
        crate::ios_unsupported("inspect_cli_process")
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = (app, request);
        Err(PROCESS_HOST_UNAVAILABLE.to_string())
    }
}

pub async fn cancel_cli_process_inner<R: Runtime>(
    app: &AppHandle<R>,
    request: &CancelCliProcessRequest,
) -> Result<CancelCliProcessResponse, String> {
    #[cfg(target_os = "android")]
    {
        let handle = app.state::<VcpMobileState<R>>().mobile_plugin_handle()?;
        return handle
            .run_mobile_plugin_async("cancelCliProcess", request)
            .await
            .map_err(|error| format!("cancelCliProcess failed: {error}"));
    }

    #[cfg(target_os = "ios")]
    {
        let _ = (app, request);
        crate::ios_unsupported("cancel_cli_process")
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = (app, request);
        Err(PROCESS_HOST_UNAVAILABLE.to_string())
    }
}

macro_rules! pty_bridge {
    ($name:ident, $method:literal, $request:ty, $response:ty) => {
        pub async fn $name<R: Runtime>(
            app: &AppHandle<R>,
            request: &$request,
        ) -> Result<$response, String> {
            #[cfg(target_os = "android")]
            {
                let handle = app.state::<VcpMobileState<R>>().mobile_plugin_handle()?;
                return handle
                    .run_mobile_plugin_async($method, request)
                    .await
                    .map_err(|error| format!(concat!($method, " failed: {}"), error));
            }
            #[cfg(target_os = "ios")]
            {
                let _ = (app, request);
                crate::ios_unsupported(stringify!($name))
            }
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            {
                let _ = (app, request);
                Err(PROCESS_HOST_UNAVAILABLE.to_string())
            }
        }
    };
}

pty_bridge!(
    open_cli_pty_inner,
    "openCliPty",
    OpenCliPtyRequest,
    OpenCliPtyResponse
);
pty_bridge!(
    read_cli_pty_inner,
    "readCliPty",
    ReadCliPtyRequest,
    ReadCliPtyResponse
);
pty_bridge!(
    write_cli_pty_inner,
    "writeCliPty",
    WriteCliPtyRequest,
    WriteCliPtyResponse
);
pty_bridge!(
    resize_cli_pty_inner,
    "resizeCliPty",
    ResizeCliPtyRequest,
    ResizeCliPtyResponse
);
pty_bridge!(
    close_cli_pty_inner,
    "closeCliPty",
    CloseCliPtyRequest,
    CloseCliPtyResponse
);
