//! Decode/cleanup compatibility for River snapshots created by older builds.

use std::fs::{self, File};
use std::io;
use std::path::Path;

use sha2::{Digest, Sha256};

const FILE_NAME: &str = "river-context.json";
const MAX_ROOT_ENTRIES: usize = 512;

pub(super) fn remove_river_projection(
    root: &Path,
    generation: u64,
    job_id: &str,
    attempt_id: &str,
    recorded_path: &Path,
) -> Result<(), String> {
    require_real_directory(root, "projection root")?;
    let directory = root.join(projection_stem(generation, job_id, attempt_id));
    if recorded_path != directory.join(FILE_NAME) {
        return Err("river projection path does not match its fenced attempt".to_string());
    }
    remove_directory(&directory)?;
    sync_directory(root)
}

pub(super) fn gc_stale_river_projections(root: &Path) -> Result<(), String> {
    require_real_directory(root, "projection root")?;
    for (index, entry) in fs::read_dir(root)
        .map_err(|error| format!("cannot scan river projection root: {error}"))?
        .enumerate()
    {
        if index >= MAX_ROOT_ENTRIES {
            return Err("river projection root exceeds its bounded entry limit".to_string());
        }
        let entry =
            entry.map_err(|error| format!("cannot read river projection entry: {error}"))?;
        let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
            continue;
        };
        if !is_stem(&name) {
            continue;
        }
        require_real_directory(&entry.path(), "stale river projection")?;
        remove_directory(&entry.path())?;
    }
    sync_directory(root)
}

pub(super) fn projection_stem(generation: u64, job_id: &str, attempt_id: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("{generation}\0{job_id}\0{attempt_id}").as_bytes())
    )
}

fn require_real_directory(path: &Path, label: &str) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("cannot inspect {label}: {error}"))?;
    if metadata.file_type().is_dir() {
        Ok(())
    } else {
        Err(format!("{label} must be a real directory"))
    }
}

fn remove_directory(directory: &Path) -> Result<(), String> {
    match fs::symlink_metadata(directory) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => return Err("river attempt projection is not a real directory".to_string()),
        Err(error) => return Err(format!("cannot inspect river attempt projection: {error}")),
    }
    for entry in fs::read_dir(directory)
        .map_err(|error| format!("cannot scan river attempt projection: {error}"))?
    {
        let entry = entry.map_err(|error| format!("cannot read river attempt entry: {error}"))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| format!("cannot inspect river attempt file: {error}"))?;
        if !metadata.file_type().is_file() {
            return Err("river attempt projection contains a non-regular entry".to_string());
        }
        let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
            return Err("river attempt projection has a non-UTF-8 entry".to_string());
        };
        if name != FILE_NAME
            && name != format!(".{FILE_NAME}.tmp")
            && !is_artifact_file_name(&name)
            && !name
                .strip_prefix('.')
                .and_then(|value| value.strip_suffix(".tmp"))
                .is_some_and(is_artifact_file_name)
        {
            return Err("river attempt projection contains an unknown entry".to_string());
        }
        fs::remove_file(entry.path())
            .map_err(|error| format!("cannot remove river projection file: {error}"))?;
    }
    fs::remove_dir(directory)
        .map_err(|error| format!("cannot remove river attempt projection: {error}"))
}

fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("cannot sync projection directory: {error}"))
}

fn is_stem(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn is_artifact_file_name(value: &str) -> bool {
    value.len() <= 96
        && value.starts_with("river-artifact-")
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        })
        && !value.contains("..")
        && Path::new(value).file_name().and_then(|name| name.to_str()) == Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_projection_cleanup_is_identity_scoped() {
        let root = tempfile::tempdir().expect("projection root");
        let stem = projection_stem(7, "job", "attempt");
        let attempt = root.path().join(&stem);
        fs::create_dir(&attempt).expect("attempt directory");
        let context = attempt.join(FILE_NAME);
        fs::write(&context, b"legacy").expect("legacy projection");
        fs::write(
            attempt.join("river-artifact-00-aaaaaaaaaaaa.bin"),
            b"legacy artifact",
        )
        .expect("legacy artifact");

        remove_river_projection(root.path(), 7, "job", "attempt", &context)
            .expect("remove legacy projection");
        assert!(!attempt.exists());
    }

    #[test]
    fn legacy_gc_ignores_unknown_root_entries() {
        let root = tempfile::tempdir().expect("projection root");
        let stale = root.path().join("a".repeat(64));
        fs::create_dir(&stale).expect("stale attempt");
        fs::write(stale.join(FILE_NAME), b"legacy").expect("legacy projection");
        fs::write(root.path().join("keep"), b"unrelated").expect("unrelated entry");

        gc_stale_river_projections(root.path()).expect("bounded legacy GC");
        assert!(!stale.exists());
        assert!(root.path().join("keep").exists());
    }
}
