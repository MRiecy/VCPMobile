//! P1 本地 Skills：显式 list/read，绝不自动注入模型提示词。

use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::result::{VcpCliSkillResult, VcpCliSkillSummary};

pub(super) const MAX_SKILL_BYTES: usize = 64 * 1024;
const MAX_SKILL_COUNT: usize = 256;
const MAX_SKILL_WARNINGS: usize = 16;
const BUILTIN_SKILL_ID: &str = "vcp-mobile-cli-basics";
const BUILTIN_SKILL: &str = r#"# VCP Mobile CLI Basics

Version: 1.0.0

- Commands run in an offline Alpine guest through `/bin/bash -lc`.
- Use `/workspace` for durable working files. Skills are read only through explicit `list_skills`/`read_skill`; they are not mounted into Bash.
- There is no Android root, `sudo`, `systemd`, Docker, GUI, ADB, or Shizuku capability.
- Treat background reliability as `foreground_only`; use `poll` for incremental output and `cancel` when work is no longer needed.
- A Skill is read only after an explicit `list_skills` or `read_skill` action. Its text is never injected automatically.
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SkillErrorKind {
    NotFound,
    Integrity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SkillError {
    pub kind: SkillErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SkillListResult {
    pub skills: Vec<VcpCliSkillSummary>,
    pub warnings: Vec<String>,
}

impl SkillError {
    fn not_found(message: impl Into<String>) -> Self {
        Self {
            kind: SkillErrorKind::NotFound,
            message: message.into(),
        }
    }

    fn integrity(message: impl Into<String>) -> Self {
        Self {
            kind: SkillErrorKind::Integrity,
            message: message.into(),
        }
    }
}

pub(super) fn install_builtin_skill(skills_root: &Path) -> Result<(), SkillError> {
    require_real_directory(skills_root, "skills root")?;
    let skill_root = skills_root.join(BUILTIN_SKILL_ID);
    match fs::symlink_metadata(&skill_root) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            let root_fd = open_directory_nofollow(skills_root)?;
            let existing =
                read_resource_from_fd(&root_fd, BUILTIN_SKILL_ID, Path::new("SKILL.md"))?;
            if existing != BUILTIN_SKILL.as_bytes() {
                return Err(SkillError::integrity(
                    "built-in Skill content differs from the frozen catalog",
                ));
            }
            freeze_skill_catalog(skills_root, &skill_root, &skill_root.join("SKILL.md"))?;
            return Ok(());
        }
        Ok(_) => {
            return Err(SkillError::integrity(
                "built-in Skill root is not a real directory",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(&skill_root).map_err(|error| {
                SkillError::integrity(format!("cannot create built-in Skill root: {error}"))
            })?;
            set_mode(&skill_root, 0o700)?;
        }
        Err(error) => {
            return Err(SkillError::integrity(format!(
                "cannot inspect built-in Skill root: {error}"
            )));
        }
    }

    let target = skill_root.join("SKILL.md");
    let temporary = skill_root.join(format!(".SKILL-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| {
                SkillError::integrity(format!("cannot create built-in Skill temporary: {error}"))
            })?;
        file.write_all(BUILTIN_SKILL.as_bytes()).map_err(|error| {
            SkillError::integrity(format!("cannot write built-in Skill: {error}"))
        })?;
        file.sync_all().map_err(|error| {
            SkillError::integrity(format!("cannot sync built-in Skill: {error}"))
        })?;
        fs::rename(&temporary, &target).map_err(|error| {
            SkillError::integrity(format!("cannot atomically install built-in Skill: {error}"))
        })?;
        freeze_skill_catalog(skills_root, &skill_root, &target)?;
        sync_directory(&skill_root)?;
        sync_directory(skills_root)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(super) fn list_skills(skills_root: &Path) -> Result<SkillListResult, SkillError> {
    let root_fd = open_directory_nofollow(skills_root)?;
    let mut names = read_directory_names(&root_fd)?;
    if names.len() > MAX_SKILL_COUNT {
        return Err(SkillError::integrity("Skill count exceeds the hard limit"));
    }
    names.sort();

    let mut summaries = Vec::with_capacity(names.len());
    let mut warnings = Vec::new();
    for id in names {
        let result = (|| {
            validate_skill_id(&id)?;
            let bytes = read_resource_from_fd(&root_fd, &id, Path::new("SKILL.md"))?;
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| SkillError::integrity(format!("Skill {id} is not UTF-8")))?;
            Ok::<VcpCliSkillSummary, SkillError>(VcpCliSkillSummary {
                id: id.clone(),
                name: parse_skill_name(text).unwrap_or_else(|| id.clone()),
                version: parse_skill_version(text),
                source: if id == BUILTIN_SKILL_ID {
                    "builtin".to_string()
                } else {
                    "local".to_string()
                },
                sha256: sha256_hex(&bytes),
            })
        })();
        match result {
            Ok(summary) => summaries.push(summary),
            Err(error) => push_bounded_warning(
                &mut warnings,
                format!("Skipped invalid Skill {id}: {}", error.message),
            ),
        }
    }
    Ok(SkillListResult {
        skills: summaries,
        warnings,
    })
}

pub(super) fn read_skill(
    skills_root: &Path,
    skill_id: &str,
    resource_path: &str,
    requested_max_bytes: usize,
) -> Result<(VcpCliSkillResult, String), SkillError> {
    let root_fd = open_directory_nofollow(skills_root)?;
    validate_skill_id(skill_id)?;
    let relative = validate_resource_path(resource_path)?;
    let bytes = read_resource_from_fd(&root_fd, skill_id, &relative)?;
    let max_bytes = requested_max_bytes.clamp(1, MAX_SKILL_BYTES);
    let truncated = bytes.len() > max_bytes;
    let full_sha256 = sha256_hex(&bytes);
    let full_content = String::from_utf8(bytes)
        .map_err(|_| SkillError::integrity("Skill resource must be UTF-8"))?;
    let content = truncate_utf8(&full_content, max_bytes).to_string();
    let name = if resource_path == "SKILL.md" {
        parse_skill_name(&content).unwrap_or_else(|| skill_id.to_string())
    } else {
        skill_id.to_string()
    };
    Ok((
        VcpCliSkillResult {
            id: skill_id.to_string(),
            name,
            resource_path: resource_path.to_string(),
            // Logical capability reference only. The catalog is deliberately not mounted into
            // PRoot because bundled PRoot fake-root can bypass host chmod and has no read-only bind.
            skill_root: format!("vcp-skill://{skill_id}"),
            sha256: full_sha256,
            truncated,
        },
        content,
    ))
}

fn validate_skill_id(id: &str) -> Result<(), SkillError> {
    let bytes = id.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 64
        || !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit()
        || bytes
            .iter()
            .any(|byte| !byte.is_ascii_lowercase() && !byte.is_ascii_digit() && *byte != b'-')
    {
        return Err(SkillError::integrity("Skill id is not a stable id"));
    }
    Ok(())
}

fn validate_resource_path(value: &str) -> Result<PathBuf, SkillError> {
    let path = Path::new(value);
    if value.is_empty() || path.is_absolute() {
        return Err(SkillError::integrity(
            "Skill resource path must be relative",
        ));
    }
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => clean.push(value),
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(SkillError::integrity(
                    "Skill resource path contains an unsafe component",
                ));
            }
        }
    }
    Ok(clean)
}

fn require_real_directory(path: &Path, label: &str) -> Result<(), SkillError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| SkillError::integrity(format!("cannot inspect {label}: {error}")))?;
    if !metadata.file_type().is_dir() {
        return Err(SkillError::integrity(format!(
            "{label} must be a real directory"
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn open_directory_nofollow(path: &Path) -> Result<OwnedFd, SkillError> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| SkillError::integrity("Skill directory path contains NUL"))?;
    let descriptor = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        )
    };
    if descriptor < 0 {
        return Err(SkillError::integrity(format!(
            "cannot securely open Skills root: {}",
            io::Error::last_os_error()
        )));
    }
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
}

#[cfg(not(unix))]
fn open_directory_nofollow(_path: &Path) -> Result<OwnedFd, SkillError> {
    Err(SkillError::integrity(
        "secure Skill access is unavailable on this host",
    ))
}

#[cfg(unix)]
fn open_directory_at(parent: &OwnedFd, name: &std::ffi::OsStr) -> Result<OwnedFd, SkillError> {
    let name = CString::new(name.as_bytes())
        .map_err(|_| SkillError::integrity("Skill path component contains NUL"))?;
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        )
    };
    if descriptor < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::NotFound {
            return Err(SkillError::not_found("Skill directory was not found"));
        }
        return Err(SkillError::integrity(format!(
            "cannot securely open Skill directory: {error}"
        )));
    }
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
}

#[cfg(unix)]
fn open_regular_file_at(parent: &OwnedFd, name: &std::ffi::OsStr) -> Result<File, SkillError> {
    let name = CString::new(name.as_bytes())
        .map_err(|_| SkillError::integrity("Skill filename contains NUL"))?;
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if descriptor < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::NotFound {
            return Err(SkillError::not_found("Skill resource was not found"));
        }
        return Err(SkillError::integrity(format!(
            "cannot securely open Skill resource: {error}"
        )));
    }
    let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(descriptor.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(SkillError::integrity(format!(
            "cannot inspect opened Skill resource: {}",
            io::Error::last_os_error()
        )));
    }
    let stat = unsafe { stat.assume_init() };
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG || stat.st_size < 0 {
        return Err(SkillError::integrity(
            "Skill resource must be a real regular file",
        ));
    }
    if stat.st_size as usize > MAX_SKILL_BYTES {
        return Err(SkillError::integrity(
            "Skill resource exceeds the 64 KiB hard limit",
        ));
    }
    Ok(File::from(descriptor))
}

fn read_resource_from_fd(
    root: &OwnedFd,
    skill_id: &str,
    relative: &Path,
) -> Result<Vec<u8>, SkillError> {
    let mut directory = open_directory_at(root, std::ffi::OsStr::new(skill_id))?;
    let components = relative.components().collect::<Vec<_>>();
    let Some((last, parents)) = components.split_last() else {
        return Err(SkillError::integrity("Skill resource path is empty"));
    };
    for component in parents {
        let Component::Normal(value) = component else {
            return Err(SkillError::integrity("invalid Skill resource component"));
        };
        directory = open_directory_at(&directory, value)?;
    }
    let Component::Normal(filename) = last else {
        return Err(SkillError::integrity("invalid Skill resource filename"));
    };
    let mut file = open_regular_file_at(&directory, filename)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| SkillError::integrity(format!("cannot read Skill resource: {error}")))?;
    if bytes.len() > MAX_SKILL_BYTES {
        return Err(SkillError::integrity(
            "Skill resource changed beyond its hard limit while reading",
        ));
    }
    Ok(bytes)
}

fn read_directory_names(root: &OwnedFd) -> Result<Vec<String>, SkillError> {
    let descriptor_path = PathBuf::from(format!("/proc/self/fd/{}", root.as_raw_fd()));
    fs::read_dir(descriptor_path)
        .map_err(|error| SkillError::integrity(format!("cannot scan Skills fd: {error}")))?
        .map(|entry| {
            entry
                .map_err(|error| {
                    SkillError::integrity(format!("cannot read Skill entry: {error}"))
                })?
                .file_name()
                .into_string()
                .map_err(|_| SkillError::integrity("Skill id must be UTF-8"))
        })
        .collect()
}

fn push_bounded_warning(warnings: &mut Vec<String>, warning: String) {
    if warnings.len() < MAX_SKILL_WARNINGS {
        let warning = truncate_utf8(&warning, 256).to_string();
        log::warn!("[VCPMobileCLI] {warning}");
        warnings.push(warning);
    }
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

fn parse_skill_name(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("# "))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

fn parse_skill_version(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("Version:"))
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .map(str::to_string)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sync_directory(path: &Path) -> Result<(), SkillError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| SkillError::integrity(format!("cannot sync Skill directory: {error}")))
}

fn freeze_skill_catalog(
    skills_root: &Path,
    skill_root: &Path,
    skill_file: &Path,
) -> Result<(), SkillError> {
    // Defense-in-depth against accidental host writes only. This is not the read-only security
    // boundary: canonical Skills are absent from PRoot argv because fake-root bypasses chmod.
    set_mode(skill_file, 0o444)?;
    set_mode(skill_root, 0o555)?;
    set_mode(skills_root, 0o555)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_skill_is_explicitly_listed_and_read_with_integrity_hash() {
        let directory = tempfile::tempdir().expect("temporary Skills directory");
        install_builtin_skill(directory.path()).expect("install built-in Skill");
        let skills = list_skills(directory.path()).expect("list Skills");
        assert_eq!(skills.skills.len(), 1);
        assert_eq!(skills.skills[0].id, BUILTIN_SKILL_ID);
        assert_eq!(skills.skills[0].source, "builtin");
        let (skill, content) = read_skill(
            directory.path(),
            BUILTIN_SKILL_ID,
            "SKILL.md",
            MAX_SKILL_BYTES,
        )
        .expect("read built-in Skill");
        assert!(content.contains("/workspace"));
        assert_eq!(skill.sha256, skills.skills[0].sha256);
        assert!(!skill.truncated);
        assert_eq!(skill.skill_root, "vcp-skill://vcp-mobile-cli-basics");
        assert!(!skill.skill_root.starts_with("/skills"));
    }

    #[test]
    fn traversal_and_oversized_skill_are_rejected() {
        assert!(validate_resource_path("../secret").is_err());
        let directory = tempfile::tempdir().expect("temporary Skills directory");
        let skill_root = directory.path().join("large-skill");
        fs::create_dir(&skill_root).expect("create Skill root");
        fs::write(skill_root.join("SKILL.md"), vec![b'x'; MAX_SKILL_BYTES + 1])
            .expect("write oversized Skill");
        let listed = list_skills(directory.path()).expect("list skips oversized Skill");
        assert!(listed.skills.is_empty());
        assert_eq!(listed.warnings.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_skill_root_and_resource_are_rejected() {
        let directory = tempfile::tempdir().expect("temporary Skills directory");
        let outside = tempfile::tempdir().expect("outside directory");
        std::os::unix::fs::symlink(outside.path(), directory.path().join("linked-skill"))
            .expect("create root symlink");
        let listed = list_skills(directory.path()).expect("list skips root symlink");
        assert!(listed.skills.is_empty());
        assert_eq!(listed.warnings.len(), 1);

        fs::remove_file(directory.path().join("linked-skill")).expect("remove root symlink");
        let root = directory.path().join("safe-skill");
        fs::create_dir(&root).expect("create safe root");
        std::os::unix::fs::symlink(outside.path().join("missing"), root.join("SKILL.md"))
            .expect("create resource symlink");
        let listed = list_skills(directory.path()).expect("list skips resource symlink");
        assert!(listed.skills.is_empty());
        assert_eq!(listed.warnings.len(), 1);
    }
}
