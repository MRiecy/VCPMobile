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

#[cfg(test)]
mod tests {
    use super::{CliProcessState, PrepareCliRuntimeRequest, StartCliProcessRequest};

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
        };

        let value = serde_json::to_value(request).expect("serialize request");
        assert_eq!(value["operationId"], "operation-1");
        assert_eq!(value["runtimeGeneration"], 7);
        assert_eq!(value["artifactMaxBytes"], 1024);
        assert_eq!(value["command"], "printf '%s' '$HOME'");
        assert!(value.get("operation_id").is_none());
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
