//! P1 输出增量读取与 opaque cursor；cursor 严格绑定 generation/job/attempt/双流 offset。

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

#[cfg(test)]
use super::protocol::DEFAULT_BOUNDED_READ_BYTES;
use super::protocol::MAX_BOUNDED_READ_BYTES;

const MAX_OUTPUT_GC_ENTRIES: usize = 2048;
const MAX_WORKSPACE_ENTRIES: usize = 100_000;
const ARTIFACT_HASH_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct OutputCursor {
    #[serde(rename = "v")]
    version: u8,
    #[serde(rename = "g")]
    runtime_generation: u64,
    #[serde(rename = "j")]
    job_id: String,
    #[serde(rename = "a")]
    attempt_id: String,
    #[serde(rename = "o")]
    stdout_offset: u64,
    #[serde(rename = "e")]
    stderr_offset: u64,
    #[serde(rename = "x")]
    stdout_projection: OutputProjectionState,
    #[serde(rename = "y")]
    stderr_projection: OutputProjectionState,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TerminalControlState {
    #[default]
    Normal,
    Escape,
    Csi,
    String,
    StringEscape,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct OutputProjectionState {
    #[serde(rename = "c")]
    control: TerminalControlState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OutputArtifactDigest {
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OutputChunk {
    pub stdout: String,
    pub stderr: String,
    pub cursor: String,
    pub stdout_offset: u64,
    pub stderr_offset: u64,
    pub truncated: bool,
    pub safety_projected: bool,
}

pub(super) struct OutputReadRequest<'a> {
    pub output_root: &'a Path,
    pub stdout_path: &'a Path,
    pub stderr_path: &'a Path,
    pub runtime_generation: u64,
    pub job_id: &'a str,
    pub attempt_id: &'a str,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub cursor: Option<&'a str>,
    pub max_output_bytes: usize,
    pub source_truncated: bool,
    pub source_terminal: bool,
}

pub(super) async fn read_incremental_output(
    request: OutputReadRequest<'_>,
) -> Result<OutputChunk, String> {
    let cursor = match request.cursor {
        Some(cursor) => decode_cursor(cursor)?,
        None => OutputCursor {
            version: 2,
            runtime_generation: request.runtime_generation,
            job_id: request.job_id.to_string(),
            attempt_id: request.attempt_id.to_string(),
            stdout_offset: 0,
            stderr_offset: 0,
            stdout_projection: OutputProjectionState::default(),
            stderr_projection: OutputProjectionState::default(),
        },
    };
    validate_cursor_binding(&cursor, &request)?;
    if cursor.stdout_offset > request.stdout_bytes || cursor.stderr_offset > request.stderr_bytes {
        return Err("output cursor is beyond the persisted output snapshot".to_string());
    }

    let stdout_path = validate_output_file(request.output_root, request.stdout_path)?;
    let stderr_path = validate_output_file(request.output_root, request.stderr_path)?;
    let maximum = request.max_output_bytes.clamp(1, MAX_BOUNDED_READ_BYTES);
    let stdout_quota = maximum.div_ceil(2);
    let stdout_available = request.stdout_bytes.saturating_sub(cursor.stdout_offset);
    let stdout_limit = (stdout_available.min(stdout_quota as u64)) as usize;
    let stdout = read_utf8_chunk(
        &stdout_path,
        cursor.stdout_offset,
        stdout_limit,
        request.source_terminal && stdout_limit as u64 == stdout_available,
        cursor.stdout_projection.clone(),
    )
    .await?;

    let stderr_quota = maximum.saturating_sub(stdout.consumed);
    let stderr_available = request.stderr_bytes.saturating_sub(cursor.stderr_offset);
    let stderr_limit = (stderr_available.min(stderr_quota as u64)) as usize;
    let stderr = read_utf8_chunk(
        &stderr_path,
        cursor.stderr_offset,
        stderr_limit,
        request.source_terminal && stderr_limit as u64 == stderr_available,
        cursor.stderr_projection.clone(),
    )
    .await?;

    let stdout_offset = cursor
        .stdout_offset
        .checked_add(stdout.consumed as u64)
        .ok_or_else(|| "stdout cursor overflow".to_string())?;
    let stderr_offset = cursor
        .stderr_offset
        .checked_add(stderr.consumed as u64)
        .ok_or_else(|| "stderr cursor overflow".to_string())?;
    let next = OutputCursor {
        stdout_offset,
        stderr_offset,
        stdout_projection: stdout.projection,
        stderr_projection: stderr.projection,
        ..cursor
    };
    Ok(OutputChunk {
        stdout: stdout.text,
        stderr: stderr.text,
        cursor: encode_cursor(&next)?,
        stdout_offset,
        stderr_offset,
        truncated: request.source_truncated
            || stdout.truncated
            || stderr.truncated
            || stdout.projected
            || stderr.projected,
        safety_projected: stdout.projected || stderr.projected,
    })
}

pub(super) fn initial_cursor(
    runtime_generation: u64,
    job_id: &str,
    attempt_id: &str,
) -> Result<String, String> {
    encode_cursor(&OutputCursor {
        version: 2,
        runtime_generation,
        job_id: job_id.to_string(),
        attempt_id: attempt_id.to_string(),
        stdout_offset: 0,
        stderr_offset: 0,
        stdout_projection: OutputProjectionState::default(),
        stderr_projection: OutputProjectionState::default(),
    })
}

fn validate_cursor_binding(
    cursor: &OutputCursor,
    request: &OutputReadRequest<'_>,
) -> Result<(), String> {
    if cursor.version != 2
        || cursor.runtime_generation != request.runtime_generation
        || cursor.job_id != request.job_id
        || cursor.attempt_id != request.attempt_id
    {
        return Err("output cursor does not belong to this job attempt".to_string());
    }
    Ok(())
}

fn encode_cursor(cursor: &OutputCursor) -> Result<String, String> {
    let payload = serde_json::to_vec(cursor)
        .map_err(|error| format!("cannot serialize output cursor: {error}"))?;
    let checksum = Sha256::digest(&payload);
    Ok(format!(
        "v1.{}.{}",
        URL_SAFE_NO_PAD.encode(payload),
        hex::encode(&checksum[..8])
    ))
}

fn decode_cursor(value: &str) -> Result<OutputCursor, String> {
    if value.len() > 512 {
        return Err("output cursor is too long".to_string());
    }
    let mut parts = value.split('.');
    let version = parts.next();
    let payload = parts.next();
    let checksum = parts.next();
    if version != Some("v1") || parts.next().is_some() {
        return Err("invalid output cursor format".to_string());
    }
    let payload = URL_SAFE_NO_PAD
        .decode(payload.unwrap_or_default())
        .map_err(|_| "invalid output cursor payload".to_string())?;
    let expected = hex::encode(&Sha256::digest(&payload)[..8]);
    if checksum != Some(expected.as_str()) {
        return Err("output cursor checksum mismatch".to_string());
    }
    serde_json::from_slice(&payload).map_err(|_| "invalid output cursor data".to_string())
}

struct Utf8Chunk {
    text: String,
    consumed: usize,
    truncated: bool,
    projected: bool,
    projection: OutputProjectionState,
}

async fn read_utf8_chunk(
    path: &Path,
    offset: u64,
    limit: usize,
    source_terminal: bool,
    projection: OutputProjectionState,
) -> Result<Utf8Chunk, String> {
    if limit == 0 {
        return Ok(Utf8Chunk {
            text: String::new(),
            consumed: 0,
            truncated: false,
            projected: false,
            projection,
        });
    }
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| format!("cannot open CLI output: {error}"))?;
    file.seek(io::SeekFrom::Start(offset))
        .await
        .map_err(|error| format!("cannot seek CLI output: {error}"))?;
    let mut bytes = Vec::with_capacity(limit);
    file.take(limit as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| format!("cannot read CLI output: {error}"))?;
    let (text, consumed, truncated) = decode_utf8_incremental(&bytes, source_terminal);
    let (text, projection, projected) = project_safe_output(
        &text,
        projection,
        source_terminal && consumed == bytes.len(),
    );
    Ok(Utf8Chunk {
        text,
        consumed,
        truncated,
        projected,
        projection,
    })
}

fn decode_utf8_incremental(bytes: &[u8], source_terminal: bool) -> (String, usize, bool) {
    let mut text = String::new();
    let mut cursor = 0_usize;
    let mut replacement_used = false;
    while cursor < bytes.len() {
        match std::str::from_utf8(&bytes[cursor..]) {
            Ok(valid) => {
                text.push_str(valid);
                cursor = bytes.len();
            }
            Err(error) => {
                let valid_end = cursor + error.valid_up_to();
                if valid_end > cursor {
                    // `valid_up_to` is guaranteed to be a UTF-8 boundary.
                    if let Ok(valid) = std::str::from_utf8(&bytes[cursor..valid_end]) {
                        text.push_str(valid);
                    }
                    cursor = valid_end;
                }
                match error.error_len() {
                    Some(invalid_bytes) => {
                        text.push('\u{fffd}');
                        replacement_used = true;
                        cursor = cursor.saturating_add(invalid_bytes).min(bytes.len());
                    }
                    None if source_terminal => {
                        text.push('\u{fffd}');
                        replacement_used = true;
                        cursor = bytes.len();
                    }
                    None => break,
                }
            }
        }
    }
    (text, cursor, replacement_used)
}

fn project_safe_output(
    text: &str,
    mut state: OutputProjectionState,
    source_terminal: bool,
) -> (String, OutputProjectionState, bool) {
    let mut output = String::with_capacity(text.len());
    let mut projected = false;
    for character in text.chars() {
        match state.control {
            TerminalControlState::Normal => match character {
                '\u{1b}' => {
                    state.control = TerminalControlState::Escape;
                    projected = true;
                }
                '\u{009b}' => {
                    state.control = TerminalControlState::Csi;
                    projected = true;
                }
                '\u{0090}' | '\u{0098}' | '\u{009d}' | '\u{009e}' | '\u{009f}' => {
                    state.control = TerminalControlState::String;
                    projected = true;
                }
                '\n' | '\t' => output.push(character),
                value
                    if value <= '\u{001f}'
                        || value == '\u{007f}'
                        || ('\u{0080}'..='\u{009f}').contains(&value) =>
                {
                    projected = true;
                }
                value => output.push(value),
            },
            TerminalControlState::Escape => {
                projected = true;
                state.control = match character {
                    '[' => TerminalControlState::Csi,
                    ']' | 'P' | 'X' | '^' | '_' => TerminalControlState::String,
                    _ => TerminalControlState::Normal,
                };
            }
            TerminalControlState::Csi => {
                projected = true;
                if ('\u{0040}'..='\u{007e}').contains(&character) {
                    state.control = TerminalControlState::Normal;
                }
            }
            TerminalControlState::String => {
                projected = true;
                state.control = match character {
                    '\u{0007}' | '\u{009c}' => TerminalControlState::Normal,
                    '\u{001b}' => TerminalControlState::StringEscape,
                    _ => TerminalControlState::String,
                };
            }
            TerminalControlState::StringEscape => {
                projected = true;
                state.control = if character == '\\' {
                    TerminalControlState::Normal
                } else if character == '\u{001b}' {
                    TerminalControlState::StringEscape
                } else {
                    TerminalControlState::String
                };
            }
        }
    }

    if source_terminal && state.control != TerminalControlState::Normal {
        projected = true;
        state.control = TerminalControlState::Normal;
    }
    (output, state, projected)
}

pub(super) fn hash_output_artifact_pair(
    output_root: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
    stdout_bytes: u64,
    stderr_bytes: u64,
) -> Result<OutputArtifactDigest, String> {
    let stdout_path = validate_output_file(output_root, stdout_path)?;
    let stderr_path = validate_output_file(output_root, stderr_path)?;
    let size_bytes = stdout_bytes
        .checked_add(stderr_bytes)
        .ok_or_else(|| "CLI output artifact size overflow".to_string())?;
    let mut hasher = Sha256::new();
    hasher.update(b"VCPMobileCLIOutputPair\0v1\0");
    let mut buffer = vec![0_u8; ARTIFACT_HASH_BUFFER_BYTES];
    hash_artifact_stream(
        &mut hasher,
        &mut buffer,
        b"stdout\0",
        &stdout_path,
        stdout_bytes,
    )?;
    hash_artifact_stream(
        &mut hasher,
        &mut buffer,
        b"stderr\0",
        &stderr_path,
        stderr_bytes,
    )?;
    Ok(OutputArtifactDigest {
        sha256: hex::encode(hasher.finalize()),
        size_bytes,
    })
}

fn hash_artifact_stream(
    hasher: &mut Sha256,
    buffer: &mut [u8],
    label: &[u8],
    path: &Path,
    expected_bytes: u64,
) -> Result<(), String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("cannot inspect CLI output artifact: {error}"))?;
    if metadata.len() < expected_bytes {
        return Err(format!(
            "CLI output artifact is shorter than its persisted snapshot: {} < {expected_bytes}",
            metadata.len()
        ));
    }
    hasher.update(label);
    hasher.update(expected_bytes.to_be_bytes());
    let mut file = std::fs::File::open(path)
        .map_err(|error| format!("cannot open CLI output artifact: {error}"))?;
    let mut remaining = expected_bytes;
    while remaining > 0 {
        let wanted = remaining.min(buffer.len() as u64) as usize;
        let read = file
            .read(&mut buffer[..wanted])
            .map_err(|error| format!("cannot hash CLI output artifact: {error}"))?;
        if read == 0 {
            return Err("CLI output artifact ended before its persisted snapshot".to_string());
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }
    Ok(())
}

fn validate_output_file(output_root: &Path, path: &Path) -> Result<PathBuf, String> {
    if !output_root.is_absolute() || !path.is_absolute() {
        return Err("CLI output paths must be absolute".to_string());
    }
    let root_metadata = std::fs::symlink_metadata(output_root)
        .map_err(|error| format!("cannot inspect CLI output root: {error}"))?;
    if !root_metadata.file_type().is_dir() {
        return Err("CLI output root must be a real directory".to_string());
    }
    let relative = path
        .strip_prefix(output_root)
        .map_err(|_| "CLI output path escapes its private root".to_string())?;
    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty() {
        return Err("CLI output path cannot be the output root".to_string());
    }
    let mut current = output_root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(value) = component else {
            return Err("CLI output path contains an unsafe component".to_string());
        };
        current.push(value);
        let metadata = std::fs::symlink_metadata(&current)
            .map_err(|error| format!("cannot inspect CLI output path: {error}"))?;
        if index + 1 == components.len() {
            if !metadata.file_type().is_file() {
                return Err("CLI output must be a real regular file".to_string());
            }
        } else if !metadata.file_type().is_dir() {
            return Err("CLI output parent must be a real directory".to_string());
        }
    }
    Ok(current)
}

pub(super) fn remove_job_outputs(
    output_root: &Path,
    paths: impl IntoIterator<Item = PathBuf>,
) -> Result<(), String> {
    require_real_directory(output_root, "CLI output root")?;
    for path in paths {
        remove_scoped_output_file(output_root, &path)?;
    }
    sync_directory(output_root, "CLI output root")
}

pub(super) fn gc_orphan_outputs(
    output_root: &Path,
    referenced_paths: &BTreeSet<PathBuf>,
) -> Result<usize, String> {
    require_real_directory(output_root, "CLI output root")?;
    let entries = std::fs::read_dir(output_root)
        .map_err(|error| format!("cannot scan CLI output root: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("cannot inspect CLI output entry: {error}"))?;
    if entries.len() > MAX_OUTPUT_GC_ENTRIES {
        return Err("CLI output GC entry limit exceeded".to_string());
    }
    let mut removed = 0;
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| "CLI output filename must be UTF-8".to_string())?;
        if !name.ends_with(".stdout") && !name.ends_with(".stderr") {
            return Err("unexpected entry in private CLI output root".to_string());
        }
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot inspect CLI output entry: {error}"))?;
        if !metadata.file_type().is_file() {
            return Err("CLI output entry must be a real regular file".to_string());
        }
        if !referenced_paths.contains(&path) {
            std::fs::remove_file(&path)
                .map_err(|error| format!("cannot remove orphan CLI output: {error}"))?;
            removed += 1;
        }
    }
    sync_directory(output_root, "CLI output root")?;
    Ok(removed)
}

pub(super) fn workspace_usage_bytes(workspace: &Path, hard_limit: u64) -> Result<u64, String> {
    require_real_directory(workspace, "CLI workspace")?;
    let mut pending = vec![workspace.to_path_buf()];
    let mut entries = 0_usize;
    let mut bytes = 0_u64;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)
            .map_err(|error| format!("cannot scan CLI workspace: {error}"))?
        {
            let entry =
                entry.map_err(|error| format!("cannot inspect workspace entry: {error}"))?;
            entries = entries
                .checked_add(1)
                .ok_or_else(|| "workspace entry count overflow".to_string())?;
            if entries > MAX_WORKSPACE_ENTRIES {
                return Err("workspace entry count exceeds its hard limit".to_string());
            }
            let metadata = std::fs::symlink_metadata(entry.path())
                .map_err(|error| format!("cannot inspect workspace metadata: {error}"))?;
            let file_type = metadata.file_type();
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                bytes = bytes
                    .checked_add(metadata.len())
                    .ok_or_else(|| "workspace size overflow".to_string())?;
                if bytes > hard_limit {
                    return Err(format!(
                        "workspace_budget_exceeded: {bytes} bytes exceeds {hard_limit}"
                    ));
                }
            } else if file_type.is_symlink() {
                // Never follow guest-created links. Their link text is negligible and cannot
                // escape the accounting walk.
            } else {
                return Err("special file in CLI workspace is not allowed".to_string());
            }
        }
    }
    Ok(bytes)
}

#[cfg(unix)]
pub(super) fn require_filesystem_headroom(
    paths: &[PathBuf],
    minimum_available_bytes: u64,
) -> Result<(), String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    for path in paths {
        let path_bytes = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| "runtime storage path contains NUL".to_string())?;
        let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::statvfs(path_bytes.as_ptr(), stats.as_mut_ptr()) } != 0 {
            return Err(format!(
                "cannot inspect runtime filesystem capacity: {}",
                io::Error::last_os_error()
            ));
        }
        let stats = unsafe { stats.assume_init() };
        let available = (stats.f_bavail as u128)
            .checked_mul(stats.f_frsize as u128)
            .ok_or_else(|| "runtime filesystem capacity overflow".to_string())?;
        if available < minimum_available_bytes as u128 {
            return Err(format!(
                "runtime_storage_low: {} has {} available bytes; requires at least {}",
                path.display(),
                available,
                minimum_available_bytes
            ));
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn require_filesystem_headroom(
    _paths: &[PathBuf],
    _minimum_available_bytes: u64,
) -> Result<(), String> {
    Err("runtime filesystem capacity checks require Android/Unix statvfs".to_string())
}

fn remove_scoped_output_file(output_root: &Path, path: &Path) -> Result<(), String> {
    let relative = path
        .strip_prefix(output_root)
        .map_err(|_| "CLI output cleanup path escapes its private root".to_string())?;
    if relative.components().count() != 1
        || !matches!(relative.components().next(), Some(Component::Normal(_)))
    {
        return Err("CLI output cleanup path is not a direct artifact".to_string());
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => std::fs::remove_file(path)
            .map_err(|error| format!("cannot remove CLI output artifact: {error}")),
        Ok(_) => Err("CLI output cleanup target is not a regular file".to_string()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot inspect CLI output cleanup target: {error}")),
    }
}

fn require_real_directory(path: &Path, label: &str) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {label}: {error}"))?;
    if !metadata.file_type().is_dir() {
        return Err(format!("{label} must be a real directory"));
    }
    Ok(())
}

fn sync_directory(path: &Path, label: &str) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("cannot sync {label}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_is_checksum_and_attempt_bound() {
        let cursor = initial_cursor(9, "job-1", "attempt-1").expect("encode cursor");
        assert!(cursor.len() <= 512);
        let decoded = decode_cursor(&cursor).expect("decode cursor");
        assert_eq!(decoded.runtime_generation, 9);
        assert_eq!(decoded.attempt_id, "attempt-1");
        let mut tampered = cursor;
        tampered.push('x');
        assert!(decode_cursor(&tampered).is_err());
    }

    #[tokio::test]
    async fn poll_reads_bounded_incremental_stdout_and_stderr() {
        let directory = tempfile::tempdir().expect("temporary output root");
        let stdout = directory.path().join("stdout");
        let stderr = directory.path().join("stderr");
        tokio::fs::write(&stdout, b"abcdefgh")
            .await
            .expect("write stdout");
        tokio::fs::write(&stderr, b"WXYZ")
            .await
            .expect("write stderr");
        let first = read_incremental_output(OutputReadRequest {
            output_root: directory.path(),
            stdout_path: &stdout,
            stderr_path: &stderr,
            runtime_generation: 1,
            job_id: "job-1",
            attempt_id: "attempt-1",
            stdout_bytes: 8,
            stderr_bytes: 4,
            cursor: None,
            max_output_bytes: 8,
            source_truncated: false,
            source_terminal: false,
        })
        .await
        .expect("first poll");
        assert_eq!(first.stdout, "abcd");
        assert_eq!(first.stderr, "WXYZ");
        let second = read_incremental_output(OutputReadRequest {
            output_root: directory.path(),
            stdout_path: &stdout,
            stderr_path: &stderr,
            runtime_generation: 1,
            job_id: "job-1",
            attempt_id: "attempt-1",
            stdout_bytes: 8,
            stderr_bytes: 4,
            cursor: Some(&first.cursor),
            max_output_bytes: DEFAULT_BOUNDED_READ_BYTES,
            source_truncated: false,
            source_terminal: true,
        })
        .await
        .expect("second poll");
        assert_eq!(second.stdout, "efgh");
        assert!(second.stderr.is_empty());
        assert_eq!(second.stdout_offset, 8);
        assert_eq!(second.stderr_offset, 4);
    }

    #[tokio::test]
    async fn utf8_sequence_split_at_cursor_boundary_is_replayed_without_replacement() {
        let directory = tempfile::tempdir().expect("temporary output root");
        let stdout = directory.path().join("unicode.stdout");
        let stderr = directory.path().join("unicode.stderr");
        tokio::fs::write(&stdout, "a你b".as_bytes())
            .await
            .expect("write Unicode stdout");
        tokio::fs::write(&stderr, b"")
            .await
            .expect("write empty stderr");
        let first = read_incremental_output(OutputReadRequest {
            output_root: directory.path(),
            stdout_path: &stdout,
            stderr_path: &stderr,
            runtime_generation: 1,
            job_id: "job-unicode",
            attempt_id: "attempt-unicode",
            stdout_bytes: 5,
            stderr_bytes: 0,
            cursor: None,
            max_output_bytes: 3,
            source_truncated: false,
            source_terminal: false,
        })
        .await
        .expect("first Unicode poll");
        assert_eq!(first.stdout, "a");
        assert_eq!(first.stdout_offset, 1);
        assert!(!first.truncated);

        let second = read_incremental_output(OutputReadRequest {
            output_root: directory.path(),
            stdout_path: &stdout,
            stderr_path: &stderr,
            runtime_generation: 1,
            job_id: "job-unicode",
            attempt_id: "attempt-unicode",
            stdout_bytes: 5,
            stderr_bytes: 0,
            cursor: Some(&first.cursor),
            max_output_bytes: 8,
            source_truncated: false,
            source_terminal: true,
        })
        .await
        .expect("second Unicode poll");
        assert_eq!(second.stdout, "你b");
        assert_eq!(second.stdout_offset, 5);
        assert!(!second.truncated);
    }

    #[test]
    fn output_projection_strips_controls_and_preserves_diagnostics_across_chunks() {
        let (first, state, projected) = project_safe_output(
            "safe\u{1b}[31m red\u{1b}[0m\nAuth",
            OutputProjectionState::default(),
            false,
        );
        assert_eq!(first, "safe red\nAuth");
        assert!(projected);

        let (second, state, projected) = project_safe_output(
            "orization: Bearer top-secret\nnext\u{1b}]0;forged",
            state,
            false,
        );
        assert_eq!(second, "orization: Bearer top-secret\nnext");
        assert!(projected);

        let (third, state, projected) =
            project_safe_output(" title\u{7}\nTOKEN=hidden\nvisible", state, true);
        assert_eq!(third, "\nTOKEN=hidden\nvisible");
        assert_eq!(state, OutputProjectionState::default());
        assert!(projected);
        let combined = format!("{first}{second}{third}");
        assert!(combined.contains("top-secret"));
        assert!(!combined.contains("forged"));
        assert!(combined.contains("hidden"));
        assert!(!combined.contains('\u{1b}'));
    }

    #[tokio::test]
    async fn diagnostics_survive_an_opaque_cursor_boundary() {
        let directory = tempfile::tempdir().expect("temporary output root");
        let stdout = directory.path().join("diagnostic.stdout");
        let stderr = directory.path().join("diagnostic.stderr");
        let raw = b"ok\nAuthorization: Bearer cursor-secret\nafter\n";
        tokio::fs::write(&stdout, raw).await.expect("write stdout");
        tokio::fs::write(&stderr, b"").await.expect("write stderr");
        let mut cursor = None;
        let mut projected = String::new();
        let mut offset = 0;
        while offset < raw.len() as u64 {
            let chunk = read_incremental_output(OutputReadRequest {
                output_root: directory.path(),
                stdout_path: &stdout,
                stderr_path: &stderr,
                runtime_generation: 3,
                job_id: "job-diagnostic",
                attempt_id: "attempt-diagnostic",
                stdout_bytes: raw.len() as u64,
                stderr_bytes: 0,
                cursor: cursor.as_deref(),
                max_output_bytes: 10,
                source_truncated: false,
                source_terminal: true,
            })
            .await
            .expect("read projected chunk");
            assert!(chunk.cursor.len() <= 512);
            projected.push_str(&chunk.stdout);
            offset = chunk.stdout_offset;
            cursor = Some(chunk.cursor);
        }
        assert_eq!(projected, String::from_utf8_lossy(raw));
        assert!(projected.contains("cursor-secret"));
    }

    #[test]
    fn private_key_text_is_preserved() {
        let (first, state, _) = project_safe_output(
            "before\n-----BEGIN RSA PRIVATE KEY-----\nsecret-material",
            OutputProjectionState::default(),
            false,
        );
        assert_eq!(
            first,
            "before\n-----BEGIN RSA PRIVATE KEY-----\nsecret-material"
        );
        let (second, _, projected) = project_safe_output(
            "\nstill-secret\n-----END RSA PRIVATE KEY-----\nafter",
            state,
            true,
        );
        assert_eq!(
            second,
            "\nstill-secret\n-----END RSA PRIVATE KEY-----\nafter"
        );
        assert!(!projected);
    }

    #[test]
    fn output_artifact_hashes_the_exact_framed_stream_pair() {
        let directory = tempfile::tempdir().expect("temporary output root");
        let stdout = directory.path().join("job.stdout");
        let stderr = directory.path().join("job.stderr");
        std::fs::write(&stdout, b"stdout-extra").expect("write stdout");
        std::fs::write(&stderr, b"stderr").expect("write stderr");
        let first = hash_output_artifact_pair(directory.path(), &stdout, &stderr, 6, 6)
            .expect("hash artifact pair");
        assert_eq!(first.size_bytes, 12);
        assert_eq!(
            first.sha256,
            "57df37a083f1cb73b61b67ce3cb9d864129f8759a1853fec80f240ac8580a204"
        );

        std::fs::write(&stderr, b"stderX").expect("mutate stderr");
        let mutated = hash_output_artifact_pair(directory.path(), &stdout, &stderr, 6, 6)
            .expect("hash mutated artifact pair");
        assert_ne!(first.sha256, mutated.sha256);
        assert!(hash_output_artifact_pair(directory.path(), &stdout, &stderr, 99, 6).is_err());
    }

    #[test]
    fn orphan_gc_is_scoped_and_workspace_budget_is_enforced() {
        let output = tempfile::tempdir().expect("temporary output root");
        let kept = output.path().join("kept.stdout");
        let orphan = output.path().join("orphan.stderr");
        std::fs::write(&kept, b"keep").expect("write kept output");
        std::fs::write(&orphan, b"remove").expect("write orphan output");
        let referenced = BTreeSet::from([kept.clone()]);
        assert_eq!(
            gc_orphan_outputs(output.path(), &referenced).expect("run output GC"),
            1
        );
        assert!(kept.exists());
        assert!(!orphan.exists());

        let workspace = tempfile::tempdir().expect("temporary workspace");
        std::fs::write(workspace.path().join("data"), vec![0_u8; 9]).expect("write workspace data");
        assert_eq!(
            workspace_usage_bytes(workspace.path(), 9).expect("measure workspace"),
            9
        );
        assert!(workspace_usage_bytes(workspace.path(), 8)
            .expect_err("workspace must exceed budget")
            .contains("workspace_budget_exceeded"));
    }
}
