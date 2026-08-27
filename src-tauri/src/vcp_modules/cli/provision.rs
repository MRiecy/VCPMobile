//! P1 rootfs provision：staged 资产校验后双遍扫描、安全展开、marker 最后提交。

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::profile::{
    embedded_command_profile, verify_staged_runtime_assets, ProfileValidationError,
    VcpCliCommandProfile,
};

const PROVISION_MARKER: &str = ".vcp-cli-profile.json";
const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const MAX_STALE_STAGING_DIRECTORIES: usize = 8;
const COPY_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionPaths {
    pub rootfs_archive: PathBuf,
    pub proot_binary: PathBuf,
    pub proot_loader: PathBuf,
    pub rootfs_parent: PathBuf,
    pub workspace: PathBuf,
    pub skills: PathBuf,
    pub output: PathBuf,
    pub projection_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionedRuntime {
    pub profile: VcpCliCommandProfile,
    pub rootfs: PathBuf,
    pub workspace: PathBuf,
    pub skills: PathBuf,
    pub output: PathBuf,
    pub projection_root: PathBuf,
    pub proot_binary: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionError {
    pub field: String,
    pub message: String,
}

impl ProvisionError {
    fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ProvisionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.field, self.message)
    }
}

impl std::error::Error for ProvisionError {}

impl From<ProfileValidationError> for ProvisionError {
    fn from(error: ProfileValidationError) -> Self {
        Self::new(error.field, error.message)
    }
}

/// async 边界仅负责流式校验；解压必须由调用方放入 `spawn_blocking`。
pub async fn verify_staged_provision_inputs(
    paths: &ProvisionPaths,
) -> Result<VcpCliCommandProfile, ProvisionError> {
    let profile = embedded_command_profile()?;
    verify_staged_runtime_assets(
        &profile,
        paths.rootfs_archive.clone(),
        paths.proot_binary.clone(),
        paths.proot_loader.clone(),
    )
    .await?;
    Ok(profile)
}

/// 阻塞双遍展开：第一遍只建 manifest/校验 hardlink graph；第二遍才写临时树。
/// 所有 hardlink 都物化成普通文件副本，避免 Android app domain 的 hard_link 拒绝。
pub fn provision_verified_runtime_blocking(
    paths: ProvisionPaths,
    profile: VcpCliCommandProfile,
) -> Result<ProvisionedRuntime, ProvisionError> {
    ensure_private_directory(&paths.rootfs_parent, "rootfsParent")?;
    ensure_private_directory(&paths.workspace, "workspace")?;
    ensure_private_directory(&paths.skills, "skills")?;
    ensure_private_directory(&paths.output, "output")?;
    ensure_private_directory(&paths.projection_root, "projectionRoot")?;

    cleanup_stale_staging_directories(&paths.rootfs_parent, &profile.profile_id)?;
    let final_root = paths.rootfs_parent.join(&profile.profile_id);
    let marker = final_root.join(PROVISION_MARKER);
    match fs::symlink_metadata(&final_root) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            if marker_is_current(&marker, &profile)? {
                return Ok(ProvisionedRuntime {
                    profile,
                    rootfs: final_root,
                    workspace: paths.workspace,
                    skills: paths.skills,
                    output: paths.output,
                    projection_root: paths.projection_root,
                    proot_binary: paths.proot_binary,
                });
            }
            return Err(ProvisionError::new(
                "rootfs.final",
                "final rootfs exists without a valid completion marker",
            ));
        }
        Ok(_) => {
            return Err(ProvisionError::new(
                "rootfs.final",
                "final rootfs must be a real directory",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(ProvisionError::new(
                "rootfs.final",
                format!("cannot inspect final rootfs: {error}"),
            ));
        }
    }

    let entries = scan_archive(
        &paths.rootfs_archive,
        profile.rootfs.archive_bytes,
        &profile.rootfs.archive_sha256,
        &profile.rootfs.tar_content_sha256,
        profile.rootfs.logical_bytes,
    )?;
    let temporary_root = paths.rootfs_parent.join(format!(
        ".{}-{}.staging",
        profile.profile_id,
        Uuid::new_v4()
    ));
    fs::create_dir(&temporary_root).map_err(|error| {
        ProvisionError::new(
            "rootfs.staging",
            format!("cannot create staging root: {error}"),
        )
    })?;

    set_mode(&temporary_root, 0o700)?;
    let result = extract_archive(
        &paths.rootfs_archive,
        &temporary_root,
        &entries,
        profile.rootfs.archive_bytes,
        &profile.rootfs.archive_sha256,
        &profile.rootfs.tar_content_sha256,
    )
    .and_then(|()| sync_directory(&temporary_root, "rootfs.staging"))
    .and_then(|()| write_marker(&temporary_root, &profile))
    .and_then(|()| sync_directory(&temporary_root, "rootfs.staging"))
    .and_then(|()| {
        fs::rename(&temporary_root, &final_root).map_err(|error| {
            ProvisionError::new(
                "rootfs.final",
                format!("atomic final rename failed: {error}"),
            )
        })
    })
    .and_then(|()| sync_directory(&paths.rootfs_parent, "rootfsParent"));
    if result.is_err() {
        let _ = fs::remove_dir_all(&temporary_root);
    }
    result?;

    Ok(ProvisionedRuntime {
        profile,
        rootfs: final_root,
        workspace: paths.workspace,
        skills: paths.skills,
        output: paths.output,
        projection_root: paths.projection_root,
        proot_binary: paths.proot_binary,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ArchiveKind {
    Directory,
    Regular,
    Symlink(PathBuf),
    Hardlink(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArchiveEntryPlan {
    path: PathBuf,
    kind: ArchiveKind,
    size: u64,
    mode: u32,
}

fn scan_archive(
    archive_path: &Path,
    expected_archive_bytes: u64,
    expected_archive_sha256: &str,
    expected_tar_sha256: &str,
    logical_limit: u64,
) -> Result<BTreeMap<PathBuf, ArchiveEntryPlan>, ProvisionError> {
    let file = open_verified_archive(
        archive_path,
        expected_archive_bytes,
        expected_archive_sha256,
    )?;
    let decoder = zstd::stream::read::Decoder::new(file).map_err(|error| {
        ProvisionError::new("rootfs.archive", format!("invalid zstd stream: {error}"))
    })?;
    let hashing_reader = HashingReader::new(decoder);
    let mut archive = tar::Archive::new(hashing_reader);
    let mut plans = BTreeMap::new();
    let mut logical_bytes = 0_u64;

    let entries = archive.entries().map_err(|error| {
        ProvisionError::new("rootfs.archive", format!("cannot scan tar: {error}"))
    })?;
    for (index, entry) in entries.enumerate() {
        if index >= MAX_ARCHIVE_ENTRIES {
            return Err(ProvisionError::new(
                "rootfs.archive",
                "archive entry limit exceeded",
            ));
        }
        let entry = entry.map_err(|error| {
            ProvisionError::new("rootfs.archive", format!("invalid tar entry: {error}"))
        })?;
        let path = sanitize_archive_path(&entry.path().map_err(|error| {
            ProvisionError::new("rootfs.archive", format!("invalid entry path: {error}"))
        })?)?;
        if plans.contains_key(&path) {
            return Err(ProvisionError::new(
                "rootfs.archive",
                format!("duplicate archive path: {}", path.display()),
            ));
        }
        let entry_type = entry.header().entry_type();
        let size = entry.size();
        let mode = entry.header().mode().unwrap_or(0o644) & 0o777;
        let kind = if entry_type.is_dir() {
            ArchiveKind::Directory
        } else if entry_type.is_file() {
            logical_bytes = logical_bytes
                .checked_add(size)
                .ok_or_else(|| ProvisionError::new("rootfs.archive", "logical size overflow"))?;
            ArchiveKind::Regular
        } else if entry_type.is_symlink() {
            let target = entry
                .link_name()
                .map_err(|error| {
                    ProvisionError::new("rootfs.archive", format!("invalid symlink: {error}"))
                })?
                .ok_or_else(|| ProvisionError::new("rootfs.archive", "missing symlink target"))?;
            validate_symlink_target(&path, &target)?;
            ArchiveKind::Symlink(target.into_owned())
        } else if entry_type.is_hard_link() {
            let target = entry
                .link_name()
                .map_err(|error| {
                    ProvisionError::new("rootfs.archive", format!("invalid hardlink: {error}"))
                })?
                .ok_or_else(|| ProvisionError::new("rootfs.archive", "missing hardlink target"))?;
            ArchiveKind::Hardlink(sanitize_archive_path(&target)?)
        } else {
            return Err(ProvisionError::new(
                "rootfs.archive",
                format!("special tar entry rejected: {}", path.display()),
            ));
        };
        plans.insert(
            path.clone(),
            ArchiveEntryPlan {
                path,
                kind,
                size,
                mode,
            },
        );
    }
    let mut hashing_reader = archive.into_inner();
    io::copy(&mut hashing_reader, &mut io::sink()).map_err(|error| {
        ProvisionError::new(
            "rootfs.archive",
            format!("cannot finish tar content hash: {error}"),
        )
    })?;
    verify_tar_content_hash(hashing_reader, expected_tar_sha256)?;
    validate_hardlink_graph(&plans)?;
    for plan in plans
        .values()
        .filter(|plan| matches!(plan.kind, ArchiveKind::Hardlink(_)))
    {
        let materialized_bytes = resolve_hardlink_source(plan, &plans)?.size;
        logical_bytes = logical_bytes
            .checked_add(materialized_bytes)
            .ok_or_else(|| {
                ProvisionError::new("rootfs.archive", "materialized logical size overflow")
            })?;
    }
    if logical_bytes > logical_limit {
        return Err(ProvisionError::new(
            "rootfs.archive",
            "materialized archive size exceeds profile logical limit",
        ));
    }
    Ok(plans)
}

fn validate_hardlink_graph(
    plans: &BTreeMap<PathBuf, ArchiveEntryPlan>,
) -> Result<(), ProvisionError> {
    for (path, plan) in plans {
        if !matches!(plan.kind, ArchiveKind::Hardlink(_)) {
            continue;
        }
        let mut current = path.as_path();
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert(current.to_path_buf()) {
                return Err(ProvisionError::new(
                    "rootfs.archive",
                    format!("hardlink cycle at {}", path.display()),
                ));
            }
            let current_plan = plans.get(current).ok_or_else(|| {
                ProvisionError::new(
                    "rootfs.archive",
                    format!("hardlink target missing for {}", path.display()),
                )
            })?;
            match &current_plan.kind {
                ArchiveKind::Regular => break,
                ArchiveKind::Hardlink(target) => current = target,
                _ => {
                    return Err(ProvisionError::new(
                        "rootfs.archive",
                        format!("hardlink target is not a regular file: {}", path.display()),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn extract_archive(
    archive_path: &Path,
    staging: &Path,
    plans: &BTreeMap<PathBuf, ArchiveEntryPlan>,
    expected_archive_bytes: u64,
    expected_archive_sha256: &str,
    expected_tar_sha256: &str,
) -> Result<(), ProvisionError> {
    for plan in plans
        .values()
        .filter(|plan| matches!(plan.kind, ArchiveKind::Directory))
    {
        create_safe_directory(staging, &plan.path, 0o700)?;
    }

    let file = open_verified_archive(
        archive_path,
        expected_archive_bytes,
        expected_archive_sha256,
    )?;
    let decoder = zstd::stream::read::Decoder::new(file).map_err(|error| {
        ProvisionError::new("rootfs.archive", format!("invalid zstd stream: {error}"))
    })?;
    let hashing_reader = HashingReader::new(decoder);
    let mut archive = tar::Archive::new(hashing_reader);
    let mut seen = BTreeSet::new();
    for entry in archive.entries().map_err(|error| {
        ProvisionError::new("rootfs.archive", format!("cannot extract tar: {error}"))
    })? {
        let mut entry = entry.map_err(|error| {
            ProvisionError::new("rootfs.archive", format!("invalid tar entry: {error}"))
        })?;
        let path = sanitize_archive_path(&entry.path().map_err(|error| {
            ProvisionError::new("rootfs.archive", format!("invalid entry path: {error}"))
        })?)?;
        let plan = plans.get(&path).ok_or_else(|| {
            ProvisionError::new(
                "rootfs.archive",
                "archive changed between validation passes",
            )
        })?;
        if !seen.insert(path.clone()) {
            return Err(ProvisionError::new(
                "rootfs.archive",
                "archive contains a duplicate path on the extraction pass",
            ));
        }
        match &plan.kind {
            ArchiveKind::Directory | ArchiveKind::Hardlink(_) => {}
            ArchiveKind::Regular => write_regular_entry(staging, plan, &mut entry)?,
            ArchiveKind::Symlink(target) => write_symlink_entry(staging, plan, target)?,
        }
    }
    if seen.len() != plans.len() {
        return Err(ProvisionError::new(
            "rootfs.archive",
            "archive changed between validation passes",
        ));
    }
    let mut hashing_reader = archive.into_inner();
    io::copy(&mut hashing_reader, &mut io::sink()).map_err(|error| {
        ProvisionError::new(
            "rootfs.archive",
            format!("cannot finish extraction tar hash: {error}"),
        )
    })?;
    verify_tar_content_hash(hashing_reader, expected_tar_sha256)?;
    // 第二遍已完全读取后再复核压缩实物，阻止两遍之间或读取期间的替换被提交。
    drop(open_verified_archive(
        archive_path,
        expected_archive_bytes,
        expected_archive_sha256,
    )?);

    // Android app domain 不允许建立 tar hardlink；从已验证的最终普通文件复制。
    for plan in plans
        .values()
        .filter(|plan| matches!(plan.kind, ArchiveKind::Hardlink(_)))
    {
        let source_plan = resolve_hardlink_source(plan, plans)?;
        let source = checked_destination(staging, &source_plan.path)?;
        let destination = checked_destination(staging, &plan.path)?;
        ensure_parent_directories(staging, &plan.path)?;
        copy_regular_file(&source, &destination, source_plan.size, plan.mode)?;
    }
    // 目录权限最后按深度逆序收紧，避免只读目录阻断后续子项创建。
    let mut directories = plans
        .values()
        .filter(|plan| matches!(plan.kind, ArchiveKind::Directory))
        .collect::<Vec<_>>();
    directories.sort_by_key(|plan| std::cmp::Reverse(plan.path.components().count()));
    for plan in directories {
        if !plan.path.as_os_str().is_empty() {
            set_mode(&checked_destination(staging, &plan.path)?, plan.mode)?;
        }
    }
    Ok(())
}

fn resolve_hardlink_source<'a>(
    plan: &'a ArchiveEntryPlan,
    plans: &'a BTreeMap<PathBuf, ArchiveEntryPlan>,
) -> Result<&'a ArchiveEntryPlan, ProvisionError> {
    let mut current = plan;
    loop {
        match &current.kind {
            ArchiveKind::Regular => return Ok(current),
            ArchiveKind::Hardlink(target) => {
                current = plans.get(target).ok_or_else(|| {
                    ProvisionError::new("rootfs.archive", "validated hardlink target disappeared")
                })?;
            }
            _ => {
                return Err(ProvisionError::new(
                    "rootfs.archive",
                    "hardlink target is not regular",
                ));
            }
        }
    }
}

fn write_regular_entry<R: Read>(
    staging: &Path,
    plan: &ArchiveEntryPlan,
    entry: &mut tar::Entry<'_, R>,
) -> Result<(), ProvisionError> {
    ensure_parent_directories(staging, &plan.path)?;
    let destination = checked_destination(staging, &plan.path)?;
    reject_existing_path(&destination)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .map_err(|error| {
            ProvisionError::new(
                "rootfs.archive",
                format!("cannot create {}: {error}", destination.display()),
            )
        })?;
    let copied = io::copy(entry, &mut file).map_err(|error| {
        ProvisionError::new(
            "rootfs.archive",
            format!("cannot write {}: {error}", destination.display()),
        )
    })?;
    if copied != plan.size {
        return Err(ProvisionError::new(
            "rootfs.archive",
            format!("entry size changed for {}", plan.path.display()),
        ));
    }
    file.sync_all().map_err(|error| {
        ProvisionError::new(
            "rootfs.archive",
            format!("cannot sync {}: {error}", destination.display()),
        )
    })?;
    set_mode(&destination, plan.mode)?;
    Ok(())
}

fn write_symlink_entry(
    staging: &Path,
    plan: &ArchiveEntryPlan,
    target: &Path,
) -> Result<(), ProvisionError> {
    ensure_parent_directories(staging, &plan.path)?;
    let destination = checked_destination(staging, &plan.path)?;
    reject_existing_path(&destination)?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, &destination).map_err(|error| {
            ProvisionError::new(
                "rootfs.archive",
                format!("cannot create symlink {}: {error}", destination.display()),
            )
        })?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = target;
        Err(ProvisionError::new(
            "rootfs.archive",
            "symlink extraction requires an Android/Unix host",
        ))
    }
}

fn copy_regular_file(
    source: &Path,
    destination: &Path,
    expected_bytes: u64,
    mode: u32,
) -> Result<(), ProvisionError> {
    let source_meta = fs::symlink_metadata(source).map_err(|error| {
        ProvisionError::new(
            "rootfs.archive",
            format!("cannot inspect hardlink source: {error}"),
        )
    })?;
    if !source_meta.file_type().is_file() || source_meta.len() != expected_bytes {
        return Err(ProvisionError::new(
            "rootfs.archive",
            "hardlink source is not the verified regular file",
        ));
    }
    reject_existing_path(destination)?;
    let mut input = File::open(source).map_err(|error| {
        ProvisionError::new(
            "rootfs.archive",
            format!("cannot open hardlink source: {error}"),
        )
    })?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| {
            ProvisionError::new(
                "rootfs.archive",
                format!("cannot materialize hardlink: {error}"),
            )
        })?;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut copied = 0_u64;
    loop {
        let read = input.read(&mut buffer).map_err(|error| {
            ProvisionError::new(
                "rootfs.archive",
                format!("cannot read hardlink source: {error}"),
            )
        })?;
        if read == 0 {
            break;
        }
        copied = copied
            .checked_add(read as u64)
            .ok_or_else(|| ProvisionError::new("rootfs.archive", "hardlink copy size overflow"))?;
        if copied > expected_bytes {
            return Err(ProvisionError::new(
                "rootfs.archive",
                "hardlink source grew during copy",
            ));
        }
        output.write_all(&buffer[..read]).map_err(|error| {
            ProvisionError::new(
                "rootfs.archive",
                format!("cannot write hardlink copy: {error}"),
            )
        })?;
    }
    if copied != expected_bytes {
        return Err(ProvisionError::new(
            "rootfs.archive",
            "hardlink source changed during copy",
        ));
    }
    output.sync_all().map_err(|error| {
        ProvisionError::new(
            "rootfs.archive",
            format!("cannot sync hardlink copy: {error}"),
        )
    })?;
    set_mode(destination, mode)?;
    Ok(())
}

struct HashingReader<R> {
    inner: R,
    hasher: Sha256,
    bytes: u64,
}

impl<R> HashingReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            bytes: 0,
        }
    }

    fn finish(self) -> (String, u64) {
        (
            crate::vcp_modules::infra::utils::finalize_sha256_hex(self.hasher),
            self.bytes,
        )
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        if read > 0 {
            self.hasher.update(&buffer[..read]);
            self.bytes = self
                .bytes
                .checked_add(read as u64)
                .ok_or_else(|| io::Error::other("hash byte count overflow"))?;
        }
        Ok(read)
    }
}

fn open_verified_archive(
    archive_path: &Path,
    expected_bytes: u64,
    expected_sha256: &str,
) -> Result<File, ProvisionError> {
    let mut file = File::open(archive_path).map_err(|error| {
        ProvisionError::new("rootfs.archive", format!("cannot open archive: {error}"))
    })?;
    let metadata = file.metadata().map_err(|error| {
        ProvisionError::new("rootfs.archive", format!("cannot inspect archive: {error}"))
    })?;
    if !metadata.file_type().is_file() || metadata.len() != expected_bytes {
        return Err(ProvisionError::new(
            "rootfs.archive",
            "archive size changed after staging verification",
        ));
    }
    let actual_sha256 = hash_reader(&mut file)?;
    if actual_sha256 != expected_sha256 {
        return Err(ProvisionError::new(
            "rootfs.archive",
            "archive SHA-256 changed after staging verification",
        ));
    }
    file.seek(SeekFrom::Start(0)).map_err(|error| {
        ProvisionError::new("rootfs.archive", format!("cannot rewind archive: {error}"))
    })?;
    Ok(file)
}

fn hash_reader(reader: &mut impl Read) -> Result<String, ProvisionError> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer).map_err(|error| {
            ProvisionError::new("rootfs.archive", format!("cannot hash archive: {error}"))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(crate::vcp_modules::infra::utils::finalize_sha256_hex(
        hasher,
    ))
}

fn verify_tar_content_hash<R>(
    hashing_reader: HashingReader<R>,
    expected_sha256: &str,
) -> Result<(), ProvisionError> {
    let (actual_sha256, bytes) = hashing_reader.finish();
    if bytes == 0 || actual_sha256 != expected_sha256 {
        return Err(ProvisionError::new(
            "rootfs.tarContentSha256",
            "decompressed tar content SHA-256 does not match the embedded profile",
        ));
    }
    Ok(())
}

fn sanitize_archive_path(path: &Path) -> Result<PathBuf, ProvisionError> {
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => clean.push(value),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ProvisionError::new(
                    "rootfs.archive",
                    format!("archive path escapes root: {}", path.display()),
                ));
            }
        }
    }
    if clean.as_os_str().is_empty() {
        return Ok(clean);
    }
    Ok(clean)
}

fn validate_symlink_target(path: &Path, target: &Path) -> Result<(), ProvisionError> {
    if target.as_os_str().is_empty() {
        return Err(ProvisionError::new(
            "rootfs.archive",
            "empty symlink target",
        ));
    }
    // Absolute guest links are allowed only because they remain PRoot-visible inside guest root.
    let mut stack = if target.is_absolute() {
        Vec::new()
    } else {
        path.parent()
            .unwrap_or_else(|| Path::new(""))
            .components()
            .filter_map(|component| match component {
                Component::Normal(value) => Some(value.to_os_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    for component in target.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(value) => stack.push(value.to_os_string()),
            Component::ParentDir => {
                if stack.pop().is_none() {
                    return Err(ProvisionError::new(
                        "rootfs.archive",
                        format!("symlink escapes guest root: {}", path.display()),
                    ));
                }
            }
            Component::Prefix(_) => {
                return Err(ProvisionError::new(
                    "rootfs.archive",
                    "prefixed symlink target rejected",
                ));
            }
        }
    }
    Ok(())
}

fn ensure_private_directory(path: &Path, field: &str) -> Result<(), ProvisionError> {
    if !path.is_absolute() {
        return Err(ProvisionError::new(
            field,
            "directory path must be absolute",
        ));
    }
    let mut current = PathBuf::from(std::path::MAIN_SEPARATOR.to_string());
    for component in path.components() {
        match component {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(value) => current.push(value),
            Component::ParentDir | Component::Prefix(_) => {
                return Err(ProvisionError::new(
                    field,
                    "directory path contains an unsafe component",
                ));
            }
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(ProvisionError::new(
                    field,
                    format!(
                        "path component is not a real directory: {}",
                        current.display()
                    ),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|error| {
                    ProvisionError::new(
                        field,
                        format!("cannot create directory {}: {error}", current.display()),
                    )
                })?;
            }
            Err(error) => {
                return Err(ProvisionError::new(
                    field,
                    format!("cannot inspect directory component: {error}"),
                ));
            }
        }
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        ProvisionError::new(field, format!("cannot inspect directory: {error}"))
    })?;
    if !metadata.file_type().is_dir() {
        return Err(ProvisionError::new(field, "must be a real directory"));
    }
    set_mode(path, 0o700)
}

fn cleanup_stale_staging_directories(
    rootfs_parent: &Path,
    profile_id: &str,
) -> Result<(), ProvisionError> {
    let prefix = format!(".{profile_id}-");
    let mut stale = Vec::new();
    for entry in fs::read_dir(rootfs_parent).map_err(|error| {
        ProvisionError::new(
            "rootfs.staging",
            format!("cannot scan staging roots: {error}"),
        )
    })? {
        let entry = entry.map_err(|error| {
            ProvisionError::new(
                "rootfs.staging",
                format!("cannot inspect staging root: {error}"),
            )
        })?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&prefix) && name.ends_with(".staging") {
            stale.push(entry.path());
            if stale.len() > MAX_STALE_STAGING_DIRECTORIES {
                return Err(ProvisionError::new(
                    "rootfs.staging",
                    "stale staging directory limit exceeded",
                ));
            }
        }
    }
    for path in stale {
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            ProvisionError::new(
                "rootfs.staging",
                format!("cannot inspect stale root: {error}"),
            )
        })?;
        if !metadata.file_type().is_dir() {
            return Err(ProvisionError::new(
                "rootfs.staging",
                "stale staging entry must be a real directory",
            ));
        }
        fs::remove_dir_all(&path).map_err(|error| {
            ProvisionError::new(
                "rootfs.staging",
                format!("cannot remove stale root: {error}"),
            )
        })?;
    }
    sync_directory(rootfs_parent, "rootfsParent")
}

fn sync_directory(path: &Path, field: &str) -> Result<(), ProvisionError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| ProvisionError::new(field, format!("cannot sync directory: {error}")))
}

fn create_safe_directory(root: &Path, relative: &Path, mode: u32) -> Result<(), ProvisionError> {
    if relative.as_os_str().is_empty() {
        return Ok(());
    }
    ensure_parent_directories(root, relative)?;
    let destination = checked_destination(root, relative)?;
    match fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(ProvisionError::new(
                "rootfs.archive",
                format!(
                    "directory collides with non-directory: {}",
                    relative.display()
                ),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(&destination).map_err(|error| {
                ProvisionError::new(
                    "rootfs.archive",
                    format!("cannot create directory {}: {error}", relative.display()),
                )
            })?;
        }
        Err(error) => {
            return Err(ProvisionError::new(
                "rootfs.archive",
                format!("cannot inspect directory {}: {error}", relative.display()),
            ));
        }
    }
    set_mode(&destination, mode)
}

fn ensure_parent_directories(root: &Path, relative: &Path) -> Result<(), ProvisionError> {
    let Some(parent) = relative.parent() else {
        return Ok(());
    };
    let mut current = root.to_path_buf();
    for component in parent.components() {
        let Component::Normal(value) = component else {
            return Err(ProvisionError::new(
                "rootfs.archive",
                "invalid parent component",
            ));
        };
        current.push(value);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(ProvisionError::new(
                    "rootfs.archive",
                    format!("parent is not a real directory: {}", current.display()),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|error| {
                    ProvisionError::new(
                        "rootfs.archive",
                        format!("cannot create parent {}: {error}", current.display()),
                    )
                })?;
                set_mode(&current, 0o755)?;
            }
            Err(error) => {
                return Err(ProvisionError::new(
                    "rootfs.archive",
                    format!("cannot inspect parent {}: {error}", current.display()),
                ));
            }
        }
    }
    Ok(())
}

fn checked_destination(root: &Path, relative: &Path) -> Result<PathBuf, ProvisionError> {
    let clean = sanitize_archive_path(relative)?;
    Ok(root.join(clean))
}

fn reject_existing_path(path: &Path) -> Result<(), ProvisionError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(ProvisionError::new(
            "rootfs.archive",
            format!("archive path collision: {}", path.display()),
        )),
        Err(error) => Err(ProvisionError::new(
            "rootfs.archive",
            format!("cannot inspect destination: {error}"),
        )),
    }
}

fn write_marker(root: &Path, profile: &VcpCliCommandProfile) -> Result<(), ProvisionError> {
    let marker = root.join(PROVISION_MARKER);
    let payload = serde_json::json!({
        "profileId": profile.profile_id,
        "archiveSha256": profile.rootfs.archive_sha256,
        "tarContentSha256": profile.rootfs.tar_content_sha256
    });
    let bytes = serde_json::to_vec(&payload).map_err(|error| {
        ProvisionError::new("rootfs.marker", format!("cannot serialize marker: {error}"))
    })?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
        .map_err(|error| {
            ProvisionError::new("rootfs.marker", format!("cannot create marker: {error}"))
        })?;
    file.write_all(&bytes).map_err(|error| {
        ProvisionError::new("rootfs.marker", format!("cannot write marker: {error}"))
    })?;
    file.sync_all().map_err(|error| {
        ProvisionError::new("rootfs.marker", format!("cannot sync marker: {error}"))
    })
}

fn marker_is_current(
    marker: &Path,
    profile: &VcpCliCommandProfile,
) -> Result<bool, ProvisionError> {
    match fs::symlink_metadata(marker) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(ProvisionError::new(
            "rootfs.marker",
            format!("cannot inspect marker: {error}"),
        )),
        Ok(metadata) if !metadata.file_type().is_file() => Ok(false),
        Ok(_) => {
            let bytes = fs::read(marker).map_err(|error| {
                ProvisionError::new("rootfs.marker", format!("cannot read marker: {error}"))
            })?;
            let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
                ProvisionError::new("rootfs.marker", format!("invalid marker: {error}"))
            })?;
            Ok(value.get("profileId").and_then(|value| value.as_str())
                == Some(profile.profile_id.as_str())
                && value.get("archiveSha256").and_then(|value| value.as_str())
                    == Some(profile.rootfs.archive_sha256.as_str())
                && value
                    .get("tarContentSha256")
                    .and_then(|value| value.as_str())
                    == Some(profile.rootfs.tar_content_sha256.as_str()))
        }
    }
}

fn set_mode(path: &Path, mode: u32) -> Result<(), ProvisionError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|error| {
            ProvisionError::new(
                "rootfs.archive",
                format!("cannot set permissions for {}: {error}", path.display()),
            )
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_special_and_escaping_links_fail_closed() {
        assert!(sanitize_archive_path(Path::new("../escape")).is_err());
        assert!(sanitize_archive_path(Path::new("/absolute")).is_err());
        assert!(validate_symlink_target(Path::new("a/link"), Path::new("../../escape")).is_err());
        assert!(validate_symlink_target(Path::new("a/link"), Path::new("../inside")).is_ok());
    }

    #[test]
    fn hardlink_cycle_or_non_regular_target_is_rejected() {
        let mut cycle = BTreeMap::new();
        cycle.insert(
            PathBuf::from("a"),
            ArchiveEntryPlan {
                path: PathBuf::from("a"),
                kind: ArchiveKind::Hardlink(PathBuf::from("b")),
                size: 0,
                mode: 0o644,
            },
        );
        cycle.insert(
            PathBuf::from("b"),
            ArchiveEntryPlan {
                path: PathBuf::from("b"),
                kind: ArchiveKind::Hardlink(PathBuf::from("a")),
                size: 0,
                mode: 0o644,
            },
        );
        assert!(validate_hardlink_graph(&cycle).is_err());
    }

    #[test]
    fn archive_mutation_between_passes_cannot_commit_final() {
        let directory = tempfile::tempdir().expect("temporary provision directory");
        let archive = directory.path().join("rootfs.tar.zst");
        write_test_archive(&archive, b"first");
        let first = test_archive_contract(&archive);
        let plans = scan_archive(&archive, first.0, &first.1, &first.2, 1024).expect("first pass");
        write_test_archive(&archive, b"second-longer");
        let staging = directory.path().join("staging");
        fs::create_dir(&staging).expect("staging directory");
        assert!(extract_archive(&archive, &staging, &plans, first.0, &first.1, &first.2,).is_err());
        assert!(!directory.path().join("final").exists());
    }

    #[test]
    fn tar_content_hash_must_match_even_when_compressed_asset_matches() {
        let directory = tempfile::tempdir().expect("temporary provision directory");
        let archive = directory.path().join("rootfs.tar.zst");
        write_test_archive(&archive, b"content");
        let contract = test_archive_contract(&archive);
        assert!(scan_archive(&archive, contract.0, &contract.1, &"0".repeat(64), 1024,).is_err());
    }

    #[test]
    fn hardlink_materialization_counts_against_logical_budget() {
        let directory = tempfile::tempdir().expect("temporary provision directory");
        let archive = directory.path().join("hardlink.tar.zst");
        write_hardlink_archive(&archive, b"12345678");
        let contract = test_archive_contract(&archive);
        assert!(scan_archive(&archive, contract.0, &contract.1, &contract.2, 8,).is_err());
        assert!(scan_archive(&archive, contract.0, &contract.1, &contract.2, 16,).is_ok());
    }

    fn write_test_archive(path: &Path, content: &[u8]) {
        let file = File::create(path).expect("create test archive");
        let encoder = zstd::stream::write::Encoder::new(file, 1).expect("zstd encoder");
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_path("file.txt").expect("test path");
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, content).expect("append test entry");
        let encoder = builder.into_inner().expect("finish tar");
        encoder.finish().expect("finish zstd");
    }

    fn write_hardlink_archive(path: &Path, content: &[u8]) {
        let file = File::create(path).expect("create hardlink archive");
        let encoder = zstd::stream::write::Encoder::new(file, 1).expect("zstd encoder");
        let mut builder = tar::Builder::new(encoder);
        let mut file_header = tar::Header::new_gnu();
        file_header.set_path("file.txt").expect("file path");
        file_header.set_size(content.len() as u64);
        file_header.set_mode(0o644);
        file_header.set_cksum();
        builder
            .append(&file_header, content)
            .expect("append regular file");
        let mut link_header = tar::Header::new_gnu();
        link_header.set_path("copy.txt").expect("hardlink path");
        link_header
            .set_link_name("file.txt")
            .expect("hardlink target");
        link_header.set_entry_type(tar::EntryType::hard_link());
        link_header.set_size(0);
        link_header.set_mode(0o644);
        link_header.set_cksum();
        builder
            .append(&link_header, io::empty())
            .expect("append hardlink");
        let encoder = builder.into_inner().expect("finish tar");
        encoder.finish().expect("finish zstd");
    }

    fn test_archive_contract(path: &Path) -> (u64, String, String) {
        let bytes = fs::metadata(path).expect("archive metadata").len();
        let mut archive = File::open(path).expect("open archive");
        let archive_sha256 = hash_reader(&mut archive).expect("hash archive");
        let file = File::open(path).expect("reopen archive");
        let decoder = zstd::stream::read::Decoder::new(file).expect("decode archive");
        let mut hashing_reader = HashingReader::new(decoder);
        io::copy(&mut hashing_reader, &mut io::sink()).expect("hash tar content");
        let (tar_sha256, _) = hashing_reader.finish();
        (bytes, archive_sha256, tar_sha256)
    }
}
