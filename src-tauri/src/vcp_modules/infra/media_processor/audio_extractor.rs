use super::native_media::{
    check_cancelled, read_cache, read_output_file, run_native_media, write_cache, AUDIO_MAX_BYTES,
};
use base64::Engine as _;
use std::path::Path;
use tauri::{AppHandle, Runtime};
use tokio_util::sync::CancellationToken;

const AUDIO_DATA_PREFIX: &str = "data:audio/mp4;base64,";

/// Normalize both mobile platforms to AAC-LC in an M4A container. VCP's
/// multimodal proxy consumes an audio data URL, not an OpenAI input_audio field.
pub async fn process_audio_for_multimodal<R: Runtime>(
    app: &AppHandle<R>,
    path: &Path,
    cancellation: &CancellationToken,
) -> Result<String, String> {
    check_cancelled(cancellation)?;
    if let Some(cached) = read_cache::<_, String>(app, path, "audio-m4a", 24 * 1024 * 1024).await {
        check_cancelled(cancellation)?;
        if cached.starts_with(AUDIO_DATA_PREFIX)
            && cached.len() > AUDIO_DATA_PREFIX.len()
            && cached.len() <= (AUDIO_MAX_BYTES as usize).div_ceil(3) * 4 + AUDIO_DATA_PREFIX.len()
        {
            return Ok(cached);
        }
    }
    let result = process_audio_for_multimodal_internal(app, path, cancellation).await?;
    check_cancelled(cancellation)?;
    let result = write_cache(app, path, "audio-m4a", result).await?;
    check_cancelled(cancellation)?;
    Ok(result)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProcessAudioResult {
    path: String,
    mime: String,
    codec: String,
    container: String,
    sample_rate: u32,
    channels: u32,
}

async fn process_audio_for_multimodal_internal<R: Runtime>(
    app: &AppHandle<R>,
    path: &Path,
    cancellation: &CancellationToken,
) -> Result<String, String> {
    run_native_media(
        app,
        path,
        "processAudio",
        AUDIO_MAX_BYTES,
        cancellation,
        |result: ProcessAudioResult, output_dir, token| {
            check_cancelled(token)?;
            if result.mime != "audio/mp4"
                || result.codec != "aac"
                || result.container != "m4a"
                || result.sample_rate != 16_000
                || result.channels != 1
            {
                return Err("原生音频输出不是约定的 AAC-LC/16kHz 单声道 M4A".into());
            }
            let bytes = read_output_file(output_dir, Path::new(&result.path), AUDIO_MAX_BYTES)?;
            // Both native writers produce an ISO BMFF file. Do not label ADTS,
            // arbitrary original bytes, or a mismatched container as audio/mp4.
            if bytes.len() < 12 || &bytes[4..8] != b"ftyp" {
                return Err("原生音频输出缺少 M4A 容器头".into());
            }
            check_cancelled(token)?;
            let prefix = AUDIO_DATA_PREFIX;
            let mut data_url = String::with_capacity(prefix.len() + bytes.len().div_ceil(3) * 4);
            data_url.push_str(prefix);
            base64::engine::general_purpose::STANDARD.encode_string(&bytes, &mut data_url);
            Ok(data_url)
        },
    )
    .await
}
