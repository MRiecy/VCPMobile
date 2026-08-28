//! P4.1 Skill ZIP 两阶段导入：owned candidate、generation fence 与原子 object publish。

use std::collections::BTreeSet;
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zip::ZipArchive;

use super::skill_catalog::{
    catalog_item, load_catalog, normalized_relative_path, record_import_operation,
    save_catalog_atomic, tree_sha256, validate_all_resources, SkillCatalogEntry, SkillCatalogItem,
    SkillImportOperationRecord, SkillResourceRecord, IMPORTS_DIRECTORY, MAX_CATALOG_SKILLS,
    MAX_SKILL_RESOURCES, MAX_SKILL_RESOURCE_BYTES, MAX_SKILL_TOTAL_BYTES, OBJECTS_DIRECTORY,
};
use super::skills::{
    parse_skill_name, parse_skill_version, sha256_hex, validate_skill_id, SkillError,
    BUILTIN_SKILL_ID, MAX_SKILL_BYTES,
};

const IMPORT_SCHEMA_VERSION: u32 = 1;
const TOKEN_PREFIX: &str = "vcp-skill-import-v1:";
const CANDIDATE_FILE_NAME: &str = "candidate.zip";
const INSPECTION_FILE_NAME: &str = "inspection.json";
const MAX_IMPORT_ARCHIVE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_IMPORT_INSPECTION_BYTES: u64 = 512 * 1024;
const MAX_IMPORTS: usize = 32;
const STALE_IMPORT_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const COPY_BUFFER_BYTES: usize = 64 * 1024;
const MAX_RESOURCE_PATH_BYTES: usize = 240;
const MAX_RESOURCE_PATH_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickedSkillImportFile {
    pub path: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectSkillImportRequest {
    pub operation_id: String,
    pub picked: PickedSkillImportFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillImportCandidate {
    pub token: String,
    pub candidate_sha256: String,
    pub catalog_generation: u64,
    pub skill_id: String,
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    pub source_name: String,
    pub resource_count: usize,
    pub total_bytes: u64,
    pub tree_sha256: String,
    pub replaces_existing: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitSkillImportRequest {
    pub operation_id: String,
    pub token: String,
    pub candidate_sha256: String,
    pub expected_catalog_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitSkillImportResponse {
    pub operation_id: String,
    pub catalog_generation: u64,
    pub skill: SkillCatalogItem,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscardSkillImportRequest {
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct PersistedInspection {
    schema_version: u32,
    token: String,
    operation_id: String,
    candidate_sha256: String,
    candidate_bytes: u64,
    source_name: String,
    source_mime: String,
    catalog_generation: u64,
    inspected_at_ms: u64,
    entry: SkillCatalogEntry,
    warnings: Vec<String>,
}

#[derive(Debug)]
struct AuditedArchive {
    entry: SkillCatalogEntry,
    files: Vec<AuditedResource>,
    warnings: Vec<String>,
}

#[derive(Debug)]
struct AuditedResource {
    record: SkillResourceRecord,
    bytes: Vec<u8>,
}

struct ArchiveFilePlan {
    index: usize,
    raw_path: PathBuf,
    normalized_path: String,
    declared_size: u64,
}

pub(super) fn inspect_skill_import(
    skills_root: &Path,
    source_path: &Path,
    request: &InspectSkillImportRequest,
    now_ms: u64,
) -> Result<SkillImportCandidate, SkillError> {
    validate_operation_id(&request.operation_id)?;
    validate_source_metadata(&request.picked)?;
    let catalog = load_catalog(skills_root)?;
    let imports_root = skills_root.join(IMPORTS_DIRECTORY);
    gc_stale_imports(&imports_root)?;

    if let Some(existing) = find_inspection_by_operation(&imports_root, &request.operation_id)? {
        if existing.candidate_sha256 != request.picked.hash
            || existing.candidate_bytes != request.picked.size
            || existing.source_name != request.picked.name
        {
            return Err(SkillError::integrity(
                "Skill inspect operation_id was already used for another candidate",
            ));
        }
        return Ok(candidate_from_inspection(&existing, &catalog));
    }
    if import_count(&imports_root)? >= MAX_IMPORTS {
        return Err(SkillError::integrity(
            "too many pending Skill imports; discard an inspection and retry",
        ));
    }

    let id = Uuid::new_v4().to_string();
    let token = format!("{TOKEN_PREFIX}{id}");
    let staging = imports_root.join(format!(".{id}.staging"));
    let final_root = imports_root.join(&id);
    fs::create_dir(&staging).map_err(|error| {
        SkillError::integrity(format!("cannot create Skill import staging: {error}"))
    })?;
    set_mode(&staging, 0o700)?;
    let owned_candidate = staging.join(CANDIDATE_FILE_NAME);
    let result = (|| {
        copy_verified_candidate(source_path, &owned_candidate, &request.picked)?;
        let audited = audit_archive(
            &owned_candidate,
            &request.picked.name,
            &request.picked.hash,
            now_ms,
        )?;
        if audited.entry.id == BUILTIN_SKILL_ID {
            return Err(SkillError::integrity(
                "an imported Skill cannot replace the built-in Skill id",
            ));
        }
        let inspection = PersistedInspection {
            schema_version: IMPORT_SCHEMA_VERSION,
            token: token.clone(),
            operation_id: request.operation_id.clone(),
            candidate_sha256: request.picked.hash.clone(),
            candidate_bytes: request.picked.size,
            source_name: request.picked.name.clone(),
            source_mime: request.picked.mime.clone(),
            catalog_generation: catalog.generation,
            inspected_at_ms: now_ms,
            entry: audited.entry,
            warnings: audited.warnings,
        };
        write_inspection(&staging, &inspection)?;
        sync_directory(&staging)?;
        fs::rename(&staging, &final_root).map_err(|error| {
            SkillError::integrity(format!("cannot publish Skill import inspection: {error}"))
        })?;
        sync_directory(&imports_root)?;
        Ok(candidate_from_inspection(&inspection, &catalog))
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

pub(super) fn replay_skill_import_inspection(
    skills_root: &Path,
    request: &InspectSkillImportRequest,
) -> Result<Option<SkillImportCandidate>, SkillError> {
    validate_operation_id(&request.operation_id)?;
    validate_source_metadata(&request.picked)?;
    let catalog = load_catalog(skills_root)?;
    let imports_root = skills_root.join(IMPORTS_DIRECTORY);
    let Some(existing) = find_inspection_by_operation(&imports_root, &request.operation_id)? else {
        return Ok(None);
    };
    if existing.candidate_sha256 != request.picked.hash
        || existing.candidate_bytes != request.picked.size
        || existing.source_name != request.picked.name
    {
        return Err(SkillError::integrity(
            "Skill inspect operation_id was already used for another candidate",
        ));
    }
    Ok(Some(candidate_from_inspection(&existing, &catalog)))
}

pub(super) fn commit_skill_import(
    skills_root: &Path,
    request: &CommitSkillImportRequest,
    now_ms: u64,
) -> Result<CommitSkillImportResponse, SkillError> {
    validate_operation_id(&request.operation_id)?;
    validate_sha256(&request.candidate_sha256, "candidate_sha256")?;
    let id = parse_token(&request.token)?;
    let mut catalog = load_catalog(skills_root)?;
    let request_sha256 = commit_request_sha256(request);
    if let Some(operation) = catalog
        .import_operations
        .iter()
        .find(|operation| operation.operation_id == request.operation_id)
    {
        if operation.request_sha256 != request_sha256 {
            return Err(SkillError::integrity(
                "Skill commit operation_id was already used for another request",
            ));
        }
        return Ok(CommitSkillImportResponse {
            operation_id: request.operation_id.clone(),
            catalog_generation: operation.catalog_generation,
            skill: operation.result.clone(),
            replayed: true,
        });
    }
    if catalog.generation != request.expected_catalog_generation {
        return Err(SkillError::integrity(format!(
            "Skill catalog generation changed: expected {}, current {}",
            request.expected_catalog_generation, catalog.generation
        )));
    }

    let import_root = skills_root.join(IMPORTS_DIRECTORY).join(&id);
    let inspection = read_inspection(&import_root)?;
    if inspection.token != request.token
        || inspection.candidate_sha256 != request.candidate_sha256
        || inspection.catalog_generation != request.expected_catalog_generation
    {
        return Err(SkillError::integrity(
            "Skill import confirmation does not match its inspection",
        ));
    }
    let candidate_path = import_root.join(CANDIDATE_FILE_NAME);
    verify_regular_file_hash(
        &candidate_path,
        inspection.candidate_bytes,
        &inspection.candidate_sha256,
        MAX_IMPORT_ARCHIVE_BYTES,
    )?;
    let audited = audit_archive(
        &candidate_path,
        &inspection.source_name,
        &inspection.candidate_sha256,
        inspection.inspected_at_ms,
    )?;
    if audited.entry != inspection.entry || audited.warnings != inspection.warnings {
        return Err(SkillError::integrity(
            "Skill candidate changed after inspection",
        ));
    }
    publish_object(skills_root, &audited)?;

    if let Some(existing) = catalog
        .skills
        .iter_mut()
        .find(|entry| entry.id == audited.entry.id)
    {
        if existing.source == "builtin" {
            return Err(SkillError::integrity(
                "an imported Skill cannot replace a built-in Skill",
            ));
        }
        *existing = audited.entry.clone();
    } else {
        if catalog.skills.len() >= MAX_CATALOG_SKILLS {
            return Err(SkillError::integrity(
                "Skill catalog count exceeds its hard limit",
            ));
        }
        catalog.skills.push(audited.entry.clone());
    }
    catalog.generation = catalog
        .generation
        .checked_add(1)
        .ok_or_else(|| SkillError::integrity("Skill catalog generation overflow"))?;
    let result = catalog_item(&audited.entry, "valid");
    let catalog_generation = catalog.generation;
    record_import_operation(
        &mut catalog,
        SkillImportOperationRecord {
            operation_id: request.operation_id.clone(),
            request_sha256,
            skill_id: audited.entry.id.clone(),
            tree_sha256: audited.entry.tree_sha256.clone(),
            catalog_generation,
            committed_at_ms: now_ms,
            result: result.clone(),
        },
    );
    save_catalog_atomic(skills_root, &catalog)?;
    fs::remove_dir_all(&import_root).map_err(|error| {
        SkillError::integrity(format!(
            "Skill committed but inspection cleanup failed: {error}"
        ))
    })?;
    sync_directory(&skills_root.join(IMPORTS_DIRECTORY))?;
    Ok(CommitSkillImportResponse {
        operation_id: request.operation_id.clone(),
        catalog_generation: catalog.generation,
        skill: result,
        replayed: false,
    })
}

pub(super) fn discard_skill_import(
    skills_root: &Path,
    request: &DiscardSkillImportRequest,
) -> Result<(), SkillError> {
    let id = parse_token(&request.token)?;
    let imports_root = skills_root.join(IMPORTS_DIRECTORY);
    let target = imports_root.join(id);
    match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            fs::remove_dir_all(&target).map_err(|error| {
                SkillError::integrity(format!("cannot discard Skill inspection: {error}"))
            })?;
            sync_directory(&imports_root)
        }
        Ok(_) => Err(SkillError::integrity(
            "Skill inspection target is not a real directory",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(SkillError::integrity(format!(
            "cannot inspect Skill import: {error}"
        ))),
    }
}

pub(super) fn validate_picker_source(
    app_cache_dir: &Path,
    source_path: &Path,
) -> Result<(), SkillError> {
    let uploads = app_cache_dir.join("uploads");
    let uploads = fs::canonicalize(&uploads).map_err(|error| {
        SkillError::integrity(format!("cannot resolve picker staging root: {error}"))
    })?;
    let metadata = fs::symlink_metadata(source_path).map_err(|error| {
        SkillError::integrity(format!("cannot inspect picked Skill file: {error}"))
    })?;
    if !metadata.file_type().is_file() {
        return Err(SkillError::integrity(
            "picked Skill must be a real regular file",
        ));
    }
    let canonical = fs::canonicalize(source_path).map_err(|error| {
        SkillError::integrity(format!("cannot resolve picked Skill file: {error}"))
    })?;
    if canonical.parent() != Some(uploads.as_path()) {
        return Err(SkillError::integrity(
            "picked Skill is outside the native picker staging root",
        ));
    }
    Ok(())
}

fn audit_archive(
    path: &Path,
    source_name: &str,
    source_sha256: &str,
    installed_at_ms: u64,
) -> Result<AuditedArchive, SkillError> {
    let file = open_regular_nofollow(path, MAX_IMPORT_ARCHIVE_BYTES)?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| SkillError::integrity(format!("invalid Skill ZIP: {error}")))?;
    if archive.is_empty() || archive.len() > MAX_SKILL_RESOURCES * 2 {
        return Err(SkillError::integrity(
            "Skill ZIP entry count is outside its hard limit",
        ));
    }
    let mut raw_files = Vec::new();
    let mut declared_total = 0u64;
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(|error| {
            SkillError::integrity(format!("cannot inspect Skill ZIP entry: {error}"))
        })?;
        if file.encrypted() {
            return Err(SkillError::integrity(
                "encrypted Skill ZIP entries are unsupported",
            ));
        }
        let name = std::str::from_utf8(file.name_raw())
            .map_err(|_| SkillError::integrity("Skill ZIP paths must be UTF-8"))?;
        let raw_path = validate_archive_path(name)?;
        validate_archive_kind(&file)?;
        if file.is_dir() {
            continue;
        }
        if file.size() > MAX_SKILL_RESOURCE_BYTES as u64 {
            return Err(SkillError::integrity(
                "Skill ZIP resource exceeds its hard limit",
            ));
        }
        declared_total = declared_total
            .checked_add(file.size())
            .ok_or_else(|| SkillError::integrity("Skill ZIP size overflow"))?;
        if declared_total > MAX_SKILL_TOTAL_BYTES {
            return Err(SkillError::integrity(
                "Skill ZIP expanded size exceeds its hard limit",
            ));
        }
        raw_files.push((index, raw_path, file.size()));
    }
    if raw_files.is_empty() || raw_files.len() > MAX_SKILL_RESOURCES {
        return Err(SkillError::integrity(
            "Skill ZIP file count is outside its hard limit",
        ));
    }
    let prefix = archive_prefix(&raw_files)?;
    let mut plans = Vec::with_capacity(raw_files.len());
    let mut exact_paths = BTreeSet::new();
    let mut folded_paths = BTreeSet::new();
    for (index, raw_path, size) in raw_files {
        let relative = if let Some(prefix) = &prefix {
            raw_path.strip_prefix(prefix).map_err(|_| {
                SkillError::integrity("Skill ZIP contains files outside its root directory")
            })?
        } else {
            raw_path.as_path()
        };
        let normalized = normalized_relative_path(relative)?;
        if normalized.len() > MAX_RESOURCE_PATH_BYTES
            || relative.components().count() > MAX_RESOURCE_PATH_DEPTH
        {
            return Err(SkillError::integrity(
                "Skill resource path exceeds its hard limit",
            ));
        }
        if !exact_paths.insert(normalized.clone())
            || !folded_paths.insert(normalized.to_lowercase())
        {
            return Err(SkillError::integrity(
                "Skill ZIP contains duplicate or case-colliding paths",
            ));
        }
        plans.push(ArchiveFilePlan {
            index,
            raw_path,
            normalized_path: normalized,
            declared_size: size,
        });
    }
    if !exact_paths.contains("SKILL.md") {
        return Err(SkillError::integrity(
            "Skill ZIP must contain SKILL.md at its logical root",
        ));
    }

    let mut files = Vec::with_capacity(plans.len());
    for plan in plans {
        let mut file = archive.by_index(plan.index).map_err(|error| {
            SkillError::integrity(format!("cannot reopen Skill ZIP entry: {error}"))
        })?;
        if validate_archive_path(
            std::str::from_utf8(file.name_raw())
                .map_err(|_| SkillError::integrity("Skill ZIP paths must remain UTF-8"))?,
        )? != plan.raw_path
        {
            return Err(SkillError::integrity(
                "Skill ZIP entry changed while reading",
            ));
        }
        let mut bytes = Vec::with_capacity(plan.declared_size as usize);
        file.by_ref()
            .take(MAX_SKILL_RESOURCE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                SkillError::integrity(format!("cannot read Skill ZIP resource: {error}"))
            })?;
        if bytes.len() as u64 != plan.declared_size || bytes.len() > MAX_SKILL_RESOURCE_BYTES {
            return Err(SkillError::integrity(
                "Skill ZIP resource size changed while reading",
            ));
        }
        let mime_type = if plan.normalized_path == "SKILL.md" {
            "text/markdown".to_string()
        } else {
            mime_guess::from_path(&plan.normalized_path)
                .first_or_octet_stream()
                .essence_str()
                .to_string()
        };
        files.push(AuditedResource {
            record: SkillResourceRecord {
                path: plan.normalized_path,
                sha256: sha256_hex(&bytes),
                size_bytes: bytes.len() as u64,
                mime_type,
            },
            bytes,
        });
    }
    files.sort_by(|left, right| left.record.path.cmp(&right.record.path));
    let skill_md = files
        .iter()
        .find(|file| file.record.path == "SKILL.md")
        .ok_or_else(|| SkillError::integrity("Skill ZIP is missing SKILL.md"))?;
    if skill_md.bytes.len() > MAX_SKILL_BYTES {
        return Err(SkillError::integrity(
            "SKILL.md exceeds the 64 KiB hard limit",
        ));
    }
    let skill_text = std::str::from_utf8(&skill_md.bytes)
        .map_err(|_| SkillError::integrity("SKILL.md must be UTF-8"))?;
    let frontmatter_name = frontmatter_value(skill_text, "name");
    let fallback_id = prefix
        .as_ref()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or_else(|| source_name.trim_end_matches(".zip"));
    let id = proposed_skill_id(frontmatter_name.as_deref(), fallback_id, source_sha256)?;
    let name = parse_skill_name(skill_text)
        .or_else(|| frontmatter_name.clone())
        .unwrap_or_else(|| id.clone());
    let description = frontmatter_value(skill_text, "description")
        .or_else(|| first_description_line(skill_text))
        .unwrap_or_else(|| "Imported Skill".to_string());
    let version =
        frontmatter_value(skill_text, "version").or_else(|| parse_skill_version(skill_text));
    let resources = files
        .iter()
        .map(|file| file.record.clone())
        .collect::<Vec<_>>();
    let total_bytes = resources.iter().map(|resource| resource.size_bytes).sum();
    let tree_sha256 = tree_sha256(&resources)?;
    let mut warnings = Vec::new();
    if resources
        .iter()
        .any(|resource| resource.path.starts_with("scripts/"))
    {
        warnings.push(
            "包含 scripts/；导入与阅读不会执行脚本，运行前必须显式 materialize_skill。".to_string(),
        );
    }
    if resources.iter().any(|resource| {
        !resource.mime_type.starts_with("text/") && resource.mime_type != "application/json"
    }) {
        warnings.push("包含二进制资源；仅作为 canonical asset 保存，不直接回灌模型。".to_string());
    }
    Ok(AuditedArchive {
        entry: SkillCatalogEntry {
            id,
            name: truncate_owned(name.trim(), 256),
            description: truncate_owned(description.trim(), 1024),
            version: version.map(|value| truncate_owned(value.trim(), 128)),
            source: "imported".to_string(),
            original_name: Some(truncate_owned(source_name, 255)),
            source_sha256: source_sha256.to_string(),
            tree_sha256,
            total_bytes,
            installed_at_ms,
            resources,
        },
        files,
        warnings,
    })
}

fn publish_object(skills_root: &Path, audited: &AuditedArchive) -> Result<(), SkillError> {
    let objects_root = skills_root.join(OBJECTS_DIRECTORY);
    let target = objects_root.join(&audited.entry.tree_sha256);
    match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            validate_all_resources(skills_root, &audited.entry)?;
            return set_tree_read_only(&target);
        }
        Ok(_) => return Err(SkillError::integrity("Skill object target is invalid")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(SkillError::integrity(format!(
                "cannot inspect Skill object target: {error}"
            )))
        }
    }
    let staging = objects_root.join(format!(
        ".{}.{}.staging",
        audited.entry.tree_sha256,
        Uuid::new_v4()
    ));
    fs::create_dir(&staging).map_err(|error| {
        SkillError::integrity(format!("cannot create Skill object staging: {error}"))
    })?;
    set_mode(&staging, 0o700)?;
    let result = (|| {
        let mut directories = BTreeSet::new();
        directories.insert(staging.clone());
        for resource in &audited.files {
            let relative = Path::new(&resource.record.path);
            let destination = staging.join(relative);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    SkillError::integrity(format!("cannot create Skill object parent: {error}"))
                })?;
                directories.insert(parent.to_path_buf());
            }
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)
                .map_err(|error| {
                    SkillError::integrity(format!("cannot create Skill object resource: {error}"))
                })?;
            file.write_all(&resource.bytes).map_err(|error| {
                SkillError::integrity(format!("cannot write Skill object resource: {error}"))
            })?;
            file.sync_all().map_err(|error| {
                SkillError::integrity(format!("cannot sync Skill object resource: {error}"))
            })?;
        }
        for directory in directories.iter().rev() {
            sync_directory(directory)?;
        }
        fs::rename(&staging, &target).map_err(|error| {
            SkillError::integrity(format!("cannot publish Skill object: {error}"))
        })?;
        set_tree_read_only(&target)?;
        sync_directory(&objects_root)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn set_tree_read_only(root: &Path) -> Result<(), SkillError> {
    let mut pending = vec![root.to_path_buf()];
    let mut directories = Vec::new();
    while let Some(directory) = pending.pop() {
        directories.push(directory.clone());
        for entry in fs::read_dir(&directory).map_err(|error| {
            SkillError::integrity(format!("cannot scan published Skill object: {error}"))
        })? {
            let entry = entry.map_err(|error| {
                SkillError::integrity(format!("cannot read published Skill object: {error}"))
            })?;
            let kind = entry.file_type().map_err(|error| {
                SkillError::integrity(format!("cannot inspect published Skill object: {error}"))
            })?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                set_mode(&entry.path(), 0o444)?;
            } else {
                return Err(SkillError::integrity(
                    "published Skill object contains a non-regular entry",
                ));
            }
        }
    }
    directories.sort();
    directories.reverse();
    for directory in directories {
        set_mode(&directory, 0o555)?;
    }
    Ok(())
}

fn archive_prefix(files: &[(usize, PathBuf, u64)]) -> Result<Option<PathBuf>, SkillError> {
    if files
        .iter()
        .any(|(_, path, _)| path == Path::new("SKILL.md"))
    {
        return Ok(None);
    }
    let first = files
        .first()
        .and_then(|(_, path, _)| path.components().next())
        .and_then(|component| match component {
            Component::Normal(value) => Some(PathBuf::from(value)),
            _ => None,
        })
        .ok_or_else(|| SkillError::integrity("Skill ZIP root is invalid"))?;
    if files.iter().any(|(_, path, _)| !path.starts_with(&first))
        || !files
            .iter()
            .any(|(_, path, _)| path == &first.join("SKILL.md"))
    {
        return Err(SkillError::integrity(
            "Skill ZIP must use one root directory containing SKILL.md",
        ));
    }
    Ok(Some(first))
}

fn validate_archive_path(value: &str) -> Result<PathBuf, SkillError> {
    if value.is_empty()
        || value.contains(['\\', '\0'])
        || value.len() > MAX_RESOURCE_PATH_BYTES + 128
    {
        return Err(SkillError::integrity("Skill ZIP path is invalid"));
    }
    let path = Path::new(value);
    if path.is_absolute() {
        return Err(SkillError::integrity("Skill ZIP path must be relative"));
    }
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => clean.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(SkillError::integrity("Skill ZIP path contains traversal"))
            }
        }
    }
    if clean.as_os_str().is_empty() {
        return Err(SkillError::integrity("Skill ZIP path is empty"));
    }
    Ok(clean)
}

fn validate_archive_kind(file: &zip::read::ZipFile<'_, File>) -> Result<(), SkillError> {
    if let Some(mode) = file.unix_mode() {
        let kind = mode & libc::S_IFMT;
        let expected = if file.is_dir() {
            libc::S_IFDIR
        } else {
            libc::S_IFREG
        };
        if kind != 0 && kind != expected {
            return Err(SkillError::integrity(
                "Skill ZIP contains a symlink or special file",
            ));
        }
    }
    Ok(())
}

fn copy_verified_candidate(
    source_path: &Path,
    destination: &Path,
    picked: &PickedSkillImportFile,
) -> Result<(), SkillError> {
    let mut source = open_regular_nofollow(source_path, MAX_IMPORT_ARCHIVE_BYTES)?;
    let mut destination_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| {
            SkillError::integrity(format!("cannot create owned Skill candidate: {error}"))
        })?;
    set_mode(destination, 0o600)?;
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let mut buffer = vec![0u8; COPY_BUFFER_BYTES];
    loop {
        let read = source.read(&mut buffer).map_err(|error| {
            SkillError::integrity(format!("cannot read picked Skill file: {error}"))
        })?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| SkillError::integrity("picked Skill size overflow"))?;
        if total > MAX_IMPORT_ARCHIVE_BYTES {
            return Err(SkillError::integrity(
                "picked Skill archive exceeds the 8 MiB hard limit",
            ));
        }
        digest.update(&buffer[..read]);
        destination_file
            .write_all(&buffer[..read])
            .map_err(|error| {
                SkillError::integrity(format!("cannot copy picked Skill file: {error}"))
            })?;
    }
    if total != picked.size
        || crate::vcp_modules::infra::utils::finalize_sha256_hex(digest) != picked.hash
    {
        return Err(SkillError::integrity(
            "picked Skill size or SHA-256 does not match the native picker receipt",
        ));
    }
    destination_file.sync_all().map_err(|error| {
        SkillError::integrity(format!("cannot sync owned Skill candidate: {error}"))
    })
}

fn verify_regular_file_hash(
    path: &Path,
    expected_bytes: u64,
    expected_sha256: &str,
    max_bytes: u64,
) -> Result<(), SkillError> {
    let mut file = open_regular_nofollow(path, max_bytes)?;
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let mut buffer = vec![0u8; COPY_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            SkillError::integrity(format!("cannot read owned Skill candidate: {error}"))
        })?;
        if read == 0 {
            break;
        }
        total += read as u64;
        digest.update(&buffer[..read]);
    }
    if total != expected_bytes
        || crate::vcp_modules::infra::utils::finalize_sha256_hex(digest) != expected_sha256
    {
        return Err(SkillError::integrity(
            "owned Skill candidate integrity failed",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn open_regular_nofollow(path: &Path, max_bytes: u64) -> Result<File, SkillError> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| SkillError::integrity("Skill file path contains NUL"))?;
    let descriptor = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if descriptor < 0 {
        return Err(SkillError::integrity(format!(
            "cannot securely open Skill file: {}",
            io::Error::last_os_error()
        )));
    }
    let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(descriptor.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(SkillError::integrity(format!(
            "cannot inspect opened Skill file: {}",
            io::Error::last_os_error()
        )));
    }
    let stat = unsafe { stat.assume_init() };
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG
        || stat.st_size < 0
        || stat.st_size as u64 > max_bytes
    {
        return Err(SkillError::integrity(
            "Skill file must be a bounded real regular file",
        ));
    }
    Ok(File::from(descriptor))
}

#[cfg(not(unix))]
fn open_regular_nofollow(_path: &Path, _max_bytes: u64) -> Result<File, SkillError> {
    Err(SkillError::integrity(
        "secure Skill import is unavailable on this host",
    ))
}

fn write_inspection(root: &Path, inspection: &PersistedInspection) -> Result<(), SkillError> {
    let bytes = serde_json::to_vec_pretty(inspection).map_err(|error| {
        SkillError::integrity(format!("cannot encode Skill inspection: {error}"))
    })?;
    if bytes.len() > MAX_IMPORT_INSPECTION_BYTES as usize {
        return Err(SkillError::integrity(
            "Skill inspection manifest exceeds its hard limit",
        ));
    }
    let path = root.join(INSPECTION_FILE_NAME);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| {
            SkillError::integrity(format!("cannot create Skill inspection manifest: {error}"))
        })?;
    set_mode(&path, 0o600)?;
    file.write_all(&bytes).map_err(|error| {
        SkillError::integrity(format!("cannot write Skill inspection manifest: {error}"))
    })?;
    file.sync_all().map_err(|error| {
        SkillError::integrity(format!("cannot sync Skill inspection manifest: {error}"))
    })
}

fn read_inspection(root: &Path) -> Result<PersistedInspection, SkillError> {
    let metadata = fs::symlink_metadata(root).map_err(|error| {
        SkillError::integrity(format!("cannot inspect Skill import root: {error}"))
    })?;
    if !metadata.file_type().is_dir() {
        return Err(SkillError::integrity(
            "Skill import root must be a real directory",
        ));
    }
    let path = root.join(INSPECTION_FILE_NAME);
    let mut file = open_regular_nofollow(&path, MAX_IMPORT_INSPECTION_BYTES)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| SkillError::integrity(format!("cannot read Skill inspection: {error}")))?;
    let inspection = serde_json::from_slice::<PersistedInspection>(&bytes)
        .map_err(|error| SkillError::integrity(format!("invalid Skill inspection: {error}")))?;
    if inspection.schema_version != IMPORT_SCHEMA_VERSION {
        return Err(SkillError::integrity("unsupported Skill inspection schema"));
    }
    Ok(inspection)
}

fn find_inspection_by_operation(
    imports_root: &Path,
    operation_id: &str,
) -> Result<Option<PersistedInspection>, SkillError> {
    for entry in fs::read_dir(imports_root).map_err(|error| {
        SkillError::integrity(format!("cannot scan pending Skill imports: {error}"))
    })? {
        let entry = entry.map_err(|error| {
            SkillError::integrity(format!("cannot read pending Skill import: {error}"))
        })?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let inspection = read_inspection(&entry.path())?;
        if inspection.operation_id == operation_id {
            return Ok(Some(inspection));
        }
    }
    Ok(None)
}

fn candidate_from_inspection(
    inspection: &PersistedInspection,
    catalog: &super::skill_catalog::SkillCatalog,
) -> SkillImportCandidate {
    SkillImportCandidate {
        token: inspection.token.clone(),
        candidate_sha256: inspection.candidate_sha256.clone(),
        catalog_generation: inspection.catalog_generation,
        skill_id: inspection.entry.id.clone(),
        name: inspection.entry.name.clone(),
        description: inspection.entry.description.clone(),
        version: inspection.entry.version.clone(),
        source_name: inspection.source_name.clone(),
        resource_count: inspection.entry.resources.len(),
        total_bytes: inspection.entry.total_bytes,
        tree_sha256: inspection.entry.tree_sha256.clone(),
        replaces_existing: catalog
            .skills
            .iter()
            .any(|entry| entry.id == inspection.entry.id),
        warnings: inspection.warnings.clone(),
    }
}

fn validate_source_metadata(picked: &PickedSkillImportFile) -> Result<(), SkillError> {
    if picked.name.is_empty()
        || picked.name.len() > 255
        || picked.name.contains(['/', '\\', '\0'])
        || picked.mime.len() > 128
        || picked.size == 0
        || picked.size > MAX_IMPORT_ARCHIVE_BYTES
    {
        return Err(SkillError::integrity(
            "native picker Skill receipt is outside its hard limits",
        ));
    }
    validate_sha256(&picked.hash, "picker hash")
}

fn validate_sha256(value: &str, field: &str) -> Result<(), SkillError> {
    if value.len() != 64
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err(SkillError::integrity(format!(
            "{field} must be lowercase SHA-256"
        )));
    }
    Ok(())
}

fn validate_operation_id(value: &str) -> Result<(), SkillError> {
    if value.is_empty()
        || value.len() > 128
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_alphanumeric() && !b"-_.:".contains(&byte))
    {
        return Err(SkillError::integrity("invalid Skill import operation_id"));
    }
    Ok(())
}

fn parse_token(token: &str) -> Result<String, SkillError> {
    let id = token
        .strip_prefix(TOKEN_PREFIX)
        .ok_or_else(|| SkillError::integrity("invalid Skill import token"))?;
    let parsed =
        Uuid::parse_str(id).map_err(|_| SkillError::integrity("invalid Skill import token"))?;
    if parsed.to_string() != id {
        return Err(SkillError::integrity("invalid Skill import token"));
    }
    Ok(id.to_string())
}

fn proposed_skill_id(
    frontmatter_name: Option<&str>,
    fallback: &str,
    source_sha256: &str,
) -> Result<String, SkillError> {
    if let Some(name) = frontmatter_name {
        if validate_skill_id(name).is_ok() {
            return Ok(name.to_string());
        }
    }
    let mut output = String::new();
    let mut separator = false;
    for character in fallback.chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !output.is_empty() {
                output.push('-');
            }
            output.push(character.to_ascii_lowercase());
            separator = false;
        } else {
            separator = true;
        }
        if output.len() >= 64 {
            break;
        }
    }
    while output.ends_with('-') {
        output.pop();
    }
    if output.is_empty() {
        output = format!("skill-{}", &source_sha256[..12]);
    }
    validate_skill_id(&output)?;
    Ok(output)
}

fn frontmatter_value(text: &str, key: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    for line in lines {
        let line = line.trim();
        if line == "---" {
            break;
        }
        let Some((field, value)) = line.split_once(':') else {
            continue;
        };
        if field.trim() == key {
            let value = value.trim().trim_matches(['\'', '"']);
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn first_description_line(text: &str) -> Option<String> {
    let mut in_frontmatter = false;
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if index == 0 && line == "---" {
            in_frontmatter = true;
            continue;
        }
        if in_frontmatter {
            if line == "---" {
                in_frontmatter = false;
            }
            continue;
        }
        if !line.is_empty() && !line.starts_with('#') && !line.starts_with("Version:") {
            return Some(line.to_string());
        }
    }
    None
}

fn commit_request_sha256(request: &CommitSkillImportRequest) -> String {
    let mut digest = Sha256::new();
    for value in [
        request.token.as_bytes(),
        request.candidate_sha256.as_bytes(),
        &request.expected_catalog_generation.to_le_bytes(),
    ] {
        digest.update((value.len() as u64).to_le_bytes());
        digest.update(value);
    }
    crate::vcp_modules::infra::utils::finalize_sha256_hex(digest)
}

fn import_count(imports_root: &Path) -> Result<usize, SkillError> {
    let mut count = 0usize;
    for entry in fs::read_dir(imports_root)
        .map_err(|error| SkillError::integrity(format!("cannot scan Skill imports: {error}")))?
    {
        let entry = entry.map_err(|error| {
            SkillError::integrity(format!("cannot read pending Skill import: {error}"))
        })?;
        let kind = entry.file_type().map_err(|error| {
            SkillError::integrity(format!("cannot inspect pending Skill import: {error}"))
        })?;
        if !kind.is_dir() || !is_managed_import_directory_name(&entry.file_name()) {
            return Err(SkillError::integrity(
                "Skill imports root contains an unmanaged entry",
            ));
        }
        count = count.saturating_add(1);
    }
    Ok(count)
}

fn gc_stale_imports(imports_root: &Path) -> Result<(), SkillError> {
    let now = SystemTime::now();
    for entry in fs::read_dir(imports_root).map_err(|error| {
        SkillError::integrity(format!("cannot scan stale Skill imports: {error}"))
    })? {
        let entry = entry.map_err(|error| {
            SkillError::integrity(format!("cannot read stale Skill import: {error}"))
        })?;
        let kind = entry.file_type().map_err(|error| {
            SkillError::integrity(format!("cannot inspect stale Skill import: {error}"))
        })?;
        if !kind.is_dir() || !is_managed_import_directory_name(&entry.file_name()) {
            return Err(SkillError::integrity(
                "Skill imports root contains an unmanaged entry",
            ));
        }
        let modified = entry.metadata().and_then(|metadata| metadata.modified());
        if modified
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > STALE_IMPORT_AGE)
        {
            fs::remove_dir_all(entry.path()).map_err(|error| {
                SkillError::integrity(format!("cannot remove stale Skill import: {error}"))
            })?;
        }
    }
    Ok(())
}

fn is_managed_import_directory_name(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    if Uuid::parse_str(name).is_ok() {
        return true;
    }
    name.strip_prefix('.')
        .and_then(|value| value.strip_suffix(".staging"))
        .is_some_and(|value| Uuid::parse_str(value).is_ok())
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
            SkillError::integrity(format!("cannot set Skill import permissions: {error}"))
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

fn truncate_owned(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::super::skill_catalog::{ensure_skill_catalog, read_skill_v2};
    use super::*;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    fn write_zip(path: &Path, entries: &[(&str, &[u8], u32)]) {
        let file = File::create(path).expect("create ZIP");
        let mut zip = ZipWriter::new(file);
        for (name, bytes, mode) in entries {
            zip.start_file(
                name,
                SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated)
                    .unix_permissions(*mode),
            )
            .expect("start ZIP file");
            zip.write_all(bytes).expect("write ZIP file");
        }
        zip.finish().expect("finish ZIP");
    }

    fn write_zip_with_symlink(path: &Path) {
        let file = File::create(path).expect("create symlink ZIP");
        let mut zip = ZipWriter::new(file);
        zip.start_file(
            "skill/SKILL.md",
            SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(0o644),
        )
        .expect("start SKILL.md");
        zip.write_all(b"# Skill\n").expect("write SKILL.md");
        zip.add_symlink(
            "skill/scripts/link",
            "../SKILL.md",
            SimpleFileOptions::default().unix_permissions(0o777),
        )
        .expect("add symlink");
        zip.finish().expect("finish symlink ZIP");
    }

    fn picked(path: &Path, name: &str) -> PickedSkillImportFile {
        let bytes = fs::read(path).expect("read ZIP");
        PickedSkillImportFile {
            path: path.to_string_lossy().to_string(),
            name: name.to_string(),
            mime: "application/zip".to_string(),
            size: bytes.len() as u64,
            hash: sha256_hex(&bytes),
        }
    }

    #[test]
    fn inspect_commit_and_replay_are_generation_fenced() {
        let directory = tempfile::tempdir().expect("temporary root");
        let skills = directory.path().join("skills");
        fs::create_dir(&skills).expect("create Skills root");
        ensure_skill_catalog(&skills, 1).expect("create catalog");
        let source = directory.path().join("sample.zip");
        write_zip(
            &source,
            &[
                (
                    "sample/SKILL.md",
                    b"---\nname: sample-skill\ndescription: Safe sample\nversion: 1.2.3\n---\n# Sample Skill\n",
                    0o100644,
                ),
                ("sample/scripts/run.sh", b"printf safe\n", 0o100755),
            ],
        );
        let picked = picked(&source, "sample.zip");
        let candidate = inspect_skill_import(
            &skills,
            &source,
            &InspectSkillImportRequest {
                operation_id: "inspect-1".to_string(),
                picked: picked.clone(),
            },
            10,
        )
        .expect("inspect Skill");
        assert_eq!(candidate.skill_id, "sample-skill");
        assert!(candidate
            .warnings
            .iter()
            .any(|warning| warning.contains("scripts/")));
        let request = CommitSkillImportRequest {
            operation_id: "commit-1".to_string(),
            token: candidate.token,
            candidate_sha256: picked.hash,
            expected_catalog_generation: candidate.catalog_generation,
        };
        let committed = commit_skill_import(&skills, &request, 20).expect("commit Skill");
        assert!(!committed.replayed);
        let replay = commit_skill_import(&skills, &request, 21).expect("replay commit");
        assert!(replay.replayed);
        assert_eq!(replay.skill, committed.skill);
        let (_, text) = read_skill_v2(&skills, "sample-skill", "SKILL.md", MAX_SKILL_BYTES)
            .expect("read imported Skill");
        assert!(text.contains("# Sample Skill"));
    }

    #[test]
    fn commit_rejects_stale_catalog_generation_without_replacing_existing_state() {
        let directory = tempfile::tempdir().expect("temporary root");
        let skills = directory.path().join("skills");
        fs::create_dir(&skills).expect("create Skills root");
        ensure_skill_catalog(&skills, 1).expect("create catalog");
        let first_source = directory.path().join("first.zip");
        let second_source = directory.path().join("second.zip");
        write_zip(&first_source, &[("first/SKILL.md", b"# First\n", 0o100644)]);
        write_zip(
            &second_source,
            &[("second/SKILL.md", b"# Second\n", 0o100644)],
        );
        let first_picked = picked(&first_source, "first.zip");
        let second_picked = picked(&second_source, "second.zip");
        let first = inspect_skill_import(
            &skills,
            &first_source,
            &InspectSkillImportRequest {
                operation_id: "inspect-first".to_string(),
                picked: first_picked.clone(),
            },
            2,
        )
        .expect("inspect first");
        let second = inspect_skill_import(
            &skills,
            &second_source,
            &InspectSkillImportRequest {
                operation_id: "inspect-second".to_string(),
                picked: second_picked.clone(),
            },
            3,
        )
        .expect("inspect second");
        commit_skill_import(
            &skills,
            &CommitSkillImportRequest {
                operation_id: "commit-second".to_string(),
                token: second.token,
                candidate_sha256: second_picked.hash,
                expected_catalog_generation: second.catalog_generation,
            },
            4,
        )
        .expect("commit second");
        let error = commit_skill_import(
            &skills,
            &CommitSkillImportRequest {
                operation_id: "commit-first".to_string(),
                token: first.token,
                candidate_sha256: first_picked.hash,
                expected_catalog_generation: first.catalog_generation,
            },
            5,
        )
        .expect_err("stale generation must fail");
        assert!(error.message.contains("generation changed"));
    }

    #[test]
    fn traversal_and_symlink_entries_fail_before_catalog_mutation() {
        let directory = tempfile::tempdir().expect("temporary root");
        let traversal = directory.path().join("traversal.zip");
        write_zip(&traversal, &[("../SKILL.md", b"# Escape\n", 0o100644)]);
        let error = audit_archive(
            &traversal,
            "traversal.zip",
            &picked(&traversal, "traversal.zip").hash,
            1,
        )
        .expect_err("traversal must fail");
        assert!(error.message.contains("traversal"));

        let symlink = directory.path().join("symlink.zip");
        write_zip_with_symlink(&symlink);
        let error = audit_archive(
            &symlink,
            "symlink.zip",
            &picked(&symlink, "symlink.zip").hash,
            1,
        )
        .expect_err("symlink must fail");
        assert!(error.message.contains("symlink or special"));
    }
}
