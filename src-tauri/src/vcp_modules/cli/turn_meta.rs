//! Local ownership of VCP meta fields. Meta never reaches Bash argv.

use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

use crate::vcp_modules::chat::context_sanitizer::strip_thought_chains;
use crate::vcp_modules::sync_logger::redact_sync_diagnostic;

use super::protocol::{
    ValidatedVcpCliRequest, VcpArcheryMode, VcpMetaCapabilities, VcpMetaFields, VcpRiverMode,
};
use super::result::VcpCliResultEnvelope;
use super::turn_types::{MAX_MARKED_HISTORY_BYTES, MAX_RIVER_MESSAGES, MAX_RIVER_PROJECTION_BYTES};

const RIVER_SCHEMA: &str = "vcp.mobile.river-context.v1";
const TRUNCATION_MARKER: &str = "\n[context truncated]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalContinuationPolicy {
    Continue,
    Parallel,
    NoReply,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiverProjection {
    pub canonical_json: String,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalMetaPlan {
    pub mark_history: bool,
    pub river_projection: Option<RiverProjection>,
    pub continuation: LocalContinuationPolicy,
}

#[derive(Serialize)]
struct RiverDocument {
    schema: &'static str,
    messages: Vec<RiverMessage>,
    truncated: bool,
}

#[derive(Clone, Serialize)]
struct RiverMessage {
    role: String,
    content: String,
}

pub fn plan_local_meta(
    request: &ValidatedVcpCliRequest,
    messages: &[Value],
) -> Result<LocalMetaPlan, String> {
    request
        .require_meta_support(VcpMetaCapabilities::LOCAL_LOOPBACK_INITIAL)
        .map_err(|error| format!("{}: {error}", error.code.as_str()))?;

    let river_projection = request
        .meta
        .river
        .map(|mode| build_river_projection(messages, mode))
        .transpose()?;
    let continuation = match request.meta.archery {
        Some(VcpArcheryMode::Parallel) => LocalContinuationPolicy::Parallel,
        Some(VcpArcheryMode::NoReply) => LocalContinuationPolicy::NoReply,
        None => LocalContinuationPolicy::Continue,
    };
    Ok(LocalMetaPlan {
        mark_history: request.meta.ink.is_some(),
        river_projection,
        continuation,
    })
}

pub fn unsupported_meta_envelope(request: &ValidatedVcpCliRequest) -> Option<VcpCliResultEnvelope> {
    request
        .require_meta_support(VcpMetaCapabilities::LOCAL_LOOPBACK_INITIAL)
        .err()
        .map(|error| {
            VcpCliResultEnvelope::error(
                error.code,
                error.to_string(),
                "Remove the unsupported meta field or choose the vcpPlugin route.",
            )
        })
}

pub fn build_river_projection(
    messages: &[Value],
    mode: VcpRiverMode,
) -> Result<RiverProjection, String> {
    let selected_limit = match mode {
        VcpRiverMode::Text => MAX_RIVER_MESSAGES,
        VcpRiverMode::Last(limit) => usize::from(limit).min(MAX_RIVER_MESSAGES),
        VcpRiverMode::Full | VcpRiverMode::Semantic(_) => {
            return Err(format!(
                "unsupported_mode: river={} is not available on localLoopback",
                mode.as_wire_value()
            ));
        }
    };

    let mut projected = messages
        .iter()
        .rev()
        .filter_map(project_river_message)
        .take(selected_limit)
        .collect::<Vec<_>>();
    projected.reverse();

    let source_count = messages
        .iter()
        .filter(|value| project_river_message(value).is_some())
        .count();
    let mut truncated = source_count > projected.len();
    loop {
        let document = RiverDocument {
            schema: RIVER_SCHEMA,
            messages: projected.clone(),
            truncated,
        };
        let bytes = serde_json::to_vec(&document)
            .map_err(|error| format!("cannot serialize river projection: {error}"))?;
        if bytes.len() <= MAX_RIVER_PROJECTION_BYTES {
            let sha256 = format!("{:x}", Sha256::digest(&bytes));
            let canonical_json = String::from_utf8(bytes)
                .map_err(|error| format!("river projection is not UTF-8: {error}"))?;
            return Ok(RiverProjection {
                size_bytes: canonical_json.len() as u64,
                canonical_json,
                sha256,
            });
        }

        truncated = true;
        if projected.len() > 1 {
            projected.remove(0);
            continue;
        }
        let Some(message) = projected.first_mut() else {
            return Err("river projection overhead exceeds its hard limit".to_string());
        };
        let target = message
            .content
            .len()
            .saturating_sub(bytes.len() - MAX_RIVER_PROJECTION_BYTES + 64);
        if target == 0 {
            message.content.clear();
        } else {
            message.content = truncate_utf8(&message.content, target);
            message.content.push_str(TRUNCATION_MARKER);
        }
    }
}

fn project_river_message(value: &Value) -> Option<RiverMessage> {
    let object = value.as_object()?;
    let role = object.get("role")?.as_str()?.trim();
    if !matches!(role, "system" | "user" | "assistant" | "tool") {
        return None;
    }
    let content = pure_text_content(object.get("content")?)?;
    let content = redact_river_text(&strip_thought_chains(&content));
    (!content.trim().is_empty()).then(|| RiverMessage {
        role: role.to_string(),
        content,
    })
}

fn pure_text_content(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    let parts = value.as_array()?;
    let text = parts
        .iter()
        .filter_map(|part| {
            let object = part.as_object()?;
            let kind = object.get("type")?.as_str()?;
            matches!(kind, "text" | "input_text" | "output_text")
                .then(|| object.get("text").and_then(Value::as_str))
                .flatten()
        })
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
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
    use serde_json::json;

    use crate::vcp_modules::content_parser::{parse_content, ContentBlock};

    use super::*;
    use crate::vcp_modules::cli::result::{
        VcpCliContentPart, VcpCliErrorCode, VcpCliResultBody, VcpCliRuntimeInfo,
    };

    #[test]
    fn river_keeps_only_role_and_redacted_plain_text() {
        let messages = vec![json!({
            "role": "user",
            "content": [{"type":"text","text":"Authorization: Bearer abc\napi_key=secret\n-----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----"}, {"type":"image_url","image_url":{"url":"file:///secret/p.png"}}],
            "internalPath": "/data/user/0/private",
            "src": "file:///private",
            "imageFrames": ["base64"],
            "thumbnailPath": "/thumb"
        })];
        let projection =
            build_river_projection(&messages, VcpRiverMode::Text).expect("build river projection");
        assert!(projection.canonical_json.contains("[redacted]"));
        assert!(projection.canonical_json.contains("[redacted PEM]"));
        for forbidden in [
            "Bearer abc",
            "api_key=secret",
            "BEGIN PRIVATE KEY",
            "internalPath",
            "file:///private",
            "imageFrames",
            "thumbnailPath",
            "base64",
        ] {
            assert!(
                !projection.canonical_json.contains(forbidden),
                "{forbidden}"
            );
        }
        assert_eq!(
            projection.size_bytes as usize,
            projection.canonical_json.len()
        );
        assert_eq!(
            projection.sha256,
            format!("{:x}", Sha256::digest(projection.canonical_json.as_bytes()))
        );
    }

    #[test]
    fn river_last_and_hard_byte_limit_are_deterministic() {
        let messages = (0..70)
            .map(|index| json!({"role":"user", "content": format!("{index}:{}", "中".repeat(10_000))}))
            .collect::<Vec<_>>();
        let projection =
            build_river_projection(&messages, VcpRiverMode::Last(50)).expect("bounded projection");
        assert!(projection.canonical_json.len() <= MAX_RIVER_PROJECTION_BYTES);
        assert!(projection.canonical_json.contains("\"truncated\":true"));
        assert!(projection.canonical_json.contains("69:"));
        assert!(!projection.canonical_json.contains("0:"));
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
