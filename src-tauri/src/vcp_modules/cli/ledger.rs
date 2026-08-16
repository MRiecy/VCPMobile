//! P1 持久化 job ledger：generation、attempt fence 与 operationId 幂等的唯一事实源。

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use super::result::VcpCliJobState;

const LEGACY_LEDGER_SCHEMA_VERSION: u32 = 1;
const PRE_PROJECTION_LEDGER_SCHEMA_VERSION: u32 = 2;
const PRE_SESSION_LEDGER_SCHEMA_VERSION: u32 = 3;
const LEDGER_SCHEMA_VERSION: u32 = 4;
const MAX_LEDGER_BYTES: u64 = 40 * 1024 * 1024;
const MAX_RETAINED_JOBS: usize = 256;
const MAX_RETAINED_OPERATIONS: usize = 32;
const MAX_OPERATION_RESPONSE_BYTES: usize = 1024 * 1024;
pub const MAX_COMMAND_PREVIEW_BYTES: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(super) struct JobRecord {
    pub id: String,
    pub attempt_id: String,
    pub runtime_generation: u64,
    #[serde(default)]
    pub session_id: Option<String>,
    pub state: VcpCliJobState,
    pub command_preview: String,
    pub description: Option<String>,
    pub cwd: String,
    pub timeout_ms: u64,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub deadline_at_ms: u64,
    pub stdout_path: Option<PathBuf>,
    pub stderr_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub river_projection_path: Option<PathBuf>,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    #[serde(default)]
    pub artifact_sha256: Option<String>,
    #[serde(default)]
    pub artifact_size_bytes: Option<u64>,
    pub exit_code: Option<i32>,
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_intent: Option<TerminalIntent>,
    #[serde(default)]
    pub mutation_operations: Vec<MutationOperationBinding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TerminalIntentTarget {
    Cancelled,
    TimedOut,
    Failed,
    Interrupted,
}

impl TerminalIntentTarget {
    pub(super) const fn job_state(self) -> VcpCliJobState {
        match self {
            Self::Cancelled => VcpCliJobState::Cancelled,
            Self::TimedOut => VcpCliJobState::TimedOut,
            Self::Failed => VcpCliJobState::Failed,
            Self::Interrupted => VcpCliJobState::Interrupted,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(super) struct TerminalIntent {
    pub target: TerminalIntentTarget,
    pub operation_id: String,
    pub requested_at_ms: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProcessObservation {
    Running,
    Exited { exit_code: Option<i32> },
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ProcessOutputFacts {
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(super) struct MutationOperationBinding {
    pub operation_id: String,
    pub action_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(super) struct RuntimePathsRecord {
    pub rootfs: PathBuf,
    pub workspace: PathBuf,
    pub skills: PathBuf,
    pub output: PathBuf,
    #[serde(default)]
    pub projection_root: PathBuf,
    pub proot_binary: PathBuf,
}

impl JobRecord {
    pub(super) fn is_terminal(&self) -> bool {
        is_terminal_state(self.state)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct OperationRecord {
    operation_id: String,
    action_sha256: String,
    response: Value,
    created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(super) struct JobLedger {
    schema_version: u32,
    pub runtime_generation: u64,
    #[serde(default)]
    pub runtime_paths: Option<RuntimePathsRecord>,
    pub jobs: Vec<JobRecord>,
    operations: Vec<OperationRecord>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum OperationLookup {
    Missing,
    Replay(Value),
    ReplayJob(String),
    Conflict,
}

impl JobLedger {
    pub(super) fn empty(runtime_generation: u64) -> Self {
        Self {
            schema_version: LEDGER_SCHEMA_VERSION,
            runtime_generation,
            runtime_paths: None,
            jobs: Vec::new(),
            operations: Vec::new(),
        }
    }

    pub(super) async fn load_and_reconcile(path: &Path, now_ms: u64) -> Result<Self, String> {
        let mut ledger = match tokio::fs::symlink_metadata(path).await {
            Ok(metadata) => {
                if !metadata.file_type().is_file() || metadata.len() > MAX_LEDGER_BYTES {
                    return Err("CLI ledger must be a bounded regular file".to_string());
                }
                let bytes = tokio::fs::read(path)
                    .await
                    .map_err(|error| format!("cannot read CLI ledger: {error}"))?;
                serde_json::from_slice::<Self>(&bytes)
                    .map_err(|error| format!("invalid CLI ledger: {error}"))?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Self::empty(0),
            Err(error) => return Err(format!("cannot inspect CLI ledger: {error}")),
        };
        match ledger.schema_version {
            LEGACY_LEDGER_SCHEMA_VERSION => {
                // v1 had no terminal_intent field. serde(default) supplies None and the next
                // atomic save upgrades the document without quarantining valid user history.
                ledger.schema_version = LEDGER_SCHEMA_VERSION;
            }
            PRE_PROJECTION_LEDGER_SCHEMA_VERSION => {
                ledger.schema_version = LEDGER_SCHEMA_VERSION;
            }
            PRE_SESSION_LEDGER_SCHEMA_VERSION => {
                ledger.schema_version = LEDGER_SCHEMA_VERSION;
            }
            LEDGER_SCHEMA_VERSION => {}
            _ => return Err("unsupported CLI ledger schema".to_string()),
        }
        ledger.runtime_generation = ledger
            .runtime_generation
            .checked_add(1)
            .ok_or_else(|| "CLI runtime generation overflow".to_string())?;
        for job in &mut ledger.jobs {
            if !job.is_terminal() {
                job.state = VcpCliJobState::Interrupted;
                job.updated_at_ms = now_ms;
                job.reason = Some("App restarted; the previous attempt was not rerun.".to_string());
                // ProcessHost ownership is process-local. A persisted intent is evidence that
                // containment was requested, not proof that the old process tree disappeared.
                job.terminal_intent = None;
            }
        }
        Ok(ledger)
    }

    pub(super) async fn load_with_scoped_recovery(
        path: &Path,
        now_ms: u64,
    ) -> Result<(Self, Option<String>), String> {
        match Self::load_and_reconcile(path, now_ms).await {
            Ok(ledger) => Ok((ledger, None)),
            Err(original_error) => {
                let metadata = tokio::fs::symlink_metadata(path).await.map_err(|error| {
                    format!("{original_error}; cannot inspect recovery target: {error}")
                })?;
                if !metadata.file_type().is_file() || metadata.len() > MAX_LEDGER_BYTES {
                    return Err(original_error);
                }
                let parent = path
                    .parent()
                    .ok_or_else(|| format!("{original_error}; ledger has no recovery parent"))?;
                let backup = parent.join(format!("job-ledger.corrupt-{}.json", Uuid::new_v4()));
                tokio::fs::rename(path, &backup).await.map_err(|error| {
                    format!("{original_error}; cannot quarantine ledger: {error}")
                })?;
                let ledger = Self::empty(now_ms.max(1));
                ledger.save_atomic(path).await?;
                Ok((
                    ledger,
                    Some(format!(
                        "Recovered a corrupt CLI ledger; quarantined as {}",
                        backup
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("job-ledger.corrupt.json")
                    )),
                ))
            }
        }
    }

    pub(super) async fn save_atomic(&self, path: &Path) -> Result<(), String> {
        let parent = path
            .parent()
            .ok_or_else(|| "CLI ledger path has no parent".to_string())?;
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("cannot create CLI ledger directory: {error}"))?;
        let parent_metadata = tokio::fs::symlink_metadata(parent)
            .await
            .map_err(|error| format!("cannot inspect CLI ledger directory: {error}"))?;
        if !parent_metadata.file_type().is_dir() {
            return Err("CLI ledger parent must be a real directory".to_string());
        }

        let bytes = serde_json::to_vec(self)
            .map_err(|error| format!("cannot serialize CLI ledger: {error}"))?;
        if bytes.len() as u64 > MAX_LEDGER_BYTES {
            return Err("CLI ledger exceeds its hard size limit".to_string());
        }
        let temporary = parent.join(format!(".job-ledger-{}.tmp", Uuid::new_v4()));
        let result = async {
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .await
                .map_err(|error| format!("cannot create CLI ledger temporary file: {error}"))?;
            file.write_all(&bytes)
                .await
                .map_err(|error| format!("cannot write CLI ledger: {error}"))?;
            file.sync_all()
                .await
                .map_err(|error| format!("cannot sync CLI ledger: {error}"))?;
            drop(file);
            tokio::fs::rename(&temporary, path)
                .await
                .map_err(|error| format!("cannot atomically replace CLI ledger: {error}"))?;
            let parent = parent.to_path_buf();
            tauri::async_runtime::spawn_blocking(move || {
                std::fs::File::open(parent).and_then(|directory| directory.sync_all())
            })
            .await
            .map_err(|error| format!("CLI ledger directory sync task failed: {error}"))?
            .map_err(|error| format!("cannot sync CLI ledger directory: {error}"))?;
            Ok::<(), String>(())
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&temporary).await;
        }
        result
    }

    pub(super) fn operation(&self, operation_id: &str, action_sha256: &str) -> OperationLookup {
        if let Some((job, binding)) = self.jobs.iter().find_map(|job| {
            job.mutation_operations
                .iter()
                .find(|binding| binding.operation_id == operation_id)
                .map(|binding| (job, binding))
        }) {
            return if binding.action_sha256 == action_sha256 {
                OperationLookup::ReplayJob(job.id.clone())
            } else {
                OperationLookup::Conflict
            };
        }
        match self
            .operations
            .iter()
            .find(|record| record.operation_id == operation_id)
        {
            Some(record) if record.action_sha256 == action_sha256 => {
                OperationLookup::Replay(record.response.clone())
            }
            Some(_) => OperationLookup::Conflict,
            None => OperationLookup::Missing,
        }
    }

    pub(super) fn record_operation(
        &mut self,
        operation_id: String,
        action_sha256: String,
        response: Value,
        created_at_ms: u64,
    ) -> Result<(), String> {
        let encoded = serde_json::to_vec(&response)
            .map_err(|error| format!("cannot serialize operation response: {error}"))?;
        if encoded.len() > MAX_OPERATION_RESPONSE_BYTES {
            return Err("operation response exceeds idempotency cache limit".to_string());
        }
        if self.operations.len() >= MAX_RETAINED_OPERATIONS {
            self.operations.remove(0);
        }
        self.operations.push(OperationRecord {
            operation_id,
            action_sha256,
            response,
            created_at_ms,
        });
        Ok(())
    }

    pub(super) fn insert_job(&mut self, job: JobRecord) -> Result<Vec<JobRecord>, String> {
        let mut evicted = Vec::new();
        while self.jobs.len() >= MAX_RETAINED_JOBS {
            if let Some(index) = self.jobs.iter().position(JobRecord::is_terminal) {
                evicted.push(self.jobs.remove(index));
            } else {
                return Err("CLI job ledger is full of nonterminal attempts".to_string());
            }
        }
        self.jobs.push(job);
        Ok(evicted)
    }

    pub(super) fn find_job(&self, job_id: &str) -> Option<&JobRecord> {
        self.jobs.iter().find(|job| job.id == job_id)
    }

    /// Find the Run job created by a given operation id, regardless of that operation's
    /// action digest. Used by the Distributed `cancel_tool` path, which only carries the
    /// requestId (reduced to an operation id) and cannot reproduce the original digest.
    pub(super) fn job_for_operation(&self, operation_id: &str) -> Option<&JobRecord> {
        self.jobs.iter().find(|job| {
            job.mutation_operations
                .iter()
                .any(|binding| binding.operation_id == operation_id)
        })
    }

    pub(super) fn find_job_mut(&mut self, job_id: &str) -> Option<&mut JobRecord> {
        self.jobs.iter_mut().find(|job| job.id == job_id)
    }

    pub(super) fn claim_terminal(
        &mut self,
        job_id: &str,
        attempt_id: &str,
        runtime_generation: u64,
        terminal: VcpCliJobState,
        now_ms: u64,
        reason: impl Into<String>,
    ) -> Option<JobRecord> {
        if !is_terminal_state(terminal) {
            return None;
        }
        let job = self.find_job_mut(job_id)?;
        if job.attempt_id != attempt_id
            || job.runtime_generation != runtime_generation
            || job.is_terminal()
            || job.terminal_intent.is_some()
        {
            return None;
        }
        job.state = terminal;
        job.updated_at_ms = now_ms;
        job.reason = Some(reason.into());
        Some(job.clone())
    }

    pub(super) fn request_terminal_intent(
        &mut self,
        job_id: &str,
        attempt_id: &str,
        runtime_generation: u64,
        requested: TerminalIntent,
        now_ms: u64,
    ) -> Option<TerminalIntent> {
        let job = self.find_job_mut(job_id)?;
        if job.attempt_id != attempt_id
            || job.runtime_generation != runtime_generation
            || job.is_terminal()
        {
            return None;
        }
        if let Some(existing) = &job.terminal_intent {
            return Some(existing.clone());
        }
        job.terminal_intent = Some(requested.clone());
        job.updated_at_ms = now_ms;
        Some(requested)
    }

    pub(super) fn claim_terminal_intent(
        &mut self,
        job_id: &str,
        attempt_id: &str,
        runtime_generation: u64,
        intent: &TerminalIntent,
        now_ms: u64,
    ) -> Option<JobRecord> {
        let job = self.find_job_mut(job_id)?;
        if job.attempt_id != attempt_id
            || job.runtime_generation != runtime_generation
            || job.is_terminal()
            || job.terminal_intent.as_ref() != Some(intent)
        {
            return None;
        }
        job.state = intent.target.job_state();
        job.updated_at_ms = now_ms;
        job.reason = Some(intent.reason.clone());
        Some(job.clone())
    }

    pub(super) fn apply_process_observation(
        &mut self,
        job_id: &str,
        attempt_id: &str,
        runtime_generation: u64,
        observation: ProcessObservation,
        output: ProcessOutputFacts,
        now_ms: u64,
    ) -> bool {
        let Some(job) = self.find_job_mut(job_id) else {
            return false;
        };
        if job.attempt_id != attempt_id
            || job.runtime_generation != runtime_generation
            || job.is_terminal()
        {
            return false;
        }
        if !matches!(observation, ProcessObservation::Missing) {
            // Concurrent inspect callbacks may be applied out of submission order. ProcessHost
            // counters are append-only, so stale snapshots must not move durable cursors back.
            job.stdout_bytes = job.stdout_bytes.max(output.stdout_bytes);
            job.stderr_bytes = job.stderr_bytes.max(output.stderr_bytes);
            job.stdout_truncated |= output.stdout_truncated;
            job.stderr_truncated |= output.stderr_truncated;
        }
        job.updated_at_ms = now_ms;

        // Once an intent is durable, inspect remains a fact collector. Only a matching
        // ProcessHost Exited snapshot may publish it because Kotlin exposes Exited only after
        // the owned group is gone and both output readers reached terminal drain.
        if let Some(intent) = job.terminal_intent.as_ref() {
            match observation {
                // A durable intent pins the job out of Running: requested-but-unconfirmed
                // termination stays visible as Stopping and never returns to Running.
                ProcessObservation::Running => job.state = VcpCliJobState::Stopping,
                ProcessObservation::Exited { exit_code } => {
                    job.exit_code = exit_code;
                    job.state = intent.target.job_state();
                    job.reason = Some(intent.reason.clone());
                }
                ProcessObservation::Missing => {}
            }
            return true;
        }

        match observation {
            ProcessObservation::Running => job.state = VcpCliJobState::Running,
            ProcessObservation::Exited { exit_code } => {
                job.exit_code = exit_code;
                job.state = if exit_code == Some(0) {
                    VcpCliJobState::Completed
                } else {
                    VcpCliJobState::Failed
                };
                job.reason = exit_code
                    .filter(|code| *code != 0)
                    .map(|code| format!("Bash exited with status {code}."));
            }
            ProcessObservation::Missing => {
                job.state = VcpCliJobState::Interrupted;
                job.reason = Some("ProcessHost lost this attempt; it was not rerun.".to_string());
            }
        }
        true
    }
}

pub(super) fn is_terminal_state(state: VcpCliJobState) -> bool {
    matches!(
        state,
        VcpCliJobState::Completed
            | VcpCliJobState::Failed
            | VcpCliJobState::TimedOut
            | VcpCliJobState::Cancelled
            | VcpCliJobState::Interrupted
    )
}

pub(super) fn command_preview(command: &str) -> String {
    let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.len() <= MAX_COMMAND_PREVIEW_BYTES {
        return normalized;
    }
    let suffix = "…";
    let limit = MAX_COMMAND_PREVIEW_BYTES.saturating_sub(suffix.len());
    let mut end = limit.min(normalized.len());
    while end > 0 && !normalized.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &normalized[..end], suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running_job(generation: u64) -> JobRecord {
        JobRecord {
            id: "job-1".to_string(),
            attempt_id: "attempt-1".to_string(),
            runtime_generation: generation,
            session_id: None,
            state: VcpCliJobState::Running,
            command_preview: "sleep 30".to_string(),
            description: None,
            cwd: "/workspace".to_string(),
            timeout_ms: 30_000,
            created_at_ms: 1,
            updated_at_ms: 1,
            deadline_at_ms: 30_001,
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
    async fn restart_increments_generation_and_never_requeues_nonterminal_jobs() {
        let directory = tempfile::tempdir().expect("temporary ledger directory");
        let path = directory.path().join("ledger.json");
        let mut ledger = JobLedger::empty(4);
        ledger.insert_job(running_job(4)).expect("insert job");
        ledger.save_atomic(&path).await.expect("persist ledger");

        let recovered = JobLedger::load_and_reconcile(&path, 100)
            .await
            .expect("recover ledger");
        assert_eq!(recovered.runtime_generation, 5);
        assert_eq!(recovered.jobs[0].state, VcpCliJobState::Interrupted);
        assert!(recovered.jobs[0]
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("not rerun")));
    }

    #[test]
    fn operation_id_is_idempotent_and_digest_bound() {
        let mut ledger = JobLedger::empty(1);
        ledger
            .record_operation(
                "operation-1".to_string(),
                "digest-a".to_string(),
                serde_json::json!({"ok": true}),
                1,
            )
            .expect("record operation");
        assert!(matches!(
            ledger.operation("operation-1", "digest-a"),
            OperationLookup::Replay(_)
        ));
        assert_eq!(
            ledger.operation("operation-1", "digest-b"),
            OperationLookup::Conflict
        );
    }

    #[test]
    fn durable_job_binding_survives_read_only_operation_cache_churn() {
        let mut ledger = JobLedger::empty(1);
        let mut job = running_job(1);
        job.mutation_operations.push(MutationOperationBinding {
            operation_id: "run-operation".to_string(),
            action_sha256: "run-digest".to_string(),
        });
        ledger.insert_job(job).expect("insert job");
        for index in 0..(MAX_RETAINED_OPERATIONS + 300) {
            ledger
                .record_operation(
                    format!("poll-{index}"),
                    format!("poll-digest-{index}"),
                    serde_json::json!({"index": index}),
                    index as u64,
                )
                .expect("record read-only operation");
        }
        assert_eq!(
            ledger.operation("run-operation", "run-digest"),
            OperationLookup::ReplayJob("job-1".to_string())
        );
        assert_eq!(ledger.jobs.len(), 1);
    }

    #[test]
    fn job_for_operation_finds_run_by_operation_without_digest() {
        let mut ledger = JobLedger::empty(1);
        let mut job = running_job(1);
        job.mutation_operations.push(MutationOperationBinding {
            operation_id: "dist:abc123".to_string(),
            action_sha256: "run-digest".to_string(),
        });
        ledger.insert_job(job).expect("insert job");
        let found = ledger
            .job_for_operation("dist:abc123")
            .expect("run job found by operation id");
        assert_eq!(found.id, "job-1");
        assert!(ledger.job_for_operation("dist:missing").is_none());
    }

    #[test]
    fn terminal_claim_is_attempt_and_generation_fenced_exactly_once() {
        let mut ledger = JobLedger::empty(8);
        ledger.insert_job(running_job(8)).expect("insert job");
        assert!(ledger
            .claim_terminal(
                "job-1",
                "wrong-attempt",
                8,
                VcpCliJobState::Cancelled,
                2,
                "cancelled",
            )
            .is_none());
        assert!(ledger
            .claim_terminal(
                "job-1",
                "attempt-1",
                8,
                VcpCliJobState::Cancelled,
                2,
                "cancelled",
            )
            .is_some());
        assert!(ledger
            .claim_terminal(
                "job-1",
                "attempt-1",
                8,
                VcpCliJobState::TimedOut,
                3,
                "late timeout",
            )
            .is_none());
        assert_eq!(ledger.jobs[0].state, VcpCliJobState::Cancelled);
    }

    #[test]
    fn first_durable_terminal_intent_wins_and_fences_natural_terminal_claims() {
        let mut ledger = JobLedger::empty(8);
        ledger.insert_job(running_job(8)).expect("insert job");
        let cancelled = TerminalIntent {
            target: TerminalIntentTarget::Cancelled,
            operation_id: "cancel-operation".to_string(),
            requested_at_ms: 2,
            reason: "cancelled".to_string(),
        };
        let timed_out = TerminalIntent {
            target: TerminalIntentTarget::TimedOut,
            operation_id: "timeout-operation".to_string(),
            requested_at_ms: 3,
            reason: "timed out".to_string(),
        };

        assert_eq!(
            ledger.request_terminal_intent("job-1", "attempt-1", 8, cancelled.clone(), 2),
            Some(cancelled.clone())
        );
        assert_eq!(
            ledger.request_terminal_intent("job-1", "attempt-1", 8, timed_out, 3),
            Some(cancelled.clone())
        );
        assert!(ledger
            .claim_terminal(
                "job-1",
                "attempt-1",
                8,
                VcpCliJobState::Failed,
                3,
                "late Bash exit",
            )
            .is_none());
        assert!(ledger
            .claim_terminal_intent("job-1", "attempt-1", 8, &cancelled, 4)
            .is_some());
        assert!(ledger
            .claim_terminal_intent("job-1", "attempt-1", 8, &cancelled, 5)
            .is_none());
        assert_eq!(ledger.jobs[0].state, VcpCliJobState::Cancelled);
    }

    #[tokio::test]
    async fn terminal_intent_persists_but_restart_never_treats_it_as_containment_proof() {
        let directory = tempfile::tempdir().expect("temporary ledger directory");
        let path = directory.path().join("ledger.json");
        let mut ledger = JobLedger::empty(4);
        ledger.insert_job(running_job(4)).expect("insert job");
        let intent = TerminalIntent {
            target: TerminalIntentTarget::TimedOut,
            operation_id: "timeout-operation".to_string(),
            requested_at_ms: 2,
            reason: "timed out".to_string(),
        };
        ledger.request_terminal_intent("job-1", "attempt-1", 4, intent.clone(), 2);
        ledger.save_atomic(&path).await.expect("persist intent");

        let persisted = serde_json::from_slice::<JobLedger>(
            &tokio::fs::read(&path).await.expect("read persisted ledger"),
        )
        .expect("deserialize persisted ledger");
        assert_eq!(persisted.jobs[0].terminal_intent, Some(intent));

        let recovered = JobLedger::load_and_reconcile(&path, 100)
            .await
            .expect("recover ledger");
        assert_eq!(recovered.jobs[0].state, VcpCliJobState::Interrupted);
        assert_eq!(recovered.jobs[0].terminal_intent, None);
    }

    #[test]
    fn inspect_exit_after_any_durable_intent_publishes_the_intended_terminal() {
        for (target, expected) in [
            (TerminalIntentTarget::Cancelled, VcpCliJobState::Cancelled),
            (TerminalIntentTarget::TimedOut, VcpCliJobState::TimedOut),
            (TerminalIntentTarget::Failed, VcpCliJobState::Failed),
            (TerminalIntentTarget::Interrupted, VcpCliJobState::Interrupted),
        ] {
            let mut ledger = JobLedger::empty(8);
            ledger.insert_job(running_job(8)).expect("insert job");
            let intent = TerminalIntent {
                target,
                operation_id: "containment-operation".to_string(),
                requested_at_ms: 2,
                reason: "contained".to_string(),
            };
            ledger.request_terminal_intent("job-1", "attempt-1", 8, intent.clone(), 2);

            assert!(ledger.apply_process_observation(
                "job-1",
                "attempt-1",
                8,
                ProcessObservation::Exited {
                    exit_code: Some(255),
                },
                ProcessOutputFacts {
                    stdout_bytes: 12,
                    stderr_bytes: 34,
                    stdout_truncated: false,
                    stderr_truncated: true,
                },
                3,
            ));
            assert_eq!(ledger.jobs[0].state, expected);
            assert_eq!(ledger.jobs[0].exit_code, Some(255));
            assert_eq!(ledger.jobs[0].stdout_bytes, 12);
            assert_eq!(ledger.jobs[0].stderr_bytes, 34);
            assert!(ledger.jobs[0].stderr_truncated);
            assert!(ledger
                .claim_terminal_intent("job-1", "attempt-1", 8, &intent, 4)
                .is_none());
            assert_eq!(ledger.jobs[0].state, expected);
        }
    }

    #[test]
    fn durable_intent_pins_live_process_to_stopping_and_never_back_to_running() {
        let mut ledger = JobLedger::empty(8);
        ledger.insert_job(running_job(8)).expect("insert job");
        let intent = TerminalIntent {
            target: TerminalIntentTarget::Cancelled,
            operation_id: "cancel-operation".to_string(),
            requested_at_ms: 2,
            reason: "requested".to_string(),
        };
        ledger.request_terminal_intent("job-1", "attempt-1", 8, intent.clone(), 2);
        assert_eq!(ledger.jobs[0].terminal_intent, Some(intent));

        assert!(ledger.apply_process_observation(
            "job-1",
            "attempt-1",
            8,
            ProcessObservation::Running,
            ProcessOutputFacts {
                stdout_bytes: 1,
                stderr_bytes: 0,
                stdout_truncated: false,
                stderr_truncated: false,
            },
            3,
        ));
        assert_eq!(ledger.jobs[0].state, VcpCliJobState::Stopping);
        assert!(!ledger.jobs[0].is_terminal());
        assert!(!is_terminal_state(VcpCliJobState::Stopping));

        // A later Running observation must not regress Stopping back to Running.
        assert!(ledger.apply_process_observation(
            "job-1",
            "attempt-1",
            8,
            ProcessObservation::Running,
            ProcessOutputFacts {
                stdout_bytes: 2,
                stderr_bytes: 0,
                stdout_truncated: false,
                stderr_truncated: false,
            },
            4,
        ));
        assert_eq!(ledger.jobs[0].state, VcpCliJobState::Stopping);

        // Confirmed containment publishes the intended terminal state.
        assert!(ledger.apply_process_observation(
            "job-1",
            "attempt-1",
            8,
            ProcessObservation::Exited {
                exit_code: Some(143),
            },
            ProcessOutputFacts {
                stdout_bytes: 2,
                stderr_bytes: 0,
                stdout_truncated: false,
                stderr_truncated: false,
            },
            5,
        ));
        assert_eq!(ledger.jobs[0].state, VcpCliJobState::Cancelled);
        assert_eq!(ledger.jobs[0].reason.as_deref(), Some("requested"));
    }

    #[test]
    fn stale_or_missing_inspect_never_regresses_durable_output_facts() {
        let mut ledger = JobLedger::empty(8);
        let mut job = running_job(8);
        job.stdout_bytes = 20;
        job.stderr_bytes = 10;
        job.stdout_truncated = true;
        ledger.insert_job(job).expect("insert job");

        assert!(ledger.apply_process_observation(
            "job-1",
            "attempt-1",
            8,
            ProcessObservation::Running,
            ProcessOutputFacts {
                stdout_bytes: 12,
                stderr_bytes: 4,
                stdout_truncated: false,
                stderr_truncated: false,
            },
            2,
        ));
        assert!(ledger.apply_process_observation(
            "job-1",
            "attempt-1",
            8,
            ProcessObservation::Missing,
            ProcessOutputFacts {
                stdout_bytes: 0,
                stderr_bytes: 0,
                stdout_truncated: false,
                stderr_truncated: false,
            },
            3,
        ));
        assert_eq!(ledger.jobs[0].stdout_bytes, 20);
        assert_eq!(ledger.jobs[0].stderr_bytes, 10);
        assert!(ledger.jobs[0].stdout_truncated);
    }

    #[tokio::test]
    async fn schema_v1_without_terminal_intent_migrates_in_place() {
        let directory = tempfile::tempdir().expect("temporary ledger directory");
        let path = directory.path().join("ledger.json");
        let mut legacy = JobLedger::empty(4);
        legacy
            .insert_job(running_job(4))
            .expect("insert legacy job");
        let mut value = serde_json::to_value(legacy).expect("serialize ledger");
        value["schema_version"] = serde_json::json!(1);
        value["jobs"][0]
            .as_object_mut()
            .expect("legacy job object")
            .remove("terminal_intent");
        tokio::fs::write(
            &path,
            serde_json::to_vec(&value).expect("encode legacy ledger"),
        )
        .await
        .expect("write legacy ledger");

        let migrated = JobLedger::load_and_reconcile(&path, 100)
            .await
            .expect("migrate v1 ledger");
        assert_eq!(migrated.schema_version, LEDGER_SCHEMA_VERSION);
        assert_eq!(migrated.runtime_generation, 5);
        assert_eq!(migrated.jobs[0].state, VcpCliJobState::Interrupted);
        assert_eq!(migrated.jobs[0].terminal_intent, None);
        migrated
            .save_atomic(&path)
            .await
            .expect("persist migrated ledger");
        let upgraded: Value =
            serde_json::from_slice(&tokio::fs::read(&path).await.expect("read upgraded ledger"))
                .expect("decode upgraded ledger");
        assert_eq!(upgraded["schema_version"], LEDGER_SCHEMA_VERSION);
    }

    #[test]
    fn command_preview_is_utf8_safe_and_bounded() {
        let preview = command_preview(&"命令 ".repeat(100));
        assert!(preview.len() <= MAX_COMMAND_PREVIEW_BYTES);
        assert!(preview.ends_with('…'));
    }

    #[tokio::test]
    async fn corrupt_regular_ledger_is_quarantined_with_new_generation() {
        let directory = tempfile::tempdir().expect("temporary ledger directory");
        let path = directory.path().join("job-ledger.json");
        tokio::fs::write(&path, b"not-json")
            .await
            .expect("write corrupt ledger");
        let (ledger, notice) = JobLedger::load_with_scoped_recovery(&path, 1234)
            .await
            .expect("recover ledger");
        assert_eq!(ledger.runtime_generation, 1234);
        assert!(notice.is_some());
        assert!(tokio::fs::try_exists(&path).await.expect("ledger exists"));
        assert_eq!(
            std::fs::read_dir(directory.path())
                .expect("read recovery directory")
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().contains(".corrupt-"))
                .count(),
            1
        );
    }
}
