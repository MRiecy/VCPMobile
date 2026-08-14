//! Durable, path-free `vcp.mobile.vref-projection.v1` planning.

use std::collections::HashMap;

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::knowledge::ActiveKnowledgeSource;
use super::semantic::KnowledgeSemanticSelection;
use super::turn_types::{
    DurableVrefOmission, DurableVrefProjection, DurableVrefSource,
    MAX_ATTEMPT_PROJECTION_TOTAL_BYTES, MAX_RIVER_ARTIFACT_TOTAL_BYTES, MAX_VREF_PROJECTION_BYTES,
    MAX_VREF_SOURCES, MAX_VREF_SOURCE_BYTES, MAX_VREF_TOTAL_BYTES,
};

const VREF_PROJECTION_SCHEMA: &str = "vcp.mobile.vref-projection.v1";
const USER_WEIGHT_MILLIS: u32 = 700;
const ASSISTANT_WEIGHT_MILLIS: u32 = 300;

#[derive(Serialize)]
struct CanonicalVrefProjection<'a> {
    schema: &'static str,
    requested: u32,
    resolved: u32,
    model_id: &'a str,
    query: CanonicalQuery<'a>,
    weights: CanonicalWeights,
    catalog_generation: u64,
    sources: &'a [DurableVrefSource],
    omissions: &'a [DurableVrefOmission],
}

#[derive(Serialize)]
struct CanonicalQuery<'a> {
    user_sha256: &'a str,
    assistant_sha256: Option<&'a str>,
}

#[derive(Serialize)]
struct CanonicalWeights {
    user_millis: u32,
    assistant_millis: u32,
}

pub(crate) fn build_durable_vref_projection(
    requested: u32,
    catalog_generation: u64,
    selection: &KnowledgeSemanticSelection,
    eligible_sources: &[ActiveKnowledgeSource],
    river_artifact_bytes: u64,
) -> Result<DurableVrefProjection, String> {
    if !(1..=MAX_VREF_SOURCES as u32).contains(&requested) {
        return Err("vref requested count is outside 1..=50".to_string());
    }
    if eligible_sources.is_empty() {
        return Err("vref requires at least one active local knowledge grant".to_string());
    }
    if river_artifact_bytes > MAX_RIVER_ARTIFACT_TOTAL_BYTES {
        return Err("River artifact bytes exceed their durable budget".to_string());
    }
    let source_lookup = eligible_sources
        .iter()
        .map(|source| (source.source_id.as_str(), source))
        .collect::<HashMap<_, _>>();
    let mut sources = Vec::new();
    let mut omissions = Vec::new();
    let mut selected_bytes = 0u64;
    for hit in selection.hits.iter().take(requested as usize) {
        let Some(source) = source_lookup.get(hit.source_id.as_str()) else {
            return Err("vref semantic result escaped the eligible catalog".to_string());
        };
        if source.source_sha256 != hit.source_sha256 {
            return Err("vref semantic source hash changed".to_string());
        }
        let next_total = selected_bytes.saturating_add(source.size_bytes);
        if source.size_bytes > MAX_VREF_SOURCE_BYTES {
            omissions.push(omission(source, "source_bytes_limit"));
            continue;
        }
        if next_total > MAX_VREF_TOTAL_BYTES {
            omissions.push(omission(source, "vref_total_bytes_limit"));
            continue;
        }
        if river_artifact_bytes.saturating_add(next_total) > MAX_ATTEMPT_PROJECTION_TOTAL_BYTES {
            omissions.push(omission(source, "combined_projection_bytes_limit"));
            continue;
        }
        let rank =
            u32::try_from(sources.len() + 1).map_err(|_| "vref rank overflowed".to_string())?;
        let basename = sanitized_basename(&source.display_name);
        let guest_relative_path = format!("{rank:04}-{}-{basename}", &source.source_sha256[..12]);
        sources.push(DurableVrefSource {
            rank,
            source_id: source.source_id.clone(),
            source_sha256: source.source_sha256.clone(),
            display_name: source.display_name.clone(),
            mime_type: source.mime_type.clone(),
            size_bytes: source.size_bytes,
            logical_ref: format!("vcp-knowledge://{}", source.source_id),
            guest_relative_path,
        });
        selected_bytes = next_total;
    }
    if selection.hits.is_empty() {
        omissions.push(DurableVrefOmission {
            source_id: None,
            source_sha256: None,
            reason: "no_usable_chunks".to_string(),
        });
    }
    let resolved = u32::try_from(sources.len()).map_err(|_| "vref resolved count overflowed")?;
    let canonical = CanonicalVrefProjection {
        schema: VREF_PROJECTION_SCHEMA,
        requested,
        resolved,
        model_id: &selection.model_id,
        query: CanonicalQuery {
            user_sha256: &selection.user_query_sha256,
            assistant_sha256: selection.assistant_query_sha256.as_deref(),
        },
        weights: CanonicalWeights {
            user_millis: USER_WEIGHT_MILLIS,
            assistant_millis: if selection.assistant_query_sha256.is_some() {
                ASSISTANT_WEIGHT_MILLIS
            } else {
                0
            },
        },
        catalog_generation,
        sources: &sources,
        omissions: &omissions,
    };
    let bytes = serde_json::to_vec(&canonical)
        .map_err(|error| format!("cannot serialize vref projection: {error}"))?;
    if bytes.len() > MAX_VREF_PROJECTION_BYTES {
        return Err("vref projection exceeds its canonical JSON budget".to_string());
    }
    let canonical_json = String::from_utf8(bytes.clone())
        .map_err(|error| format!("vref projection is not UTF-8: {error}"))?;
    Ok(DurableVrefProjection {
        canonical_json,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        size_bytes: bytes.len() as u64,
        catalog_generation,
        requested,
        resolved,
        model_id: selection.model_id.clone(),
        user_query_sha256: selection.user_query_sha256.clone(),
        assistant_query_sha256: selection.assistant_query_sha256.clone(),
        sources,
        omissions,
    })
}

pub(crate) fn validate_durable_vref_projection(
    projection: &DurableVrefProjection,
) -> Result<(), String> {
    if projection.canonical_json.len() > MAX_VREF_PROJECTION_BYTES
        || projection.size_bytes != projection.canonical_json.len() as u64
        || projection.resolved as usize != projection.sources.len()
        || projection.sources.len() > MAX_VREF_SOURCES
        || projection.requested == 0
        || projection.requested > MAX_VREF_SOURCES as u32
    {
        return Err("durable vref projection violates its count or byte fence".to_string());
    }
    let actual = format!("{:x}", Sha256::digest(projection.canonical_json.as_bytes()));
    if projection.sha256 != actual {
        return Err("durable vref projection SHA-256 mismatch".to_string());
    }
    let canonical: serde_json::Value = serde_json::from_str(&projection.canonical_json)
        .map_err(|error| format!("durable vref projection JSON is invalid: {error}"))?;
    if canonical.get("schema").and_then(serde_json::Value::as_str) != Some(VREF_PROJECTION_SCHEMA)
        || canonical
            .get("catalog_generation")
            .and_then(serde_json::Value::as_u64)
            != Some(projection.catalog_generation)
        || canonical
            .get("requested")
            .and_then(serde_json::Value::as_u64)
            != Some(u64::from(projection.requested))
        || canonical
            .get("resolved")
            .and_then(serde_json::Value::as_u64)
            != Some(u64::from(projection.resolved))
    {
        return Err("durable vref canonical identity disagrees with its DTO".to_string());
    }
    let descriptors = canonical
        .get("sources")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "durable vref projection has no source array".to_string())?;
    if descriptors.len() != projection.sources.len() {
        return Err("durable vref source count disagrees with canonical JSON".to_string());
    }
    let mut total = 0u64;
    for (index, source) in projection.sources.iter().enumerate() {
        let rank = u32::try_from(index + 1).map_err(|_| "durable vref rank overflowed")?;
        if source.rank != rank
            || source.size_bytes > MAX_VREF_SOURCE_BYTES
            || !is_sha256(&source.source_sha256)
            || source.guest_relative_path
                != format!(
                    "{rank:04}-{}-{}",
                    &source.source_sha256[..12],
                    sanitized_basename(&source.display_name)
                )
            || source.guest_relative_path.contains(['/', '\\'])
            || source.logical_ref != format!("vcp-knowledge://{}", source.source_id)
        {
            return Err("durable vref source identity is invalid".to_string());
        }
        total = total
            .checked_add(source.size_bytes)
            .ok_or_else(|| "durable vref byte total overflowed".to_string())?;
    }
    if total > MAX_VREF_TOTAL_BYTES {
        return Err("durable vref sources exceed their byte budget".to_string());
    }
    Ok(())
}

fn omission(source: &ActiveKnowledgeSource, reason: &str) -> DurableVrefOmission {
    DurableVrefOmission {
        source_id: Some(source.source_id.clone()),
        source_sha256: Some(source.source_sha256.clone()),
        reason: reason.to_string(),
    }
}

fn sanitized_basename(value: &str) -> String {
    let basename = value.rsplit(['/', '\\']).next().unwrap_or_default();
    let mut output = basename
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    output = output.trim_matches(['.', '_', '-']).to_string();
    if output.is_empty() {
        output = "knowledge.txt".to_string();
    }
    while output.len() > 96 {
        output.pop();
    }
    output
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(id: &str, sha: char, bytes: u64, name: &str) -> ActiveKnowledgeSource {
        ActiveKnowledgeSource {
            source_id: id.to_string(),
            display_name: name.to_string(),
            mime_type: "text/plain".to_string(),
            size_bytes: bytes,
            source_sha256: sha.to_string().repeat(64),
            object_path: "/private/not-serialized".into(),
        }
    }

    #[test]
    fn projection_is_relevance_ordered_bounded_and_path_free() {
        let sources = vec![
            source("source-b", 'b', 7, "../unsafe name.md"),
            source("source-a", 'a', 5, "a.txt"),
        ];
        let selection = KnowledgeSemanticSelection {
            hits: vec![
                super::super::semantic::KnowledgeSemanticHit {
                    source_id: "source-b".to_string(),
                    source_sha256: "b".repeat(64),
                    score: 0.9,
                },
                super::super::semantic::KnowledgeSemanticHit {
                    source_id: "source-a".to_string(),
                    source_sha256: "a".repeat(64),
                    score: 0.8,
                },
            ],
            model_id: "model".to_string(),
            user_query_sha256: "c".repeat(64),
            assistant_query_sha256: Some("d".repeat(64)),
        };
        let projection = build_durable_vref_projection(2, 9, &selection, &sources, 0)
            .expect("projection should build");
        assert_eq!(projection.resolved, 2);
        assert_eq!(
            projection.sources[0].guest_relative_path,
            "0001-bbbbbbbbbbbb-unsafe_name.md"
        );
        assert!(!projection.canonical_json.contains("/private"));
        validate_durable_vref_projection(&projection).expect("projection should validate");
    }

    #[test]
    fn empty_valid_catalog_has_stable_zero_hit_projection() {
        let sources = vec![source("source-a", 'a', 5, "a.txt")];
        let selection = KnowledgeSemanticSelection {
            hits: Vec::new(),
            model_id: "model".to_string(),
            user_query_sha256: "c".repeat(64),
            assistant_query_sha256: None,
        };
        let projection = build_durable_vref_projection(5, 2, &selection, &sources, 0)
            .expect("empty selection remains executable");
        assert_eq!(projection.resolved, 0);
        assert_eq!(projection.omissions[0].reason, "no_usable_chunks");
    }
}
