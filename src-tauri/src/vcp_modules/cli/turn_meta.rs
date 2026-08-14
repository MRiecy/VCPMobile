//! Local ownership of VCP meta fields. Meta never reaches Bash argv.

use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::vcp_modules::chat::context_sanitizer::strip_thought_chains;
use crate::vcp_modules::file_manager::AttachmentCasFile;
use crate::vcp_modules::sync_logger::redact_sync_diagnostic;

use super::protocol::{
    ValidatedVcpCliRequest, VcpArcheryMode, VcpMetaCapabilities, VcpMetaFields, VcpRiverMode,
};
use super::result::VcpCliResultEnvelope;
use super::semantic::{MAX_SEMANTIC_CANDIDATES, MAX_SEMANTIC_CANDIDATE_BYTES};
use super::turn_types::{
    MAX_MARKED_HISTORY_BYTES, MAX_RIVER_ARTIFACTS, MAX_RIVER_ARTIFACT_BYTES,
    MAX_RIVER_ARTIFACT_TOTAL_BYTES, MAX_RIVER_ATTACHMENT_DESCRIPTORS, MAX_RIVER_MESSAGES,
    MAX_RIVER_PROJECTION_BYTES,
};

const ATTEMPT_PROJECTION_SCHEMA: &str = "vcp.mobile.attempt-projection.v1";
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
    pub artifact_grants: Vec<ArtifactGrantV1>,
}

/// Host-only source grant. `source_path` is deliberately absent from the serialized bundle and
/// from every public Tauri command. Runtime copies this source into an attempt-owned snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactGrantV1 {
    pub source_path: PathBuf,
    pub file_name: String,
    pub guest_path: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalMetaPlan {
    pub mark_history: bool,
    pub river_projection: Option<RiverProjection>,
    pub continuation: LocalContinuationPolicy,
    pub optional_context_notices: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticProjectionPlan {
    pub canonical_json: String,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Serialize)]
struct AttemptProjectionBundleV1 {
    schema: &'static str,
    river: RiverDocumentV1,
    artifacts: Vec<ArtifactDescriptorV1>,
    omissions: Vec<ProjectionOmissionV1>,
}

#[derive(Serialize)]
struct RiverDocumentV1 {
    mode: String,
    messages: Vec<RiverMessage>,
    truncated: bool,
}

#[derive(Clone, Serialize)]
struct RiverMessage {
    source_index: usize,
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    artifact_ids: Vec<String>,
}

#[derive(Clone)]
struct ProjectedRiverMessage {
    message: RiverMessage,
    attachments: Vec<LocalAttachmentDescriptor>,
}

#[derive(Clone)]
struct LocalAttachmentDescriptor {
    name: String,
    declared_size_bytes: u64,
    sha256: Option<String>,
    availability: String,
}

#[derive(Serialize)]
struct ArtifactDescriptorV1 {
    id: String,
    name: String,
    mime_type: String,
    size_bytes: u64,
    sha256: String,
    guest_path: String,
    source_unreachable: bool,
    non_writeback: bool,
}

#[derive(Serialize)]
struct ProjectionOmissionV1 {
    message_index: usize,
    attachment_name: String,
    reason: &'static str,
}

pub fn plan_local_meta(
    request: &ValidatedVcpCliRequest,
    messages: &[Value],
    resolved_artifacts: &HashMap<String, AttachmentCasFile>,
    semantic_projection: Option<SemanticProjectionPlan>,
) -> Result<LocalMetaPlan, String> {
    let (mark_history, continuation) = plan_local_policy(request)?;

    let river_projection = match request.meta.river {
        Some(VcpRiverMode::Semantic(_)) => semantic_projection.map(|projection| RiverProjection {
            canonical_json: projection.canonical_json,
            sha256: projection.sha256,
            size_bytes: projection.size_bytes,
            artifact_grants: Vec::new(),
        }),
        Some(mode) => {
            match build_river_projection_with_artifacts(messages, mode, resolved_artifacts) {
                Ok(projection) => Some(projection),
                Err(error) => {
                    log::warn!(
                    "[VCPMobileCLI] optional river projection unavailable; continuing without it: {error}"
                );
                    None
                }
            }
        }
        None => None,
    };
    let optional_context_notices = local_optional_context_notices(
        request,
        river_projection
            .as_ref()
            .map(|projection| projection.canonical_json.as_str()),
    );
    Ok(LocalMetaPlan {
        mark_history,
        river_projection,
        continuation,
        optional_context_notices,
    })
}

pub fn plan_local_policy(
    request: &ValidatedVcpCliRequest,
) -> Result<(bool, LocalContinuationPolicy), String> {
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

pub fn local_optional_context_notices(
    request: &ValidatedVcpCliRequest,
    river_projection_json: Option<&str>,
) -> Vec<String> {
    let mut notices = Vec::new();
    if request.meta.vref.is_some() {
        notices.push("提示：本次未能应用 vref；命令已在无知识召回上下文下继续执行。".to_string());
    }
    if request.meta.river.is_some() && river_projection_json.is_none() {
        notices.push("提示：本次未能应用 river；命令已在无附加上下文下继续执行。".to_string());
    } else if matches!(request.meta.river, Some(VcpRiverMode::Semantic(_)))
        && river_projection_json.is_some_and(|json| {
            serde_json::from_str::<Value>(json)
                .ok()
                .and_then(|value| {
                    value
                        .pointer("/river/resolved_mode")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .as_deref()
                == Some("fallback_last")
        })
    {
        notices.push("提示：本次 river 语义召回不可用，已回退为最近上下文后继续执行。".to_string());
    }
    notices
}

pub fn semantic_candidates(
    messages: &[Value],
) -> Result<Vec<super::semantic::SemanticCandidate>, String> {
    let candidates = messages
        .iter()
        .enumerate()
        .filter_map(|(source_index, value)| {
            let projected = project_river_message(source_index, value, false)?;
            let (content_sha256, text) =
                super::semantic::semantic_content(&projected.message.content)?;
            Some(super::semantic::SemanticCandidate {
                source_index,
                content_sha256,
                text,
            })
        })
        .collect::<Vec<_>>();
    let total_bytes = candidates
        .iter()
        .try_fold(0_usize, |total, candidate| {
            total.checked_add(candidate.text.len())
        })
        .ok_or_else(|| "semantic candidate byte count overflowed".to_string())?;
    if candidates.len() > MAX_SEMANTIC_CANDIDATES || total_bytes > MAX_SEMANTIC_CANDIDATE_BYTES {
        return Err("semantic candidate set exceeds its bounded local budget".to_string());
    }
    Ok(candidates)
}

pub fn fallback_last_semantic_selection(
    messages: &[Value],
    limit: u8,
    model_id: String,
) -> super::semantic::SemanticSelection {
    let mut source_indices = messages
        .iter()
        .enumerate()
        .rev()
        .filter_map(|(index, value)| project_river_message(index, value, false).map(|_| index))
        .take(usize::from(limit).min(MAX_RIVER_MESSAGES))
        .collect::<Vec<_>>();
    source_indices.reverse();
    super::semantic::SemanticSelection {
        source_indices,
        model_id,
    }
}

pub fn build_semantic_projection(
    messages: &[Value],
    selection: &super::semantic::SemanticSelection,
    requested_limit: u8,
    fallback_reason: Option<&str>,
) -> Result<SemanticProjectionPlan, String> {
    let selected = selection
        .source_indices
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let mut projected = messages
        .iter()
        .enumerate()
        .filter(|(index, _)| selected.contains(index))
        .filter_map(|(index, value)| project_river_message(index, value, false))
        .collect::<Vec<_>>();
    let resolved_mode = if fallback_reason.is_some() {
        "fallback_last"
    } else {
        "semantic"
    };
    let mut truncated = false;
    loop {
        let document = serde_json::json!({
            "schema": ATTEMPT_PROJECTION_SCHEMA,
            "river": {
                "mode": format!("semantic:{requested_limit}"),
                "resolved_mode": resolved_mode,
                "model_id": selection.model_id,
                "fallback_reason": fallback_reason,
                "messages": projected.iter().map(|entry| &entry.message).collect::<Vec<_>>(),
                "truncated": truncated
            },
            "artifacts": [],
            "omissions": []
        });
        let bytes = serde_json::to_vec(&document)
            .map_err(|error| format!("cannot serialize semantic projection: {error}"))?;
        if bytes.len() <= MAX_RIVER_PROJECTION_BYTES {
            return Ok(SemanticProjectionPlan {
                canonical_json: String::from_utf8(bytes.clone())
                    .map_err(|error| format!("semantic projection is not UTF-8: {error}"))?,
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                size_bytes: bytes.len() as u64,
            });
        }
        truncated = true;
        if projected.is_empty() {
            return Err("semantic projection overhead exceeds its hard limit".to_string());
        }
        projected.remove(0);
    }
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
    build_river_projection_with_artifacts(messages, mode, &HashMap::new())
}

fn build_river_projection_with_artifacts(
    messages: &[Value],
    mode: VcpRiverMode,
    resolved_artifacts: &HashMap<String, AttachmentCasFile>,
) -> Result<RiverProjection, String> {
    let selected_limit = match mode {
        VcpRiverMode::Text => MAX_RIVER_MESSAGES,
        VcpRiverMode::Last(limit) => usize::from(limit).min(MAX_RIVER_MESSAGES),
        VcpRiverMode::Full => MAX_RIVER_MESSAGES,
        VcpRiverMode::Semantic(_) => {
            return Err(format!(
                "unsupported_mode: river={} is not available on localLoopback",
                mode.as_wire_value()
            ));
        }
    };

    let include_artifacts = mode == VcpRiverMode::Full;
    let mut projected = messages
        .iter()
        .enumerate()
        .rev()
        .filter_map(|(index, value)| project_river_message(index, value, include_artifacts))
        .take(selected_limit)
        .collect::<Vec<_>>();
    projected.reverse();

    let source_count = messages
        .iter()
        .enumerate()
        .filter(|(index, value)| project_river_message(*index, value, include_artifacts).is_some())
        .count();
    let mut truncated = source_count > projected.len();
    loop {
        let (document, artifact_grants) =
            assemble_projection_bundle(&projected, mode, truncated, resolved_artifacts);
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
                artifact_grants,
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
            .message
            .content
            .len()
            .saturating_sub(bytes.len() - MAX_RIVER_PROJECTION_BYTES + 64);
        if target == 0 {
            if message.message.content.is_empty() {
                projected.remove(0);
            } else {
                message.message.content.clear();
            }
        } else {
            message.message.content = truncate_utf8(&message.message.content, target);
            message.message.content.push_str(TRUNCATION_MARKER);
        }
    }
}

pub fn river_full_candidate_hashes(messages: &[Value]) -> Vec<String> {
    let mut seen = HashSet::new();
    selected_full_messages(messages)
        .into_iter()
        .flat_map(|message| message.attachments)
        .take(MAX_RIVER_ATTACHMENT_DESCRIPTORS)
        .filter(|attachment| matches!(attachment.availability.as_str(), "ready" | "done"))
        .filter_map(|attachment| attachment.sha256)
        .filter(|hash| crate::vcp_modules::infra::utils::is_valid_cas_hash(hash))
        .filter(|hash| seen.insert(hash.clone()))
        .collect()
}

pub fn selected_river_full_artifact_hashes(
    messages: &[Value],
    resolved_artifacts: &HashMap<String, AttachmentCasFile>,
) -> Vec<String> {
    let mut selected = Vec::new();
    let mut selected_set = HashSet::new();
    let mut total_bytes = 0_u64;
    for attachment in selected_full_messages(messages)
        .into_iter()
        .flat_map(|message| message.attachments)
        .take(MAX_RIVER_ATTACHMENT_DESCRIPTORS)
    {
        if !matches!(attachment.availability.as_str(), "ready" | "done") {
            continue;
        }
        let Some(hash) = attachment.sha256.as_ref() else {
            continue;
        };
        let Some(record) = resolved_artifacts.get(hash) else {
            continue;
        };
        if record.size_bytes != attachment.declared_size_bytes
            || record.size_bytes > MAX_RIVER_ARTIFACT_BYTES
            || selected_set.contains(hash)
            || selected.len() >= MAX_RIVER_ARTIFACTS
            || total_bytes.saturating_add(record.size_bytes) > MAX_RIVER_ARTIFACT_TOTAL_BYTES
        {
            continue;
        }
        total_bytes = total_bytes.saturating_add(record.size_bytes);
        selected_set.insert(hash.clone());
        selected.push(hash.clone());
    }
    selected
}

fn selected_full_messages(messages: &[Value]) -> Vec<ProjectedRiverMessage> {
    let mut projected = messages
        .iter()
        .enumerate()
        .rev()
        .filter_map(|(index, value)| project_river_message(index, value, true))
        .take(MAX_RIVER_MESSAGES)
        .collect::<Vec<_>>();
    projected.reverse();
    projected
}

fn project_river_message(
    source_index: usize,
    value: &Value,
    include_artifacts: bool,
) -> Option<ProjectedRiverMessage> {
    let object = value.as_object()?;
    let role = object.get("role")?.as_str()?.trim();
    if !matches!(role, "system" | "user" | "assistant" | "tool") {
        return None;
    }
    let content = object
        .get("content")
        .and_then(pure_text_content)
        .map(|content| redact_river_text(&strip_thought_chains(&content)))
        .unwrap_or_default();
    let attachments = if include_artifacts {
        local_attachment_descriptors(object.get("__vcpLocalAttachments"))
    } else {
        Vec::new()
    };
    (!content.trim().is_empty() || !attachments.is_empty()).then(|| ProjectedRiverMessage {
        message: RiverMessage {
            source_index,
            role: role.to_string(),
            content,
            artifact_ids: Vec::new(),
        },
        attachments,
    })
}

fn local_attachment_descriptors(value: Option<&Value>) -> Vec<LocalAttachmentDescriptor> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let object = value.as_object()?;
            Some(LocalAttachmentDescriptor {
                name: bounded_attachment_name(
                    object.get("name").and_then(Value::as_str).unwrap_or("附件"),
                ),
                declared_size_bytes: object
                    .get("size_bytes")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                sha256: object
                    .get("sha256")
                    .and_then(Value::as_str)
                    .map(str::to_ascii_lowercase),
                availability: object
                    .get("availability")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_ascii_lowercase(),
            })
        })
        .collect()
}

fn assemble_projection_bundle(
    projected: &[ProjectedRiverMessage],
    mode: VcpRiverMode,
    truncated: bool,
    resolved_artifacts: &HashMap<String, AttachmentCasFile>,
) -> (AttemptProjectionBundleV1, Vec<ArtifactGrantV1>) {
    let mut messages = projected
        .iter()
        .map(|projected| projected.message.clone())
        .collect::<Vec<_>>();
    let mut artifacts = Vec::new();
    let mut grants = Vec::new();
    let mut omissions = Vec::new();
    let mut included = HashMap::<String, String>::new();
    let mut total_bytes = 0_u64;
    let mut descriptor_count = 0_usize;
    let mut descriptor_limit_reported = false;

    if mode == VcpRiverMode::Full {
        for (projected_index, projected_message) in projected.iter().enumerate() {
            for attachment in &projected_message.attachments {
                if descriptor_count >= MAX_RIVER_ATTACHMENT_DESCRIPTORS {
                    if !descriptor_limit_reported {
                        omissions.push(ProjectionOmissionV1 {
                            message_index: projected_message.message.source_index,
                            attachment_name: "additional attachments".to_string(),
                            reason: "attachment_descriptor_limit",
                        });
                        descriptor_limit_reported = true;
                    }
                    continue;
                }
                descriptor_count += 1;
                let omission = |reason| ProjectionOmissionV1 {
                    message_index: projected_message.message.source_index,
                    attachment_name: attachment.name.clone(),
                    reason,
                };
                if !matches!(attachment.availability.as_str(), "ready" | "done") {
                    omissions.push(omission("source_unavailable"));
                    continue;
                }
                let Some(hash) = attachment
                    .sha256
                    .as_ref()
                    .filter(|hash| crate::vcp_modules::infra::utils::is_valid_cas_hash(hash))
                else {
                    omissions.push(omission("missing_integrity_metadata"));
                    continue;
                };
                if let Some(id) = included.get(hash) {
                    messages[projected_index].artifact_ids.push(id.clone());
                    continue;
                }
                let Some(record) = resolved_artifacts.get(hash) else {
                    omissions.push(omission("local_cas_unavailable"));
                    continue;
                };
                if record.sha256 != *hash || record.size_bytes != attachment.declared_size_bytes {
                    omissions.push(omission("integrity_metadata_mismatch"));
                    continue;
                }
                if record.size_bytes > MAX_RIVER_ARTIFACT_BYTES {
                    omissions.push(omission("artifact_too_large"));
                    continue;
                }
                if artifacts.len() >= MAX_RIVER_ARTIFACTS {
                    omissions.push(omission("artifact_count_limit"));
                    continue;
                }
                if total_bytes.saturating_add(record.size_bytes) > MAX_RIVER_ARTIFACT_TOTAL_BYTES {
                    omissions.push(omission("artifact_total_limit"));
                    continue;
                }

                let ordinal = artifacts.len();
                let id = format!("river-artifact-{ordinal:02}-{}", &hash[..12]);
                let extension = safe_artifact_extension(&record.mime_type);
                let file_name = format!("{id}.{extension}");
                let guest_path = format!("/run/{file_name}");
                total_bytes = total_bytes.saturating_add(record.size_bytes);
                included.insert(hash.clone(), id.clone());
                messages[projected_index].artifact_ids.push(id.clone());
                artifacts.push(ArtifactDescriptorV1 {
                    id,
                    name: attachment.name.clone(),
                    mime_type: record.mime_type.clone(),
                    size_bytes: record.size_bytes,
                    sha256: hash.clone(),
                    guest_path: guest_path.clone(),
                    source_unreachable: true,
                    non_writeback: true,
                });
                grants.push(ArtifactGrantV1 {
                    source_path: record.path.clone(),
                    file_name,
                    guest_path,
                    size_bytes: record.size_bytes,
                    sha256: hash.clone(),
                });
            }
        }
    }

    (
        AttemptProjectionBundleV1 {
            schema: ATTEMPT_PROJECTION_SCHEMA,
            river: RiverDocumentV1 {
                mode: mode.as_wire_value(),
                messages,
                truncated,
            },
            artifacts,
            omissions,
        },
        grants,
    )
}

fn bounded_attachment_name(value: &str) -> String {
    let basename = value.rsplit(['/', '\\']).next().unwrap_or_default();
    let filtered = basename
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    let trimmed = filtered.trim();
    if trimmed.is_empty() {
        "附件".to_string()
    } else {
        truncate_utf8(trimmed, 256)
    }
}

fn safe_artifact_extension(mime: &str) -> &'static str {
    match mime.to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/heic" | "image/heif" => "heic",
        "image/avif" => "avif",
        "audio/mpeg" => "mp3",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/ogg" => "ogg",
        "audio/flac" => "flac",
        "audio/aac" => "aac",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        _ => "bin",
    }
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
    use std::path::PathBuf;

    use serde_json::json;

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
    fn unavailable_optional_context_does_not_block_local_command_planning() {
        let vref = run_request(VcpMetaFields {
            vref: Some(3),
            ..VcpMetaFields::default()
        });
        let plan = plan_local_meta(&vref, &[], &HashMap::new(), None)
            .expect("well-formed vref is best-effort on local loopback");
        assert!(plan.river_projection.is_none());
        assert_eq!(
            plan.optional_context_notices,
            vec!["提示：本次未能应用 vref；命令已在无知识召回上下文下继续执行。"]
        );

        let river = run_request(VcpMetaFields {
            river: Some(VcpRiverMode::Semantic(2)),
            ..VcpMetaFields::default()
        });
        let plan = plan_local_meta(&river, &[], &HashMap::new(), None)
            .expect("unavailable semantic projection is best-effort");
        assert!(plan.river_projection.is_none());
        assert_eq!(
            plan.optional_context_notices,
            vec!["提示：本次未能应用 river；命令已在无附加上下文下继续执行。"]
        );
    }

    #[test]
    fn semantic_last_fallback_is_visible_to_agent() {
        let request = run_request(VcpMetaFields {
            river: Some(VcpRiverMode::Semantic(2)),
            ..VcpMetaFields::default()
        });
        let json = r#"{"schema":"vcp.mobile.attempt-projection.v1","river":{"mode":"semantic:2","resolved_mode":"fallback_last"}}"#;
        assert_eq!(
            local_optional_context_notices(&request, Some(json)),
            vec!["提示：本次 river 语义召回不可用，已回退为最近上下文后继续执行。"]
        );
    }

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
    fn semantic_projection_preserves_selected_source_order_and_marks_fallback() {
        let messages = vec![
            json!({"role":"user", "content":"first eligible semantic message"}),
            json!({"role":"assistant", "content":"second eligible semantic message"}),
            json!({"role":"user", "content":"third eligible semantic message"}),
        ];
        let candidates = semantic_candidates(&messages).expect("bounded candidates");
        assert_eq!(candidates.len(), 3);
        let selection = super::super::semantic::SemanticSelection {
            source_indices: vec![0, 2],
            model_id: "model-r2".to_string(),
        };
        let projection =
            build_semantic_projection(&messages, &selection, 2, None).expect("semantic projection");
        let parsed: Value =
            serde_json::from_str(&projection.canonical_json).expect("projection JSON");
        assert_eq!(parsed["river"]["resolved_mode"], "semantic");
        assert_eq!(parsed["river"]["messages"][0]["source_index"], 0);
        assert_eq!(parsed["river"]["messages"][1]["source_index"], 2);
        assert!(parsed["river"]["fallback_reason"].is_null());

        let fallback = fallback_last_semantic_selection(&messages, 2, "model-r2".to_string());
        let projection =
            build_semantic_projection(&messages, &fallback, 2, Some("semantic_unavailable"))
                .expect("fallback projection");
        let parsed: Value =
            serde_json::from_str(&projection.canonical_json).expect("fallback JSON");
        assert_eq!(parsed["river"]["resolved_mode"], "fallback_last");
        assert_eq!(parsed["river"]["fallback_reason"], "semantic_unavailable");
        assert_eq!(parsed["river"]["messages"][0]["source_index"], 1);
        assert_eq!(parsed["river"]["messages"][1]["source_index"], 2);
        let durable = super::super::turn_types::DurableRiverProjection {
            canonical_json: projection.canonical_json,
            sha256: projection.sha256,
            size_bytes: projection.size_bytes,
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
        assert!(visible.contains("回退原因: 本地语义召回暂不可用"));
        assert!(!visible.contains("hidden internal failure"));
    }

    #[test]
    fn river_full_serializes_only_attempt_descriptors_and_explicit_omissions() {
        let good_hash = "a".repeat(64);
        let oversized_hash = "b".repeat(64);
        let missing_hash = "c".repeat(64);
        let unavailable_hash = "d".repeat(64);
        let messages = vec![json!({
            "role": "user",
            "content": [{"type":"text", "text":"inspect the attachments"}],
            "__vcpLocalAttachments": [
                {"name":"/private/photo.png", "mime":"image/png", "size_bytes":5, "sha256":good_hash, "availability":"ready"},
                {"name":"duplicate.png", "mime":"image/png", "size_bytes":5, "sha256":good_hash, "availability":"done"},
                {"name":"large.bin", "mime":"application/octet-stream", "size_bytes":MAX_RIVER_ARTIFACT_BYTES + 1, "sha256":oversized_hash, "availability":"ready"},
                {"name":"missing.bin", "mime":"application/octet-stream", "size_bytes":3, "sha256":missing_hash, "availability":"ready"},
                {"name":"syncing.bin", "mime":"application/octet-stream", "size_bytes":3, "sha256":unavailable_hash, "availability":"syncing"}
            ]
        })];
        let resolved = HashMap::from([
            (
                good_hash.clone(),
                AttachmentCasFile {
                    path: PathBuf::from("/canonical/private/attachment.png"),
                    mime_type: "image/png".to_string(),
                    size_bytes: 5,
                    sha256: good_hash.clone(),
                },
            ),
            (
                oversized_hash.clone(),
                AttachmentCasFile {
                    path: PathBuf::from("/canonical/private/large.bin"),
                    mime_type: "application/octet-stream".to_string(),
                    size_bytes: MAX_RIVER_ARTIFACT_BYTES + 1,
                    sha256: oversized_hash.clone(),
                },
            ),
        ]);

        let projection =
            build_river_projection_with_artifacts(&messages, VcpRiverMode::Full, &resolved)
                .expect("build river full projection");
        assert_eq!(projection.artifact_grants.len(), 1);
        assert_eq!(
            projection.artifact_grants[0].source_path,
            PathBuf::from("/canonical/private/attachment.png")
        );
        assert!(projection
            .canonical_json
            .contains("vcp.mobile.attempt-projection.v1"));
        assert!(projection
            .canonical_json
            .contains("/run/river-artifact-00-"));
        assert!(projection
            .canonical_json
            .contains("\"source_unreachable\":true"));
        assert!(projection.canonical_json.contains("\"non_writeback\":true"));
        assert!(projection.canonical_json.contains("artifact_too_large"));
        assert!(projection.canonical_json.contains("local_cas_unavailable"));
        assert!(projection.canonical_json.contains("source_unavailable"));
        assert!(!projection.canonical_json.contains("/canonical/private"));
        assert!(!projection.canonical_json.contains("/private/photo.png"));

        assert_eq!(
            river_full_candidate_hashes(&messages),
            vec![good_hash.clone(), oversized_hash.clone(), missing_hash]
        );
        assert_eq!(
            selected_river_full_artifact_hashes(&messages, &resolved),
            vec![good_hash]
        );
    }

    #[test]
    fn river_full_count_and_total_budgets_are_visible_without_host_paths() {
        let mut descriptors = Vec::new();
        let mut resolved = HashMap::new();
        for index in 0..17_u8 {
            let hash = format!("{:064x}", index + 1);
            descriptors.push(json!({
                "name": format!("item-{index}.bin"),
                "mime": "application/octet-stream",
                "size_bytes": 1,
                "sha256": hash,
                "availability": "ready"
            }));
            resolved.insert(
                hash.clone(),
                AttachmentCasFile {
                    path: PathBuf::from(format!("/host/cas/{hash}.bin")),
                    mime_type: "application/octet-stream".to_string(),
                    size_bytes: 1,
                    sha256: hash,
                },
            );
        }
        let projection = build_river_projection_with_artifacts(
            &[json!({
                "role":"user",
                "content":"count budget",
                "__vcpLocalAttachments": descriptors
            })],
            VcpRiverMode::Full,
            &resolved,
        )
        .expect("count-bounded full projection");
        assert_eq!(projection.artifact_grants.len(), MAX_RIVER_ARTIFACTS);
        assert!(projection.canonical_json.contains("artifact_count_limit"));
        assert!(!projection.canonical_json.contains("/host/cas"));

        let mut total_descriptors = Vec::new();
        let mut total_resolved = HashMap::new();
        for index in 0..5_u8 {
            let hash = format!("{:064x}", index + 32);
            total_descriptors.push(json!({
                "name": format!("large-{index}.bin"),
                "mime": "application/octet-stream",
                "size_bytes": MAX_RIVER_ARTIFACT_BYTES,
                "sha256": hash,
                "availability": "ready"
            }));
            total_resolved.insert(
                hash.clone(),
                AttachmentCasFile {
                    path: PathBuf::from(format!("/host/cas/{hash}.bin")),
                    mime_type: "application/octet-stream".to_string(),
                    size_bytes: MAX_RIVER_ARTIFACT_BYTES,
                    sha256: hash,
                },
            );
        }
        let projection = build_river_projection_with_artifacts(
            &[json!({
                "role":"user",
                "content":"total budget",
                "__vcpLocalAttachments": total_descriptors
            })],
            VcpRiverMode::Full,
            &total_resolved,
        )
        .expect("total-bounded full projection");
        assert_eq!(projection.artifact_grants.len(), 4);
        assert!(projection.canonical_json.contains("artifact_total_limit"));
        assert!(!projection.canonical_json.contains("/host/cas"));
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
