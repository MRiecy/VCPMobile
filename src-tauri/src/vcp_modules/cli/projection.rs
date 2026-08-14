//! Host-owned, per-attempt river snapshots consumed by ProcessHost.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tauri_plugin_vcp_mobile::cli::{CliProjectedArtifact, CliRiverContextProjection};

use super::runtime::{VcpCliArtifactGrantInput, VcpCliRiverProjectionInput};
use super::turn_types::{
    MAX_RIVER_ARTIFACTS, MAX_RIVER_ARTIFACT_BYTES, MAX_RIVER_ARTIFACT_TOTAL_BYTES,
    MAX_RIVER_PROJECTION_BYTES,
};

const FILE_NAME: &str = "river-context.json";
const ATTEMPT_PROJECTION_SCHEMA: &str = "vcp.mobile.attempt-projection.v1";
const MAX_ROOT_ENTRIES: usize = 512;

pub(super) fn prepare_river_projection(
    root: &Path,
    attachments_root: &Path,
    generation: u64,
    job_id: &str,
    attempt_id: &str,
    input: &VcpCliRiverProjectionInput,
) -> Result<CliRiverContextProjection, String> {
    validate_input(input, attachments_root)?;
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
        let artifacts = input
            .artifact_grants
            .iter()
            .map(|grant| copy_artifact_snapshot(&directory, grant))
            .collect::<Result<Vec<_>, _>>()?;
        sync_directory(&directory)?;
        sync_directory(root)?;
        Ok((path, artifacts))
    });
    let (path, artifacts) = match result {
        Ok(result) => result,
        Err(error) => {
            let _ = remove_directory(&directory);
            return Err(error);
        }
    };
    Ok(CliRiverContextProjection {
        host_path: path.to_string_lossy().into_owned(),
        size_bytes: input.size_bytes,
        sha256: input.sha256.clone(),
        artifacts,
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

fn validate_input(
    input: &VcpCliRiverProjectionInput,
    attachments_root: &Path,
) -> Result<(), String> {
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
    let document = serde_json::from_str::<serde_json::Value>(&input.canonical_json)
        .map_err(|error| format!("river projection is not valid JSON: {error}"))?;
    if document.get("schema").and_then(serde_json::Value::as_str) != Some(ATTEMPT_PROJECTION_SCHEMA)
    {
        return Err("river projection schema is not supported".to_string());
    }
    let descriptors = document
        .get("artifacts")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "river projection artifacts must be an array".to_string())?;
    if descriptors.len() != input.artifact_grants.len() || descriptors.len() > MAX_RIVER_ARTIFACTS {
        return Err("river projection artifact grants do not match the bundle".to_string());
    }

    let mut total_bytes = 0_u64;
    let mut file_names = HashSet::new();
    let mut guest_paths = HashSet::new();
    let mut hashes = HashSet::new();
    if !input.artifact_grants.is_empty() {
        require_real_directory(attachments_root, "attachment CAS root")?;
    }
    for (descriptor, grant) in descriptors.iter().zip(&input.artifact_grants) {
        validate_artifact_grant(attachments_root, grant)?;
        if descriptor
            .get("guest_path")
            .and_then(serde_json::Value::as_str)
            != Some(grant.guest_path.as_str())
            || descriptor
                .get("size_bytes")
                .and_then(serde_json::Value::as_u64)
                != Some(grant.size_bytes)
            || descriptor.get("sha256").and_then(serde_json::Value::as_str)
                != Some(grant.sha256.as_str())
            || descriptor
                .get("source_unreachable")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
            || descriptor
                .get("non_writeback")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
            || descriptor.get("id").and_then(serde_json::Value::as_str)
                != Path::new(&grant.file_name)
                    .file_stem()
                    .and_then(|value| value.to_str())
        {
            return Err(
                "river projection artifact descriptor does not match its grant".to_string(),
            );
        }
        total_bytes = total_bytes
            .checked_add(grant.size_bytes)
            .ok_or_else(|| "river artifact byte budget overflowed".to_string())?;
        if total_bytes > MAX_RIVER_ARTIFACT_TOTAL_BYTES {
            return Err("river artifacts exceed the 256 MiB attempt budget".to_string());
        }
        if !file_names.insert(grant.file_name.clone())
            || !guest_paths.insert(grant.guest_path.clone())
            || !hashes.insert(grant.sha256.clone())
        {
            return Err("river artifact grants contain duplicate identities".to_string());
        }
    }
    Ok(())
}

fn validate_artifact_grant(
    attachments_root: &Path,
    grant: &VcpCliArtifactGrantInput,
) -> Result<(), String> {
    if grant.size_bytes > MAX_RIVER_ARTIFACT_BYTES {
        return Err("river artifact exceeds the 64 MiB item budget".to_string());
    }
    if !is_lower_sha256(&grant.sha256)
        || !is_artifact_file_name(&grant.file_name)
        || grant.guest_path != format!("/run/{}", grant.file_name)
    {
        return Err("river artifact identity is invalid".to_string());
    }
    let root_metadata = fs::symlink_metadata(attachments_root)
        .map_err(|error| format!("cannot inspect attachment CAS root: {error}"))?;
    let source_metadata = fs::symlink_metadata(&grant.source_path)
        .map_err(|error| format!("cannot inspect river artifact source: {error}"))?;
    if !root_metadata.file_type().is_dir()
        || !source_metadata.file_type().is_file()
        || source_metadata.len() != grant.size_bytes
    {
        return Err("river artifact source is not the expected real file".to_string());
    }
    let canonical_root = fs::canonicalize(attachments_root)
        .map_err(|error| format!("cannot resolve attachment CAS root: {error}"))?;
    let canonical_source = fs::canonicalize(&grant.source_path)
        .map_err(|error| format!("cannot resolve river artifact source: {error}"))?;
    if canonical_source.parent() != Some(canonical_root.as_path())
        || canonical_source
            .file_stem()
            .and_then(|value| value.to_str())
            != Some(grant.sha256.as_str())
    {
        return Err("river artifact source escaped the canonical CAS root".to_string());
    }
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

fn copy_artifact_snapshot(
    directory: &Path,
    grant: &VcpCliArtifactGrantInput,
) -> Result<CliProjectedArtifact, String> {
    let temporary = directory.join(format!(".{}.tmp", grant.file_name));
    let destination = directory.join(&grant.file_name);
    let mut source = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&grant.source_path)
        .map_err(|error| format!("cannot open river artifact source: {error}"))?;
    let source_metadata = source
        .metadata()
        .map_err(|error| format!("cannot inspect opened river artifact source: {error}"))?;
    if !source_metadata.is_file() || source_metadata.len() != grant.size_bytes {
        return Err("river artifact source changed before snapshot copy".to_string());
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| format!("cannot create river artifact snapshot: {error}"))?;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = source
            .read(&mut buffer)
            .map_err(|error| format!("cannot read river artifact source: {error}"))?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| "river artifact size overflowed".to_string())?;
        if total > grant.size_bytes || total > MAX_RIVER_ARTIFACT_BYTES {
            return Err("river artifact exceeded its frozen size while copying".to_string());
        }
        hasher.update(&buffer[..read]);
        output
            .write_all(&buffer[..read])
            .map_err(|error| format!("cannot write river artifact snapshot: {error}"))?;
    }
    if total != grant.size_bytes || format!("{:x}", hasher.finalize()) != grant.sha256 {
        return Err("river artifact content did not match its frozen CAS identity".to_string());
    }
    output
        .sync_all()
        .map_err(|error| format!("cannot sync river artifact snapshot: {error}"))?;
    drop(output);
    fs::rename(&temporary, &destination)
        .map_err(|error| format!("cannot publish river artifact snapshot: {error}"))?;
    let destination_metadata = fs::symlink_metadata(&destination)
        .map_err(|error| format!("cannot inspect river artifact snapshot: {error}"))?;
    if !destination_metadata.file_type().is_file() || destination_metadata.len() != grant.size_bytes
    {
        return Err("river artifact snapshot changed before ProcessHost start".to_string());
    }
    Ok(CliProjectedArtifact {
        host_path: destination.to_string_lossy().into_owned(),
        guest_path: grant.guest_path.clone(),
        size_bytes: grant.size_bytes,
        sha256: grant.sha256.clone(),
    })
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

fn is_lower_sha256(value: &str) -> bool {
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

    fn input(
        body: &str,
        artifact_grants: Vec<VcpCliArtifactGrantInput>,
    ) -> VcpCliRiverProjectionInput {
        let expected_artifact_grants = artifact_grants.len();
        VcpCliRiverProjectionInput {
            canonical_json: body.to_string(),
            sha256: format!("{:x}", Sha256::digest(body.as_bytes())),
            size_bytes: body.len() as u64,
            artifact_grants,
            expected_artifact_grants,
        }
    }

    fn empty_input() -> VcpCliRiverProjectionInput {
        input(
            r#"{"schema":"vcp.mobile.attempt-projection.v1","river":{"mode":"text","messages":[],"truncated":false},"artifacts":[],"omissions":[]}"#,
            Vec::new(),
        )
    }

    #[test]
    fn strict_attempt_layout_is_written_verified_and_removed() {
        let root = tempfile::tempdir().expect("projection root");
        let projection = prepare_river_projection(
            root.path(),
            root.path(),
            7,
            "job",
            "attempt",
            &empty_input(),
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
        let mut invalid = empty_input();
        invalid.sha256 = "0".repeat(64);
        assert!(
            prepare_river_projection(root.path(), root.path(), 1, "job", "attempt", &invalid)
                .is_err()
        );
        std::os::unix::fs::symlink(root.path(), root.path().join("a".repeat(64)))
            .expect("malicious symlink");
        assert!(gc_stale_river_projections(root.path()).is_err());
    }

    #[test]
    fn artifact_is_copied_by_hash_and_guest_mutation_never_writes_back() {
        let projection_root = tempfile::tempdir().expect("projection root");
        let attachments_root = tempfile::tempdir().expect("attachment root");
        let source_bytes = b"canonical attachment";
        let hash = format!("{:x}", Sha256::digest(source_bytes));
        let source = attachments_root.path().join(format!("{hash}.png"));
        fs::write(&source, source_bytes).expect("write canonical source");
        let file_name = format!("river-artifact-00-{}.png", &hash[..12]);
        let guest_path = format!("/run/{file_name}");
        let body = serde_json::json!({
            "schema": ATTEMPT_PROJECTION_SCHEMA,
            "river": {"mode":"full", "messages":[], "truncated":false},
            "artifacts": [{
                "id": format!("river-artifact-00-{}", &hash[..12]),
                "name": "photo.png",
                "mime_type": "image/png",
                "size_bytes": source_bytes.len(),
                "sha256": hash,
                "guest_path": guest_path,
                "source_unreachable": true,
                "non_writeback": true
            }],
            "omissions": []
        })
        .to_string();
        let projection = prepare_river_projection(
            projection_root.path(),
            attachments_root.path(),
            2,
            "job",
            "attempt",
            &input(
                &body,
                vec![VcpCliArtifactGrantInput {
                    source_path: source.clone(),
                    file_name: file_name.clone(),
                    guest_path: guest_path.clone(),
                    size_bytes: source_bytes.len() as u64,
                    sha256: hash.clone(),
                }],
            ),
        )
        .expect("prepare full projection");
        assert_eq!(projection.artifacts.len(), 1);
        assert_eq!(projection.artifacts[0].guest_path, guest_path);
        let snapshot = PathBuf::from(&projection.artifacts[0].host_path);
        assert_eq!(fs::read(&snapshot).expect("read snapshot"), source_bytes);
        fs::write(&snapshot, b"guest mutation").expect("mutate attempt copy");
        assert_eq!(
            fs::read(&source).expect("read canonical source"),
            source_bytes
        );

        remove_river_projection(
            projection_root.path(),
            2,
            "job",
            "attempt",
            Path::new(&projection.host_path),
        )
        .expect("remove full projection");
        assert!(!snapshot.exists());
        assert!(source.exists());
    }
}
