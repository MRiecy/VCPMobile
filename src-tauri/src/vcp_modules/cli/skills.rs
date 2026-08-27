//! Skill catalog 的安全路径与有界读取原语；绝不自动注入模型提示词。

use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

pub(super) const MAX_SKILL_BYTES: usize = 64 * 1024;
pub(super) const BUILTIN_SKILL_ID: &str = "vcp-mobile-cli-basics";
pub(super) const BUILTIN_SKILL: &str = r#"# VCP Mobile CLI Basics

Version: 1.0.0

- Commands run in an offline Alpine guest through `/bin/bash -lc`.
- Use `/workspace` for durable working files. Canonical Skills are never mounted into Bash; use `list_skills`, `read_skill`, then explicit `materialize_skill` for a non-writeback copy.
- There is no Android root, `sudo`, `systemd`, Docker, GUI, ADB, or Shizuku capability.
- Treat background reliability as `foreground_only`; use `poll` for incremental output and `cancel` when work is no longer needed.
- A Skill is visible only after an explicit Skill action. Its text is never injected automatically, and materialization never executes it.
"#;
pub(super) const LEGACY_P1_BUILTIN_SKILL: &str = r#"# VCP Mobile CLI Basics

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

impl SkillError {
    pub(super) fn not_found(message: impl Into<String>) -> Self {
        Self {
            kind: SkillErrorKind::NotFound,
            message: message.into(),
        }
    }

    pub(super) fn integrity(message: impl Into<String>) -> Self {
        Self {
            kind: SkillErrorKind::Integrity,
            message: message.into(),
        }
    }
}

pub(super) fn validate_skill_id(id: &str) -> Result<(), SkillError> {
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

pub(super) fn validate_resource_path(value: &str) -> Result<PathBuf, SkillError> {
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
fn open_regular_file_at(
    parent: &OwnedFd,
    name: &std::ffi::OsStr,
    max_bytes: usize,
) -> Result<File, SkillError> {
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
    if stat.st_size as usize > max_bytes {
        return Err(SkillError::integrity(
            "Skill resource exceeds its hard limit",
        ));
    }
    Ok(File::from(descriptor))
}

fn read_resource_from_fd_bounded(
    root: &OwnedFd,
    skill_id: &str,
    relative: &Path,
    max_bytes: usize,
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
    let mut file = open_regular_file_at(&directory, filename, max_bytes)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| SkillError::integrity(format!("cannot read Skill resource: {error}")))?;
    if bytes.len() > max_bytes {
        return Err(SkillError::integrity(
            "Skill resource changed beyond its hard limit while reading",
        ));
    }
    Ok(bytes)
}

pub(super) fn read_catalog_resource(
    catalog_root: &Path,
    object_id: &str,
    resource_path: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, SkillError> {
    let root_fd = open_directory_nofollow(catalog_root)?;
    validate_skill_id(object_id)?;
    let relative = validate_resource_path(resource_path)?;
    read_resource_from_fd_bounded(&root_fd, object_id, &relative, max_bytes)
}

pub(super) fn parse_skill_name(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("# "))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

pub(super) fn parse_skill_version(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("Version:"))
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .map(str::to_string)
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    crate::vcp_modules::infra::utils::calculate_sha256(bytes)
}
