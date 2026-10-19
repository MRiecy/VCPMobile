//! Imported voice references belong to the application, independently of the
//! native picker's temporary copy. A narration reads and encodes its reference
//! once; subsequent segments share the resulting data URL even after replacement.

use crate::vcp_modules::settings_manager;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
};
use tauri::{AppHandle, Manager, Runtime};
use tokio::{io::AsyncReadExt, sync::Mutex};
use tokio_util::sync::CancellationToken;

const MAX_BASE64_BYTES: usize = 10_000_000;
pub(super) const MAX_SAMPLE_BYTES: u64 = 7_500_000;
const MAX_METADATA_BYTES: u64 = 4096;
const MAX_RECOVERY_ENTRIES: usize = 256;
static SAMPLE_FILES: LazyLock<Mutex<SampleFiles>> =
    LazyLock::new(|| Mutex::new(SampleFiles::default()));

#[derive(Default)]
struct SampleFiles {
    recovered_roots: HashSet<PathBuf>,
}

impl SampleFiles {
    async fn recover_from_snapshot(
        &mut self,
        root: &Path,
        snapshot: &settings_manager::ConfigSnapshot,
    ) -> Result<(), String> {
        match selected_sample_id(snapshot) {
            Some(selected) => self.recover(root, selected).await,
            None => {
                // The original persisted reference is unknown. Leave existing
                // files alone for this process, including any drafts imported
                // to repair the configuration before its next successful read.
                self.recovered_roots.insert(root.to_owned());
                Ok(())
            }
        }
    }

    async fn discard_from_snapshot(
        &mut self,
        root: &Path,
        id: &str,
        snapshot: &settings_manager::ConfigSnapshot,
    ) -> Result<(), String> {
        let path = sample_path(root, id)?;
        self.recover_from_snapshot(root, snapshot).await?;
        let Some(selected) = selected_sample_id(snapshot) else {
            return Ok(());
        };
        if selected == id {
            return Ok(());
        }
        remove_sample_files(&path).await
    }

    async fn recover(&mut self, root: &Path, selected: &str) -> Result<(), String> {
        if self.recovered_roots.contains(root) {
            return Ok(());
        }
        let mut entries = match tokio::fs::read_dir(root).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                self.recovered_roots.insert(root.to_owned());
                return Ok(());
            }
            Err(error) => return Err(error.to_string()),
        };
        // This runs before the first import/reader for this root. Subsequent
        // accesses preserve all in-process drafts, even when not selected yet.
        for _ in 0..MAX_RECOVERY_ENTRIES {
            let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? else {
                self.recovered_roots.insert(root.to_owned());
                return Ok(());
            };
            if !entry.file_type().await.map_err(|e| e.to_string())?.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name == selected {
                continue;
            }
            let id = name.strip_prefix(".pending-").unwrap_or(name);
            if sample_path(root, id).is_err() {
                continue;
            }
            if let Err(error) = remove_sample_files(&entry.path()).await {
                log::warn!("[VoiceSample] orphan cleanup skipped: {error}");
            }
        }
        // A pathological directory cannot delay first narration indefinitely.
        // Any remainder remains eligible on the next process start.
        log::warn!("[VoiceSample] startup cleanup reached its entry budget");
        self.recovered_roots.insert(root.to_owned());
        Ok(())
    }

    async fn recover_from_settings<R: Runtime>(
        &mut self,
        app: &AppHandle<R>,
        root: &Path,
    ) -> Result<(), String> {
        if self.recovered_roots.contains(root) {
            return Ok(());
        }
        let snapshot = settings_manager::read_config_snapshot(app.clone(), app.state()).await?;
        self.recover_from_snapshot(root, &snapshot).await
    }
}

/// None means that the effective default cannot establish which file the raw
/// persisted configuration still owns. An empty but healthy ID means no owner.
fn selected_sample_id(snapshot: &settings_manager::ConfigSnapshot) -> Option<&str> {
    use settings_manager::{ConfigDomain, DomainHealth};
    let status = snapshot.domains.get(&ConfigDomain::Voice)?;
    if status.health == DomainHealth::Unavailable
        || status.issues.iter().any(|issue| {
            issue.field.is_empty()
                || issue.field == "tts"
                || issue.field == "tts.cloneSampleId"
                || issue.field.starts_with("tts.cloneSampleId.")
        })
    {
        return None;
    }
    let selected = snapshot.data.voice.tts.clone_sample_id.as_str();
    if !selected.is_empty() && sample_path(Path::new(""), selected).is_err() {
        return None;
    }
    Some(selected)
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CloneSampleFormat {
    Mp3,
    Wav,
}

impl CloneSampleFormat {
    fn mime(self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::Wav => "audio/wav",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceCloneSample {
    pub id: String,
    pub file_name: String,
    pub format: CloneSampleFormat,
    pub bytes: u64,
}

#[derive(Clone)]
pub(super) struct PreparedCloneSample {
    pub data_url: Arc<str>,
}

pub(super) struct VoiceSettingsSnapshot {
    pub settings: Arc<settings_manager::Settings>,
    /// Only narration is affected by missing/corrupt reference audio. The chat
    /// request can keep using its settings and continue normally.
    pub sample: Result<Option<PreparedCloneSample>, String>,
}

fn sample_root<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("voice-samples"))
}

fn sample_path(root: &Path, id: &str) -> Result<PathBuf, String> {
    if id.len() != 32
        || !id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("声音样本标识无效，请重新导入。".into());
    }
    Ok(root.join(id))
}

async fn picked_path(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let root = tokio::fs::canonicalize(root)
        .await
        .map_err(|_| "文件选择临时目录不可用，请重新选择。")?;
    let path = tokio::fs::canonicalize(path)
        .await
        .map_err(|_| "选择的音频文件已不存在，请重新选择。")?;
    if path.parent() != Some(root.as_path()) {
        return Err("声音样本必须来自系统文件选择器。".into());
    }
    if !tokio::fs::metadata(&path)
        .await
        .map_err(|e| e.to_string())?
        .is_file()
    {
        return Err("选择的声音样本不是文件。".into());
    }
    Ok(path)
}

async fn bounded_read(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| format!("读取声音样本失败：{e}"))?;
    let metadata = file.metadata().await.map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err("声音样本过大，Base64 编码后不能超过 10 MB。".into());
    }
    let mut data = Vec::with_capacity(metadata.len() as usize);
    file.take(limit + 1)
        .read_to_end(&mut data)
        .await
        .map_err(|e| format!("读取声音样本失败：{e}"))?;
    if data.len() as u64 > limit {
        return Err("声音样本过大，Base64 编码后不能超过 10 MB。".into());
    }
    Ok(data)
}

fn valid_wav(bytes: &[u8]) -> bool {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return false;
    }
    let u32_at = |offset| {
        u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]) as usize
    };
    let Some(end) = u32_at(4)
        .checked_add(8)
        .filter(|end| *end <= bytes.len() && *end >= 12)
    else {
        return false;
    };
    let mut position = 12;
    let mut format = false;
    let mut audio = false;
    while position < end {
        if end - position < 8 {
            return false;
        }
        let size = u32_at(position + 4);
        let start = position + 8;
        let Some(chunk_end) = start
            .checked_add(size)
            .filter(|end_of_chunk| *end_of_chunk <= end)
        else {
            return false;
        };
        match &bytes[position..position + 4] {
            b"fmt " => {
                if size < 16 || format {
                    return false;
                }
                // Reference audio is passed through: stereo, other sample rates,
                // and float/extended WAV are not forced into the playback PCM DTO.
                let encoding = u16::from_le_bytes([bytes[start], bytes[start + 1]]);
                let channels = u16::from_le_bytes([bytes[start + 2], bytes[start + 3]]);
                if encoding == 0 || channels == 0 || u32_at(start + 4) == 0 {
                    return false;
                }
                format = true;
            }
            b"data" => audio |= size > 0,
            _ => {}
        }
        let Some(next) = chunk_end.checked_add(size % 2).filter(|next| *next <= end) else {
            return false;
        };
        position = next;
    }
    format && audio
}

fn valid_mp3(bytes: &[u8]) -> bool {
    let mut start = 0usize;
    if bytes.starts_with(b"ID3") {
        if bytes.len() < 10
            || !(2..=4).contains(&bytes[3])
            || bytes[6..10].iter().any(|byte| byte & 0x80 != 0)
        {
            return false;
        }
        let size = bytes[6..10]
            .iter()
            .fold(0usize, |size, byte| (size << 7) | usize::from(*byte));
        start = 10 + size;
        if bytes[3] == 4 && bytes[5] & 0x10 != 0 {
            start += 10;
        }
    }
    let Some(header) = bytes.get(start..start.saturating_add(4)) else {
        return false;
    };
    if header[0] != 0xff || header[1] & 0xe0 != 0xe0 || (header[1] >> 1) & 3 != 1 {
        return false;
    }
    let version = (header[1] >> 3) & 3;
    let bitrate_index = usize::from(header[2] >> 4);
    let rate_index = usize::from((header[2] >> 2) & 3);
    if version == 1 || bitrate_index == 0 || bitrate_index == 15 || rate_index == 3 {
        return false;
    }
    let bitrate = if version == 3 {
        [
            0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
        ][bitrate_index]
    } else {
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160][bitrate_index]
    };
    let mut rate = [44100, 48000, 32000][rate_index];
    if version == 2 {
        rate /= 2;
    }
    if version == 0 {
        rate /= 4;
    }
    let length = (if version == 3 { 144000 } else { 72000 }) * bitrate / rate
        + usize::from((header[2] >> 1) & 1);
    // Checking a complete Layer III frame rejects a renamed file or ID3-only
    // metadata without introducing a decoder or transcoding the user's sample.
    length >= 4
        && start
            .checked_add(length)
            .is_some_and(|end| end <= bytes.len())
}

fn detect_format(data: &[u8]) -> Result<CloneSampleFormat, String> {
    if data.is_empty() || data.len().div_ceil(3) * 4 > MAX_BASE64_BYTES {
        return Err("声音样本为空或过大，Base64 编码后不能超过 10 MB。".into());
    }
    if valid_wav(data) {
        return Ok(CloneSampleFormat::Wav);
    }
    if valid_mp3(data) {
        return Ok(CloneSampleFormat::Mp3);
    }
    Err("请选择包含有效音频的 MP3 或 WAV 文件。".into())
}

async fn remove_sample_files(directory: &Path) -> Result<(), String> {
    // Only these two application-owned files are removed. Unexpected contents
    // are never recursively deleted.
    for file in ["audio", "metadata.json"] {
        if let Err(error) = tokio::fs::remove_file(directory.join(file)).await {
            if error.kind() != ErrorKind::NotFound {
                return Err(error.to_string());
            }
        }
    }
    if let Err(error) = tokio::fs::remove_dir(directory).await {
        if error.kind() != ErrorKind::NotFound {
            return Err(error.to_string());
        }
    }
    Ok(())
}

fn display_name(name: Option<&str>, format: CloneSampleFormat) -> String {
    let name = name
        .unwrap_or("")
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim();
    let name: String = name
        .chars()
        .filter(|character| !character.is_control())
        .take(180)
        .collect();
    if name.is_empty() {
        match format {
            CloneSampleFormat::Mp3 => "声音样本.mp3",
            CloneSampleFormat::Wav => "声音样本.wav",
        }
        .into()
    } else {
        name
    }
}

async fn import_picked(
    root: &Path,
    source: &Path,
    name: Option<&str>,
) -> Result<VoiceCloneSample, String> {
    let bytes = bounded_read(source, MAX_SAMPLE_BYTES).await?;
    let format = detect_format(&bytes)?;
    tokio::fs::create_dir_all(root)
        .await
        .map_err(|e| e.to_string())?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let sample = VoiceCloneSample {
        id: id.clone(),
        file_name: display_name(name, format),
        format,
        bytes: bytes.len() as u64,
    };
    let destination = sample_path(root, &id)?;
    let pending = root.join(format!(".pending-{id}"));
    tokio::fs::create_dir(&pending)
        .await
        .map_err(|e| e.to_string())?;
    let result = async {
        tokio::fs::write(pending.join("audio"), bytes)
            .await
            .map_err(|e| e.to_string())?;
        let metadata = serde_json::to_vec(&sample).map_err(|e| e.to_string())?;
        tokio::fs::write(pending.join("metadata.json"), metadata)
            .await
            .map_err(|e| e.to_string())?;
        tokio::fs::rename(&pending, &destination)
            .await
            .map_err(|e| e.to_string())?;
        Ok(sample)
    }
    .await;
    if result.is_err() {
        let _ = remove_sample_files(&pending).await;
    }
    result
}

#[tauri::command]
pub async fn import_voice_clone_sample<R: Runtime>(
    app: AppHandle<R>,
    path: String,
    name: Option<String>,
) -> Result<VoiceCloneSample, String> {
    let staging = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("uploads");
    let source = picked_path(&staging, Path::new(&path)).await?;
    let result = async {
        let root = sample_root(&app)?;
        let mut guard = SAMPLE_FILES.lock().await;
        guard.recover_from_settings(&app, &root).await?;
        import_picked(&root, &source, name.as_deref()).await
    }
    .await;
    // Once admitted, this owner consumes the selected temporary copy regardless
    // of format, size or persistence failure. The original external file is untouched.
    let _ = tokio::fs::remove_file(source).await;
    result
}

async fn read_metadata(root: &Path, id: &str) -> Result<Option<VoiceCloneSample>, String> {
    let path = sample_path(root, id)?;
    match tokio::fs::metadata(path.join("metadata.json")).await {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
        Ok(_) => {}
    }
    let bytes = bounded_read(&path.join("metadata.json"), MAX_METADATA_BYTES).await?;
    let metadata: VoiceCloneSample =
        serde_json::from_slice(&bytes).map_err(|_| "声音样本信息损坏，请重新导入。")?;
    if metadata.id != id || metadata.bytes == 0 || metadata.bytes > MAX_SAMPLE_BYTES {
        return Err("声音样本信息损坏，请重新导入。".into());
    }
    match tokio::fs::metadata(path.join("audio")).await {
        Ok(file) if file.is_file() && file.len() == metadata.bytes => Ok(Some(metadata)),
        Err(error) if error.kind() != ErrorKind::NotFound => Err(error.to_string()),
        _ => Ok(None),
    }
}

#[tauri::command]
pub async fn get_voice_clone_sample<R: Runtime>(
    app: AppHandle<R>,
    sample_id: String,
) -> Result<Option<VoiceCloneSample>, String> {
    if sample_id.is_empty() {
        return Ok(None);
    }
    let root = sample_root(&app)?;
    let mut guard = SAMPLE_FILES.lock().await;
    guard.recover_from_settings(&app, &root).await?;
    read_metadata(&root, &sample_id).await
}

#[tauri::command]
pub async fn discard_voice_clone_sample<R: Runtime>(
    app: AppHandle<R>,
    sample_id: String,
) -> Result<(), String> {
    if sample_id.is_empty() {
        return Ok(());
    }
    let root = sample_root(&app)?;
    let mut guard = SAMPLE_FILES.lock().await;
    let snapshot = settings_manager::read_config_snapshot(app.clone(), app.state()).await?;
    guard
        .discard_from_snapshot(&root, &sample_id, &snapshot)
        .await
}

fn encode_sample(
    bytes: Vec<u8>,
    format: CloneSampleFormat,
    cancel: &CancellationToken,
) -> Result<PreparedCloneSample, String> {
    let mut data_url = String::with_capacity(bytes.len().div_ceil(3) * 4 + 32);
    data_url.push_str("data:");
    data_url.push_str(format.mime());
    data_url.push_str(";base64,");
    // Full chunks are divisible by three, so joining their encodings does not
    // introduce intermediate padding. Cancellation is checked between chunks.
    for chunk in bytes.chunks(48 * 1024) {
        if cancel.is_cancelled() {
            return Err(super::network::CANCELLED.into());
        }
        STANDARD.encode_string(chunk, &mut data_url);
    }
    if cancel.is_cancelled() {
        return Err(super::network::CANCELLED.into());
    }
    Ok(PreparedCloneSample {
        data_url: Arc::from(data_url),
    })
}

async fn read_sample(root: &Path, id: &str) -> Result<(Vec<u8>, CloneSampleFormat), String> {
    if id.is_empty() {
        return Err("请先导入声音复刻的参考音频。".into());
    }
    let path = sample_path(root, id)?;
    let metadata = read_metadata(root, id)
        .await?
        .ok_or("声音样本在本机不存在，请重新导入。")?;
    let bytes = bounded_read(&path.join("audio"), MAX_SAMPLE_BYTES).await?;
    if detect_format(&bytes)? != metadata.format || bytes.len() as u64 != metadata.bytes {
        return Err("声音样本文件已损坏，请重新导入。".into());
    }
    Ok((bytes, metadata.format))
}

async fn encode_prepared(
    bytes: Vec<u8>,
    format: CloneSampleFormat,
    cancel: &CancellationToken,
) -> Result<PreparedCloneSample, String> {
    let cancellation = cancel.clone();
    // Await the real worker to keep its memory/resource lifetime with this job.
    tokio::task::spawn_blocking(move || encode_sample(bytes, format, &cancellation))
        .await
        .map_err(|_| "声音样本编码任务异常结束。".to_string())?
}

pub(super) async fn prepare_clone_sample<R: Runtime>(
    app: &AppHandle<R>,
    id: &str,
    cancel: &CancellationToken,
) -> Result<PreparedCloneSample, String> {
    let root = sample_root(app)?;
    let mut guard = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(super::network::CANCELLED.into()),
        guard = SAMPLE_FILES.lock() => guard,
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(super::network::CANCELLED.into()),
        result = guard.recover_from_settings(app, &root) => result?,
    };
    let (bytes, format) = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(super::network::CANCELLED.into()),
        result = read_sample(&root, id) => result?,
    };
    drop(guard);
    encode_prepared(bytes, format, cancel).await
}

pub(super) async fn snapshot_voice_settings<R: Runtime>(
    app: &AppHandle<R>,
    cancel: &CancellationToken,
) -> Result<VoiceSettingsSnapshot, String> {
    // Lock ordering is sample files -> settings. Settings writers run reference
    // cleanup only after publishing the committed configuration. A replacement may commit
    // while this snapshot reads, but its cleanup cannot remove the captured file.
    let mut guard = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(super::network::CANCELLED.into()),
        guard = SAMPLE_FILES.lock() => guard,
    };
    let snapshot = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(super::network::CANCELLED.into()),
        result = settings_manager::read_config_snapshot(app.clone(), app.state()) => result?,
    };
    let root = sample_root(app);
    if let Ok(root) = &root {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(super::network::CANCELLED.into()),
            result = guard.recover_from_snapshot(root, &snapshot) => {
                // Recovery is best-effort maintenance. A failure must not stop
                // chat or narration that does not need a sample.
                if let Err(error) = result { log::warn!("[VoiceSample] cleanup unavailable: {error}"); }
            },
        }
    }
    let settings = Arc::new(snapshot.data);
    let audio = if settings.voice.tts.mode == settings_manager::VoiceTtsMode::Clone
        && !settings.voice.connection.api_key.trim().is_empty() {
        let read = async { read_sample(&root?, &settings.voice.tts.clone_sample_id).await };
        Some(tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(super::network::CANCELLED.into()),
            result = read => result,
        })
    } else {
        None
    };
    drop(guard);
    let sample = match audio {
        None => Ok(None),
        Some(Err(error)) => Err(error),
        Some(Ok((bytes, format))) => encode_prepared(bytes, format, cancel).await.map(Some),
    };
    if cancel.is_cancelled() {
        return Err(super::network::CANCELLED.into());
    }
    Ok(VoiceSettingsSnapshot { settings, sample })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection_snapshot(selected: &str) -> settings_manager::ConfigSnapshot {
        let mut data = settings_manager::Settings::default();
        data.voice.tts.clone_sample_id = selected.into();
        settings_manager::ConfigSnapshot {
            data,
            domains: std::collections::BTreeMap::from([(
                settings_manager::ConfigDomain::Voice,
                settings_manager::DomainStatus {
                    revision: "1".into(),
                    schema_version: 1,
                    issues: Vec::new(),
                    health: settings_manager::DomainHealth::Ready,
                },
            )]),
        }
    }

    fn wav() -> Vec<u8> {
        let mut bytes = b"RIFF\x28\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0".to_vec();
        bytes.extend_from_slice(&16000u32.to_le_bytes());
        bytes.extend_from_slice(&32000u32.to_le_bytes());
        bytes.extend_from_slice(b"\x02\0\x10\0data\x04\0\0\0\0\0\x01\0");
        bytes
    }

    fn mp3() -> Vec<u8> {
        let mut bytes = vec![0; 417];
        bytes[..4].copy_from_slice(&[0xff, 0xfb, 0x90, 0x00]);
        bytes
    }

    #[test]
    fn accepts_audio_contents_instead_of_trusting_file_names() {
        assert_eq!(detect_format(&wav()).unwrap(), CloneSampleFormat::Wav);
        assert_eq!(detect_format(&mp3()).unwrap(), CloneSampleFormat::Mp3);
        let mut with_tag = b"ID3\x04\0\0\0\0\0\x03abc".to_vec();
        with_tag.extend_from_slice(&mp3());
        assert_eq!(detect_format(&with_tag).unwrap(), CloneSampleFormat::Mp3);
        assert!(detect_format(b"ID3\x04\0\0\0\0\0\0").is_err());
        assert!(detect_format(&mp3()[..416]).is_err());
        assert!(detect_format(&wav()[..47]).is_err());
        assert!(detect_format(b"not an audio file.mp3").is_err());
    }

    #[test]
    fn validates_sample_identity_before_building_any_storage_path() {
        let root = Path::new("voice-samples");
        assert!(sample_path(root, "../../outside").is_err());
        assert!(sample_path(root, "0123456789abcdef0123456789abcdef").is_ok());
        assert_eq!(
            display_name(Some("/picked/name\n.wav"), CloneSampleFormat::Wav),
            "name.wav"
        );
    }

    #[test]
    fn base64_budget_and_cancel_apply_before_submission() {
        let mut bytes = mp3();
        bytes.resize(MAX_SAMPLE_BYTES as usize, 0);
        assert!(detect_format(&bytes).is_ok());
        bytes.push(0);
        assert!(detect_format(&bytes).is_err());
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(encode_sample(mp3(), CloneSampleFormat::Mp3, &cancel).is_err());
    }

    #[test]
    fn chunked_encoding_preserves_the_whole_file_and_is_shared() {
        let bytes: Vec<_> = (0..100_001).map(|n| (n % 251) as u8).collect();
        let prepared = encode_sample(
            bytes.clone(),
            CloneSampleFormat::Mp3,
            &CancellationToken::new(),
        )
        .unwrap();
        let encoded = prepared
            .data_url
            .strip_prefix("data:audio/mpeg;base64,")
            .unwrap();
        assert_eq!(STANDARD.decode(encoded).unwrap(), bytes);
        assert!(Arc::ptr_eq(&prepared.data_url, &prepared.clone().data_url));
    }

    #[tokio::test]
    async fn importing_and_removing_one_sample_preserves_other_files_and_prepared_audio() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let uploads = temp.path().join("uploads");
        let root = temp.path().join("voice-samples");
        tokio::fs::create_dir(&uploads).await.unwrap();
        let source = uploads.join("picked");
        tokio::fs::write(&source, wav()).await.unwrap();
        let source = picked_path(&uploads, &source).await.unwrap();
        let sample = import_picked(&root, &source, Some("spoken.wav"))
            .await
            .unwrap();
        let directory = sample_path(&root, &sample.id).unwrap();
        assert_eq!(
            read_metadata(&root, &sample.id)
                .await
                .unwrap()
                .unwrap()
                .file_name,
            "spoken.wav"
        );
        let prepared = encode_sample(
            bounded_read(&directory.join("audio"), MAX_SAMPLE_BYTES)
                .await
                .unwrap(),
            sample.format,
            &CancellationToken::new(),
        )
        .unwrap();
        let unrelated = root.join("unrelated");
        tokio::fs::write(&unrelated, "keep").await.unwrap();
        remove_sample_files(&directory).await.unwrap();
        assert!(read_metadata(&root, &sample.id).await.unwrap().is_none());
        assert!(prepared.data_url.starts_with("data:audio/wav;base64,"));
        assert_eq!(tokio::fs::read(&unrelated).await.unwrap(), b"keep");
        assert!(picked_path(&uploads, &unrelated).await.is_err());
    }

    #[tokio::test]
    async fn first_access_recovers_only_unselected_owned_files_and_keeps_live_drafts() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let root = temp.path().join("voice-samples");
        let source = temp.path().join("spoken.wav");
        tokio::fs::write(&source, wav()).await.unwrap();
        let selected = import_picked(&root, &source, None).await.unwrap();
        let abandoned = import_picked(&root, &source, None).await.unwrap();
        let pending = root.join(format!(".pending-{}", uuid::Uuid::new_v4().simple()));
        tokio::fs::create_dir(&pending).await.unwrap();
        tokio::fs::write(pending.join("audio"), wav())
            .await
            .unwrap();
        let unknown = root.join(uuid::Uuid::new_v4().simple().to_string());
        tokio::fs::create_dir(&unknown).await.unwrap();
        tokio::fs::write(unknown.join("do-not-delete"), b"keep")
            .await
            .unwrap();
        let mut files = SampleFiles::default();
        let snapshot = selection_snapshot(&selected.id);
        files.recover_from_snapshot(&root, &snapshot).await.unwrap();
        assert!(read_metadata(&root, &selected.id).await.unwrap().is_some());
        assert!(read_metadata(&root, &abandoned.id).await.unwrap().is_none());
        assert!(!pending.exists());
        assert_eq!(
            tokio::fs::read(unknown.join("do-not-delete"))
                .await
                .unwrap(),
            b"keep"
        );

        let draft_one = import_picked(&root, &source, None).await.unwrap();
        let draft_two = import_picked(&root, &source, None).await.unwrap();
        files.recover_from_snapshot(&root, &snapshot).await.unwrap();
        assert!(read_metadata(&root, &draft_one.id).await.unwrap().is_some());
        assert!(read_metadata(&root, &draft_two.id).await.unwrap().is_some());
        files.discard_from_snapshot(&root, &selected.id, &snapshot).await.unwrap();
        assert!(read_metadata(&root, &selected.id).await.unwrap().is_some());
        files.discard_from_snapshot(&root, &draft_one.id, &snapshot).await.unwrap();
        assert!(read_metadata(&root, &draft_one.id).await.unwrap().is_none());
        assert!(read_metadata(&root, &draft_two.id).await.unwrap().is_some());

        // Test applications use distinct storage roots in one process. Their
        // recovery state must not be conflated with the first application.
        let other_root = temp.path().join("second-app");
        let other = import_picked(&other_root, &source, None).await.unwrap();
        files.recover_from_snapshot(&other_root, &selection_snapshot("")).await.unwrap();
        assert!(read_metadata(&other_root, &other.id)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn damaged_or_future_configuration_preserves_unknown_references_and_repair_drafts() {
        use settings_manager::{ConfigDomain, DomainHealth};
        let mut snapshots = Vec::new();
        for raw in [
            serde_json::json!({ "tts": { "cloneSampleId": 42 } }),
            serde_json::json!({ "tts": false }),
            serde_json::json!(false),
            serde_json::json!({ "tts": { "cloneSampleId": "malformed-id" } }),
        ] {
            let (effective, issues) = settings_manager::schema::project_domain(ConfigDomain::Voice, &raw);
            let mut snapshot = selection_snapshot("");
            snapshot.data.voice = serde_json::from_value(effective).unwrap();
            let status = snapshot.domains.get_mut(&ConfigDomain::Voice).unwrap();
            status.health = if issues.is_empty() { DomainHealth::Ready } else { DomainHealth::Recovered };
            status.issues = issues;
            snapshots.push(snapshot);
        }
        let mut future = selection_snapshot("");
        let status = future.domains.get_mut(&ConfigDomain::Voice).unwrap();
        status.schema_version = 99;
        status.health = DomainHealth::Unavailable;
        snapshots.push(future);

        for snapshot in snapshots {
            let temp = tempfile::tempdir_in(".").unwrap();
            let root = temp.path().join("voice-samples");
            let source = temp.path().join("spoken.wav");
            tokio::fs::write(&source, wav()).await.unwrap();
            let existing = import_picked(&root, &source, None).await.unwrap();
            let mut files = SampleFiles::default();
            files.recover_from_snapshot(&root, &snapshot).await.unwrap();
            files.discard_from_snapshot(&root, &existing.id, &snapshot).await.unwrap();
            assert!(read_metadata(&root, &existing.id).await.unwrap().is_some());

            // The user can still import a replacement while the persisted
            // reference is unknown. Repairing it later must not let a delayed
            // startup sweep erase this process's unsaved sample.
            let draft = import_picked(&root, &source, None).await.unwrap();
            let repaired = selection_snapshot(&existing.id);
            files.recover_from_snapshot(&root, &repaired).await.unwrap();
            assert!(read_metadata(&root, &draft.id).await.unwrap().is_some());
            files.discard_from_snapshot(&root, &draft.id, &repaired).await.unwrap();
            assert!(read_metadata(&root, &draft.id).await.unwrap().is_none());
            assert!(read_metadata(&root, &existing.id).await.unwrap().is_some());
        }
    }
}
