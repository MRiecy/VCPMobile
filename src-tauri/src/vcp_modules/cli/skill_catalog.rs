//! P4.1 Skill catalog v2：canonical object、版本元数据与 non-writeback workspace 副本。

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::result::{VcpCliSkillResult, VcpCliSkillSummary};
use super::skills::{
    parse_skill_name, parse_skill_version, read_catalog_resource, sha256_hex,
    validate_resource_path, validate_skill_id, SkillError, BUILTIN_SKILL, BUILTIN_SKILL_ID,
    LEGACY_P1_BUILTIN_SKILL, MAX_SKILL_BYTES,
};

pub(super) const CATALOG_SCHEMA_VERSION: u32 = 2;
pub(super) const CATALOG_FILE_NAME: &str = ".catalog-v2.json";
pub(super) const OBJECTS_DIRECTORY: &str = "objects";
pub(super) const IMPORTS_DIRECTORY: &str = "imports";
pub(super) const MAX_CATALOG_SKILLS: usize = 128;
pub(super) const MAX_SKILL_RESOURCES: usize = 256;
pub(super) const MAX_SKILL_TOTAL_BYTES: u64 = 16 * 1024 * 1024;
pub(super) const MAX_SKILL_RESOURCE_BYTES: usize = 8 * 1024 * 1024;
const MAX_CATALOG_BYTES: usize = 16 * 1024 * 1024;
const MAX_IMPORT_OPERATIONS: usize = 64;
const MAX_CATALOG_WARNINGS: usize = 32;
const BUILTIN_DESCRIPTION: &str = "VCP Mobile CLI shell, workspace, Job and Skill safety basics.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(super) struct SkillResourceRecord {
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub mime_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(super) struct SkillCatalogEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    pub source: String,
    pub original_name: Option<String>,
    pub source_sha256: String,
    pub tree_sha256: String,
    pub total_bytes: u64,
    pub installed_at_ms: u64,
    pub resources: Vec<SkillResourceRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(super) struct SkillImportOperationRecord {
    pub operation_id: String,
    pub request_sha256: String,
    pub skill_id: String,
    pub tree_sha256: String,
    pub catalog_generation: u64,
    pub committed_at_ms: u64,
    pub result: SkillCatalogItem,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(super) struct SkillCatalog {
    pub schema_version: u32,
    pub generation: u64,
    pub skills: Vec<SkillCatalogEntry>,
    #[serde(default)]
    pub import_operations: Vec<SkillImportOperationRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SkillCatalogItem {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    pub source: String,
    pub tree_sha256: String,
    pub resource_count: usize,
    pub total_bytes: u64,
    pub integrity_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SkillCatalogSnapshot {
    pub schema_version: u32,
    pub generation: u64,
    pub skills: Vec<SkillCatalogItem>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SkillListResult {
    pub skills: Vec<VcpCliSkillSummary>,
    pub warnings: Vec<String>,
}

pub(super) fn ensure_skill_catalog(
    skills_root: &Path,
    now_ms: u64,
) -> Result<SkillCatalog, SkillError> {
    ensure_real_directory(skills_root, "Skills root")?;
    set_mode(skills_root, 0o700)?;
    ensure_private_directory(&skills_root.join(OBJECTS_DIRECTORY), "Skill objects")?;
    ensure_private_directory(&skills_root.join(IMPORTS_DIRECTORY), "Skill imports")?;

    let catalog_path = skills_root.join(CATALOG_FILE_NAME);
    let mut catalog = match fs::symlink_metadata(&catalog_path) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() || metadata.len() > MAX_CATALOG_BYTES as u64 {
                return Err(SkillError::integrity(
                    "Skill catalog must be a bounded regular file",
                ));
            }
            let bytes = fs::read(&catalog_path).map_err(|error| {
                SkillError::integrity(format!("cannot read Skill catalog: {error}"))
            })?;
            serde_json::from_slice::<SkillCatalog>(&bytes)
                .map_err(|error| SkillError::integrity(format!("invalid Skill catalog: {error}")))?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => SkillCatalog {
            schema_version: CATALOG_SCHEMA_VERSION,
            generation: 0,
            skills: Vec::new(),
            import_operations: Vec::new(),
        },
        Err(error) => {
            return Err(SkillError::integrity(format!(
                "cannot inspect Skill catalog: {error}"
            )))
        }
    };
    validate_catalog(&catalog)?;

    let builtin = builtin_entry(now_ms);
    install_builtin_object(skills_root, &builtin)?;
    let mut changed = false;
    match catalog
        .skills
        .iter_mut()
        .find(|entry| entry.id == BUILTIN_SKILL_ID)
    {
        Some(existing) if builtin_identity_matches(existing, &builtin) => {}
        Some(existing) => {
            *existing = builtin;
            changed = true;
        }
        None => {
            catalog.skills.push(builtin);
            changed = true;
        }
    }
    if catalog.generation == 0 || changed {
        catalog.generation = catalog.generation.saturating_add(1).max(1);
        save_catalog_atomic(skills_root, &catalog)?;
    }
    Ok(catalog)
}

fn builtin_identity_matches(existing: &SkillCatalogEntry, canonical: &SkillCatalogEntry) -> bool {
    existing.id == canonical.id
        && existing.name == canonical.name
        && existing.description == canonical.description
        && existing.version == canonical.version
        && existing.source == canonical.source
        && existing.original_name == canonical.original_name
        && existing.source_sha256 == canonical.source_sha256
        && existing.tree_sha256 == canonical.tree_sha256
        && existing.total_bytes == canonical.total_bytes
        && existing.resources == canonical.resources
}

pub(super) fn load_catalog(skills_root: &Path) -> Result<SkillCatalog, SkillError> {
    let path = skills_root.join(CATALOG_FILE_NAME);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| SkillError::integrity(format!("cannot inspect Skill catalog: {error}")))?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_CATALOG_BYTES as u64 {
        return Err(SkillError::integrity(
            "Skill catalog must be a bounded regular file",
        ));
    }
    let bytes = fs::read(&path)
        .map_err(|error| SkillError::integrity(format!("cannot read Skill catalog: {error}")))?;
    let catalog = serde_json::from_slice::<SkillCatalog>(&bytes)
        .map_err(|error| SkillError::integrity(format!("invalid Skill catalog: {error}")))?;
    validate_catalog(&catalog)?;
    Ok(catalog)
}

pub(super) fn save_catalog_atomic(
    skills_root: &Path,
    catalog: &SkillCatalog,
) -> Result<(), SkillError> {
    validate_catalog(catalog)?;
    let bytes = serde_json::to_vec_pretty(catalog)
        .map_err(|error| SkillError::integrity(format!("cannot encode Skill catalog: {error}")))?;
    if bytes.len() > MAX_CATALOG_BYTES {
        return Err(SkillError::integrity(
            "Skill catalog exceeds its hard limit",
        ));
    }
    let target = skills_root.join(CATALOG_FILE_NAME);
    let temporary = skills_root.join(format!(".{CATALOG_FILE_NAME}-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| {
                SkillError::integrity(format!("cannot create Skill catalog temporary: {error}"))
            })?;
        set_mode(&temporary, 0o600)?;
        file.write_all(&bytes).map_err(|error| {
            SkillError::integrity(format!("cannot write Skill catalog: {error}"))
        })?;
        file.sync_all().map_err(|error| {
            SkillError::integrity(format!("cannot sync Skill catalog: {error}"))
        })?;
        fs::rename(&temporary, &target).map_err(|error| {
            SkillError::integrity(format!("cannot atomically replace Skill catalog: {error}"))
        })?;
        sync_directory(skills_root)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(super) fn list_skills_v2(skills_root: &Path) -> Result<SkillListResult, SkillError> {
    let catalog = load_catalog(skills_root)?;
    let mut summaries = Vec::with_capacity(catalog.skills.len());
    let mut warnings = legacy_warnings(skills_root);
    for entry in &catalog.skills {
        match validate_entry_head(skills_root, entry) {
            Ok(()) => summaries.push(VcpCliSkillSummary {
                id: entry.id.clone(),
                name: entry.name.clone(),
                version: entry.version.clone(),
                source: entry.source.clone(),
                sha256: entry.tree_sha256.clone(),
            }),
            Err(error) => push_warning(
                &mut warnings,
                format!("Skipped invalid Skill {}: {}", entry.id, error.message),
            ),
        }
    }
    summaries.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(SkillListResult {
        skills: summaries,
        warnings,
    })
}

pub(super) fn catalog_snapshot(skills_root: &Path) -> Result<SkillCatalogSnapshot, SkillError> {
    let catalog = load_catalog(skills_root)?;
    let mut warnings = legacy_warnings(skills_root);
    let mut skills = Vec::with_capacity(catalog.skills.len());
    for entry in &catalog.skills {
        match validate_entry_head(skills_root, entry) {
            Ok(()) => skills.push(catalog_item(entry, "valid")),
            Err(error) => {
                push_warning(
                    &mut warnings,
                    format!("Invalid Skill {}: {}", entry.id, error.message),
                );
                skills.push(catalog_item(entry, "invalid"));
            }
        }
    }
    skills.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(SkillCatalogSnapshot {
        schema_version: CATALOG_SCHEMA_VERSION,
        generation: catalog.generation,
        skills,
        warnings,
    })
}

pub(super) fn read_skill_v2(
    skills_root: &Path,
    skill_id: &str,
    resource_path: &str,
    requested_max_bytes: usize,
) -> Result<(VcpCliSkillResult, String), SkillError> {
    validate_skill_id(skill_id)?;
    let relative = validate_resource_path(resource_path)?;
    let relative = relative.to_string_lossy().replace('\\', "/");
    let catalog = load_catalog(skills_root)?;
    let entry = catalog
        .skills
        .iter()
        .find(|entry| entry.id == skill_id)
        .ok_or_else(|| SkillError::not_found("Skill was not found in the catalog"))?;
    let resource = entry
        .resources
        .iter()
        .find(|resource| resource.path == relative)
        .ok_or_else(|| SkillError::not_found("Skill resource was not found in the catalog"))?;
    if resource.size_bytes > MAX_SKILL_BYTES as u64 {
        return Err(SkillError::integrity(
            "Skill text resource exceeds the 64 KiB read limit",
        ));
    }
    let bytes = read_object_resource(skills_root, entry, resource, MAX_SKILL_BYTES)?;
    let max_bytes = requested_max_bytes.clamp(1, MAX_SKILL_BYTES);
    let text = String::from_utf8(bytes)
        .map_err(|_| SkillError::integrity("Skill resource must be UTF-8"))?;
    let truncated = text.len() > max_bytes;
    let content = truncate_utf8(&text, max_bytes).to_string();
    Ok((
        VcpCliSkillResult {
            id: entry.id.clone(),
            name: entry.name.clone(),
            resource_path: relative,
            skill_root: format!("vcp-skill://{}", entry.id),
            sha256: resource.sha256.clone(),
            truncated,
            materialized_path: None,
        },
        content,
    ))
}

pub(super) fn materialize_skill(
    skills_root: &Path,
    workspace: &Path,
    skill_id: &str,
) -> Result<(VcpCliSkillResult, String), SkillError> {
    validate_skill_id(skill_id)?;
    let catalog = load_catalog(skills_root)?;
    let entry = catalog
        .skills
        .iter()
        .find(|entry| entry.id == skill_id)
        .ok_or_else(|| SkillError::not_found("Skill was not found in the catalog"))?;
    validate_all_resources(skills_root, entry)?;

    ensure_real_directory(workspace, "workspace")?;
    let managed_root = workspace.join(".vcp-skills");
    ensure_or_create_directory(&managed_root, "workspace Skill root")?;
    let id_root = managed_root.join(&entry.id);
    ensure_or_create_directory(&id_root, "workspace Skill id root")?;
    let target = id_root.join(&entry.tree_sha256);
    let staging = id_root.join(format!(".{}.{}.staging", entry.tree_sha256, Uuid::new_v4()));
    fs::create_dir(&staging).map_err(|error| {
        SkillError::integrity(format!("cannot create workspace Skill staging: {error}"))
    })?;
    set_mode(&staging, 0o700)?;
    let result = (|| {
        for resource in &entry.resources {
            let bytes =
                read_object_resource(skills_root, entry, resource, MAX_SKILL_RESOURCE_BYTES)?;
            let relative = validate_resource_path(&resource.path)?;
            let destination = staging.join(&relative);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    SkillError::integrity(format!(
                        "cannot create workspace Skill resource parent: {error}"
                    ))
                })?;
            }
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)
                .map_err(|error| {
                    SkillError::integrity(format!(
                        "cannot create workspace Skill resource: {error}"
                    ))
                })?;
            file.write_all(&bytes).map_err(|error| {
                SkillError::integrity(format!("cannot write workspace Skill resource: {error}"))
            })?;
            file.sync_all().map_err(|error| {
                SkillError::integrity(format!("cannot sync workspace Skill resource: {error}"))
            })?;
        }
        sync_tree_directories(&staging)?;
        match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.file_type().is_dir() => {
                let stale = id_root.join(format!(".stale-{}", Uuid::new_v4()));
                fs::rename(&target, &stale).map_err(|error| {
                    SkillError::integrity(format!("cannot rotate old workspace Skill: {error}"))
                })?;
                fs::rename(&staging, &target).map_err(|error| {
                    let _ = fs::rename(&stale, &target);
                    SkillError::integrity(format!("cannot publish workspace Skill: {error}"))
                })?;
                let _ = fs::remove_dir_all(stale);
            }
            Ok(_) => {
                return Err(SkillError::integrity(
                    "workspace Skill target is not a real directory",
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::rename(&staging, &target).map_err(|error| {
                    SkillError::integrity(format!("cannot publish workspace Skill: {error}"))
                })?;
            }
            Err(error) => {
                return Err(SkillError::integrity(format!(
                    "cannot inspect workspace Skill target: {error}"
                )))
            }
        }
        sync_directory(&id_root)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result?;

    let guest_path = format!("/workspace/.vcp-skills/{}/{}", entry.id, entry.tree_sha256);
    Ok((
        VcpCliSkillResult {
            id: entry.id.clone(),
            name: entry.name.clone(),
            resource_path: ".".to_string(),
            skill_root: format!("vcp-skill://{}", entry.id),
            sha256: entry.tree_sha256.clone(),
            truncated: false,
            materialized_path: Some(guest_path.clone()),
        },
        guest_path,
    ))
}

pub(super) fn tree_sha256(resources: &[SkillResourceRecord]) -> Result<String, SkillError> {
    let mut ordered = resources.to_vec();
    ordered.sort_by(|left, right| left.path.cmp(&right.path));
    let mut digest = Sha256::new();
    digest.update(b"vcp-mobile-skill-tree-v1\0");
    for resource in ordered {
        let path = resource.path.as_bytes();
        digest.update(
            u32::try_from(path.len())
                .map_err(|_| SkillError::integrity("Skill resource path is too long"))?
                .to_le_bytes(),
        );
        digest.update(path);
        digest.update(resource.size_bytes.to_le_bytes());
        let hash = hex::decode(&resource.sha256)
            .map_err(|_| SkillError::integrity("Skill resource hash is invalid"))?;
        if hash.len() != 32 {
            return Err(SkillError::integrity("Skill resource hash is invalid"));
        }
        digest.update(hash);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub(super) fn record_import_operation(
    catalog: &mut SkillCatalog,
    operation: SkillImportOperationRecord,
) {
    catalog
        .import_operations
        .retain(|existing| existing.operation_id != operation.operation_id);
    catalog.import_operations.push(operation);
    if catalog.import_operations.len() > MAX_IMPORT_OPERATIONS {
        let remove = catalog.import_operations.len() - MAX_IMPORT_OPERATIONS;
        catalog.import_operations.drain(0..remove);
    }
}

fn builtin_entry(now_ms: u64) -> SkillCatalogEntry {
    let resource = SkillResourceRecord {
        path: "SKILL.md".to_string(),
        sha256: sha256_hex(BUILTIN_SKILL.as_bytes()),
        size_bytes: BUILTIN_SKILL.len() as u64,
        mime_type: "text/markdown".to_string(),
    };
    let resources = vec![resource];
    SkillCatalogEntry {
        id: BUILTIN_SKILL_ID.to_string(),
        name: parse_skill_name(BUILTIN_SKILL).unwrap_or_else(|| BUILTIN_SKILL_ID.to_string()),
        description: BUILTIN_DESCRIPTION.to_string(),
        version: parse_skill_version(BUILTIN_SKILL),
        source: "builtin".to_string(),
        original_name: None,
        source_sha256: sha256_hex(BUILTIN_SKILL.as_bytes()),
        tree_sha256: tree_sha256(&resources).expect("built-in Skill tree is valid"),
        total_bytes: BUILTIN_SKILL.len() as u64,
        installed_at_ms: now_ms,
        resources,
    }
}

fn install_builtin_object(skills_root: &Path, entry: &SkillCatalogEntry) -> Result<(), SkillError> {
    let objects_root = skills_root.join(OBJECTS_DIRECTORY);
    let target = objects_root.join(&entry.tree_sha256);
    match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            validate_all_resources(skills_root, entry)?;
            return Ok(());
        }
        Ok(_) => return Err(SkillError::integrity("built-in Skill object is invalid")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(SkillError::integrity(format!(
                "cannot inspect built-in Skill object: {error}"
            )))
        }
    }
    let staging = objects_root.join(format!(".builtin-{}.staging", Uuid::new_v4()));
    fs::create_dir(&staging).map_err(|error| {
        SkillError::integrity(format!("cannot create built-in Skill staging: {error}"))
    })?;
    let result = (|| {
        let path = staging.join("SKILL.md");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                SkillError::integrity(format!("cannot create built-in Skill: {error}"))
            })?;
        file.write_all(BUILTIN_SKILL.as_bytes()).map_err(|error| {
            SkillError::integrity(format!("cannot write built-in Skill: {error}"))
        })?;
        file.sync_all().map_err(|error| {
            SkillError::integrity(format!("cannot sync built-in Skill: {error}"))
        })?;
        set_mode(&path, 0o444)?;
        set_mode(&staging, 0o555)?;
        sync_directory(&staging)?;
        fs::rename(&staging, &target).map_err(|error| {
            SkillError::integrity(format!("cannot publish built-in Skill object: {error}"))
        })?;
        sync_directory(&objects_root)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn validate_catalog(catalog: &SkillCatalog) -> Result<(), SkillError> {
    if catalog.schema_version != CATALOG_SCHEMA_VERSION {
        return Err(SkillError::integrity("unsupported Skill catalog schema"));
    }
    if catalog.skills.len() > MAX_CATALOG_SKILLS {
        return Err(SkillError::integrity(
            "Skill catalog count exceeds its hard limit",
        ));
    }
    let mut ids = BTreeSet::new();
    for entry in &catalog.skills {
        validate_skill_id(&entry.id)?;
        if !ids.insert(entry.id.as_str()) {
            return Err(SkillError::integrity(
                "Skill catalog contains duplicate ids",
            ));
        }
        if entry.name.is_empty()
            || entry.name.len() > 256
            || entry.description.len() > 1024
            || entry
                .version
                .as_ref()
                .is_some_and(|value| value.len() > 128)
            || !matches!(entry.source.as_str(), "builtin" | "imported")
            || entry
                .original_name
                .as_ref()
                .is_some_and(|value| value.len() > 255)
        {
            return Err(SkillError::integrity(
                "Skill catalog metadata exceeds its hard limit",
            ));
        }
        if !is_sha256(&entry.source_sha256) || !is_sha256(&entry.tree_sha256) {
            return Err(SkillError::integrity("Skill catalog hash is invalid"));
        }
        if entry.resources.is_empty() || entry.resources.len() > MAX_SKILL_RESOURCES {
            return Err(SkillError::integrity("Skill resource count is invalid"));
        }
        let mut paths = BTreeSet::new();
        let mut total = 0u64;
        for resource in &entry.resources {
            validate_resource_path(&resource.path)?;
            if !paths.insert(resource.path.as_str()) || !is_sha256(&resource.sha256) {
                return Err(SkillError::integrity("Skill resource manifest is invalid"));
            }
            if resource.size_bytes > MAX_SKILL_RESOURCE_BYTES as u64 {
                return Err(SkillError::integrity(
                    "Skill resource exceeds its hard limit",
                ));
            }
            if resource.mime_type.is_empty() || resource.mime_type.len() > 128 {
                return Err(SkillError::integrity(
                    "Skill resource MIME metadata is invalid",
                ));
            }
            total = total
                .checked_add(resource.size_bytes)
                .ok_or_else(|| SkillError::integrity("Skill size overflow"))?;
        }
        if total != entry.total_bytes || total > MAX_SKILL_TOTAL_BYTES {
            return Err(SkillError::integrity("Skill total size is invalid"));
        }
        if tree_sha256(&entry.resources)? != entry.tree_sha256 {
            return Err(SkillError::integrity("Skill tree digest is invalid"));
        }
        if !paths.contains("SKILL.md") {
            return Err(SkillError::integrity("Skill is missing SKILL.md"));
        }
    }
    let mut operation_ids = BTreeSet::new();
    if catalog.import_operations.len() > MAX_IMPORT_OPERATIONS {
        return Err(SkillError::integrity(
            "Skill import operation history is oversized",
        ));
    }
    for operation in &catalog.import_operations {
        if operation.operation_id.is_empty()
            || operation.operation_id.len() > 128
            || !operation_ids.insert(operation.operation_id.as_str())
            || !is_sha256(&operation.request_sha256)
            || !is_sha256(&operation.tree_sha256)
            || operation.result.id != operation.skill_id
            || operation.result.tree_sha256 != operation.tree_sha256
            || operation.catalog_generation > catalog.generation
            || operation.result.name.is_empty()
            || operation.result.name.len() > 256
            || operation.result.description.len() > 1024
            || operation
                .result
                .version
                .as_ref()
                .is_some_and(|value| value.len() > 128)
            || operation.result.integrity_status != "valid"
        {
            return Err(SkillError::integrity(
                "Skill import operation history is invalid",
            ));
        }
    }
    Ok(())
}

fn validate_entry_head(skills_root: &Path, entry: &SkillCatalogEntry) -> Result<(), SkillError> {
    let resource = entry
        .resources
        .iter()
        .find(|resource| resource.path == "SKILL.md")
        .ok_or_else(|| SkillError::integrity("Skill is missing SKILL.md"))?;
    let bytes = read_object_resource(skills_root, entry, resource, MAX_SKILL_BYTES)?;
    std::str::from_utf8(&bytes).map_err(|_| SkillError::integrity("SKILL.md must be UTF-8"))?;
    Ok(())
}

pub(super) fn validate_all_resources(
    skills_root: &Path,
    entry: &SkillCatalogEntry,
) -> Result<(), SkillError> {
    for resource in &entry.resources {
        let _ = read_object_resource(skills_root, entry, resource, MAX_SKILL_RESOURCE_BYTES)?;
    }
    Ok(())
}

fn read_object_resource(
    skills_root: &Path,
    entry: &SkillCatalogEntry,
    resource: &SkillResourceRecord,
    max_bytes: usize,
) -> Result<Vec<u8>, SkillError> {
    let objects_root = skills_root.join(OBJECTS_DIRECTORY);
    let bytes =
        read_catalog_resource(&objects_root, &entry.tree_sha256, &resource.path, max_bytes)?;
    if bytes.len() as u64 != resource.size_bytes || sha256_hex(&bytes) != resource.sha256 {
        return Err(SkillError::integrity(format!(
            "Skill resource integrity failed: {}",
            resource.path
        )));
    }
    Ok(bytes)
}

pub(super) fn catalog_item(entry: &SkillCatalogEntry, integrity_status: &str) -> SkillCatalogItem {
    SkillCatalogItem {
        id: entry.id.clone(),
        name: entry.name.clone(),
        description: entry.description.clone(),
        version: entry.version.clone(),
        source: entry.source.clone(),
        tree_sha256: entry.tree_sha256.clone(),
        resource_count: entry.resources.len(),
        total_bytes: entry.total_bytes,
        integrity_status: integrity_status.to_string(),
    }
}

fn legacy_warnings(skills_root: &Path) -> Vec<String> {
    let mut warnings = Vec::new();
    let entries = match fs::read_dir(skills_root) {
        Ok(entries) => entries,
        Err(error) => {
            push_warning(
                &mut warnings,
                format!("Cannot inspect legacy Skill entries: {error}"),
            );
            return warnings;
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == OBJECTS_DIRECTORY
            || name == IMPORTS_DIRECTORY
            || name == CATALOG_FILE_NAME
            || name.starts_with(&format!(".{CATALOG_FILE_NAME}-"))
        {
            continue;
        }
        if name == BUILTIN_SKILL_ID && is_frozen_legacy_builtin(skills_root) {
            continue;
        }
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            push_warning(
                &mut warnings,
                format!("Legacy unmanaged Skill directory is not active: {name}"),
            );
        }
    }
    warnings
}

fn is_frozen_legacy_builtin(skills_root: &Path) -> bool {
    let legacy_root = skills_root.join(BUILTIN_SKILL_ID);
    let Ok(mut entries) = fs::read_dir(&legacy_root) else {
        return false;
    };
    let Some(Ok(entry)) = entries.next() else {
        return false;
    };
    if entries.next().is_some() || entry.file_name() != "SKILL.md" {
        return false;
    }
    read_catalog_resource(skills_root, BUILTIN_SKILL_ID, "SKILL.md", MAX_SKILL_BYTES).is_ok_and(
        |bytes| bytes == BUILTIN_SKILL.as_bytes() || bytes == LEGACY_P1_BUILTIN_SKILL.as_bytes(),
    )
}

fn push_warning(warnings: &mut Vec<String>, warning: String) {
    if warnings.len() >= MAX_CATALOG_WARNINGS {
        return;
    }
    let warning = truncate_utf8(&warning, 384).to_string();
    log::warn!("[VCPMobileCLI] {warning}");
    warnings.push(warning);
}

fn ensure_real_directory(path: &Path, label: &str) -> Result<(), SkillError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| SkillError::integrity(format!("cannot inspect {label}: {error}")))?;
    if !metadata.file_type().is_dir() {
        return Err(SkillError::integrity(format!(
            "{label} must be a real directory"
        )));
    }
    Ok(())
}

fn ensure_private_directory(path: &Path, label: &str) -> Result<(), SkillError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(SkillError::integrity(format!(
                "{label} must be a real directory"
            )))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|error| {
                SkillError::integrity(format!("cannot create {label}: {error}"))
            })?;
        }
        Err(error) => {
            return Err(SkillError::integrity(format!(
                "cannot inspect {label}: {error}"
            )))
        }
    }
    set_mode(path, 0o700)
}

fn ensure_or_create_directory(path: &Path, label: &str) -> Result<(), SkillError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(SkillError::integrity(format!(
            "{label} must be a real directory"
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(path)
            .map_err(|error| SkillError::integrity(format!("cannot create {label}: {error}"))),
        Err(error) => Err(SkillError::integrity(format!(
            "cannot inspect {label}: {error}"
        ))),
    }
}

fn sync_tree_directories(root: &Path) -> Result<(), SkillError> {
    let mut directories = vec![root.to_path_buf()];
    for entry in walk_files(root)? {
        if let Some(parent) = entry.parent() {
            directories.push(parent.to_path_buf());
        }
    }
    directories.sort();
    directories.dedup();
    directories.reverse();
    for directory in directories {
        sync_directory(&directory)?;
    }
    Ok(())
}

fn walk_files(root: &Path) -> Result<Vec<PathBuf>, SkillError> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .map_err(|error| SkillError::integrity(format!("cannot scan Skill tree: {error}")))?
        {
            let entry = entry.map_err(|error| {
                SkillError::integrity(format!("cannot read Skill tree entry: {error}"))
            })?;
            let kind = entry.file_type().map_err(|error| {
                SkillError::integrity(format!("cannot inspect Skill tree entry: {error}"))
            })?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                files.push(entry.path());
            } else {
                return Err(SkillError::integrity(
                    "Skill tree contains a non-regular entry",
                ));
            }
        }
    }
    Ok(files)
}

fn sync_directory(path: &Path) -> Result<(), SkillError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| SkillError::integrity(format!("cannot sync Skill directory: {error}")))
}

fn set_mode(path: &Path, mode: u32) -> Result<(), SkillError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|error| {
            SkillError::integrity(format!("cannot set Skill catalog permissions: {error}"))
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

pub(super) fn normalized_relative_path(path: &Path) -> Result<String, SkillError> {
    let mut output = String::new();
    for component in path.components() {
        let Component::Normal(value) = component else {
            return Err(SkillError::integrity(
                "Skill resource path contains an unsafe component",
            ));
        };
        let value = value
            .to_str()
            .ok_or_else(|| SkillError::integrity("Skill resource path must be UTF-8"))?;
        if !output.is_empty() {
            output.push('/');
        }
        output.push_str(value);
    }
    validate_resource_path(&output)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_bootstrap_is_idempotent_and_exposes_no_host_path() {
        let directory = tempfile::tempdir().expect("temporary Skills root");
        let first = ensure_skill_catalog(directory.path(), 10).expect("create catalog");
        let second = ensure_skill_catalog(directory.path(), 20).expect("reopen catalog");
        assert_eq!(first, second);
        let listed = list_skills_v2(directory.path()).expect("list catalog");
        assert_eq!(listed.skills.len(), 1);
        assert_eq!(listed.skills[0].id, BUILTIN_SKILL_ID);
        let (skill, content) = read_skill_v2(
            directory.path(),
            BUILTIN_SKILL_ID,
            "SKILL.md",
            MAX_SKILL_BYTES,
        )
        .expect("read built-in");
        assert!(content.contains("/workspace"));
        assert_eq!(skill.skill_root, "vcp-skill://vcp-mobile-cli-basics");
        assert!(skill.materialized_path.is_none());
    }

    #[test]
    fn frozen_p1_builtin_is_ignored_but_other_unmanaged_directories_are_visible() {
        let directory = tempfile::tempdir().expect("temporary Skills root");
        let legacy = directory.path().join(BUILTIN_SKILL_ID);
        fs::create_dir(&legacy).expect("create frozen legacy built-in");
        fs::write(legacy.join("SKILL.md"), LEGACY_P1_BUILTIN_SKILL).expect("write legacy built-in");
        fs::create_dir(directory.path().join("unmanaged-skill"))
            .expect("create unmanaged directory");
        ensure_skill_catalog(directory.path(), 10).expect("create v2 catalog");

        let snapshot = catalog_snapshot(directory.path()).expect("read catalog snapshot");
        assert!(!snapshot
            .warnings
            .iter()
            .any(|warning| warning.contains(BUILTIN_SKILL_ID)));
        assert!(snapshot
            .warnings
            .iter()
            .any(|warning| warning.contains("unmanaged-skill")));
    }

    #[test]
    fn materialized_copy_can_change_without_writing_back_to_canonical_object() {
        let directory = tempfile::tempdir().expect("temporary root");
        let skills = directory.path().join("skills");
        let workspace = directory.path().join("workspace");
        fs::create_dir(&skills).expect("create skills");
        fs::create_dir(&workspace).expect("create workspace");
        ensure_skill_catalog(&skills, 10).expect("create catalog");
        let (result, guest_path) =
            materialize_skill(&skills, &workspace, BUILTIN_SKILL_ID).expect("materialize Skill");
        assert_eq!(
            result.materialized_path.as_deref(),
            Some(guest_path.as_str())
        );
        let relative = guest_path
            .strip_prefix("/workspace/")
            .expect("guest prefix");
        let copy = workspace.join(relative).join("SKILL.md");
        fs::write(&copy, "mutated guest copy").expect("mutate copy");
        let (_, canonical) = read_skill_v2(&skills, BUILTIN_SKILL_ID, "SKILL.md", MAX_SKILL_BYTES)
            .expect("read canonical");
        assert_eq!(canonical, BUILTIN_SKILL);
    }

    #[cfg(unix)]
    #[test]
    fn catalog_rejects_symlinked_object_resource() {
        let directory = tempfile::tempdir().expect("temporary Skills root");
        let catalog = ensure_skill_catalog(directory.path(), 10).expect("create catalog");
        let entry = catalog
            .skills
            .iter()
            .find(|entry| entry.id == BUILTIN_SKILL_ID)
            .expect("built-in entry");
        let object = directory
            .path()
            .join(OBJECTS_DIRECTORY)
            .join(&entry.tree_sha256)
            .join("SKILL.md");
        fs::set_permissions(
            object.parent().expect("object parent"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .expect("make object writable for test");
        fs::remove_file(&object).expect("remove canonical file");
        std::os::unix::fs::symlink("/does/not/exist", &object).expect("create symlink");
        let listed = list_skills_v2(directory.path()).expect("list with warning");
        assert!(listed.skills.is_empty());
        assert!(listed
            .warnings
            .iter()
            .any(|warning| warning.contains(BUILTIN_SKILL_ID)));
    }
}
