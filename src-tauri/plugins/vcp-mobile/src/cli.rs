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
pub struct PrepareCliSemanticAssetsRequest {
    pub operation_id: String,
    pub model_id: String,
    pub model_bytes: u64,
    pub model_sha256: String,
    pub tokenizer_bytes: u64,
    pub tokenizer_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrepareCliSemanticAssetsResponse {
    pub operation_id: String,
    pub model_id: String,
    pub model_path: String,
    pub tokenizer_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CliProjectedArtifact {
    pub host_path: String,
    pub guest_path: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CliRiverContextProjection {
    pub host_path: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub artifacts: Vec<CliProjectedArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CliVrefFileProjection {
    pub relative_name: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CliVrefProjection {
    pub host_dir: String,
    pub manifest_path: String,
    pub manifest_size_bytes: u64,
    pub manifest_sha256: String,
    pub files: Vec<CliVrefFileProjection>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub river_context_projection: Option<CliRiverContextProjection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vref_projection: Option<CliVrefProjection>,
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

pub async fn prepare_cli_semantic_assets_inner<R: Runtime>(
    app: &AppHandle<R>,
    request: &PrepareCliSemanticAssetsRequest,
) -> Result<PrepareCliSemanticAssetsResponse, String> {
    #[cfg(target_os = "android")]
    {
        let handle = app.state::<VcpMobileState<R>>().mobile_plugin_handle()?;
        return handle
            .run_mobile_plugin_async("prepareCliSemanticAssets", request)
            .await
            .map_err(|error| format!("prepareCliSemanticAssets failed: {error}"));
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
    use super::{
        CliProcessState, CliProjectedArtifact, CliRiverContextProjection, CliVrefFileProjection,
        CliVrefProjection, PrepareCliRuntimeRequest, PrepareCliRuntimeResponse,
        PrepareCliSemanticAssetsRequest, PrepareCliSemanticAssetsResponse, StartCliProcessRequest,
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
    fn semantic_asset_prepare_contract_is_camel_case_and_requires_both_paths() {
        let request = PrepareCliSemanticAssetsRequest {
            operation_id: "operation-1".to_string(),
            model_id: "model-r2".to_string(),
            model_bytes: 24_471_328,
            model_sha256: "4".repeat(64),
            tokenizer_bytes: 10_437_027,
            tokenizer_sha256: "5".repeat(64),
        };
        let value = serde_json::to_value(request).expect("serialize semantic prepare request");
        assert_eq!(value["modelBytes"], 24_471_328);
        assert_eq!(value["tokenizerSha256"], "5".repeat(64));
        assert!(value.get("model_bytes").is_none());

        let response: PrepareCliSemanticAssetsResponse =
            serde_json::from_value(serde_json::json!({
                "operationId": "operation-1",
                "modelId": "model-r2",
                "modelPath": "/private/assets/model.safetensors",
                "tokenizerPath": "/private/assets/tokenizer.vcpbpe"
            }))
            .expect("deserialize semantic prepare response");
        assert_eq!(response.model_id, "model-r2");
        assert!(response.model_path.ends_with("model.safetensors"));
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
            river_context_projection: Some(CliRiverContextProjection {
                host_path: "/private/projections/attempt/river-context.json".to_string(),
                size_bytes: 17,
                sha256: "a".repeat(64),
                artifacts: vec![CliProjectedArtifact {
                    host_path: "/private/projections/attempt/river-artifact-00-aaaaaaaaaaaa.png"
                        .to_string(),
                    guest_path: "/run/river-artifact-00-aaaaaaaaaaaa.png".to_string(),
                    size_bytes: 23,
                    sha256: "b".repeat(64),
                }],
            }),
            vref_projection: Some(CliVrefProjection {
                host_dir: "/private/projections/attempt/vref".to_string(),
                manifest_path: "/private/projections/attempt/vref/vref-projection.json".to_string(),
                manifest_size_bytes: 31,
                manifest_sha256: "c".repeat(64),
                files: vec![CliVrefFileProjection {
                    relative_name: "0001-bbbbbbbbbbbb-notes.md".to_string(),
                    size_bytes: 29,
                    sha256: "d".repeat(64),
                }],
            }),
        };

        let value = serde_json::to_value(request).expect("serialize request");
        assert_eq!(value["operationId"], "operation-1");
        assert_eq!(value["runtimeGeneration"], 7);
        assert_eq!(value["artifactMaxBytes"], 1024);
        assert_eq!(value["command"], "printf '%s' '$HOME'");
        assert_eq!(
            value["riverContextProjection"]["hostPath"],
            "/private/projections/attempt/river-context.json"
        );
        assert_eq!(value["riverContextProjection"]["sizeBytes"], 17);
        assert_eq!(value["riverContextProjection"]["sha256"], "a".repeat(64));
        assert_eq!(
            value["riverContextProjection"]["artifacts"][0]["guestPath"],
            "/run/river-artifact-00-aaaaaaaaaaaa.png"
        );
        assert_eq!(
            value["vrefProjection"]["manifestPath"],
            "/private/projections/attempt/vref/vref-projection.json"
        );
        assert_eq!(
            value["vrefProjection"]["files"][0]["relativeName"],
            "0001-bbbbbbbbbbbb-notes.md"
        );
        assert!(value.get("operation_id").is_none());

        let mut missing_artifacts = value;
        missing_artifacts["riverContextProjection"]
            .as_object_mut()
            .expect("river projection object")
            .remove("artifacts");
        assert!(serde_json::from_value::<StartCliProcessRequest>(missing_artifacts).is_err());
    }

    #[test]
    fn start_request_omits_absent_river_projection() {
        let request = StartCliProcessRequest {
            operation_id: "operation-2".to_string(),
            job_id: "job-2".to_string(),
            attempt_id: "attempt-2".to_string(),
            runtime_generation: 8,
            command: "true".to_string(),
            rootfs_path: "/private/rootfs".to_string(),
            cwd: "/workspace".to_string(),
            artifact_max_bytes: 0,
            river_context_projection: None,
            vref_projection: None,
        };

        let value = serde_json::to_value(request).expect("serialize request");
        assert!(value.get("riverContextProjection").is_none());
        assert!(value.get("vrefProjection").is_none());
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
