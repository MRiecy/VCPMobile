//! Host-owned, per-attempt river snapshots consumed by ProcessHost.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tauri_plugin_vcp_mobile::cli::CliRiverContextProjection;

use super::runtime::VcpCliRiverProjectionInput;
use super::turn_types::MAX_RIVER_PROJECTION_BYTES;

const FILE_NAME: &str = "river-context.json";
const MAX_ROOT_ENTRIES: usize = 512;

pub(super) fn prepare_river_projection(
    root: &Path,
    generation: u64,
    job_id: &str,
    attempt_id: &str,
    input: &VcpCliRiverProjectionInput,
) -> Result<CliRiverContextProjection, String> {
    validate_input(input)?;
    require_real_directory(root, "projection root")?;
    let directory = root.join(projection_stem(generation, job_id, attempt_id));
    match fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Ok(_) => return Err("river attempt projection already exists".to_string()),
        Err(error) => return Err(format!("cannot inspect river attempt projection: {error}")),
    }
    fs::create_dir(&directory)
        .map_err(|error| format!("cannot create river attempt projection: {error}"))?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("cannot protect river attempt projection: {error}"))?;
    let result = write_snapshot(&directory, input).and_then(|path| {
        verify_snapshot(&path, input)?;
        sync_directory(&directory)?;
        sync_directory(root)?;
        Ok(path)
    });
    let path = match result {
        Ok(path) => path,
        Err(error) => {
            let _ = remove_directory(&directory);
            return Err(error);
        }
    };
    Ok(CliRiverContextProjection {
        host_path: path.to_string_lossy().into_owned(),
        size_bytes: input.size_bytes,
        sha256: input.sha256.clone(),
    })
}

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

fn validate_input(input: &VcpCliRiverProjectionInput) -> Result<(), String> {
    if input.canonical_json.len() > MAX_RIVER_PROJECTION_BYTES {
        return Err("river projection exceeds 128 KiB".to_string());
    }
    if input.size_bytes != input.canonical_json.len() as u64 {
        return Err("river projection size mismatch".to_string());
    }
    if input.sha256.len() != 64 || !input.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("river projection SHA-256 must be hexadecimal".to_string());
    }
    let actual = format!("{:x}", Sha256::digest(input.canonical_json.as_bytes()));
    if !actual.eq_ignore_ascii_case(&input.sha256) {
        return Err("river projection SHA-256 mismatch".to_string());
    }
    serde_json::from_str::<serde_json::Value>(&input.canonical_json)
        .map_err(|error| format!("river projection is not valid JSON: {error}"))?;
    Ok(())
}

fn write_snapshot(directory: &Path, input: &VcpCliRiverProjectionInput) -> Result<PathBuf, String> {
    let temporary = directory.join(format!(".{FILE_NAME}.tmp"));
    let path = directory.join(FILE_NAME);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| format!("cannot create river snapshot: {error}"))?;
    file.write_all(input.canonical_json.as_bytes())
        .map_err(|error| format!("cannot write river snapshot: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("cannot sync river snapshot: {error}"))?;
    drop(file);
    fs::rename(&temporary, &path)
        .map_err(|error| format!("cannot publish river snapshot: {error}"))?;
    Ok(path)
}

fn verify_snapshot(path: &Path, input: &VcpCliRiverProjectionInput) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect river snapshot: {error}"))?;
    if !metadata.file_type().is_file() || metadata.len() != input.size_bytes {
        return Err("river snapshot is not the expected regular file".to_string());
    }
    let mut file =
        File::open(path).map_err(|error| format!("cannot open river snapshot: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("cannot hash river snapshot: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    if !format!("{:x}", hasher.finalize()).eq_ignore_ascii_case(&input.sha256) {
        return Err("river snapshot changed before ProcessHost start".to_string());
    }
    Ok(())
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
        if name != FILE_NAME && name != format!(".{FILE_NAME}.tmp") {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn input(body: &str) -> VcpCliRiverProjectionInput {
        VcpCliRiverProjectionInput {
            canonical_json: body.to_string(),
            sha256: format!("{:x}", Sha256::digest(body.as_bytes())),
            size_bytes: body.len() as u64,
        }
    }

    #[test]
    fn strict_attempt_layout_is_written_verified_and_removed() {
        let root = tempfile::tempdir().expect("projection root");
        let projection = prepare_river_projection(
            root.path(),
            7,
            "job",
            "attempt",
            &input(r#"{"messages":[]}"#),
        )
        .expect("prepare snapshot");
        let path = PathBuf::from(&projection.host_path);
        assert_eq!(
            path,
            root.path()
                .join(projection_stem(7, "job", "attempt"))
                .join(FILE_NAME)
        );
        remove_river_projection(root.path(), 7, "job", "attempt", &path).expect("remove snapshot");
        assert!(!path.exists());
    }

    #[test]
    fn mutation_and_stale_symlink_fail_closed() {
        let root = tempfile::tempdir().expect("projection root");
        let mut invalid = input(r#"{"messages":[]}"#);
        invalid.sha256 = "0".repeat(64);
        assert!(prepare_river_projection(root.path(), 1, "job", "attempt", &invalid).is_err());
        std::os::unix::fs::symlink(root.path(), root.path().join("a".repeat(64)))
            .expect("malicious symlink");
        assert!(gc_stale_river_projections(root.path()).is_err());
    }
}
