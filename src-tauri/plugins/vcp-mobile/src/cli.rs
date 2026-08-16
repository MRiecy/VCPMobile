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

#[cfg(not(target_os = "android"))]
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

    #[cfg(not(target_os = "android"))]
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

    #[cfg(not(target_os = "android"))]
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

    #[cfg(not(target_os = "android"))]
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

    #[cfg(not(target_os = "android"))]
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
            #[cfg(not(target_os = "android"))]
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

#[cfg(test)]
mod tests {
    use super::{
        CliProcessState, PrepareCliRuntimeRequest, PrepareCliRuntimeResponse,
        StartCliProcessRequest,
    };

    #[test]
    fn prepare_request_freezes_both_apk_native_executable_identities() {
        let request = PrepareCliRuntimeRequest {
            operation_id: "operation-1".to_string(),
            profile_id: "profile-1".to_string(),
            runtime_generation: 7,
            rootfs_archive_bytes: 10,
            rootfs_archive_sha256: "1".repeat(64),
            proot_bytes: 20,
            proot_sha256: "2".repeat(64),
            proot_loader_bytes: 30,
            proot_loader_sha256: "3".repeat(64),
        };

        let value = serde_json::to_value(request).expect("serialize prepare request");
        assert_eq!(value["prootBytes"], 20);
        assert_eq!(value["prootSha256"], "2".repeat(64));
        assert_eq!(value["prootLoaderBytes"], 30);
        assert_eq!(value["prootLoaderSha256"], "3".repeat(64));
        assert!(value.get("proot_loader_bytes").is_none());
    }

    #[test]
    fn prepare_response_requires_projection_owner_root() {
        let value = serde_json::json!({
            "operationId": "operation-1",
            "profileId": "profile-1",
            "runtimeGeneration": 7,
            "archivePath": "/private/assets/rootfs.tar.zst",
            "rootfsParentPath": "/private/rootfs",
            "workspacePath": "/private/workspace",
            "skillsPath": "/private/skills",
            "outputPath": "/private/output",
            "projectionRootPath": "/private/projections",
            "prootPath": "/native/libvcp_proot.so",
            "prootLoaderPath": "/native/libvcp_proot_loader.so"
        });
        let response: PrepareCliRuntimeResponse =
            serde_json::from_value(value.clone()).expect("deserialize prepare response");

        assert_eq!(response.projection_root_path, "/private/projections");
        let mut missing_projection_root = value;
        missing_projection_root
            .as_object_mut()
            .expect("response object")
            .remove("projectionRootPath");
        assert!(
            serde_json::from_value::<PrepareCliRuntimeResponse>(missing_projection_root).is_err()
        );
    }

    #[test]
    fn start_request_uses_camel_case_and_keeps_command_as_data() {
        let request = StartCliProcessRequest {
            operation_id: "operation-1".to_string(),
            job_id: "job-1".to_string(),
            attempt_id: "attempt-1".to_string(),
            runtime_generation: 7,
            command: "printf '%s' '$HOME'".to_string(),
            rootfs_path: "/private/rootfs".to_string(),
            cwd: "/workspace/topic".to_string(),
            artifact_max_bytes: 1024,
            background_lease: true,
            timeout_ms: 30_000,
            display_label: "run unit test".to_string(),
            session_id: Some("dist-session:abc".to_string()),
        };

        let value = serde_json::to_value(request).expect("serialize request");
        assert_eq!(value["operationId"], "operation-1");
        assert_eq!(value["runtimeGeneration"], 7);
        assert_eq!(value["artifactMaxBytes"], 1024);
        assert_eq!(value["backgroundLease"], true);
        assert_eq!(value["timeoutMs"], 30_000);
        assert_eq!(value["displayLabel"], "run unit test");
        assert_eq!(value["sessionId"], "dist-session:abc");
        assert_eq!(value["command"], "printf '%s' '$HOME'");
        assert!(value.get("riverContextProjection").is_none());
        assert!(value.get("operation_id").is_none());
    }

    #[test]
    fn start_request_accepts_absent_session_id() {
        let value = serde_json::json!({
            "operationId": "operation-1",
            "jobId": "job-1",
            "attemptId": "attempt-1",
            "runtimeGeneration": 7,
            "command": "true",
            "rootfsPath": "/private/rootfs",
            "cwd": "/workspace",
            "artifactMaxBytes": 1024,
            "backgroundLease": true,
            "timeoutMs": 30_000,
            "displayLabel": "no session"
        });
        let request: StartCliProcessRequest =
            serde_json::from_value(value).expect("deserialize without sessionId");
        assert_eq!(request.session_id, None);
    }

    #[test]
    fn process_state_wire_values_are_stable() {
        assert_eq!(
            serde_json::to_string(&CliProcessState::Running).expect("serialize state"),
            "\"running\""
        );
        assert_eq!(
            serde_json::to_string(&CliProcessState::Exited).expect("serialize state"),
            "\"exited\""
        );
        assert_eq!(
            serde_json::to_string(&CliProcessState::Missing).expect("serialize state"),
            "\"missing\""
        );
    }
}
