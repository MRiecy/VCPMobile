//! Local ownership of VCP meta fields. Meta never reaches Bash argv.

use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

use crate::vcp_modules::sync_logger::redact_sync_diagnostic;

use super::protocol::{ValidatedVcpCliRequest, VcpArcheryMode, VcpMetaCapabilities, VcpMetaFields};
use super::result::VcpCliResultEnvelope;
use super::turn_types::MAX_MARKED_HISTORY_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalContinuationPolicy {
    Continue,
    Parallel,
    NoReply,
}

pub fn plan_local_policy(
    request: &ValidatedVcpCliRequest,
) -> Result<(bool, LocalContinuationPolicy), String> {
    // river/vref are protocol-compatibility hints on localLoopback. They remain part of the raw
    // request digest, but only ink/archery participate in local execution policy.
    let policy_request = ValidatedVcpCliRequest {
        action: request.action.clone(),
        meta: VcpMetaFields {
            ink: request.meta.ink,
            river: None,
            vref: None,
            archery: request.meta.archery,
        },
    };
    policy_request
        .require_meta_support(VcpMetaCapabilities::LOCAL_LOOPBACK_INITIAL)
        .map_err(|error| format!("{}: {error}", error.code.as_str()))?;
    let continuation = match request.meta.archery {
        Some(VcpArcheryMode::Parallel) => LocalContinuationPolicy::Parallel,
        Some(VcpArcheryMode::NoReply) => LocalContinuationPolicy::NoReply,
        None => LocalContinuationPolicy::Continue,
    };
    Ok((request.meta.ink.is_some(), continuation))
}

pub fn local_optional_context_notices(request: &ValidatedVcpCliRequest) -> Vec<String> {
    let mut ignored = Vec::with_capacity(2);
    if request.meta.river.is_some() {
        ignored.push("river");
    }
    if request.meta.vref.is_some() {
        ignored.push("vref");
    }
    if ignored.is_empty() {
        Vec::new()
    } else {
        vec![format!(
            "提示：本地回环未应用可选 VCP 上下文（{}）；命令已继续执行。",
            ignored.join("、")
        )]
    }
}

fn redact_river_text(value: &str) -> String {
    static AUTHORIZATION: OnceLock<Regex> = OnceLock::new();
    static PEM: OnceLock<Regex> = OnceLock::new();
    let authorization = AUTHORIZATION.get_or_init(|| {
        Regex::new(r"(?i)(\bAuthorization\s*:\s*)[^\r\n]+")
            .expect("static authorization redaction regex must compile")
    });
    let pem = PEM.get_or_init(|| {
        Regex::new(
            r"(?s)-----BEGIN [A-Z0-9 ]*(?:PRIVATE KEY|CERTIFICATE)-----.*?-----END [A-Z0-9 ]*(?:PRIVATE KEY|CERTIFICATE)-----",
        )
        .expect("static PEM redaction regex must compile")
    });
    let redacted = redact_sync_diagnostic(value);
    let redacted = authorization.replace_all(&redacted, "${1}[redacted]");
    pem.replace_all(&redacted, "[redacted PEM]").into_owned()
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

pub fn marked_history_block(operation_id: &str, result: &VcpCliResultEnvelope) -> String {
    marked_history_block_with_projection(operation_id, result, None)
}

pub fn marked_history_block_with_projection(
    operation_id: &str,
    result: &VcpCliResultEnvelope,
    projection: Option<&super::turn_types::DurableRiverProjection>,
) -> String {
    let (status, code) = match result {
        VcpCliResultEnvelope::Success { .. } => ("success", None),
        VcpCliResultEnvelope::Error { code, .. } => ("error", Some(code.as_str())),
    };
    let body = result.result();
    let mut details = vec![
        format!("- 工具名称: VCPMobileCLI"),
        format!("- 执行状态: {status}"),
        format!("- 操作ID: {}", redact_river_text(operation_id)),
    ];
    if let Some(code) = code {
        details.push(format!("- 错误码: {code}"));
    }
    details.extend(projection_summary_lines(projection));
    if let Some(job) = &body.job {
        details.push(format!("- Job ID: {}", redact_river_text(&job.id)));
        details.push(format!("- Job状态: {:?}", job.state).to_ascii_lowercase());
        if let Some(exit_code) = job.exit_code {
            details.push(format!("- 退出码: {exit_code}"));
        }
        details.push("- 输出: 请通过该 Job 的 poll/cursor 有界读取".to_string());
    } else if let Some(skill) = &body.skill {
        details.push(format!("- Skill: {}", redact_river_text(&skill.id)));
        details.push(format!("- SHA256: {}", skill.sha256));
    } else if let Some(jobs) = &body.jobs {
        details.push(format!("- Job数量: {}", jobs.len()));
    } else if let Some(skills) = &body.skills {
        details.push(format!("- Skill数量: {}", skills.len()));
    } else if !body.content.is_empty() {
        // `content` can be stdout/stderr. mark_history is a bounded display projection, not an
        // alternate artifact channel, so never copy arbitrary tool content into chat history.
        details.push("- 摘要: 结构化结果已记录；完整输出请使用 Job cursor 读取".to_string());
    }
    let mut block = format!(
        "[[VCP调用结果信息汇总:\n{}\nVCP调用结果结束]]",
        details.join("\n")
    );
    if block.len() > MAX_MARKED_HISTORY_BYTES {
        block = truncate_utf8(&block, MAX_MARKED_HISTORY_BYTES.saturating_sub(32));
        block.push_str("\nVCP调用结果结束]]");
    }
    block
}

fn projection_summary_lines(
    projection: Option<&super::turn_types::DurableRiverProjection>,
) -> Vec<String> {
    let Some(river) = projection
        .and_then(|projection| serde_json::from_str::<Value>(&projection.canonical_json).ok())
        .and_then(|document| document.get("river").cloned())
    else {
        return Vec::new();
    };
    let Some(mode) = river.get("mode").and_then(Value::as_str) else {
        return Vec::new();
    };
    if let Some(limit) = mode
        .strip_prefix("semantic:")
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|limit| (1..=50).contains(limit))
    {
        if river.get("resolved_mode").and_then(Value::as_str) == Some("fallback_last") {
            return vec![
                format!("- 上下文选择: semantic:{limit} → last:{limit}"),
                "- 回退原因: 本地语义召回暂不可用".to_string(),
            ];
        }
        return vec![format!("- 上下文选择: semantic:{limit}")];
    }
    let safe_mode = mode == "text"
        || mode == "full"
        || mode
            .strip_prefix("last:")
            .and_then(|value| value.parse::<u8>().ok())
            .is_some_and(|limit| (1..=50).contains(&limit));
    if safe_mode {
        vec![format!("- 上下文选择: {mode}")]
    } else {
        Vec::new()
    }
}

pub fn append_marked_history(final_content: &str, blocks: &[String]) -> String {
    if blocks.is_empty() {
        return final_content.to_string();
    }
    let mut output = blocks.join("\n\n");
    if !final_content.is_empty() {
        output.push_str("\n\n");
        output.push_str(final_content);
    }
    output
}

pub fn meta_fields_digest(meta: &VcpMetaFields) -> Result<String, String> {
    let bytes = serde_json::to_vec(meta)
        .map_err(|error| format!("cannot serialize VCP meta fields: {error}"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use crate::vcp_modules::content_parser::{parse_content, ContentBlock};

    use super::*;
    use crate::vcp_modules::cli::result::{
        VcpCliContentPart, VcpCliErrorCode, VcpCliResultBody, VcpCliRuntimeInfo,
    };

    fn run_request(meta: VcpMetaFields) -> ValidatedVcpCliRequest {
        ValidatedVcpCliRequest {
            action: super::super::protocol::VcpCliAction::Run {
                command: "printf ok".to_string(),
                description: None,
                cwd: Some("/workspace".to_string()),
                timeout_ms: Some(1_000),
                run_in_background: Some(false),
            },
            meta,
        }
    }

    #[test]
    fn optional_context_is_ignored_without_blocking_local_policy() {
        let request = run_request(VcpMetaFields {
            river: Some(super::super::protocol::VcpRiverMode::Semantic(2)),
            vref: Some(3),
            ..VcpMetaFields::default()
        });
        let (mark_history, continuation) =
            plan_local_policy(&request).expect("optional context does not own local execution");
        assert!(!mark_history);
        assert_eq!(continuation, LocalContinuationPolicy::Continue);
        assert_eq!(
            local_optional_context_notices(&request),
            vec!["提示：本地回环未应用可选 VCP 上下文（river、vref）；命令已继续执行。"]
        );
    }

    #[test]
    fn optional_context_notice_is_absent_without_river_or_vref() {
        assert!(local_optional_context_notices(&run_request(VcpMetaFields::default())).is_empty());
    }

    #[test]
    fn legacy_projection_summary_remains_decode_compatible() {
        let json = r#"{"schema":"vcp.mobile.attempt-projection.v1","river":{"mode":"semantic:2","resolved_mode":"fallback_last"}}"#;
        let durable = super::super::turn_types::DurableRiverProjection {
            canonical_json: json.to_string(),
            sha256: format!("{:x}", Sha256::digest(json.as_bytes())),
            size_bytes: json.len() as u64,
            artifacts: Vec::new(),
        };
        let visible = marked_history_block_with_projection(
            "operation",
            &VcpCliResultEnvelope::error(
                VcpCliErrorCode::RuntimeUnavailable,
                "hidden internal failure",
                "retry",
            ),
            Some(&durable),
        );
        assert!(visible.contains("上下文选择: semantic:2 → last:2"));
        assert!(!visible.contains("hidden internal failure"));
    }

    #[test]
    fn marked_history_is_existing_parser_compatible_and_excludes_stdout() {
        let result = VcpCliResultEnvelope::success(VcpCliResultBody {
            content: vec![VcpCliContentPart::text("very secret full stdout")],
            job: None,
            jobs: None,
            skill: None,
            skills: None,
            runtime: Some(VcpCliRuntimeInfo::local_loopback()),
        });
        let block = marked_history_block("operation-1", &result);
        let parsed = parse_content(&append_marked_history(
            "最终答复",
            std::slice::from_ref(&block),
        ));
        assert!(parsed
            .iter()
            .any(|item| matches!(item, ContentBlock::ToolResult { .. })));
        assert!(block.contains("VCP调用结果信息汇总"));
        assert!(!block.contains("very secret full stdout"));

        let job_result = VcpCliResultEnvelope::error(
            VcpCliErrorCode::RuntimeUnavailable,
            "failed",
            "diagnostic",
        );
        assert!(marked_history_block("op", &job_result).contains("runtime_unavailable"));
    }
}
