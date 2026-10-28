//! Imported voice references belong to the application, independently of the
//! native picker's temporary copy. A narration reads and encodes its reference
//! once; subsequent segments share the resulting data URL even after replacement.

mod registry;

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
};
use tauri::{AppHandle, Manager, Runtime};
use tokio::{io::AsyncReadExt, sync::{Mutex, OwnedMutexGuard}};
use tokio_util::sync::CancellationToken;

const MAX_BASE64_BYTES: usize = 10_000_000;
pub(super) const MAX_SAMPLE_BYTES: u64 = 7_000_000;
static SAMPLE_FILES: LazyLock<Arc<Mutex<()>>> = LazyLock::new(|| Arc::new(Mutex::new(())));

/// Serializes publication, reads and explicit deletion of local audio assets.
struct VoiceLibraryGuard {
    root: PathBuf,
    _guard: OwnedMutexGuard<()>,
}

async fn lock_voice_library<R: Runtime>(app: &AppHandle<R>) -> Result<VoiceLibraryGuard, String> {
    // This acquisition is selected against narration cancellation. It must
    // remain side-effect free; maintenance belongs to a durable task below.
    Ok(VoiceLibraryGuard { root: sample_root(app)?, _guard: SAMPLE_FILES.clone().lock_owned().await })
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CloneSampleFormat {
    Mp3,
    Wav,
}

impl CloneSampleFormat {
    fn extension(self) -> &'static str {
        match self { Self::Mp3 => "mp3", Self::Wav => "wav" }
    }

    fn mime(self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::Wav => "audio/wav",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceCloneModelDto {
    pub id: String,
    pub name: String,
    pub file_name: String,
    pub format: CloneSampleFormat,
    pub file_bytes: u64,
    pub duration_ms: u32,
    pub file_hash: String,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Clone)]
pub(super) struct PreparedCloneSample {
    pub data_url: Arc<str>,
}

fn sample_root<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("voice_library"))
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
    if !metadata.is_file() {
        return Err("选择的声音样本不是文件。".into());
    }
    if metadata.len() > limit {
        return Err(format!("声音样本文件超过读取上限（{limit} 字节）。"));
    }
    let mut data = Vec::with_capacity(metadata.len() as usize);
    file.take(limit + 1)
        .read_to_end(&mut data)
        .await
        .map_err(|e| format!("读取声音样本失败：{e}"))?;
    if data.len() as u64 > limit {
        return Err(format!("声音样本文件超过读取上限（{limit} 字节）。"));
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

fn duration_ms(bytes: &[u8]) -> Result<u32, String> {
    use std::io::Cursor;
    use symphonia::core::{formats::{probe::Hint, TrackType}, io::MediaSourceStream};
    let stream = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
    let mut reader = symphonia::default::get_probe()
        .probe(&Hint::new(), stream, Default::default(), Default::default())
        .map_err(|_| "无法读取参考音频时长。".to_string())?;
    let track = reader.default_track(TrackType::Audio).ok_or("参考音频没有音轨。")?;
    let track_id = track.id;
    let time_base = track.time_base.ok_or("参考音频缺少时间基准。")?;
    let mut ticks = 0u64;
    while let Some(packet) = reader.next_packet().map_err(|_| "参考音频数据损坏，无法读取时长。")? {
        if packet.track_id == track_id {
            ticks = ticks.checked_add(packet.dur.get()).ok_or("参考音频时长超出范围。")?;
        }
    }
    let millis = (u128::from(ticks) * u128::from(time_base.numer.get()) * 1000)
        .div_ceil(u128::from(time_base.denom.get()));
    u32::try_from(millis).ok().filter(|value| *value > 0).ok_or_else(|| "参考音频时长无效。".into())
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
    name: &str,
    file_name: Option<&str>,
) -> Result<VoiceCloneModelDto, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err("音色名称需要包含 1–80 个字符，不能包含控制字符。".into());
    }
    let bytes = bounded_read(source, MAX_SAMPLE_BYTES).await?;
    let (bytes, format, duration_ms, file_hash) = tokio::task::spawn_blocking(move || {
        let format = detect_format(&bytes)?;
        let duration_ms = duration_ms(&bytes)?;
        let file_hash = hex::encode(Sha256::digest(&bytes));
        Ok::<_, String>((bytes, format, duration_ms, file_hash))
    }).await.map_err(|_| "参考音频检查任务异常结束。".to_string())??;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
    let sample = VoiceCloneModelDto {
        id,
        name: name.into(),
        file_name: display_name(file_name, format),
        format,
        file_bytes: bytes.len() as u64,
        duration_ms,
        file_hash,
        created_at: now,
        updated_at: now,
    };
    registry::publish(root, sample, &bytes).await
}

#[tauri::command]
pub async fn import_voice_clone_sample<R: Runtime>(
    app: AppHandle<R>,
    path: String,
    name: String,
    file_name: Option<String>,
) -> Result<VoiceCloneModelDto, String> {
    let staging = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("uploads");
    let source = picked_path(&staging, Path::new(&path)).await?;
    let guard = lock_voice_library(&app).await?;
    // Publication and cleanup remain owned even if the requesting page leaves.
    tokio::spawn(async move {
        let guard = guard;
        let result = async {
            registry::finish_deletions(&guard.root).await?;
            registry::cleanup_pending(&guard.root).await?;
            import_picked(&guard.root, &source, &name, file_name.as_deref()).await
        }.await;
        let _ = tokio::fs::remove_file(source).await;
        result
    }).await.map_err(|_| "声音模型导入任务异常结束。".to_string())?
}

async fn read_metadata(root: &Path, id: &str) -> Result<Option<VoiceCloneModelDto>, String> {
    sample_path(root, id)?;
    let Some(sample) = registry::load(root).await?.samples.into_iter().find(|sample| sample.id == id) else { return Ok(None); };
    let file = tokio::fs::metadata(registry::audio_path(root, &sample)?).await
        .map_err(|_| "声音模型音频在本机缺失，请重新导入。".to_string())?;
    if !file.is_file() || file.len() != sample.file_bytes {
        return Err("声音模型音频已损坏，请重新导入。".into());
    }
    Ok(Some(sample))
}

#[tauri::command]
pub async fn get_voice_clone_sample<R: Runtime>(
    app: AppHandle<R>,
    sample_id: String,
) -> Result<Option<VoiceCloneModelDto>, String> {
    if sample_id.is_empty() {
        return Ok(None);
    }
    let guard = lock_voice_library(&app).await?;
    read_metadata(&guard.root, &sample_id).await
}

#[tauri::command]
pub async fn list_voice_clone_samples<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Vec<VoiceCloneModelDto>, String> {
    let guard = lock_voice_library(&app).await?;
    tokio::spawn(async move {
        let guard = guard;
        registry::finish_deletions(&guard.root).await?;
        registry::cleanup_pending(&guard.root).await?;
        let mut samples = registry::load(&guard.root).await?.samples;
        samples.sort_by(|left, right| right.created_at.cmp(&left.created_at).then_with(|| left.id.cmp(&right.id)));
        Ok(samples)
    }).await.map_err(|_| "声音库读取任务异常结束。".to_string())?
}

#[tauri::command]
pub async fn delete_voice_clone_sample<R: Runtime>(app: AppHandle<R>, sample_id: String) -> Result<bool, String> {
    let guard = lock_voice_library(&app).await?;
    sample_path(&guard.root, &sample_id)?;
    tokio::spawn(async move {
        let guard = guard;
        registry::delete(&guard.root, &sample_id).await
    }).await.map_err(|_| "声音模型删除任务异常结束。".to_string())?
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

async fn read_sample(root: &Path, id: &str) -> Result<Option<(VoiceCloneModelDto, Vec<u8>)>, String> {
    if id.is_empty() {
        return Ok(None);
    }
    let Some(metadata) = read_metadata(root, id).await? else { return Ok(None); };
    let bytes = bounded_read(&registry::audio_path(root, &metadata)?, MAX_SAMPLE_BYTES).await?;
    tokio::task::spawn_blocking(move || {
        if detect_format(&bytes)? != metadata.format || bytes.len() as u64 != metadata.file_bytes
            || hex::encode(Sha256::digest(&bytes)) != metadata.file_hash {
            return Err("声音样本文件已损坏，请重新导入。".into());
        }
        Ok(Some((metadata, bytes)))
    }).await.map_err(|_| "声音模型校验任务异常结束。".to_string())?
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
) -> Result<Option<PreparedCloneSample>, String> {
    let Some((metadata, bytes)) = read_clone_sample_audio(app, id, cancel).await? else { return Ok(None); };
    encode_prepared(bytes, metadata.format, cancel).await.map(Some)
}

pub(super) async fn read_clone_sample_audio<R: Runtime>(
    app: &AppHandle<R>,
    id: &str,
    cancel: &CancellationToken,
) -> Result<Option<(VoiceCloneModelDto, Vec<u8>)>, String> {
    let guard = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(super::network::CANCELLED.into()),
        guard = lock_voice_library(app) => guard?,
    };
    // Await the real read/hash owner before releasing the library lock. A token
    // cancels admission and subsequent work, never abandons a running IO owner.
    let result = read_sample(&guard.root, id).await;
    if cancel.is_cancelled() {
        return Err(super::network::CANCELLED.into());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn measures_pcm_frames_and_real_mp3_packets_instead_of_file_size() {
        let mut pcm_wav = wav();
        pcm_wav.resize(44 + 32_000, 0);
        pcm_wav[4..8].copy_from_slice(&(36u32 + 32_000).to_le_bytes());
        pcm_wav[40..44].copy_from_slice(&32_000u32.to_le_bytes());
        assert_eq!(duration_ms(&pcm_wav).unwrap(), 1000);
        let encoded = vcp_voice_mp3::encode_pcm16_mono(&vec![0; 16_000], || false).unwrap();
        assert_eq!(detect_format(&encoded).unwrap(), CloneSampleFormat::Mp3);
        // This encoder omits LAME/Xing metadata, so measured packets retain the
        // actual encoder delay/padding instead of inventing a trim interval.
        let duration = duration_ms(&encoded).unwrap();
        assert!((500..670).contains(&duration), "duration={duration}");
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
        bytes.resize(MAX_BASE64_BYTES / 4 * 3, 0);
        assert!(detect_format(&bytes).is_ok());
        bytes.push(0);
        assert!(detect_format(&bytes).is_err());
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(encode_sample(mp3(), CloneSampleFormat::Mp3, &cancel).is_err());
    }

    #[test]
    fn chunked_encoding_preserves_the_whole_file() {
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
    }

    #[tokio::test]
    async fn import_and_read_enforce_seven_decimal_megabytes() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let root = temp.path().join("voice-samples");
        let source = temp.path().join("reference.wav");
        let mut bytes = wav();
        bytes.resize(7_000_000, 0);
        bytes[4..8].copy_from_slice(&6_999_992u32.to_le_bytes());
        bytes[40..44].copy_from_slice(&6_999_956u32.to_le_bytes());
        tokio::fs::write(&source, &bytes).await.unwrap();

        let sample = import_picked(&root, &source, "声音", None).await.unwrap();
        let (metadata, read) = read_sample(&root, &sample.id).await.unwrap().unwrap();
        assert_eq!(read, bytes);
        assert_eq!(metadata.format, CloneSampleFormat::Wav);

        bytes.push(0);
        tokio::fs::write(&source, &bytes).await.unwrap();
        assert!(import_picked(&root, &source, "声音", None).await.is_err());

        tokio::fs::write(registry::audio_path(&root, &sample).unwrap(), &bytes).await.unwrap();
        assert!(read_sample(&root, &sample.id).await.is_err());
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
        let sample = import_picked(&root, &source, "声音", Some("spoken.wav"))
            .await
            .unwrap();
        let audio = registry::audio_path(&root, &sample).unwrap();
        assert_eq!(
            read_metadata(&root, &sample.id)
                .await
                .unwrap()
                .unwrap()
                .file_name,
            "spoken.wav"
        );
        let prepared = encode_sample(
            bounded_read(&audio, MAX_SAMPLE_BYTES)
                .await
                .unwrap(),
            sample.format,
            &CancellationToken::new(),
        )
        .unwrap();
        let unrelated = root.join("unrelated");
        tokio::fs::write(&unrelated, "keep").await.unwrap();
        tokio::fs::remove_file(audio).await.unwrap();
        assert!(read_metadata(&root, &sample.id).await.is_err());
        assert!(prepared.data_url.starts_with("data:audio/wav;base64,"));
        assert_eq!(tokio::fs::read(&unrelated).await.unwrap(), b"keep");
        assert!(picked_path(&uploads, &unrelated).await.is_err());
    }

    #[tokio::test]
    async fn registry_reload_preserves_all_registered_assets_and_unknown_files() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let root = temp.path().join("voice-samples");
        let source = temp.path().join("spoken.wav");
        tokio::fs::write(&source, wav()).await.unwrap();
        let first = import_picked(&root, &source, "第一", None).await.unwrap();
        let second = import_picked(&root, &source, "第二", None).await.unwrap();
        let third = import_picked(&root, &source, "第三", None).await.unwrap();
        let unknown = root.join(uuid::Uuid::new_v4().simple().to_string());
        tokio::fs::create_dir(&unknown).await.unwrap();
        tokio::fs::write(unknown.join("do-not-delete"), b"keep")
            .await
            .unwrap();
        let reopened = registry::load(&root).await.unwrap();
        assert_eq!(reopened.samples.len(), 3);
        for sample in [&first, &second, &third] {
            assert_eq!(read_sample(&root, &sample.id).await.unwrap().unwrap().1, wav());
        }
        assert_eq!(
            tokio::fs::read(unknown.join("do-not-delete"))
                .await
                .unwrap(),
            b"keep"
        );
    }

    #[tokio::test]
    async fn corrupt_registry_or_audio_never_erases_other_assets() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let root = temp.path().join("voice-samples");
        let source = temp.path().join("spoken.wav");
        tokio::fs::write(&source, wav()).await.unwrap();
        let existing = import_picked(&root, &source, "声音", None).await.unwrap();
        let audio = registry::audio_path(&root, &existing).unwrap();
        let mut corrupt = wav();
        corrupt[47] ^= 1;
        tokio::fs::write(&audio, &corrupt).await.unwrap();
        assert!(read_sample(&root, &existing.id).await.is_err());
        tokio::fs::write(root.join("registry.json"), b"{broken").await.unwrap();
        assert!(registry::load(&root).await.is_err());
        assert!(import_picked(&root, &source, "新声音", None).await.is_err());
        assert_eq!(tokio::fs::read(audio).await.unwrap(), corrupt);
    }

    #[tokio::test]
    async fn deleted_asset_resolves_to_none_and_prepared_audio_remains_owned() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let root = temp.path().join("library");
        let source = temp.path().join("spoken.wav");
        tokio::fs::write(&source, wav()).await.unwrap();
        let sample = import_picked(&root, &source, "声音", Some("spoken.wav")).await.unwrap();
        let (metadata, bytes) = read_sample(&root, &sample.id).await.unwrap().unwrap();
        assert_eq!(metadata.duration_ms, 1);
        let prepared = encode_prepared(bytes, metadata.format, &CancellationToken::new()).await.unwrap();
        assert!(registry::delete(&root, &sample.id).await.unwrap());
        assert!(read_sample(&root, &sample.id).await.unwrap().is_none());
        assert!(!registry::delete(&root, &sample.id).await.unwrap());
        assert_eq!(STANDARD.decode(prepared.data_url.split_once(',').unwrap().1).unwrap(), wav());
    }
}
