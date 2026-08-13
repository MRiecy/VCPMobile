use serde::{Deserialize, Serialize};

pub const VCP_TOOL_PAYLOAD_MARKER: &str = "<!-- VCP_TOOL_PAYLOAD -->";

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
    Running,
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
    pub state: VcpCliJobState,
    pub description: Option<String>,
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
    LocalLoopback,
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
    pub fn local_loopback_shell() -> Self {
        Self {
            platform: "android".to_string(),
            shell: Some("/bin/bash".to_string()),
            distribution: Some("alpine".to_string()),
            libc: Some("musl".to_string()),
            source: VcpCliRuntimeSource::LocalLoopback,
        }
    }

    pub fn local_loopback() -> Self {
        Self {
            platform: "android".to_string(),
            shell: None,
            distribution: None,
            libc: None,
            source: VcpCliRuntimeSource::LocalLoopback,
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

    fn to_distributed(&self, request_id: &str) -> VcpDistributedToolResult {
        let data = match self {
            Self::Success { result } => VcpDistributedToolResultData {
                request_id: request_id.to_string(),
                status: VcpDistributedToolResultStatus::Success,
                result: Some(result.clone()),
                error: None,
            },
            Self::Error {
                error,
                code,
                result,
            } => VcpDistributedToolResultData {
                request_id: request_id.to_string(),
                status: VcpDistributedToolResultStatus::Error,
                result: None,
                // 当前 Distributed wire 只有 string error；稳定前缀保留规范 code，
                // 并携带规范 content，避免远端 route 丢失 Agent 可修正诊断。
                error: Some(distributed_error_text(*code, error, result)),
            },
        };

        VcpDistributedToolResult {
            message_type: VcpDistributedMessageType::ToolResult,
            data,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum VcpDistributedMessageType {
    ToolResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum VcpDistributedToolResultStatus {
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VcpDistributedToolResult {
    #[serde(rename = "type")]
    message_type: VcpDistributedMessageType,
    data: VcpDistributedToolResultData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct VcpDistributedToolResultData {
    #[serde(rename = "requestId")]
    request_id: String,
    status: VcpDistributedToolResultStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<VcpCliResultBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// 生成本地续轮 user message 的正文。调用方仍须保留上一轮 assistant 原始请求。
pub fn serialize_local_model_payload(
    result: &VcpCliResultEnvelope,
) -> Result<String, serde_json::Error> {
    let content = serde_json::to_string(&result.result().content)?;
    Ok(format!("{VCP_TOOL_PAYLOAD_MARKER}\n{content}"))
}

/// 将同一规范 Runtime 结果投影为现有 Distributed `tool_result` wire。
/// 错误 wire 仅允许 string error，因此编码为 `<code>: <summary>\n<diagnostic>`。
pub fn serialize_distributed_tool_result(
    request_id: &str,
    result: &VcpCliResultEnvelope,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(&result.to_distributed(request_id))
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde_json::Value;

    use super::*;

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ResultGolden {
        success_envelope: Value,
        success_local_payload: String,
        success_distributed: Value,
        skill_envelope: Value,
        error_envelope: Value,
        error_local_payload: String,
        error_distributed: Value,
    }

    #[test]
    fn canonical_result_projects_to_local_and_distributed_golden() {
        let fixture: ResultGolden =
            serde_json::from_str(include_str!("fixtures/vcp_cli_result.golden.json"))
                .expect("parse result golden fixture");

        let success = job_success_fixture();
        assert_eq!(
            serde_json::to_value(&success).expect("serialize success envelope"),
            fixture.success_envelope
        );
        assert_eq!(
            serialize_local_model_payload(&success).expect("serialize local payload"),
            fixture.success_local_payload
        );
        let distributed: Value = serde_json::from_str(
            &serialize_distributed_tool_result("request_01", &success)
                .expect("serialize distributed success"),
        )
        .expect("parse distributed success");
        assert_eq!(distributed, fixture.success_distributed);

        let skill = skill_success_fixture();
        assert_eq!(
            serde_json::to_value(skill).expect("serialize skill envelope"),
            fixture.skill_envelope
        );
    }

    #[test]
    fn canonical_error_remains_agent_readable_on_both_routes() {
        let fixture: ResultGolden =
            serde_json::from_str(include_str!("fixtures/vcp_cli_result.golden.json"))
                .expect("parse result golden fixture");
        let error = VcpCliResultEnvelope::error(
            VcpCliErrorCode::UnsupportedMode,
            "river=full is unavailable",
            "Use river=text or remove river and retry.",
        );

        assert_eq!(
            serde_json::to_value(&error).expect("serialize error envelope"),
            fixture.error_envelope
        );
        assert_eq!(
            serialize_local_model_payload(&error).expect("serialize local error payload"),
            fixture.error_local_payload
        );
        let distributed: Value = serde_json::from_str(
            &serialize_distributed_tool_result("request_02", &error)
                .expect("serialize distributed error"),
        )
        .expect("parse distributed error");
        assert_eq!(distributed, fixture.error_distributed);
    }

    fn job_success_fixture() -> VcpCliResultEnvelope {
        VcpCliResultEnvelope::success(VcpCliResultBody {
            content: vec![VcpCliContentPart::text("ok\n")],
            job: Some(VcpCliJobResult {
                id: "job_01".to_string(),
                state: VcpCliJobState::Completed,
                stdout: "ok\n".to_string(),
                stderr: String::new(),
                exit_code: Some(0),
                cursor: Some("c_3".to_string()),
                truncated: false,
                artifact: None,
                reason: None,
            }),
            jobs: None,
            skill: None,
            skills: None,
            runtime: Some(VcpCliRuntimeInfo::local_loopback_shell()),
        })
    }

    fn skill_success_fixture() -> VcpCliResultEnvelope {
        VcpCliResultEnvelope::success(VcpCliResultBody {
            content: vec![VcpCliContentPart::text("# Skill instructions")],
            job: None,
            jobs: None,
            skill: Some(VcpCliSkillResult {
                id: "example-skill".to_string(),
                name: "Example Skill".to_string(),
                resource_path: "SKILL.md".to_string(),
                skill_root: "/skills/example-skill".to_string(),
                sha256: "abc123".to_string(),
                truncated: false,
            }),
            skills: None,
            runtime: Some(VcpCliRuntimeInfo::local_loopback()),
        })
    }
}
