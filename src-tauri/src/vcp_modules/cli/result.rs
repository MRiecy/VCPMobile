use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum VcpCliContentPart {
    Text { text: String },
}

impl VcpCliContentPart {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VcpCliErrorCode {
    InvalidRequest,
    UnsupportedMode,
    JobNotFound,
    SkillNotFound,
    SkillIntegrityFailed,
    PermissionDenied,
    UserDisabled,
    RemoteDisconnected,
    RuntimeUnavailable,
    InternalError,
}

impl VcpCliErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::UnsupportedMode => "unsupported_mode",
            Self::JobNotFound => "job_not_found",
            Self::SkillNotFound => "skill_not_found",
            Self::SkillIntegrityFailed => "skill_integrity_failed",
            Self::PermissionDenied => "permission_denied",
            Self::UserDisabled => "user_disabled",
            Self::RemoteDisconnected => "remote_disconnected",
            Self::RuntimeUnavailable => "runtime_unavailable",
            Self::InternalError => "internal_error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VcpCliJobState {
    Queued,
    Starting,
    Running,
    Stopping,
    Completed,
    Failed,
    TimedOut,
    Cancelled,
    Interrupted,
    WaitingUser,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VcpCliArtifactRef {
    pub id: String,
    pub sha256: String,
    pub size_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VcpCliJobResult {
    pub id: String,
    pub state: VcpCliJobState,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub cursor: Option<String>,
    pub truncated: bool,
    pub artifact: Option<VcpCliArtifactRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VcpCliJobSummary {
    pub id: String,
    pub attempt_id: String,
    pub state: VcpCliJobState,
    pub command_preview: String,
    pub description: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VcpCliSkillResult {
    pub id: String,
    pub name: String,
    pub resource_path: String,
    pub skill_root: String,
    pub sha256: String,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub materialized_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VcpCliSkillSummary {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub source: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VcpCliRuntimeSource {
    VcpPlugin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VcpCliRuntimeInfo {
    pub platform: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distribution: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub libc: Option<String>,
    pub source: VcpCliRuntimeSource,
}

impl VcpCliRuntimeInfo {
    pub fn vcp_plugin_shell() -> Self {
        Self {
            platform: "android".to_string(),
            shell: Some("/bin/bash".to_string()),
            distribution: Some("alpine".to_string()),
            libc: Some("musl".to_string()),
            source: VcpCliRuntimeSource::VcpPlugin,
        }
    }

    pub fn vcp_plugin() -> Self {
        Self {
            platform: "android".to_string(),
            shell: None,
            distribution: None,
            libc: None,
            source: VcpCliRuntimeSource::VcpPlugin,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VcpCliResultBody {
    pub content: Vec<VcpCliContentPart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<VcpCliJobResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jobs: Option<Vec<VcpCliJobSummary>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill: Option<VcpCliSkillResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<VcpCliSkillSummary>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<VcpCliRuntimeInfo>,
}

impl VcpCliResultBody {
    pub fn content_only(content: Vec<VcpCliContentPart>) -> Self {
        Self {
            content,
            job: None,
            jobs: None,
            skill: None,
            skills: None,
            runtime: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum VcpCliResultEnvelope {
    Success {
        result: VcpCliResultBody,
    },
    Error {
        error: String,
        code: VcpCliErrorCode,
        result: VcpCliResultBody,
    },
}

impl VcpCliResultEnvelope {
    pub fn success(result: VcpCliResultBody) -> Self {
        Self::Success { result }
    }

    pub fn error(
        code: VcpCliErrorCode,
        summary: impl Into<String>,
        diagnostic: impl Into<String>,
    ) -> Self {
        Self::Error {
            error: summary.into(),
            code,
            result: VcpCliResultBody::content_only(vec![VcpCliContentPart::text(diagnostic)]),
        }
    }

    pub fn result(&self) -> &VcpCliResultBody {
        match self {
            Self::Success { result } | Self::Error { result, .. } => result,
        }
    }
}

fn distributed_error_text(
    code: VcpCliErrorCode,
    summary: &str,
    result: &VcpCliResultBody,
) -> String {
    let diagnostics = result
        .content
        .iter()
        .map(|part| match part {
            VcpCliContentPart::Text { text } => text.trim(),
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if diagnostics.is_empty() {
        format!("{}: {summary}", code.as_str())
    } else {
        format!("{}: {summary}\n{diagnostics}", code.as_str())
    }
}

/// Project the canonical Runtime envelope into the value/error shape expected by the existing
/// Distributed client. Success returns the body directly; an error remains an outer error rather
/// than being nested below a successful generic tool result.
pub(crate) fn project_vcp_plugin_outcome(envelope: VcpCliResultEnvelope) -> Result<Value, String> {
    match envelope {
        VcpCliResultEnvelope::Success { mut result } => {
            project_runtime_source(&mut result, VcpCliRuntimeSource::VcpPlugin);
            serde_json::to_value(result)
                .map_err(|error| format!("internal_error: cannot serialize CLI result: {error}"))
        }
        VcpCliResultEnvelope::Error {
            error,
            code,
            mut result,
        } => {
            project_runtime_source(&mut result, VcpCliRuntimeSource::VcpPlugin);
            Err(distributed_error_text(code, &error, &result))
        }
    }
}

fn project_runtime_source(result: &mut VcpCliResultBody, source: VcpCliRuntimeSource) {
    if let Some(runtime) = result.runtime.as_mut() {
        runtime.source = source;
    }
}
