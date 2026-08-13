//! P1 `MobileCliRuntimeState`：唯一 generation/operation/job/timeout owner 与 Tauri API。

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::sync::Mutex;
use uuid::Uuid;

use super::ledger::{
    command_preview, JobLedger, JobRecord, MutationOperationBinding, OperationLookup,
    ProcessObservation, ProcessOutputFacts, RuntimePathsRecord, TerminalIntent,
    TerminalIntentTarget,
};
use super::output::{
    gc_orphan_outputs, hash_output_artifact_pair, initial_cursor, read_incremental_output,
    remove_job_outputs, require_filesystem_headroom, workspace_usage_bytes, OutputReadRequest,
};
use super::profile::{embedded_command_profile, VcpCliCommandProfile};
use super::projection::{
    gc_stale_river_projections, prepare_river_projection, remove_river_projection,
};
use super::protocol::{
    validate_structured_vcp_cli_action, VcpCliAction, DEFAULT_BOUNDED_READ_BYTES, MAX_POLL_WAIT_MS,
};
use super::provision::{
    provision_verified_runtime_blocking, verify_staged_provision_inputs, ProvisionPaths,
    ProvisionedRuntime,
};
use super::result::{
    VcpCliArtifactRef, VcpCliContentPart, VcpCliErrorCode, VcpCliJobResult, VcpCliJobState,
    VcpCliJobSummary, VcpCliResultBody, VcpCliResultEnvelope, VcpCliRuntimeInfo,
};
use super::skills::{
    install_builtin_skill, list_skills, read_skill, SkillError, SkillErrorKind, MAX_SKILL_BYTES,
};
use tauri_plugin_vcp_mobile::cli::{
    cancel_cli_process_inner, inspect_cli_process_inner, prepare_cli_runtime_inner,
    start_cli_process_inner, CancelCliProcessRequest, CancelCliProcessResponse, CliProcessState,
    InspectCliProcessRequest, InspectCliProcessResponse, PrepareCliRuntimeRequest,
    StartCliProcessRequest, StartCliProcessResponse,
};

const LEDGER_RELATIVE_PATH: &str = "vcp-cli/job-ledger.json";
const MAX_OPERATION_ID_BYTES: usize = 128;
const MAX_CANCEL_GRACE_MS: u64 = 2_000;
const INSPECT_INTERVAL_MS: u64 = 125;
const MONITOR_INTERVAL_MS: u64 = 250;
const WORKSPACE_CHECK_INTERVAL_MS: u64 = 5_000;
const MIN_RUNTIME_STORAGE_HEADROOM_BYTES: u64 = 256 * 1024 * 1024;
const MAX_TIMEOUT_CANCEL_RETRIES: u8 = 3;
const TIMEOUT_CANCEL_RETRY_MS: u64 = 500;
const FINAL_OUTPUT_REFRESH_ATTEMPTS: usize = 3;
const FINAL_OUTPUT_REFRESH_RETRY_MS: u64 = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobileCliPhase {
    Unavailable,
    Unprovisioned,
    Preparing,
    Ready,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MobileCliStatus {
    pub available: bool,
    pub availability_reason: Option<String>,
    pub background_reliability: String,
    pub runtime_generation: u64,
    pub phase: MobileCliPhase,
    pub profile_id: String,
    pub max_concurrent_jobs: usize,
    pub running_jobs: usize,
    pub jobs: Vec<VcpCliJobSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecuteVcpMobileCliRequest {
    pub operation_id: String,
    pub action: VcpCliAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub river_projection: Option<VcpCliRiverProjectionInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VcpCliRiverProjectionInput {
    pub canonical_json: String,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecuteVcpMobileCliResponse {
    pub operation_id: String,
    pub runtime_generation: u64,
    pub envelope: VcpCliResultEnvelope,
}

pub struct MobileCliRuntimeState {
    inner: Mutex<RuntimeOwner>,
    operation_gate: Mutex<()>,
    provision_gate: Mutex<()>,
    workspace_gate: Mutex<()>,
}

struct RuntimeOwner {
    initialized: bool,
    phase: MobileCliPhase,
    availability_reason: Option<String>,
    recovery_notice: Option<String>,
    ledger_path: Option<PathBuf>,
    ledger: JobLedger,
    provisioned: Option<ProvisionedRuntime>,
    last_workspace_check_ms: u64,
}

struct RunParameters {
    command: String,
    description: Option<String>,
    cwd: String,
    timeout_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CancelBindingOutcome {
    Bound,
    Replay,
    Conflict,
}

impl MobileCliRuntimeState {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RuntimeOwner {
                initialized: false,
                phase: if cfg!(target_os = "android") {
                    MobileCliPhase::Unprovisioned
                } else {
                    MobileCliPhase::Unavailable
                },
                availability_reason: (!cfg!(target_os = "android")).then(|| {
                    "VCPMobileCLI is unavailable on the host scaffold; Android foreground execution only."
                        .to_string()
                }),
                recovery_notice: None,
                ledger_path: None,
                ledger: JobLedger::empty(0),
                provisioned: None,
                last_workspace_check_ms: 0,
            }),
            operation_gate: Mutex::new(()),
            provision_gate: Mutex::new(()),
            workspace_gate: Mutex::new(()),
        }
    }

    async fn ensure_initialized<R: Runtime>(&self, app: &AppHandle<R>) -> Result<(), String> {
        let mut owner = self.inner.lock().await;
        if owner.initialized {
            return Ok(());
        }
        let ledger_path = app
            .path()
            .app_config_dir()
            .map_err(|error| format!("cannot resolve CLI private state directory: {error}"))?
            .join(LEDGER_RELATIVE_PATH);
        let (ledger, recovery_notice) =
            JobLedger::load_with_scoped_recovery(&ledger_path, now_ms()?).await?;
        // Kotlin ProcessHost state is process-local. Persisted paths are evidence only until this
        // process has called prepare, reverified assets, and checked the completion marker.
        ledger.save_atomic(&ledger_path).await?;
        owner.phase = if cfg!(target_os = "android") {
            MobileCliPhase::Unprovisioned
        } else {
            MobileCliPhase::Unavailable
        };
        owner.recovery_notice = recovery_notice;
        owner.ledger_path = Some(ledger_path);
        owner.ledger = ledger;
        owner.provisioned = None;
        owner.last_workspace_check_ms = 0;
        owner.initialized = true;
        Ok(())
    }

    async fn status<R: Runtime>(&self, app: &AppHandle<R>) -> Result<MobileCliStatus, String> {
        self.ensure_initialized(app).await?;
        if cfg!(target_os = "android") {
            let operation_id = format!("status-prepare-{}", Uuid::new_v4());
            // Status is the lazy-provision entry. Failure is reflected as phase=error/reason so the
            // UI always has an actionable status rather than a rejected invoke.
            let _ = self.ensure_provisioned(app, &operation_id).await;
        }
        let owner = self.inner.lock().await;
        let profile = embedded_command_profile().map_err(|error| error.to_string())?;
        Ok(status_from_owner(&owner, &profile))
    }

    pub(crate) async fn execute<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        request: ExecuteVcpMobileCliRequest,
    ) -> Result<ExecuteVcpMobileCliResponse, String> {
        validate_operation_id(&request.operation_id)?;
        let action = match validate_structured_vcp_cli_action(request.action.clone()) {
            Ok(action) => action,
            Err(error) => {
                let generation = self.current_generation(app).await?;
                return Ok(ExecuteVcpMobileCliResponse {
                    operation_id: request.operation_id,
                    runtime_generation: generation,
                    envelope: protocol_error_envelope(error.code, &error.to_string()),
                });
            }
        };
        if request.river_projection.is_some() && !matches!(action, VcpCliAction::Run { .. }) {
            let generation = self.current_generation(app).await?;
            return Ok(ExecuteVcpMobileCliResponse {
                operation_id: request.operation_id,
                runtime_generation: generation,
                envelope: VcpCliResultEnvelope::error(
                    VcpCliErrorCode::InvalidRequest,
                    "river is only valid for action=run",
                    "Remove river from this action and retry.",
                ),
            });
        }
        validate_session_id(request.session_id.as_deref())?;
        let action_sha256 = action_digest(
            &action,
            request.river_projection.as_ref(),
            request.session_id.as_deref(),
        )?;
        self.ensure_initialized(app).await?;

        if let Some(response) = self
            .replay_operation(&request.operation_id, &action_sha256)
            .await?
        {
            return Ok(response);
        }

        let response = match action {
            VcpCliAction::Run {
                command,
                description,
                cwd,
                timeout_ms,
                run_in_background,
            } => {
                let run_in_background = run_in_background
                    .ok_or_else(|| "validated background flag is missing".to_string())?;
                let response = {
                    // Only run admission/start is serialized. The foreground yield happens after
                    // this guard is dropped, so poll/cancel never wait behind an 8-second action.
                    let _operation_guard = self.operation_gate.lock().await;
                    if let Some(replay) = self
                        .replay_operation(&request.operation_id, &action_sha256)
                        .await?
                    {
                        replay
                    } else {
                        self.run_action(
                            app,
                            &request.operation_id,
                            RunParameters {
                                command,
                                description,
                                cwd: cwd
                                    .ok_or_else(|| "validated run cwd is missing".to_string())?,
                                timeout_ms: timeout_ms.ok_or_else(|| {
                                    "validated run timeout is missing".to_string()
                                })?,
                            },
                            &action_sha256,
                            request.river_projection.clone(),
                            request.session_id.clone(),
                        )
                        .await?
                    }
                };
                if !run_in_background {
                    if let Some(job_id) = response
                        .envelope
                        .result()
                        .job
                        .as_ref()
                        .map(|job| job.id.clone())
                    {
                        let yield_ms = embedded_command_profile()
                            .map_err(|error| error.to_string())?
                            .budgets
                            .foreground_yield_ms;
                        self.wait_for_progress(app, &request.operation_id, &job_id, yield_ms)
                            .await?;
                        self.job_response(
                            &request.operation_id,
                            &job_id,
                            None,
                            DEFAULT_BOUNDED_READ_BYTES,
                        )
                        .await?
                    } else {
                        response
                    }
                } else {
                    response
                }
            }
            VcpCliAction::ListSkills => self.list_skills_action(app, &request.operation_id).await?,
            VcpCliAction::ReadSkill {
                skill_id,
                resource_path,
                max_bytes,
            } => {
                self.read_skill_action(
                    app,
                    &request.operation_id,
                    skill_id,
                    resource_path
                        .ok_or_else(|| "validated Skill resource path is missing".to_string())?,
                    max_bytes.ok_or_else(|| "validated Skill max_bytes is missing".to_string())?,
                )
                .await?
            }
            VcpCliAction::Poll {
                job_id,
                cursor,
                max_output_bytes,
                wait_ms,
            } => {
                self.poll_action(
                    app,
                    &request.operation_id,
                    &job_id,
                    cursor.as_deref(),
                    max_output_bytes
                        .ok_or_else(|| "validated poll max_output_bytes is missing".to_string())?,
                    wait_ms.ok_or_else(|| "validated poll wait_ms is missing".to_string())?,
                )
                .await?
            }
            VcpCliAction::Cancel { job_id } => {
                if let Some(replay) = self
                    .replay_operation(&request.operation_id, &action_sha256)
                    .await?
                {
                    return Ok(replay);
                }
                match self
                    .bind_cancel_operation(
                        &job_id,
                        &request.operation_id,
                        &action_sha256,
                        TerminalIntent {
                            target: TerminalIntentTarget::Cancelled,
                            operation_id: request.operation_id.clone(),
                            requested_at_ms: now_ms()?,
                            reason: "Cancelled; the owned process tree is gone.".to_string(),
                        },
                    )
                    .await?
                {
                    CancelBindingOutcome::Bound => {}
                    CancelBindingOutcome::Replay | CancelBindingOutcome::Conflict => {
                        return self
                            .replay_operation(&request.operation_id, &action_sha256)
                            .await?
                            .ok_or_else(|| {
                                "cancel operation binding vanished during replay".to_string()
                            });
                    }
                }
                self.cancel_action(app, &request.operation_id, &job_id, false)
                    .await?
            }
            VcpCliAction::List => self.list_action(app, &request.operation_id).await?,
        };

        let mut owner = self.inner.lock().await;
        owner.ledger.record_operation(
            request.operation_id,
            action_sha256,
            serde_json::to_value(&response)
                .map_err(|error| format!("cannot cache CLI operation response: {error}"))?,
            now_ms()?,
        )?;
        persist_owner(&owner).await?;
        Ok(response)
    }

    async fn replay_operation(
        &self,
        operation_id: &str,
        action_sha256: &str,
    ) -> Result<Option<ExecuteVcpMobileCliResponse>, String> {
        let lookup = {
            let owner = self.inner.lock().await;
            (
                owner.ledger.operation(operation_id, action_sha256),
                owner.ledger.runtime_generation,
            )
        };
        match lookup.0 {
            OperationLookup::Replay(value) => serde_json::from_value(value)
                .map(Some)
                .map_err(|error| format!("invalid cached CLI operation response: {error}")),
            OperationLookup::ReplayJob(job_id) => self
                .job_response(operation_id, &job_id, None, DEFAULT_BOUNDED_READ_BYTES)
                .await
                .map(Some),
            OperationLookup::Conflict => Ok(Some(ExecuteVcpMobileCliResponse {
                operation_id: operation_id.to_string(),
                runtime_generation: lookup.1,
                envelope: VcpCliResultEnvelope::error(
                    VcpCliErrorCode::InvalidRequest,
                    "operation_id was already used",
                    "Retry with a fresh operation_id; an id cannot name two actions.",
                ),
            })),
            OperationLookup::Missing => Ok(None),
        }
    }

    async fn current_generation<R: Runtime>(&self, app: &AppHandle<R>) -> Result<u64, String> {
        self.ensure_initialized(app).await?;
        Ok(self.inner.lock().await.ledger.runtime_generation)
    }

    async fn ensure_provisioned<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        operation_id: &str,
    ) -> Result<ProvisionedRuntime, DomainError> {
        if !cfg!(target_os = "android") {
            return Err(DomainError::new(
                VcpCliErrorCode::RuntimeUnavailable,
                "Android CLI runtime is unavailable on this host scaffold.",
            ));
        }
        let _provision_guard = self.provision_gate.lock().await;
        {
            let owner = self.inner.lock().await;
            if let Some(runtime) = &owner.provisioned {
                return Ok(runtime.clone());
            }
        }
        let result = self.provision_once(app, operation_id).await;
        if let Err(error) = &result {
            self.set_phase_error(error.message.clone()).await;
        }
        result
    }

    async fn provision_once<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        operation_id: &str,
    ) -> Result<ProvisionedRuntime, DomainError> {
        let profile = embedded_command_profile()
            .map_err(|error| DomainError::internal(format!("invalid embedded profile: {error}")))?;
        let generation = {
            let mut owner = self.inner.lock().await;
            owner.phase = MobileCliPhase::Preparing;
            owner.ledger.runtime_generation
        };
        let prepare = PrepareCliRuntimeRequest {
            operation_id: operation_id.to_string(),
            profile_id: profile.profile_id.clone(),
            runtime_generation: generation,
            rootfs_archive_bytes: profile.rootfs.archive_bytes,
            rootfs_archive_sha256: profile.rootfs.archive_sha256.clone(),
            proot_bytes: profile.proot.binary_bytes,
            proot_sha256: profile.proot.binary_sha256.clone(),
            proot_loader_bytes: profile.proot.loader_bytes,
            proot_loader_sha256: profile.proot.loader_sha256.clone(),
        };
        let staged = match prepare_cli_runtime_inner(app, &prepare).await {
            Ok(staged) => staged,
            Err(error) => return Err(DomainError::new(VcpCliErrorCode::RuntimeUnavailable, error)),
        };
        if staged.operation_id != operation_id
            || staged.profile_id != profile.profile_id
            || staged.runtime_generation != generation
        {
            let error = "ProcessHost prepare response identity mismatch".to_string();
            return Err(DomainError::internal(error));
        }
        let paths = ProvisionPaths {
            rootfs_archive: PathBuf::from(staged.archive_path),
            proot_binary: PathBuf::from(staged.proot_path),
            proot_loader: PathBuf::from(staged.proot_loader_path),
            rootfs_parent: PathBuf::from(staged.rootfs_parent_path),
            workspace: PathBuf::from(staged.workspace_path),
            skills: PathBuf::from(staged.skills_path),
            output: PathBuf::from(staged.output_path),
            projection_root: PathBuf::from(staged.projection_root_path),
        };
        let verified_profile = verify_staged_provision_inputs(&paths)
            .await
            .map_err(|error| DomainError::internal(error.to_string()))?;
        let runtime = tauri::async_runtime::spawn_blocking(move || {
            provision_verified_runtime_blocking(paths, verified_profile)
        })
        .await
        .map_err(|error| DomainError::internal(format!("provision task failed: {error}")))?
        .map_err(|error| DomainError::internal(error.to_string()))?;
        let skills = runtime.skills.clone();
        tauri::async_runtime::spawn_blocking(move || install_builtin_skill(&skills))
            .await
            .map_err(|error| DomainError::internal(format!("Skill install task failed: {error}")))?
            .map_err(domain_skill_error)?;

        let referenced = {
            let owner = self.inner.lock().await;
            owner
                .ledger
                .jobs
                .iter()
                .flat_map(|job| [job.stdout_path.clone(), job.stderr_path.clone()])
                .flatten()
                .collect::<BTreeSet<_>>()
        };
        let output = runtime.output.clone();
        tauri::async_runtime::spawn_blocking(move || gc_orphan_outputs(&output, &referenced))
            .await
            .map_err(|error| DomainError::internal(format!("CLI output GC task failed: {error}")))?
            .map_err(DomainError::internal)?;

        let projection_root = runtime.projection_root.clone();
        tauri::async_runtime::spawn_blocking(move || gc_stale_river_projections(&projection_root))
            .await
            .map_err(|error| DomainError::internal(format!("river projection GC failed: {error}")))?
            .map_err(DomainError::internal)?;

        let mut owner = self.inner.lock().await;
        if owner.ledger.runtime_generation != generation {
            return Err(DomainError::internal(
                "runtime generation changed during provision",
            ));
        }
        owner.ledger.runtime_paths = Some(runtime_paths_record(&runtime));
        owner.provisioned = Some(runtime.clone());
        owner.phase = MobileCliPhase::Ready;
        owner.availability_reason = owner.recovery_notice.clone();
        persist_owner(&owner).await.map_err(DomainError::internal)?;
        Ok(runtime)
    }

    async fn run_action<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        operation_id: &str,
        parameters: RunParameters,
        action_sha256: &str,
        river_projection: Option<VcpCliRiverProjectionInput>,
        session_id: Option<String>,
    ) -> Result<ExecuteVcpMobileCliResponse, String> {
        let runtime = match self.ensure_provisioned(app, operation_id).await {
            Ok(runtime) => runtime,
            Err(error) => return self.domain_response(operation_id, error).await,
        };
        let profile = runtime.profile.clone();
        let workspace = runtime.workspace.clone();
        let workspace_limit = profile.budgets.workspace_default_bytes;
        let storage_paths = vec![
            runtime.rootfs.clone(),
            runtime.workspace.clone(),
            runtime.output.clone(),
        ];
        let usage = tauri::async_runtime::spawn_blocking(move || {
            workspace_usage_bytes(&workspace, workspace_limit)?;
            require_filesystem_headroom(&storage_paths, MIN_RUNTIME_STORAGE_HEADROOM_BYTES)
        })
        .await
        .map_err(|error| format!("workspace scan task failed: {error}"))?;
        if let Err(error) = usage {
            return self
                .domain_response(
                    operation_id,
                    DomainError::new(VcpCliErrorCode::RuntimeUnavailable, error),
                )
                .await;
        }

        let now = now_ms()?;
        let job_id = format!("job-{}", Uuid::new_v4());
        let attempt_id = format!("attempt-{}", Uuid::new_v4());
        let generation = self.inner.lock().await.ledger.runtime_generation;
        let river_context_projection = if let Some(input) = river_projection {
            let root = runtime.projection_root.clone();
            let job = job_id.clone();
            let attempt = attempt_id.clone();
            Some(
                tauri::async_runtime::spawn_blocking(move || {
                    prepare_river_projection(&root, generation, &job, &attempt, &input)
                })
                .await
                .map_err(|error| format!("river projection task failed: {error}"))??,
            )
        } else {
            None
        };
        let projection_path = river_context_projection
            .as_ref()
            .map(|projection| PathBuf::from(&projection.host_path));
        let insertion = async {
            let mut owner = self.inner.lock().await;
            if owner.ledger.runtime_generation != generation {
                return Err("runtime generation changed before CLI job admission".to_string());
            }
            let running = owner
                .ledger
                .jobs
                .iter()
                .filter(|job| !job.is_terminal())
                .count();
            let concurrent_limit = profile
                .budgets
                .default_concurrent_jobs
                .min(profile.budgets.max_concurrent_jobs)
                .min(4);
            if running >= concurrent_limit {
                return Err(format!(
                    "CLI concurrency limit reached; at most {concurrent_limit} jobs may run"
                ));
            }
            if let Some(session_id) = session_id.as_deref() {
                if session_concurrency_reached(
                    &owner.ledger.jobs,
                    session_id,
                    concurrent_limit.min(2),
                ) {
                    return Err(
                        "CLI session concurrency limit reached; at most 2 jobs may run in one chat session"
                            .to_string(),
                    );
                }
            }
            let evicted = owner.ledger.insert_job(JobRecord {
                id: job_id.clone(),
                attempt_id: attempt_id.clone(),
                runtime_generation: generation,
                session_id: session_id.clone(),
                state: VcpCliJobState::Starting,
                command_preview: command_preview(&parameters.command),
                description: parameters.description,
                cwd: parameters.cwd.clone(),
                timeout_ms: parameters.timeout_ms,
                created_at_ms: now,
                updated_at_ms: now,
                deadline_at_ms: now.saturating_add(parameters.timeout_ms),
                stdout_path: None,
                stderr_path: None,
                river_projection_path: projection_path.clone(),
                stdout_bytes: 0,
                stderr_bytes: 0,
                stdout_truncated: false,
                stderr_truncated: false,
                artifact_sha256: None,
                artifact_size_bytes: None,
                exit_code: None,
                reason: None,
                terminal_intent: None,
                mutation_operations: vec![MutationOperationBinding {
                    operation_id: operation_id.to_string(),
                    action_sha256: action_sha256.to_string(),
                }],
            })?;
            let stale_paths = evicted
                .iter()
                .flat_map(|job| [job.stdout_path.clone(), job.stderr_path.clone()])
                .flatten()
                .collect::<Vec<_>>();
            if !stale_paths.is_empty() {
                let output = runtime.output.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    remove_job_outputs(&output, stale_paths)
                });
            }
            for stale in evicted {
                if let Some(path) = stale.river_projection_path {
                    let root = runtime.projection_root.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        remove_river_projection(
                            &root,
                            stale.runtime_generation,
                            &stale.id,
                            &stale.attempt_id,
                            &path,
                        )
                    });
                }
            }
            persist_owner(&owner).await?;
            Ok::<(), String>(())
        }
        .await;
        if let Err(error) = insertion {
            if let Some(path) = projection_path {
                let root = runtime.projection_root.clone();
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    remove_river_projection(&root, generation, &job_id, &attempt_id, &path)
                })
                .await;
            }
            return self
                .domain_response(
                    operation_id,
                    DomainError::new(VcpCliErrorCode::RuntimeUnavailable, error),
                )
                .await;
        }

        let start = StartCliProcessRequest {
            operation_id: operation_id.to_string(),
            job_id: job_id.clone(),
            attempt_id: attempt_id.clone(),
            runtime_generation: generation,
            command: parameters.command,
            rootfs_path: runtime.rootfs.to_string_lossy().into_owned(),
            cwd: parameters.cwd,
            artifact_max_bytes: profile.budgets.artifact_bytes_per_job,
            river_context_projection,
        };
        let job = self
            .job_snapshot(&job_id)
            .await
            .ok_or_else(|| "new CLI job disappeared before start".to_string())?;
        match self.start_or_adopt(app, &start, &job).await {
            Ok(started) => self.adopt_start_response(&job, started).await?,
            Err(error) => {
                let containment_operation = format!("start-contain-{}", Uuid::new_v4());
                let requested = TerminalIntent {
                    target: TerminalIntentTarget::Failed,
                    operation_id: containment_operation.clone(),
                    requested_at_ms: now_ms()?,
                    reason: format!("ProcessHost start failed and was contained: {error}"),
                };
                let contained = self
                    .cancel_attempt(app, &containment_operation, &job, requested)
                    .await
                    .ok()
                    .flatten();
                if let Some(intent) = contained {
                    self.claim_terminal_intent(&job, &intent, None).await?;
                } else {
                    self.set_job_reason(
                        &job,
                        format!("Ambiguous ProcessHost start; monitor owns containment: {error}"),
                    )
                    .await?;
                }
            }
        }

        spawn_job_monitor(app.clone(), job_id.clone(), attempt_id, generation);
        self.job_response(operation_id, &job_id, None, DEFAULT_BOUNDED_READ_BYTES)
            .await
    }

    async fn start_or_adopt<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        request: &StartCliProcessRequest,
        job: &JobRecord,
    ) -> Result<StartCliProcessResponse, String> {
        match start_cli_process_inner(app, request).await {
            Ok(response) if start_identity_matches(request, &response) => Ok(response),
            Ok(_) => Err("ProcessHost start response identity mismatch".to_string()),
            Err(start_error) => {
                let probe_operation = format!("start-probe-{}", Uuid::new_v4());
                let probe = inspect_cli_process_inner(
                    app,
                    &InspectCliProcessRequest {
                        operation_id: probe_operation.clone(),
                        job_id: job.id.clone(),
                        attempt_id: job.attempt_id.clone(),
                        runtime_generation: job.runtime_generation,
                    },
                )
                .await
                .map_err(|probe_error| {
                    format!("start failed ({start_error}); adoption inspect failed ({probe_error})")
                })?;
                if !inspect_identity_matches(job, &probe_operation, &probe) {
                    return Err(format!(
                        "start failed ({start_error}); adoption inspect identity mismatch"
                    ));
                }
                if probe.state == CliProcessState::Missing {
                    return Err(format!(
                        "ProcessHost start failed and the expected attempt is missing: {start_error}"
                    ));
                }
                // Start is idempotent by the fenced attempt identity and fingerprint. Reissuing it
                // recovers output paths without spawning a second process.
                let response = start_cli_process_inner(app, request).await.map_err(|retry_error| {
                    format!(
                        "attempt exists after ambiguous start, but handle recovery failed: {retry_error}"
                    )
                })?;
                if !start_identity_matches(request, &response) {
                    return Err("recovered start response identity mismatch".to_string());
                }
                Ok(response)
            }
        }
    }

    async fn adopt_start_response(
        &self,
        job: &JobRecord,
        started: StartCliProcessResponse,
    ) -> Result<(), String> {
        let mut owner = self.inner.lock().await;
        if let Some(current) = owner.ledger.find_job_mut(&job.id) {
            if current.attempt_id == job.attempt_id
                && current.runtime_generation == job.runtime_generation
                && !current.is_terminal()
            {
                current.state = VcpCliJobState::Running;
                current.updated_at_ms = now_ms()?;
                current.stdout_path = Some(PathBuf::from(started.stdout_path));
                current.stderr_path = Some(PathBuf::from(started.stderr_path));
                current.reason = None;
            }
        }
        persist_owner(&owner).await
    }

    async fn set_job_reason(&self, job: &JobRecord, reason: String) -> Result<(), String> {
        let mut owner = self.inner.lock().await;
        if let Some(current) = owner.ledger.find_job_mut(&job.id) {
            if current.attempt_id == job.attempt_id
                && current.runtime_generation == job.runtime_generation
                && !current.is_terminal()
            {
                current.reason = Some(reason);
                current.updated_at_ms = now_ms()?;
            }
        }
        persist_owner(&owner).await
    }

    async fn poll_action<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        operation_id: &str,
        job_id: &str,
        cursor: Option<&str>,
        max_output_bytes: usize,
        wait_ms: u64,
    ) -> Result<ExecuteVcpMobileCliResponse, String> {
        let wait_ms = wait_ms.min(MAX_POLL_WAIT_MS);
        self.wait_for_progress(app, operation_id, job_id, wait_ms)
            .await?;
        self.job_response(operation_id, job_id, cursor, max_output_bytes)
            .await
    }

    async fn wait_for_progress<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        operation_id: &str,
        job_id: &str,
        wait_ms: u64,
    ) -> Result<(), String> {
        let started = tokio::time::Instant::now();
        let initial = self.job_snapshot(job_id).await;
        loop {
            let Some(job) = self.job_snapshot(job_id).await else {
                return Ok(());
            };
            if job.is_terminal() {
                return Ok(());
            }
            self.inspect_job(app, operation_id, &job).await?;
            let current = self.job_snapshot(job_id).await;
            if current.as_ref().is_none_or(JobRecord::is_terminal)
                || current
                    .as_ref()
                    .zip(initial.as_ref())
                    .is_some_and(|(now, then)| {
                        now.stdout_bytes != then.stdout_bytes
                            || now.stderr_bytes != then.stderr_bytes
                    })
                || started.elapsed() >= Duration::from_millis(wait_ms)
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(INSPECT_INTERVAL_MS)).await;
        }
    }

    async fn inspect_job<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        operation_id: &str,
        job: &JobRecord,
    ) -> Result<(), String> {
        let job = self.ensure_deadline_intent(job).await?;
        if job.is_terminal() {
            return Ok(());
        }
        let response = inspect_cli_process_inner(
            app,
            &InspectCliProcessRequest {
                operation_id: operation_id.to_string(),
                job_id: job.id.clone(),
                attempt_id: job.attempt_id.clone(),
                runtime_generation: job.runtime_generation,
            },
        )
        .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                log::warn!("[VCPMobileCLI] inspect failed for {}: {error}", job.id);
                self.set_job_reason(
                    &job,
                    format!("ProcessHost inspect failed and will retry: {error}"),
                )
                .await?;
                return Ok(());
            }
        };
        if !inspect_identity_matches(&job, operation_id, &response) {
            return Err("ProcessHost inspect response identity mismatch".to_string());
        }
        let mut owner = self.inner.lock().await;
        let mut next = owner.ledger.clone();
        let observation = match response.state {
            CliProcessState::Running => ProcessObservation::Running,
            CliProcessState::Exited => ProcessObservation::Exited {
                exit_code: response.exit_code,
            },
            CliProcessState::Missing => ProcessObservation::Missing,
        };
        if !next.apply_process_observation(
            &job.id,
            &job.attempt_id,
            job.runtime_generation,
            observation,
            ProcessOutputFacts {
                stdout_bytes: response.stdout_bytes,
                stderr_bytes: response.stderr_bytes,
                stdout_truncated: response.stdout_truncated,
                stderr_truncated: response.stderr_truncated,
            },
            now_ms()?,
        ) {
            return Ok(());
        }
        let terminal = next
            .find_job(&job.id)
            .filter(|current| current.is_terminal())
            .cloned();
        replace_ledger_atomically(&mut owner, next).await?;
        drop(owner);
        if let Some(terminal) = terminal {
            self.cleanup_river_projection(&terminal).await?;
        }
        Ok(())
    }

    async fn monitor_job<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        job_id: &str,
        attempt_id: &str,
        runtime_generation: u64,
    ) -> Result<(), String> {
        let mut timeout_cancel_failures = 0_u8;
        loop {
            let Some(job) = self.job_snapshot(job_id).await else {
                return Ok(());
            };
            if job.attempt_id != attempt_id || job.runtime_generation != runtime_generation {
                return Ok(());
            }
            let job = self.ensure_deadline_intent(&job).await?;
            if job.is_terminal() {
                self.cleanup_river_projection(&job).await?;
                return Ok(());
            }
            if let Some(intent) = job.terminal_intent.clone() {
                match self
                    .cancel_attempt(app, &intent.operation_id, &job, intent.clone())
                    .await
                {
                    Ok(Some(winning_intent)) => {
                        self.claim_terminal_intent(&job, &winning_intent, None)
                            .await?;
                        return Ok(());
                    }
                    Ok(None) => {
                        self.set_job_reason(
                            &job,
                            "Terminal intent is durable, but ProcessHost has not confirmed containment; monitor will retry."
                                .to_string(),
                        )
                        .await?;
                    }
                    Err(error) => {
                        self.set_job_reason(
                            &job,
                            format!(
                                "Terminal intent is durable; containment callback failed and monitor will retry: {error}"
                            ),
                        )
                        .await?;
                    }
                }
                tokio::time::sleep(Duration::from_millis(TIMEOUT_CANCEL_RETRY_MS)).await;
                continue;
            }
            let inspect_operation = format!("monitor-{}", Uuid::new_v4());
            if let Err(error) = self.inspect_job(app, &inspect_operation, &job).await {
                self.set_job_reason(
                    &job,
                    format!("Background monitor inspect failed and will retry: {error}"),
                )
                .await?;
                tokio::time::sleep(Duration::from_millis(MONITOR_INTERVAL_MS)).await;
                continue;
            }
            let Some(current) = self.job_snapshot(job_id).await else {
                return Ok(());
            };
            if current.attempt_id != attempt_id
                || current.runtime_generation != runtime_generation
                || current.is_terminal()
            {
                return Ok(());
            }
            match monitor_decision(&current, now_ms()?) {
                MonitorDecision::Timeout => {
                    let operation_id = format!("timeout-{}", Uuid::new_v4());
                    let requested = TerminalIntent {
                        target: TerminalIntentTarget::TimedOut,
                        operation_id: operation_id.clone(),
                        requested_at_ms: now_ms()?,
                        reason: "Command exceeded its absolute deadline and its owned process tree is gone."
                            .to_string(),
                    };
                    match self
                        .cancel_attempt(app, &operation_id, &current, requested)
                        .await
                    {
                        Ok(Some(intent)) => {
                            self.claim_terminal_intent(&current, &intent, None).await?;
                            return Ok(());
                        }
                        Ok(None) => {
                            timeout_cancel_failures = timeout_cancel_failures.saturating_add(1);
                            self.set_job_reason(
                                &job,
                                format!(
                                    "Timeout reached; containment attempt {timeout_cancel_failures} did not confirm the process tree was gone."
                                ),
                            )
                            .await?;
                        }
                        Err(error) => {
                            timeout_cancel_failures = timeout_cancel_failures.saturating_add(1);
                            self.set_job_reason(
                                &job,
                                format!(
                                    "Timeout reached; containment attempt {timeout_cancel_failures} failed: {error}"
                                ),
                            )
                            .await?;
                        }
                    }
                    if timeout_cancel_failures >= MAX_TIMEOUT_CANCEL_RETRIES {
                        log::error!(
                            "[VCPMobileCLI] timeout containment remains unconfirmed for {}; monitor keeps retrying",
                            current.id
                        );
                        timeout_cancel_failures = 0;
                    }
                    tokio::time::sleep(Duration::from_millis(TIMEOUT_CANCEL_RETRY_MS)).await;
                }
                MonitorDecision::Inspect { sleep_ms } => {
                    if let Err(error) = self.enforce_workspace_budget(app).await {
                        self.set_job_reason(
                            &current,
                            format!("Runtime storage monitor failed and will retry: {error}"),
                        )
                        .await?;
                    }
                    tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
                }
            }
        }
    }

    async fn enforce_workspace_budget<R: Runtime>(&self, app: &AppHandle<R>) -> Result<(), String> {
        let _workspace_guard = self.workspace_gate.lock().await;
        let now = now_ms()?;
        let (workspace, limit, storage_paths) = {
            let mut owner = self.inner.lock().await;
            if now.saturating_sub(owner.last_workspace_check_ms) < WORKSPACE_CHECK_INTERVAL_MS {
                return Ok(());
            }
            owner.last_workspace_check_ms = now;
            let Some(runtime) = owner.provisioned.as_ref() else {
                return Ok(());
            };
            (
                runtime.workspace.clone(),
                runtime.profile.budgets.workspace_default_bytes,
                vec![
                    runtime.rootfs.clone(),
                    runtime.workspace.clone(),
                    runtime.output.clone(),
                ],
            )
        };
        let usage = tauri::async_runtime::spawn_blocking(move || {
            workspace_usage_bytes(&workspace, limit)?;
            require_filesystem_headroom(&storage_paths, MIN_RUNTIME_STORAGE_HEADROOM_BYTES)
        })
        .await
        .map_err(|error| format!("workspace monitor task failed: {error}"))?;
        if let Err(error) = usage {
            let jobs = {
                let owner = self.inner.lock().await;
                owner
                    .ledger
                    .jobs
                    .iter()
                    .filter(|job| !job.is_terminal())
                    .cloned()
                    .collect::<Vec<_>>()
            };
            let reason = format!(
                "workspace_budget_exceeded: shared workspace integrity/budget check failed ({error}); all active attempts were cancelled."
            );
            for job in jobs {
                let operation_id = format!("workspace-stop-{}", Uuid::new_v4());
                let requested = TerminalIntent {
                    target: TerminalIntentTarget::Failed,
                    operation_id: operation_id.clone(),
                    requested_at_ms: now_ms()?,
                    reason: reason.clone(),
                };
                match self
                    .cancel_attempt(app, &operation_id, &job, requested)
                    .await
                {
                    Ok(Some(intent)) => {
                        self.claim_terminal_intent(&job, &intent, None).await?;
                    }
                    Ok(None) => log::error!(
                        "[VCPMobileCLI] workspace stop did not confirm process tree gone: {}",
                        job.id
                    ),
                    Err(cancel_error) => log::error!(
                        "[VCPMobileCLI] workspace stop failed for {}: {cancel_error}",
                        job.id
                    ),
                }
            }
        }
        Ok(())
    }

    async fn cancel_attempt<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        operation_id: &str,
        job: &JobRecord,
        requested_intent: TerminalIntent,
    ) -> Result<Option<TerminalIntent>, String> {
        if requested_intent.operation_id != operation_id {
            return Err("terminal intent operation identity mismatch".to_string());
        }
        let Some(intent) = self.persist_terminal_intent(job, requested_intent).await? else {
            return Ok(None);
        };
        // Always reuse the first durable intent's operation id. A callback may have been lost
        // while ProcessHost still completed containment; a retry must remain the same mutation.
        let process_operation_id = intent.operation_id.as_str();
        let response = cancel_cli_process_inner(
            app,
            &CancelCliProcessRequest {
                operation_id: process_operation_id.to_string(),
                job_id: job.id.clone(),
                attempt_id: job.attempt_id.clone(),
                runtime_generation: job.runtime_generation,
                grace_ms: MAX_CANCEL_GRACE_MS,
            },
        )
        .await?;
        if response.operation_id != process_operation_id
            || response.job_id != job.id
            || response.attempt_id != job.attempt_id
            || response.runtime_generation != job.runtime_generation
        {
            return Err("ProcessHost cancel response identity mismatch".to_string());
        }
        // `missing` after an app-process ownership loss is not containment evidence. Only the
        // registered attempt owner may certify that its process group disappeared.
        if !process_host_confirmed_containment(&response) {
            return Ok(None);
        }
        // ProcessHost exits only after both output drainers reached EOF. Refresh counters before
        // claiming the terminal state so the final bytes cannot be lost behind cancel/timeout.
        let mut refresh_failure = "ProcessHost final output snapshot was unavailable".to_string();
        for attempt in 0..FINAL_OUTPUT_REFRESH_ATTEMPTS {
            let refresh_operation = format!("cancel-refresh-{}", Uuid::new_v4());
            match inspect_cli_process_inner(
                app,
                &InspectCliProcessRequest {
                    operation_id: refresh_operation.clone(),
                    job_id: job.id.clone(),
                    attempt_id: job.attempt_id.clone(),
                    runtime_generation: job.runtime_generation,
                },
            )
            .await
            {
                Ok(snapshot) if inspect_identity_matches(job, &refresh_operation, &snapshot) => {
                    self.apply_output_snapshot(job, &snapshot).await?;
                    return Ok(Some(intent));
                }
                Ok(_) => {
                    refresh_failure =
                        "ProcessHost final output snapshot identity mismatch".to_string();
                }
                Err(error) => {
                    refresh_failure = format!("ProcessHost final output snapshot failed: {error}");
                }
            }
            if attempt + 1 < FINAL_OUTPUT_REFRESH_ATTEMPTS {
                tokio::time::sleep(Duration::from_millis(FINAL_OUTPUT_REFRESH_RETRY_MS)).await;
            }
        }
        self.mark_output_snapshot_incomplete(job, &refresh_failure)
            .await?;
        Ok(Some(intent))
    }

    async fn persist_terminal_intent(
        &self,
        job: &JobRecord,
        requested: TerminalIntent,
    ) -> Result<Option<TerminalIntent>, String> {
        let mut owner = self.inner.lock().await;
        let mut next = owner.ledger.clone();
        let existing = next
            .find_job(&job.id)
            .and_then(|current| current.terminal_intent.clone());
        let intent = next.request_terminal_intent(
            &job.id,
            &job.attempt_id,
            job.runtime_generation,
            requested,
            now_ms()?,
        );
        if existing.is_none() && intent.is_some() {
            // This fsync completes before any signal crosses the Android bridge.
            replace_ledger_atomically(&mut owner, next).await?;
        }
        Ok(intent)
    }

    async fn ensure_deadline_intent(&self, job: &JobRecord) -> Result<JobRecord, String> {
        if job.is_terminal() || job.terminal_intent.is_some() || now_ms()? < job.deadline_at_ms {
            return Ok(job.clone());
        }
        let operation_id = format!("timeout-{}", Uuid::new_v4());
        let requested = TerminalIntent {
            target: TerminalIntentTarget::TimedOut,
            operation_id,
            requested_at_ms: now_ms()?,
            reason: "Command exceeded its absolute deadline and its owned process tree is gone."
                .to_string(),
        };
        self.persist_terminal_intent(job, requested).await?;
        Ok(self
            .job_snapshot(&job.id)
            .await
            .unwrap_or_else(|| job.clone()))
    }

    async fn mark_output_snapshot_incomplete(
        &self,
        job: &JobRecord,
        reason: &str,
    ) -> Result<(), String> {
        let mut owner = self.inner.lock().await;
        if let Some(current) = owner.ledger.find_job_mut(&job.id) {
            if current.attempt_id == job.attempt_id
                && current.runtime_generation == job.runtime_generation
                && !current.is_terminal()
            {
                current.stdout_truncated = true;
                current.stderr_truncated = true;
                current.reason = Some(format!("{reason}; final output may be incomplete."));
                current.updated_at_ms = now_ms()?;
            }
        }
        persist_owner(&owner).await
    }

    async fn apply_output_snapshot(
        &self,
        job: &JobRecord,
        snapshot: &InspectCliProcessResponse,
    ) -> Result<(), String> {
        let mut owner = self.inner.lock().await;
        if let Some(current) = owner.ledger.find_job_mut(&job.id) {
            if current.attempt_id == job.attempt_id
                && current.runtime_generation == job.runtime_generation
                && !current.is_terminal()
            {
                current.stdout_bytes = snapshot.stdout_bytes;
                current.stderr_bytes = snapshot.stderr_bytes;
                current.stdout_truncated = snapshot.stdout_truncated;
                current.stderr_truncated = snapshot.stderr_truncated;
                current.exit_code = snapshot.exit_code;
                current.updated_at_ms = now_ms()?;
            }
        }
        persist_owner(&owner).await
    }

    async fn cancel_action<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        operation_id: &str,
        job_id: &str,
        timed_out: bool,
    ) -> Result<ExecuteVcpMobileCliResponse, String> {
        let Some(job) = self.job_snapshot(job_id).await else {
            return self
                .domain_response(
                    operation_id,
                    DomainError::new(VcpCliErrorCode::JobNotFound, "CLI job was not found."),
                )
                .await;
        };
        if !job.is_terminal() {
            let requested = job.terminal_intent.clone().unwrap_or(TerminalIntent {
                target: if timed_out {
                    TerminalIntentTarget::TimedOut
                } else {
                    TerminalIntentTarget::Cancelled
                },
                operation_id: operation_id.to_string(),
                requested_at_ms: now_ms()?,
                reason: if timed_out {
                    "Command exceeded timeout and its owned process tree is gone.".to_string()
                } else {
                    "Cancelled; the owned process tree is gone.".to_string()
                },
            });
            let process_operation_id = requested.operation_id.clone();
            let Some(intent) = self
                .cancel_attempt(app, &process_operation_id, &job, requested)
                .await?
            else {
                return self
                    .domain_response(
                        operation_id,
                        DomainError::internal(
                            "ProcessHost did not confirm the owned process tree was gone",
                        ),
                    )
                    .await;
            };
            self.claim_terminal_intent(&job, &intent, None).await?;
        }
        self.job_response(operation_id, job_id, None, DEFAULT_BOUNDED_READ_BYTES)
            .await
    }

    async fn list_action<R: Runtime>(
        &self,
        _app: &AppHandle<R>,
        operation_id: &str,
    ) -> Result<ExecuteVcpMobileCliResponse, String> {
        let owner = self.inner.lock().await;
        let summaries = owner
            .ledger
            .jobs
            .iter()
            .rev()
            .map(job_summary)
            .collect::<Vec<_>>();
        Ok(ExecuteVcpMobileCliResponse {
            operation_id: operation_id.to_string(),
            runtime_generation: owner.ledger.runtime_generation,
            envelope: VcpCliResultEnvelope::success(VcpCliResultBody {
                content: vec![VcpCliContentPart::text(render_job_list(&summaries))],
                job: None,
                jobs: Some(summaries),
                skill: None,
                skills: None,
                runtime: Some(VcpCliRuntimeInfo::local_loopback()),
            }),
        })
    }

    async fn list_skills_action<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        operation_id: &str,
    ) -> Result<ExecuteVcpMobileCliResponse, String> {
        let runtime = match self.ensure_provisioned(app, operation_id).await {
            Ok(runtime) => runtime,
            Err(error) => return self.domain_response(operation_id, error).await,
        };
        let root = runtime.skills;
        let listed = tauri::async_runtime::spawn_blocking(move || list_skills(&root))
            .await
            .map_err(|error| format!("Skill scan task failed: {error}"))?;
        let listed = match listed {
            Ok(listed) => listed,
            Err(error) => {
                return self
                    .domain_response(operation_id, domain_skill_error(error))
                    .await
            }
        };
        let mut content = vec![VcpCliContentPart::text(render_skill_list(&listed.skills))];
        content.extend(listed.warnings.into_iter().map(VcpCliContentPart::text));
        let generation = self.inner.lock().await.ledger.runtime_generation;
        Ok(ExecuteVcpMobileCliResponse {
            operation_id: operation_id.to_string(),
            runtime_generation: generation,
            envelope: VcpCliResultEnvelope::success(VcpCliResultBody {
                content,
                job: None,
                jobs: None,
                skill: None,
                skills: Some(listed.skills),
                runtime: Some(VcpCliRuntimeInfo::local_loopback()),
            }),
        })
    }

    async fn read_skill_action<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        operation_id: &str,
        skill_id: String,
        resource_path: String,
        max_bytes: usize,
    ) -> Result<ExecuteVcpMobileCliResponse, String> {
        let runtime = match self.ensure_provisioned(app, operation_id).await {
            Ok(runtime) => runtime,
            Err(error) => return self.domain_response(operation_id, error).await,
        };
        let root = runtime.skills;
        let result = tauri::async_runtime::spawn_blocking(move || {
            read_skill(
                &root,
                &skill_id,
                &resource_path,
                max_bytes.min(MAX_SKILL_BYTES),
            )
        })
        .await
        .map_err(|error| format!("Skill read task failed: {error}"))?;
        let (skill, content) = match result {
            Ok(result) => result,
            Err(error) => {
                return self
                    .domain_response(operation_id, domain_skill_error(error))
                    .await
            }
        };
        let generation = self.inner.lock().await.ledger.runtime_generation;
        Ok(ExecuteVcpMobileCliResponse {
            operation_id: operation_id.to_string(),
            runtime_generation: generation,
            envelope: VcpCliResultEnvelope::success(VcpCliResultBody {
                content: vec![VcpCliContentPart::text(content)],
                job: None,
                jobs: None,
                skill: Some(skill),
                skills: None,
                runtime: Some(VcpCliRuntimeInfo::local_loopback()),
            }),
        })
    }

    async fn job_response(
        &self,
        operation_id: &str,
        job_id: &str,
        cursor: Option<&str>,
        max_output_bytes: usize,
    ) -> Result<ExecuteVcpMobileCliResponse, String> {
        let (job, output_root) = {
            let owner = self.inner.lock().await;
            let Some(job) = owner.ledger.find_job(job_id).cloned() else {
                return Ok(ExecuteVcpMobileCliResponse {
                    operation_id: operation_id.to_string(),
                    runtime_generation: owner.ledger.runtime_generation,
                    envelope: VcpCliResultEnvelope::error(
                        VcpCliErrorCode::JobNotFound,
                        "CLI job was not found",
                        "Use action=list to refresh job identifiers.",
                    ),
                });
            };
            let output = owner
                .provisioned
                .as_ref()
                .map(|runtime| runtime.output.clone());
            (job, output)
        };
        let (artifact, artifact_error) =
            match self.output_artifact_ref(&job, output_root.as_deref()).await {
                Ok(artifact) => (artifact, None),
                Err(error) => {
                    log::error!(
                    "[VCPMobileCLI] terminal output artifact verification failed for {}: {error}",
                    job.id
                );
                    (None, Some(error))
                }
            };
        let (stdout, stderr, next_cursor, mut truncated, safety_projected) = match (
            output_root,
            job.stdout_path.as_deref(),
            job.stderr_path.as_deref(),
        ) {
            (Some(output_root), Some(stdout_path), Some(stderr_path)) => {
                let chunk = read_incremental_output(OutputReadRequest {
                    output_root: &output_root,
                    stdout_path,
                    stderr_path,
                    runtime_generation: job.runtime_generation,
                    job_id: &job.id,
                    attempt_id: &job.attempt_id,
                    stdout_bytes: job.stdout_bytes,
                    stderr_bytes: job.stderr_bytes,
                    cursor,
                    max_output_bytes,
                    source_truncated: job.stdout_truncated || job.stderr_truncated,
                    source_terminal: job.is_terminal(),
                })
                .await?;
                (
                    chunk.stdout,
                    chunk.stderr,
                    Some(chunk.cursor),
                    chunk.truncated,
                    chunk.safety_projected,
                )
            }
            _ => (
                String::new(),
                String::new(),
                Some(initial_cursor(
                    job.runtime_generation,
                    &job.id,
                    &job.attempt_id,
                )?),
                job.stdout_truncated || job.stderr_truncated,
                false,
            ),
        };
        let mut output_content = String::new();
        if !stdout.is_empty() {
            output_content.push_str(&stdout);
        }
        if !stderr.is_empty() {
            if !output_content.is_empty() {
                output_content.push('\n');
            }
            output_content.push_str("[stderr]\n");
            output_content.push_str(&stderr);
        }
        if safety_projected {
            output_content.push_str(
                "\n[output safety projection removed terminal controls and/or redacted sensitive text]",
            );
        }
        if artifact_error.is_some() {
            truncated = true;
            output_content.push_str(
                "\n[output artifact integrity metadata unavailable; cursor text remains bounded]",
            );
        }
        let content = render_job_result(&job, next_cursor.as_deref(), truncated, &output_content);
        Ok(ExecuteVcpMobileCliResponse {
            operation_id: operation_id.to_string(),
            runtime_generation: job.runtime_generation,
            envelope: VcpCliResultEnvelope::success(VcpCliResultBody {
                content: vec![VcpCliContentPart::text(content)],
                job: Some(VcpCliJobResult {
                    id: job.id,
                    state: job.state,
                    stdout,
                    stderr,
                    exit_code: job.exit_code,
                    cursor: next_cursor,
                    truncated,
                    artifact,
                    reason: job.reason,
                }),
                jobs: None,
                skill: None,
                skills: None,
                runtime: Some(VcpCliRuntimeInfo::local_loopback_shell()),
            }),
        })
    }

    async fn output_artifact_ref(
        &self,
        job: &JobRecord,
        output_root: Option<&std::path::Path>,
    ) -> Result<Option<VcpCliArtifactRef>, String> {
        if !job.is_terminal() {
            return Ok(None);
        }
        let (Some(output_root), Some(stdout_path), Some(stderr_path)) = (
            output_root,
            job.stdout_path.as_deref(),
            job.stderr_path.as_deref(),
        ) else {
            return Ok(None);
        };
        let expected_size = job
            .stdout_bytes
            .checked_add(job.stderr_bytes)
            .ok_or_else(|| "CLI output artifact size overflow".to_string())?;
        let digest = match (&job.artifact_sha256, job.artifact_size_bytes) {
            (Some(sha256), Some(size_bytes)) if size_bytes == expected_size => {
                super::output::OutputArtifactDigest {
                    sha256: sha256.clone(),
                    size_bytes,
                }
            }
            _ => {
                let output_root = output_root.to_path_buf();
                let stdout_path = stdout_path.to_path_buf();
                let stderr_path = stderr_path.to_path_buf();
                let stdout_bytes = job.stdout_bytes;
                let stderr_bytes = job.stderr_bytes;
                let digest = tauri::async_runtime::spawn_blocking(move || {
                    hash_output_artifact_pair(
                        &output_root,
                        &stdout_path,
                        &stderr_path,
                        stdout_bytes,
                        stderr_bytes,
                    )
                })
                .await
                .map_err(|error| format!("output artifact hash task failed: {error}"))??;
                let mut owner = self.inner.lock().await;
                if let Some(current) = owner.ledger.find_job_mut(&job.id) {
                    if current.attempt_id == job.attempt_id
                        && current.runtime_generation == job.runtime_generation
                        && current.is_terminal()
                        && current.stdout_bytes == job.stdout_bytes
                        && current.stderr_bytes == job.stderr_bytes
                    {
                        current.artifact_sha256 = Some(digest.sha256.clone());
                        current.artifact_size_bytes = Some(digest.size_bytes);
                        persist_owner(&owner).await?;
                    }
                }
                digest
            }
        };
        Ok(Some(VcpCliArtifactRef {
            id: output_artifact_id(job),
            sha256: digest.sha256,
            size_bytes: digest.size_bytes,
            mime_type: Some("application/vnd.vcp-mobile.cli-output-pair.v1".to_string()),
        }))
    }

    async fn claim_terminal_intent(
        &self,
        job: &JobRecord,
        intent: &TerminalIntent,
        exit_code: Option<i32>,
    ) -> Result<(), String> {
        let mut owner = self.inner.lock().await;
        let mut next = owner.ledger.clone();
        let snapshot_reason = next
            .find_job(&job.id)
            .and_then(|job| job.reason.as_deref())
            .filter(|reason| reason.contains("final output may be incomplete"))
            .map(ToOwned::to_owned);
        if let Some(claimed) = next.claim_terminal_intent(
            &job.id,
            &job.attempt_id,
            job.runtime_generation,
            intent,
            now_ms()?,
        ) {
            if let Some(current) = next.find_job_mut(&job.id) {
                current.exit_code = exit_code.or(claimed.exit_code);
                if let Some(snapshot_reason) = snapshot_reason {
                    let reason = current.reason.get_or_insert_with(|| intent.reason.clone());
                    reason.push(' ');
                    reason.push_str(&snapshot_reason);
                }
            }
            replace_ledger_atomically(&mut owner, next).await?;
        }
        drop(owner);
        if let Some(terminal) = self
            .job_snapshot(&job.id)
            .await
            .filter(JobRecord::is_terminal)
        {
            self.cleanup_river_projection(&terminal).await?;
        }
        Ok(())
    }

    async fn cleanup_river_projection(&self, job: &JobRecord) -> Result<(), String> {
        let Some(path) = job.river_projection_path.clone() else {
            return Ok(());
        };
        let root = {
            let owner = self.inner.lock().await;
            owner
                .provisioned
                .as_ref()
                .map(|runtime| runtime.projection_root.clone())
                .ok_or_else(|| "river projection root is unavailable".to_string())?
        };
        let generation = job.runtime_generation;
        let job_id = job.id.clone();
        let attempt_id = job.attempt_id.clone();
        let cleanup_path = path.clone();
        tauri::async_runtime::spawn_blocking(move || {
            remove_river_projection(&root, generation, &job_id, &attempt_id, &cleanup_path)
        })
        .await
        .map_err(|error| format!("river projection cleanup task failed: {error}"))??;

        let mut owner = self.inner.lock().await;
        if let Some(current) = owner.ledger.find_job_mut(&job.id) {
            if current.attempt_id == job.attempt_id
                && current.runtime_generation == job.runtime_generation
                && current.is_terminal()
                && current.river_projection_path.as_ref() == Some(&path)
            {
                current.river_projection_path = None;
                persist_owner(&owner).await?;
            }
        }
        Ok(())
    }

    async fn job_snapshot(&self, job_id: &str) -> Option<JobRecord> {
        self.inner.lock().await.ledger.find_job(job_id).cloned()
    }

    async fn bind_cancel_operation(
        &self,
        job_id: &str,
        operation_id: &str,
        action_sha256: &str,
        requested_intent: TerminalIntent,
    ) -> Result<CancelBindingOutcome, String> {
        if requested_intent.operation_id != operation_id {
            return Err("cancel binding and terminal intent identities differ".to_string());
        }
        let mut owner = self.inner.lock().await;
        let mut next = owner.ledger.clone();
        match next.operation(operation_id, action_sha256) {
            OperationLookup::Conflict => return Ok(CancelBindingOutcome::Conflict),
            OperationLookup::ReplayJob(existing_job) => {
                return Ok(if existing_job == job_id {
                    CancelBindingOutcome::Replay
                } else {
                    CancelBindingOutcome::Conflict
                });
            }
            OperationLookup::Replay(_) => return Ok(CancelBindingOutcome::Replay),
            OperationLookup::Missing => {}
        }
        let Some(job) = next.find_job_mut(job_id) else {
            // No durable job exists to bind. The caller will return JobNotFound and cache that
            // read-like response through the normal operation record path.
            return Ok(CancelBindingOutcome::Bound);
        };
        job.mutation_operations.push(MutationOperationBinding {
            operation_id: operation_id.to_string(),
            action_sha256: action_sha256.to_string(),
        });
        if !job.is_terminal() && job.terminal_intent.is_none() {
            job.terminal_intent = Some(requested_intent);
            job.updated_at_ms = now_ms()?;
        }
        // The public mutation binding and first terminal intent are one durable ledger write.
        replace_ledger_atomically(&mut owner, next).await?;
        Ok(CancelBindingOutcome::Bound)
    }

    async fn domain_response(
        &self,
        operation_id: &str,
        error: DomainError,
    ) -> Result<ExecuteVcpMobileCliResponse, String> {
        let generation = self.inner.lock().await.ledger.runtime_generation;
        Ok(ExecuteVcpMobileCliResponse {
            operation_id: operation_id.to_string(),
            runtime_generation: generation,
            envelope: VcpCliResultEnvelope::error(error.code, error.message.clone(), error.message),
        })
    }

    async fn set_phase_error(&self, error: String) {
        let mut owner = self.inner.lock().await;
        owner.phase = MobileCliPhase::Error;
        owner.availability_reason = Some(error);
    }
}

impl Default for MobileCliRuntimeState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct DomainError {
    code: VcpCliErrorCode,
    message: String,
}

impl DomainError {
    fn new(code: VcpCliErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(VcpCliErrorCode::InternalError, message)
    }
}

#[tauri::command]
pub async fn get_vcp_mobile_cli_status(
    app: AppHandle,
    state: State<'_, MobileCliRuntimeState>,
) -> Result<MobileCliStatus, String> {
    state.status(&app).await
}

#[tauri::command]
pub async fn execute_vcp_mobile_cli_action(
    app: AppHandle,
    state: State<'_, MobileCliRuntimeState>,
    request: ExecuteVcpMobileCliRequest,
) -> Result<ExecuteVcpMobileCliResponse, String> {
    state.execute(&app, request).await
}

fn status_from_owner(owner: &RuntimeOwner, profile: &VcpCliCommandProfile) -> MobileCliStatus {
    let running_jobs = owner
        .ledger
        .jobs
        .iter()
        .filter(|job| !job.is_terminal())
        .count();
    MobileCliStatus {
        available: cfg!(target_os = "android") && owner.phase == MobileCliPhase::Ready,
        availability_reason: owner
            .availability_reason
            .clone()
            .or_else(|| owner.recovery_notice.clone()),
        background_reliability: "foreground_only".to_string(),
        runtime_generation: owner.ledger.runtime_generation,
        phase: owner.phase.clone(),
        profile_id: profile.profile_id.clone(),
        max_concurrent_jobs: profile
            .budgets
            .default_concurrent_jobs
            .min(profile.budgets.max_concurrent_jobs)
            .min(4),
        running_jobs,
        jobs: owner.ledger.jobs.iter().rev().map(job_summary).collect(),
    }
}

fn job_summary(job: &JobRecord) -> VcpCliJobSummary {
    VcpCliJobSummary {
        id: job.id.clone(),
        attempt_id: job.attempt_id.clone(),
        state: job.state,
        command_preview: job.command_preview.clone(),
        description: job.description.clone(),
        created_at_ms: job.created_at_ms,
        updated_at_ms: job.updated_at_ms,
    }
}

fn render_job_list(jobs: &[VcpCliJobSummary]) -> String {
    let mut text = format!("retained_jobs: {}", jobs.len());
    for job in jobs {
        text.push_str(&format!(
            "\n- job_id: {} | state: {} | command: {}",
            job.id,
            job_state_name(job.state),
            job.command_preview
        ));
    }
    if jobs.is_empty() {
        text.push_str("\nNo retained jobs. Use action=run to start one.");
    } else {
        text.push_str("\nUse action=poll or action=cancel with job_id.");
    }
    text
}

fn render_skill_list(skills: &[super::result::VcpCliSkillSummary]) -> String {
    let mut text = format!("validated_skills: {}", skills.len());
    for skill in skills {
        text.push_str(&format!(
            "\n- skill_id: {} | name: {} | version: {} | source: {} | sha256: {}",
            skill.id,
            skill.name,
            skill.version.as_deref().unwrap_or("unknown"),
            skill.source,
            skill.sha256
        ));
    }
    text.push_str("\nUse action=read_skill with skill_id and resource_path=SKILL.md.");
    text
}

fn render_job_result(
    job: &JobRecord,
    cursor: Option<&str>,
    truncated: bool,
    output: &str,
) -> String {
    let mut text = format!(
        "job_id: {}\nstate: {}\ncursor: {}\ntruncated: {}\nexit_code: {}\nreason: {}",
        job.id,
        job_state_name(job.state),
        cursor.unwrap_or("none"),
        truncated,
        job.exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "none".to_string()),
        job.reason.as_deref().unwrap_or("none")
    );
    if output.is_empty() {
        text.push_str("\noutput: no new bounded output");
    } else {
        text.push_str("\noutput:\n");
        text.push_str(output);
    }
    if !job.is_terminal() {
        text.push_str("\nnext: poll again with job_id and cursor, or cancel with job_id");
    }
    text
}

const fn job_state_name(state: VcpCliJobState) -> &'static str {
    match state {
        VcpCliJobState::Queued => "queued",
        VcpCliJobState::Starting => "starting",
        VcpCliJobState::Running => "running",
        VcpCliJobState::Completed => "completed",
        VcpCliJobState::Failed => "failed",
        VcpCliJobState::TimedOut => "timed_out",
        VcpCliJobState::Cancelled => "cancelled",
        VcpCliJobState::Interrupted => "interrupted",
        VcpCliJobState::WaitingUser => "waiting_user",
    }
}

fn runtime_paths_record(runtime: &ProvisionedRuntime) -> RuntimePathsRecord {
    RuntimePathsRecord {
        rootfs: runtime.rootfs.clone(),
        workspace: runtime.workspace.clone(),
        skills: runtime.skills.clone(),
        output: runtime.output.clone(),
        projection_root: runtime.projection_root.clone(),
        proot_binary: runtime.proot_binary.clone(),
    }
}

async fn persist_owner(owner: &RuntimeOwner) -> Result<(), String> {
    let path = owner
        .ledger_path
        .as_deref()
        .ok_or_else(|| "CLI ledger is not initialized".to_string())?;
    owner.ledger.save_atomic(path).await
}

async fn replace_ledger_atomically(
    owner: &mut RuntimeOwner,
    replacement: JobLedger,
) -> Result<(), String> {
    let path = owner
        .ledger_path
        .as_deref()
        .ok_or_else(|| "CLI ledger is not initialized".to_string())?;
    replacement.save_atomic(path).await?;
    owner.ledger = replacement;
    Ok(())
}

fn validate_operation_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_OPERATION_ID_BYTES
        || !value.is_ascii()
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err("operation_id must be a stable ASCII identifier".to_string());
    }
    Ok(())
}

fn validate_session_id(value: Option<&str>) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_empty()
        || value.len() > 96
        || !value.is_ascii()
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err("session_id must be a bounded stable ASCII identifier".to_string());
    }
    Ok(())
}

fn session_concurrency_reached(jobs: &[JobRecord], session_id: &str, limit: usize) -> bool {
    jobs.iter()
        .filter(|job| !job.is_terminal() && job.session_id.as_deref() == Some(session_id))
        .count()
        >= limit
}

fn action_digest(
    action: &VcpCliAction,
    river_projection: Option<&VcpCliRiverProjectionInput>,
    session_id: Option<&str>,
) -> Result<String, String> {
    let bytes = serde_json::to_vec(&(action, river_projection, session_id))
        .map_err(|error| format!("cannot serialize CLI action: {error}"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn output_artifact_id(job: &JobRecord) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"VCPMobileCLIOutputPairHandle\0v1\0");
    hasher.update(job.runtime_generation.to_be_bytes());
    hasher.update(job.id.as_bytes());
    hasher.update([0]);
    hasher.update(job.attempt_id.as_bytes());
    // This is an opaque identity for the cursor-readable stdout/stderr pair. It is deliberately
    // neither a host path nor a URL and does not advertise a direct download command.
    format!("vcp-cli-output-pair.v1:{}", hex::encode(hasher.finalize()))
}

fn now_ms() -> Result<u64, String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock is before Unix epoch: {error}"))?
        .as_millis();
    u64::try_from(millis).map_err(|_| "system clock millisecond value overflow".to_string())
}

fn protocol_error_envelope(code: VcpCliErrorCode, diagnostic: &str) -> VcpCliResultEnvelope {
    VcpCliResultEnvelope::error(code, "Invalid VCPMobileCLI action", diagnostic)
}

fn domain_skill_error(error: SkillError) -> DomainError {
    let code = match error.kind {
        SkillErrorKind::NotFound => VcpCliErrorCode::SkillNotFound,
        SkillErrorKind::Integrity => VcpCliErrorCode::SkillIntegrityFailed,
    };
    DomainError::new(code, error.message)
}

fn inspect_identity_matches(
    job: &JobRecord,
    operation_id: &str,
    response: &InspectCliProcessResponse,
) -> bool {
    response.operation_id == operation_id
        && response.job_id == job.id
        && response.attempt_id == job.attempt_id
        && response.runtime_generation == job.runtime_generation
}

fn process_host_confirmed_containment(response: &CancelCliProcessResponse) -> bool {
    response.found && response.group_gone
}

fn start_identity_matches(
    request: &StartCliProcessRequest,
    response: &StartCliProcessResponse,
) -> bool {
    response.operation_id == request.operation_id
        && response.job_id == request.job_id
        && response.attempt_id == request.attempt_id
        && response.runtime_generation == request.runtime_generation
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MonitorDecision {
    Timeout,
    Inspect { sleep_ms: u64 },
}

fn monitor_decision(job: &JobRecord, now_ms: u64) -> MonitorDecision {
    if now_ms >= job.deadline_at_ms {
        MonitorDecision::Timeout
    } else {
        MonitorDecision::Inspect {
            sleep_ms: job
                .deadline_at_ms
                .saturating_sub(now_ms)
                .clamp(1, MONITOR_INTERVAL_MS),
        }
    }
}

fn spawn_job_monitor<R: Runtime>(
    app: AppHandle<R>,
    job_id: String,
    attempt_id: String,
    runtime_generation: u64,
) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<MobileCliRuntimeState>();
        if let Err(error) = state
            .monitor_job(&app, &job_id, &attempt_id, runtime_generation)
            .await
        {
            log::error!("[VCPMobileCLI] job monitor failed for {job_id}: {error}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running_test_job(generation: u64) -> JobRecord {
        JobRecord {
            id: "job-race".to_string(),
            attempt_id: "attempt-race".to_string(),
            runtime_generation: generation,
            session_id: None,
            state: VcpCliJobState::Running,
            command_preview: "sleep 300".to_string(),
            description: None,
            cwd: "/workspace".to_string(),
            timeout_ms: 300_000,
            created_at_ms: 1,
            updated_at_ms: 1,
            deadline_at_ms: 300_001,
            stdout_path: None,
            stderr_path: None,
            river_projection_path: None,
            stdout_bytes: 0,
            stderr_bytes: 0,
            stdout_truncated: false,
            stderr_truncated: false,
            artifact_sha256: None,
            artifact_size_bytes: None,
            exit_code: None,
            reason: None,
            terminal_intent: None,
            mutation_operations: Vec::new(),
        }
    }

    #[tokio::test]
    async fn explicit_cancel_binding_and_intent_share_one_durable_commit() {
        let directory = tempfile::tempdir().expect("temporary ledger directory");
        let path = directory.path().join("job-ledger.json");
        let state = MobileCliRuntimeState::new();
        {
            let mut owner = state.inner.lock().await;
            owner.initialized = true;
            owner.ledger_path = Some(path.clone());
            owner.ledger = JobLedger::empty(9);
            owner
                .ledger
                .insert_job(running_test_job(9))
                .expect("insert job");
            persist_owner(&owner).await.expect("persist initial ledger");
        }
        let intent = TerminalIntent {
            target: TerminalIntentTarget::Cancelled,
            operation_id: "cancel-operation".to_string(),
            requested_at_ms: 2,
            reason: "cancelled".to_string(),
        };

        let outcome = state
            .bind_cancel_operation(
                "job-race",
                "cancel-operation",
                "cancel-digest",
                intent.clone(),
            )
            .await
            .expect("bind cancel intent");
        assert_eq!(outcome, CancelBindingOutcome::Bound);

        let persisted: JobLedger =
            serde_json::from_slice(&tokio::fs::read(&path).await.expect("read durable ledger"))
                .expect("decode durable ledger");
        let job = persisted.find_job("job-race").expect("persisted job");
        assert_eq!(job.terminal_intent, Some(intent));
        assert!(job.mutation_operations.iter().any(|binding| {
            binding.operation_id == "cancel-operation" && binding.action_sha256 == "cancel-digest"
        }));
    }

    #[tokio::test]
    async fn concurrent_cancel_operation_id_cannot_bind_two_jobs() {
        let directory = tempfile::tempdir().expect("temporary ledger directory");
        let path = directory.path().join("job-ledger.json");
        let state = MobileCliRuntimeState::new();
        {
            let mut owner = state.inner.lock().await;
            owner.initialized = true;
            owner.ledger_path = Some(path);
            owner.ledger = JobLedger::empty(9);
            owner
                .ledger
                .insert_job(running_test_job(9))
                .expect("insert first job");
            let mut second = running_test_job(9);
            second.id = "job-race-2".to_string();
            second.attempt_id = "attempt-race-2".to_string();
            owner.ledger.insert_job(second).expect("insert second job");
            persist_owner(&owner).await.expect("persist initial ledger");
        }
        let first = state.bind_cancel_operation(
            "job-race",
            "shared-operation",
            "digest-job-1",
            TerminalIntent {
                target: TerminalIntentTarget::Cancelled,
                operation_id: "shared-operation".to_string(),
                requested_at_ms: 2,
                reason: "cancel first".to_string(),
            },
        );
        let second = state.bind_cancel_operation(
            "job-race-2",
            "shared-operation",
            "digest-job-2",
            TerminalIntent {
                target: TerminalIntentTarget::Cancelled,
                operation_id: "shared-operation".to_string(),
                requested_at_ms: 2,
                reason: "cancel second".to_string(),
            },
        );
        let (first, second) = tokio::join!(first, second);
        let outcomes = [first.expect("first bind"), second.expect("second bind")];
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == CancelBindingOutcome::Bound)
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == CancelBindingOutcome::Conflict)
                .count(),
            1
        );
        let owner = state.inner.lock().await;
        let bound_jobs = owner
            .ledger
            .jobs
            .iter()
            .filter(|job| {
                job.mutation_operations
                    .iter()
                    .any(|binding| binding.operation_id == "shared-operation")
            })
            .count();
        assert_eq!(bound_jobs, 1);
    }

    #[tokio::test]
    async fn expired_attempt_persists_timeout_intent_before_process_observation() {
        let directory = tempfile::tempdir().expect("temporary ledger directory");
        let path = directory.path().join("job-ledger.json");
        let state = MobileCliRuntimeState::new();
        let mut expired = running_test_job(9);
        expired.deadline_at_ms = 1;
        {
            let mut owner = state.inner.lock().await;
            owner.initialized = true;
            owner.ledger_path = Some(path.clone());
            owner.ledger = JobLedger::empty(9);
            owner
                .ledger
                .insert_job(expired.clone())
                .expect("insert expired job");
            persist_owner(&owner).await.expect("persist initial ledger");
        }

        let fenced = state
            .ensure_deadline_intent(&expired)
            .await
            .expect("persist timeout intent");
        assert_eq!(
            fenced.terminal_intent.as_ref().map(|intent| intent.target),
            Some(TerminalIntentTarget::TimedOut)
        );
        let persisted: JobLedger =
            serde_json::from_slice(&tokio::fs::read(&path).await.expect("read durable ledger"))
                .expect("decode durable ledger");
        assert_eq!(
            persisted
                .find_job("job-race")
                .and_then(|job| job.terminal_intent.as_ref())
                .map(|intent| intent.target),
            Some(TerminalIntentTarget::TimedOut)
        );
    }

    #[test]
    fn missing_process_host_handle_is_never_containment_proof() {
        let response = CancelCliProcessResponse {
            operation_id: "cancel-operation".to_string(),
            job_id: "job-race".to_string(),
            attempt_id: "attempt-race".to_string(),
            runtime_generation: 9,
            found: false,
            term_sent: false,
            kill_sent: false,
            group_gone: true,
            exit_code: None,
        };
        assert!(!process_host_confirmed_containment(&response));
        assert!(process_host_confirmed_containment(
            &CancelCliProcessResponse {
                found: true,
                ..response
            }
        ));
    }

    #[test]
    fn status_contract_has_starting_and_bounded_command_preview() {
        let profile = embedded_command_profile().expect("embedded profile");
        let mut owner = RuntimeOwner {
            initialized: true,
            phase: MobileCliPhase::Ready,
            availability_reason: None,
            recovery_notice: None,
            ledger_path: None,
            ledger: JobLedger::empty(3),
            provisioned: None,
            last_workspace_check_ms: 0,
        };
        owner
            .ledger
            .insert_job(JobRecord {
                id: "job-1".to_string(),
                attempt_id: "attempt-1".to_string(),
                runtime_generation: 3,
                session_id: None,
                state: VcpCliJobState::Starting,
                command_preview: command_preview(&"打印 ".repeat(100)),
                description: None,
                cwd: "/workspace".to_string(),
                timeout_ms: 1_000,
                created_at_ms: 1,
                updated_at_ms: 1,
                deadline_at_ms: 1_001,
                stdout_path: None,
                stderr_path: None,
                river_projection_path: None,
                stdout_bytes: 0,
                stderr_bytes: 0,
                stdout_truncated: false,
                stderr_truncated: false,
                artifact_sha256: None,
                artifact_size_bytes: None,
                exit_code: None,
                reason: None,
                terminal_intent: None,
                mutation_operations: Vec::new(),
            })
            .expect("insert job");
        let status = status_from_owner(&owner, &profile);
        assert_eq!(status.running_jobs, 1);
        assert_eq!(status.jobs[0].state, VcpCliJobState::Starting);
        assert!(status.jobs[0].command_preview.len() <= 160);
        assert_eq!(status.background_reliability, "foreground_only");
    }

    #[test]
    fn chat_session_allows_two_parallel_jobs_and_rejects_the_third() {
        let mut first = running_test_job(1);
        first.session_id = Some("chat:a".to_string());
        let mut second = first.clone();
        second.id = "job-second".to_string();
        second.attempt_id = "attempt-second".to_string();
        assert!(!session_concurrency_reached(
            std::slice::from_ref(&first),
            "chat:a",
            2
        ));
        assert!(session_concurrency_reached(
            &[first.clone(), second.clone()],
            "chat:a",
            2
        ));
        assert!(!session_concurrency_reached(
            &[first.clone(), second],
            "chat:b",
            2
        ));
        first.state = VcpCliJobState::Completed;
        assert!(!session_concurrency_reached(&[first], "chat:a", 2));
        assert!(validate_session_id(Some("chat:0123abcdef")).is_ok());
        assert!(validate_session_id(Some("chat path")).is_err());
    }

    #[test]
    fn structured_action_wire_and_operation_id_are_strict() {
        let action: VcpCliAction = serde_json::from_value(serde_json::json!({
            "action": "run",
            "command": "printf ok",
            "cwd": "/workspace",
            "timeout_ms": 1800000,
            "run_in_background": false
        }))
        .expect("parse structured action");
        assert!(validate_structured_vcp_cli_action(action).is_ok());
        assert!(validate_operation_id("operation-1").is_ok());
        assert!(validate_operation_id("bad operation").is_err());
    }

    #[test]
    fn terminal_states_never_include_starting_or_running() {
        assert!(!super::super::ledger::is_terminal_state(
            VcpCliJobState::Starting
        ));
        assert!(!super::super::ledger::is_terminal_state(
            VcpCliJobState::Running
        ));
        assert!(super::super::ledger::is_terminal_state(
            VcpCliJobState::Interrupted
        ));
    }

    #[test]
    fn monitor_deadline_is_absolute_and_never_extended_by_foreground_wait() {
        let mut job = JobLedger::empty(1);
        let record = JobRecord {
            id: "job-deadline".to_string(),
            attempt_id: "attempt-deadline".to_string(),
            runtime_generation: 1,
            session_id: None,
            state: VcpCliJobState::Running,
            command_preview: "sleep 1".to_string(),
            description: None,
            cwd: "/workspace".to_string(),
            timeout_ms: 1_000,
            created_at_ms: 100,
            updated_at_ms: 100,
            deadline_at_ms: 1_100,
            stdout_path: None,
            stderr_path: None,
            river_projection_path: None,
            stdout_bytes: 0,
            stderr_bytes: 0,
            stdout_truncated: false,
            stderr_truncated: false,
            artifact_sha256: None,
            artifact_size_bytes: None,
            exit_code: None,
            reason: None,
            terminal_intent: None,
            mutation_operations: Vec::new(),
        };
        let artifact_id = output_artifact_id(&record);
        assert!(artifact_id.starts_with("vcp-cli-output-pair.v1:"));
        assert!(!artifact_id.contains(&record.id));
        assert!(!artifact_id.contains('/'));
        let mut other_attempt = record.clone();
        other_attempt.attempt_id.push_str("-other");
        assert_ne!(artifact_id, output_artifact_id(&other_attempt));
        job.insert_job(record.clone()).expect("insert deadline job");
        assert_eq!(
            monitor_decision(&record, 1_099),
            MonitorDecision::Inspect { sleep_ms: 1 }
        );
        assert_eq!(monitor_decision(&record, 1_100), MonitorDecision::Timeout);
        assert_eq!(monitor_decision(&record, 9_100), MonitorDecision::Timeout);
    }
}
