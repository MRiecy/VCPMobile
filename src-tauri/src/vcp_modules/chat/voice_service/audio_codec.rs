//! Complete-file TTS decoding. Network chunks are not independent media files.
//!
//! Keep one decoder and one resampler for the whole sentence. Only complete,
//! validated containers reach the cache; playback always receives PCM16/24k/mono.

use std::fmt;
use std::io::Cursor;
use std::ops::Range;

use bytes::Bytes;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as MediaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::well_known::{FORMAT_ID_MP3, FORMAT_ID_WAVE};
use symphonia::core::formats::TrackType;
use symphonia::core::io::MediaSourceStream;
use tokio_util::sync::CancellationToken;

use super::network::CANCELLED;
use super::pipeline::PacketConsumer;

pub(super) const MAX_PCM_BYTES: usize = 8 * 1024 * 1024;
pub(super) const MAX_AUDIO_FILE_BYTES: usize = 16 * 1024 * 1024;
pub(super) const PCM_PACKET_BYTES: usize = 32_768;
const OUTPUT_RATE: u32 = 24_000;
const CONVERSION_FRAMES: usize = 1024;
// Bound the FFT workspace before construction, including uncommon coprime rates.
const MAX_RESAMPLE_BLOCK_FRAMES: usize = 32_768;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ContainerFormat {
    Wav,
    Mp3,
}

impl ContainerFormat {
    pub(super) fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
        }
    }
}

#[derive(Debug)]
pub(super) struct DecodedAudio {
    pub format: ContainerFormat,
    pub pcm_24k_mono: Vec<u8>,
}

/// Only confirmed malformed media warrants removing an existing cache entry.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum DecodeError {
    Corrupt(String),
    Unsupported(String),
    Resource(String),
    Worker(String),
    Cancelled,
}

impl DecodeError {
    pub fn is_corrupt(&self) -> bool {
        matches!(self, Self::Corrupt(_))
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Corrupt(message)
            | Self::Unsupported(message)
            | Self::Resource(message)
            | Self::Worker(message) => formatter.write_str(message),
            Self::Cancelled => formatter.write_str(CANCELLED),
        }
    }
}

impl std::error::Error for DecodeError {}

fn media_error(context: &str, error: MediaError) -> DecodeError {
    let message = format!("{context}：{error}");
    match error {
        MediaError::DecodeError(_) => DecodeError::Corrupt(message),
        // The source is an already received in-memory file, so an unexpected
        // EOF reported by the demuxer/decoder is malformed media, not disk IO.
        MediaError::IoError(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
            DecodeError::Corrupt(message)
        }
        MediaError::Unsupported(_) | MediaError::ResetRequired => DecodeError::Unsupported(message),
        MediaError::LimitError(_) => DecodeError::Resource(message),
        _ => DecodeError::Worker(message),
    }
}

/// The caller must await this future during cancellation. Dropping a JoinHandle
/// does not stop a blocking decoder; this function always joins its worker.
pub(super) async fn decode(
    bytes: Bytes,
    cancel: &CancellationToken,
) -> Result<DecodedAudio, DecodeError> {
    check_cancelled(cancel)?;
    if bytes.is_empty() {
        return Err(DecodeError::Corrupt("语音音频文件为空。".into()));
    }
    if bytes.len() > MAX_AUDIO_FILE_BYTES {
        return Err(DecodeError::Resource(
            "语音音频文件超过 16 MiB 上限。".into(),
        ));
    }
    let cancel = cancel.clone();
    tokio::task::spawn_blocking(move || decode_inner(bytes, &cancel))
        .await
        .map_err(|error| DecodeError::Worker(format!("语音音频解码任务失败：{error}")))?
}

fn decode_inner(bytes: Bytes, cancel: &CancellationToken) -> Result<DecodedAudio, DecodeError> {
    let mut pcm_24k_mono = Vec::new();
    let mut collect = |packet: &[u8]| {
        pcm_24k_mono.try_reserve(packet.len())
            .map_err(|_| DecodeError::Resource("无法分配语音 PCM 缓冲。".into()))?;
        pcm_24k_mono.extend_from_slice(packet);
        Ok(())
    };
    let format = decode_to(bytes, cancel, &mut PcmSink {
        max_bytes: Some(MAX_PCM_BYTES), frames: 0, write: &mut collect,
    })?;
    Ok(DecodedAudio { format, pcm_24k_mono })
}

/// Local references are bounded compressed files, not bounded TTS sentences.
/// Decode on one worker into two native-sized queued packets. Cancellation drops
/// the receiver to release blocking_send and always joins the real decoder.
pub(super) async fn decode_stream<C: PacketConsumer>(
    bytes: Bytes,
    cancel: &CancellationToken,
    consumer: &mut C,
) -> Result<(), String> {
    check_cancelled(cancel).map_err(|error| error.to_string())?;
    if bytes.is_empty() || bytes.len() > MAX_AUDIO_FILE_BYTES {
        return Err("原声音频文件为空或超过大小限制。".into());
    }
    let local = cancel.child_token();
    let decoding = local.clone();
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<Vec<u8>>(2);
    let worker = tokio::task::spawn_blocking(move || {
        let result = (|| {
        let mut pending = Vec::with_capacity(PCM_PACKET_BYTES);
        let mut emit = |mut pcm: &[u8]| {
            while !pcm.is_empty() {
                check_cancelled(&decoding)?;
                let count = pcm.len().min(PCM_PACKET_BYTES - pending.len());
                pending.extend_from_slice(&pcm[..count]);
                pcm = &pcm[count..];
                if pending.len() == PCM_PACKET_BYTES {
                    sender.blocking_send(std::mem::replace(&mut pending, Vec::with_capacity(PCM_PACKET_BYTES)))
                        .map_err(|_| DecodeError::Cancelled)?;
                }
            }
            Ok(())
        };
        decode_to(bytes, &decoding, &mut PcmSink { max_bytes: None, frames: 0, write: &mut emit })?;
        check_cancelled(&decoding)?;
        if !pending.is_empty() {
            sender.blocking_send(pending).map_err(|_| DecodeError::Cancelled)?;
        }
        Ok::<_, DecodeError>(())
        })();
        if result.is_err() { decoding.cancel(); }
        result
    });
    let consumed = async {
        loop {
            let packet = tokio::select! {
                biased;
                _ = local.cancelled() => return Err(CANCELLED.to_string()),
                packet = receiver.recv() => packet,
            };
            let Some(packet) = packet else { return Ok(()); };
            consumer.append(packet, &local).await?;
        }
    }.await;
    drop(receiver);
    local.cancel();
    let decoded = worker.await.map_err(|error| format!("原声解码任务失败：{error}"))?;
    match decoded {
        Err(DecodeError::Cancelled) => consumed.and(Err(CANCELLED.into())),
        Err(error) => Err(error.to_string()),
        Ok(()) => consumed,
    }
}

struct PcmSink<'a> {
    max_bytes: Option<usize>,
    frames: usize,
    write: &'a mut dyn FnMut(&[u8]) -> Result<(), DecodeError>,
}

impl PcmSink<'_> {
    fn append_bytes(&mut self, pcm: &[u8]) -> Result<(), DecodeError> {
        let frames = self.frames.checked_add(pcm.len() / 2).ok_or_else(pcm_limit)?;
        if self.max_bytes.is_some_and(|limit| frames > limit / 2) {
            return Err(pcm_limit());
        }
        for packet in pcm.chunks(PCM_PACKET_BYTES) { (self.write)(packet)?; }
        self.frames = frames;
        Ok(())
    }

    fn append_samples(&mut self, samples: &[f32]) -> Result<(), DecodeError> {
        let mut packet = [0_u8; PCM_PACKET_BYTES];
        for samples in samples.chunks(PCM_PACKET_BYTES / 2) {
            for (sample, bytes) in samples.iter().zip(packet.chunks_exact_mut(2)) {
                if !sample.is_finite() {
                    return Err(DecodeError::Worker("语音重采样产生了无效采样值。".into()));
                }
                let value = (sample * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
                bytes.copy_from_slice(&value.to_le_bytes());
            }
            self.append_bytes(&packet[..samples.len() * 2])?;
        }
        Ok(())
    }
}

fn decode_to(bytes: Bytes, cancel: &CancellationToken, output: &mut PcmSink<'_>) -> Result<ContainerFormat, DecodeError> {
    check_cancelled(cancel)?;
    if bytes.starts_with(b"RIFF") {
        if let Some(range) = validate_wav(&bytes, cancel, output.max_bytes)? {
            // validate_wav has checked the complete container and PCM limit
            // before allocation. Keep PCM16 samples exact without f32 decoding.
            check_cancelled(cancel)?;
            output.append_bytes(&bytes[range])?;
            check_cancelled(cancel)?;
            return Ok(ContainerFormat::Wav);
        }
    }
    // Let the media library locate metadata, frame boundaries and padding.
    // A second MP3 scanner would impose a narrower format than the decoder.
    let stream = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
    let mut reader = symphonia::default::get_probe()
        .probe(&Hint::new(), stream, Default::default(), Default::default())
        .map_err(|error| media_error("无法读取语音音频容器", error))?;
    let format = match reader.format_info().format {
        FORMAT_ID_WAVE => ContainerFormat::Wav,
        FORMAT_ID_MP3 => ContainerFormat::Mp3,
        _ => {
            return Err(DecodeError::Unsupported(
                "语音服务返回了不支持的音频格式，需要完整 WAV 或 MP3。".into(),
            ));
        }
    };
    check_cancelled(cancel)?;
    let track = reader
        .default_track(TrackType::Audio)
        .ok_or_else(|| DecodeError::Unsupported("语音音频中没有可解码音轨。".into()))?;
    let track_id = track.id;
    let parameters = track
        .codec_params
        .as_ref()
        .and_then(|parameters| parameters.audio())
        .ok_or_else(|| DecodeError::Corrupt("语音音频缺少解码参数。".into()))?;
    if let Some(channels) = &parameters.channels {
        if !(1..=2).contains(&channels.count()) {
            return Err(DecodeError::Unsupported(
                "语音音频仅支持单声道或双声道。".into(),
            ));
        }
    }
    // Symphonia 0.6 moved enable_gapless to the decoder options. It trims only
    // delay/padding recorded in the file, never guessed silence at the edges.
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(parameters, &AudioDecoderOptions::default().gapless(true))
        .map_err(|error| media_error("无法创建语音音频解码器", error))?;
    let mut normalizer: Option<MonoNormalizer> = None;
    let mut interleaved = [0.0_f32; CONVERSION_FRAMES * 2];
    let mut mono = [0.0_f32; CONVERSION_FRAMES];
    loop {
        check_cancelled(cancel)?;
        let Some(packet) = reader
            .next_packet()
            .map_err(|error| media_error("无法读取语音音频数据", error))?
        else {
            break;
        };
        if packet.track_id != track_id {
            continue;
        }
        let audio = decoder
            .decode(&packet)
            .map_err(|error| media_error("语音音频解码失败", error))?;
        let channels = audio.spec().channels().count();
        if !(1..=2).contains(&channels) || audio.spec().rate() == 0 {
            return Err(DecodeError::Unsupported(
                "语音音频的采样率或声道数不受支持。".into(),
            ));
        }
        if normalizer.is_none() {
            normalizer = Some(MonoNormalizer::new(audio.spec().rate())?);
        }
        let converter = normalizer
            .as_mut()
            .ok_or_else(|| DecodeError::Worker("语音音频归一化未初始化。".into()))?;
        if converter.rate != audio.spec().rate() {
            return Err(DecodeError::Unsupported(
                "语音音频在同一句内改变了采样率。".into(),
            ));
        }
        for start in (0..audio.frames()).step_by(CONVERSION_FRAMES) {
            check_cancelled(cancel)?;
            let count = CONVERSION_FRAMES.min(audio.frames() - start);
            audio
                .slice(start..start + count)
                .copy_to_slice_interleaved(&mut interleaved[..count * channels]);
            for (destination, frame) in mono[..count]
                .iter_mut()
                .zip(interleaved[..count * channels].chunks_exact(channels))
            {
                if frame.iter().any(|sample| !sample.is_finite()) {
                    return Err(DecodeError::Corrupt("语音音频包含无效采样值。".into()));
                }
                *destination = frame.iter().map(|sample| sample / channels as f32).sum();
            }
            converter.push(&mono[..count], cancel, output)?;
        }
    }
    normalizer
        .ok_or_else(|| DecodeError::Corrupt("语音服务返回了空音频。".into()))?
        .finish(cancel, output)?;
    check_cancelled(cancel)?;
    Ok(format)
}

fn check_cancelled(cancel: &CancellationToken) -> Result<(), DecodeError> {
    if cancel.is_cancelled() {
        Err(DecodeError::Cancelled)
    } else {
        Ok(())
    }
}

fn pcm_limit() -> DecodeError {
    DecodeError::Resource("单句语音解码后的 PCM 超过 8 MiB 上限。".into())
}

fn output_frames(input_frames: u64, source_rate: u32, max_bytes: Option<usize>) -> Result<usize, DecodeError> {
    let frames = input_frames
        .checked_mul(u64::from(OUTPUT_RATE))
        .ok_or_else(pcm_limit)?
        .div_ceil(u64::from(source_rate));
    if max_bytes.is_some_and(|limit| frames > (limit / 2) as u64) {
        return Err(pcm_limit());
    }
    usize::try_from(frames).map_err(|_| pcm_limit())
}

struct MonoNormalizer {
    rate: u32,
    source_frames: u64,
    resampling: Option<Resampling>,
}

struct Resampling {
    converter: Fft<f32>,
    input: Vec<f32>,
    output: Vec<f32>,
    pending: usize,
    trim_start: usize,
}

impl MonoNormalizer {
    fn new(rate: u32) -> Result<Self, DecodeError> {
        if rate == 0 {
            return Err(DecodeError::Corrupt("语音音频采样率无效。".into()));
        }
        let resampling = if rate == OUTPUT_RATE {
            None
        } else {
            let mut a = rate;
            let mut b = OUTPUT_RATE;
            while b != 0 {
                (a, b) = (b, a % b);
            }
            let minimum_input = rate as usize / a as usize;
            let multiples = CONVERSION_FRAMES.div_ceil(minimum_input);
            let input_frames = multiples * minimum_input;
            let output_frames = multiples * OUTPUT_RATE as usize / a as usize;
            if input_frames > MAX_RESAMPLE_BLOCK_FRAMES || output_frames > MAX_RESAMPLE_BLOCK_FRAMES
            {
                return Err(DecodeError::Resource(
                    "语音音频采样率需要的重采样工作区过大。".into(),
                ));
            }
            let converter = Fft::<f32>::new(
                rate as usize,
                OUTPUT_RATE as usize,
                CONVERSION_FRAMES,
                1,
                FixedSync::Both,
            )
            .map_err(|error| DecodeError::Worker(format!("无法创建语音重采样器：{error}")))?;
            let workspace = |frames| {
                let mut samples = Vec::new();
                samples
                    .try_reserve_exact(frames)
                    .map_err(|_| DecodeError::Resource("无法分配语音重采样工作区。".into()))?;
                samples.resize(frames, 0.0);
                Ok::<_, DecodeError>(samples)
            };
            Some(Resampling {
                input: workspace(converter.input_frames_next())?,
                output: workspace(converter.output_frames_max())?,
                pending: 0,
                trim_start: converter.output_delay(),
                converter,
            })
        };
        Ok(Self {
            rate,
            source_frames: 0,
            resampling,
        })
    }

    fn push(&mut self, mut samples: &[f32], cancel: &CancellationToken, output: &mut PcmSink<'_>) -> Result<(), DecodeError> {
        check_cancelled(cancel)?;
        self.source_frames = self
            .source_frames
            .checked_add(samples.len() as u64)
            .ok_or_else(pcm_limit)?;
        let target_frames = output_frames(self.source_frames, self.rate, output.max_bytes)?;
        let Some(resampling) = &mut self.resampling else {
            return output.append_samples(samples);
        };
        while !samples.is_empty() {
            check_cancelled(cancel)?;
            let count = samples
                .len()
                .min(resampling.input.len() - resampling.pending);
            resampling.input[resampling.pending..resampling.pending + count]
                .copy_from_slice(&samples[..count]);
            resampling.pending += count;
            samples = &samples[count..];
            if resampling.pending == resampling.input.len() {
                resampling.process(output, target_frames)?;
            }
        }
        Ok(())
    }

    fn finish(mut self, cancel: &CancellationToken, output: &mut PcmSink<'_>) -> Result<(), DecodeError> {
        let target_frames = output_frames(self.source_frames, self.rate, output.max_bytes)?;
        if target_frames == 0 {
            return Err(DecodeError::Corrupt("语音服务返回了空音频。".into()));
        }
        if let Some(resampling) = &mut self.resampling {
            // Flush the remaining source samples and filter tail, remove the
            // documented filter delay, and emit exactly the source duration.
            while output.frames < target_frames {
                check_cancelled(cancel)?;
                resampling.process(output, target_frames)?;
            }
        }
        check_cancelled(cancel)?;
        Ok(())
    }
}

impl Resampling {
    fn process(&mut self, sink: &mut PcmSink<'_>, target_frames: usize) -> Result<(), DecodeError> {
        let input = InterleavedSlice::new(&self.input, 1, self.input.len())
            .map_err(|error| DecodeError::Worker(format!("语音重采样输入无效：{error}")))?;
        let capacity = self.output.len();
        let mut output = InterleavedSlice::new_mut(&mut self.output, 1, capacity)
            .map_err(|error| DecodeError::Worker(format!("语音重采样输出无效：{error}")))?;
        let indexing = Indexing::new().partial_len(self.pending);
        let (_, written) = self
            .converter
            .process_into_buffer(&input, &mut output, Some(&indexing))
            .map_err(|error| DecodeError::Worker(format!("语音重采样失败：{error}")))?;
        self.pending = 0;
        let skip = self.trim_start.min(written);
        self.trim_start -= skip;
        let take = (written - skip).min(target_frames.saturating_sub(sink.frames));
        sink.append_samples(&self.output[skip..skip + take])
    }
}

fn invalid_wav() -> DecodeError {
    DecodeError::Corrupt("语音服务返回的 WAV 文件不完整或格式无效。".into())
}

/// Validate all RIFF chunks and the projected output size before allocating.
/// Return a direct PCM slice only for the exact native playback format.
fn validate_wav(
    bytes: &[u8],
    cancel: &CancellationToken,
    max_pcm_bytes: Option<usize>,
) -> Result<Option<Range<usize>>, DecodeError> {
    if bytes.len() < 12 {
        return Err(invalid_wav());
    }
    if &bytes[8..12] != b"WAVE" {
        return Err(DecodeError::Unsupported("语音音频不是 WAV 容器。".into()));
    }
    let end = (read_u32(&bytes[4..8]) as usize)
        .checked_add(8)
        .filter(|end| *end >= 12 && *end <= bytes.len())
        .ok_or_else(invalid_wav)?;
    let mut position = 12;
    let mut audio_format = None;
    let mut data_bytes = 0usize;
    let mut data_found = false;
    let mut direct_pcm = false;
    let mut data_range = None;
    while position < end {
        check_cancelled(cancel)?;
        if end - position < 8 {
            return Err(invalid_wav());
        }
        let tag = &bytes[position..position + 4];
        let size = read_u32(&bytes[position + 4..position + 8]) as usize;
        let start = position + 8;
        let chunk_end = start
            .checked_add(size)
            .filter(|value| *value <= end)
            .ok_or_else(invalid_wav)?;
        if tag == b"fmt " {
            if size < 16 || audio_format.is_some() {
                return Err(invalid_wav());
            }
            let format = &bytes[start..chunk_end];
            let channels = u16::from_le_bytes([format[2], format[3]]);
            let rate = read_u32(&format[4..8]);
            let align = u16::from_le_bytes([format[12], format[13]]) as usize;
            if channels == 0 || rate == 0 || align == 0 {
                return Err(invalid_wav());
            }
            if channels > 2 {
                return Err(DecodeError::Unsupported(
                    "语音音频仅支持单声道或双声道。".into(),
                ));
            }
            direct_pcm = u16::from_le_bytes([format[0], format[1]]) == 1
                && channels == 1
                && rate == OUTPUT_RATE
                && align == 2
                && u16::from_le_bytes([format[14], format[15]]) == 16;
            audio_format = Some((rate, align));
        } else if tag == b"data" {
            // The WAV demuxer exposes one data chunk. Reject additional audio
            // chunks rather than silently caching a file with unheard samples.
            if data_found {
                return Err(DecodeError::Unsupported(
                    "语音 WAV 音频不支持多个 data 块。".into(),
                ));
            }
            data_found = true;
            let Some((_, align)) = audio_format else {
                return Err(invalid_wav());
            };
            if size % align != 0 {
                return Err(invalid_wav());
            }
            data_bytes = data_bytes.checked_add(size).ok_or_else(invalid_wav)?;
            data_range = Some(start..chunk_end);
        }
        position = chunk_end
            .checked_add(size % 2)
            .filter(|value| *value <= end)
            .ok_or_else(invalid_wav)?;
    }
    let (rate, align) = audio_format.ok_or_else(invalid_wav)?;
    if data_bytes == 0 {
        return Err(invalid_wav());
    }
    output_frames((data_bytes / align) as u64, rate, max_pcm_bytes)?;
    Ok(if direct_pcm { data_range } else { None })
}

fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// Wrap a completed PCM16/24k/mono response in a conventional 44-byte WAV header.
pub(super) fn pcm_wav(pcm: &[u8]) -> Result<Vec<u8>, String> {
    if pcm.is_empty() || pcm.len() % 2 != 0 || pcm.len() > MAX_PCM_BYTES {
        return Err("待缓存的 PCM 音频为空、不完整或超过 8 MiB 上限。".into());
    }
    let mut wav = Vec::new();
    wav.try_reserve_exact(44 + pcm.len())
        .map_err(|_| "无法分配语音 WAV 缓冲。".to_string())?;
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0");
    wav.extend_from_slice(&OUTPUT_RATE.to_le_bytes());
    wav.extend_from_slice(&(OUTPUT_RATE * 2).to_le_bytes());
    wav.extend_from_slice(b"\x02\0\x10\0data");
    wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(pcm);
    Ok(wav)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(rate: u32, channels: u16, pcm: &[u8]) -> Vec<u8> {
        let mut bytes = pcm_wav(pcm).unwrap();
        bytes[22..24].copy_from_slice(&channels.to_le_bytes());
        bytes[24..28].copy_from_slice(&rate.to_le_bytes());
        bytes[28..32].copy_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
        bytes[32..34].copy_from_slice(&(channels * 2).to_le_bytes());
        bytes
    }

    fn sine(rate: u32, frames: usize, frequency: f32, channels: u16) -> Vec<u8> {
        (0..frames)
            .flat_map(|index| {
                let value = ((index as f32 * frequency * std::f32::consts::TAU / rate as f32).sin()
                    * 12_000.0) as i16;
                (0..channels).flat_map(move |_| value.to_le_bytes())
            })
            .collect()
    }

    fn rms(pcm: &[u8]) -> f64 {
        let sum = pcm
            .chunks_exact(2)
            .map(|bytes| f64::from(i16::from_le_bytes([bytes[0], bytes[1]])).powi(2))
            .sum::<f64>();
        (sum / (pcm.len() / 2) as f64).sqrt()
    }

    #[tokio::test]
    async fn wav_24k_preserves_every_pcm16_sample_and_skips_unknown_chunks() {
        let pcm: Vec<_> = (i16::MIN..=i16::MAX).flat_map(i16::to_le_bytes).collect();
        let mut bytes = pcm_wav(&pcm).unwrap();
        bytes.splice(12..12, *b"JUNK\x03\0\0\0abc\0");
        let size = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&size.to_le_bytes());
        let result = decode(bytes.into(), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result.format, ContainerFormat::Wav);
        assert_eq!(result.pcm_24k_mono, pcm);
    }

    #[tokio::test]
    async fn common_sample_rates_have_exact_output_duration_and_preserve_tone() {
        for rate in [16_000, 32_000, 44_100, 48_000] {
            let pcm = sine(rate, rate as usize / 10, 1000.0, 2);
            let result = decode(wav(rate, 2, &pcm).into(), &CancellationToken::new())
                .await
                .unwrap();
            assert_eq!(result.pcm_24k_mono.len(), 4800, "{rate}");
            assert!((rms(&result.pcm_24k_mono) - 8485.0).abs() < 200.0, "{rate}");
        }
    }

    #[tokio::test]
    async fn downsampling_filters_frequencies_above_new_nyquist_limit() {
        let pcm = sine(48_000, 4800, 18_000.0, 1);
        let result = decode(wav(48_000, 1, &pcm).into(), &CancellationToken::new())
            .await
            .unwrap();
        assert!(rms(&result.pcm_24k_mono[500..4300]) < 100.0);
    }

    #[tokio::test]
    async fn stereo_is_mean_mixed_without_clipping() {
        let pcm: Vec<_> = [i16::MAX, i16::MIN, 12_000, -4000]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect();
        let result = decode(wav(24_000, 2, &pcm).into(), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            result.pcm_24k_mono,
            [-1_i16, 4000]
                .into_iter()
                .flat_map(i16::to_le_bytes)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn real_project_lame_mp3_decodes_with_metadata_and_padding() {
        let pcm = sine(16_000, 8000, 440.0, 1);
        let encoded = vcp_voice_mp3::encode_pcm16_mono(&pcm, || false).unwrap();
        let result = decode(encoded.clone().into(), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result.format, ContainerFormat::Mp3);
        // This project's encoder deliberately omits the LAME/Xing tag. Preserve
        // its unknown delay/padding rather than guessing silence to remove.
        assert!(result.pcm_24k_mono.len() >= 24_000);
        assert!(result.pcm_24k_mono.len() < 32_000);
        assert!(rms(&result.pcm_24k_mono) > 5000.0);
        let mut tagged = b"ID3\x04\0\0\0\0\0\0".to_vec();
        tagged.extend_from_slice(&encoded);
        tagged.extend_from_slice(b"TAG");
        tagged.extend_from_slice(&[0; 125]);
        let tagged_result = decode(tagged.into(), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(tagged_result.pcm_24k_mono, result.pcm_24k_mono);

        // Standard metadata and surrounding padding are the media library's
        // responsibility. No project-specific tag or MPEG frame scanner runs.
        let mut padded = vec![0; 16];
        padded.extend_from_slice(&encoded);
        padded.extend_from_slice(&[0; 32]);
        let padded_result = decode(padded.into(), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(padded_result.pcm_24k_mono, result.pcm_24k_mono);
    }

    #[tokio::test]
    async fn malformed_and_truncated_containers_are_rejected() {
        let bytes = pcm_wav(&[1, 0, 2, 0]).unwrap();
        assert!(decode(
            Bytes::copy_from_slice(&bytes[..bytes.len() - 1]),
            &CancellationToken::new()
        )
        .await
        .is_err());
        assert!(decode(
            Bytes::from_static(b"ID3\x04\0\0\0\0\0\0"),
            &CancellationToken::new()
        )
        .await
        .is_err());
        assert!(
            decode(Bytes::from_static(b"not audio"), &CancellationToken::new())
                .await
                .is_err()
        );
        assert!(pcm_wav(&[1]).is_err());
    }

    #[tokio::test]
    async fn upsampling_limit_is_checked_before_expanding_pcm() {
        let input = vec![0_u8; MAX_PCM_BYTES / 2];
        let result = decode(wav(8000, 1, &input).into(), &CancellationToken::new()).await;
        assert_eq!(result.unwrap_err(), pcm_limit());
    }

    #[derive(Default)]
    struct CountingConsumer {
        bytes: usize,
        largest_packet: usize,
    }

    impl PacketConsumer for CountingConsumer {
        async fn append(&mut self, packet: Vec<u8>, cancel: &CancellationToken) -> Result<(), String> {
            check_cancelled(cancel).map_err(|error| error.to_string())?;
            assert!(!packet.is_empty() && packet.len().is_multiple_of(2));
            self.bytes += packet.len();
            self.largest_packet = self.largest_packet.max(packet.len());
            Ok(())
        }

        async fn end_segment(&mut self, _: &CancellationToken) -> Result<(), String> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn long_local_wav_streams_in_bounded_packets_without_relaxing_tts_limit() {
        // A valid 6.4 MB reference expands to 9.6 MB at native playback rate.
        let reference = Bytes::from(wav(16_000, 1, &vec![0; 16_000 * 2 * 200]));
        assert!(reference.len() < 7_000_000);
        assert_eq!(decode(reference.clone(), &CancellationToken::new()).await.unwrap_err(), pcm_limit());
        let mut consumer = CountingConsumer::default();
        decode_stream(reference, &CancellationToken::new(), &mut consumer).await.unwrap();
        assert_eq!(consumer.bytes, 24_000 * 2 * 200);
        assert_eq!(consumer.largest_packet, PCM_PACKET_BYTES);
    }

    #[tokio::test]
    async fn long_real_mp3_streams_beyond_the_complete_tts_sentence_limit() {
        // The project encoder emits plain MPEG frames without a LAME/Xing tag.
        // Concatenating these independent frame sequences yields a long MP3.
        let encoded = vcp_voice_mp3::encode_pcm16_mono(&sine(16_000, 160_000, 440.0, 1), || false).unwrap();
        let reference = Bytes::from(encoded.repeat(20));
        assert!(reference.len() < 7_000_000);
        assert_eq!(decode(reference.clone(), &CancellationToken::new()).await.unwrap_err(), pcm_limit());
        let mut consumer = CountingConsumer::default();
        decode_stream(reference, &CancellationToken::new(), &mut consumer).await.unwrap();
        assert!(consumer.bytes > 24_000 * 2 * 200, "retain encoder padding rather than guessing it away");
        assert!(consumer.bytes < 24_000 * 2 * 205);
        assert_eq!(consumer.largest_packet, PCM_PACKET_BYTES);
    }

    #[tokio::test]
    async fn streaming_and_complete_decode_share_exact_normalization() {
        struct Collected(Vec<u8>);
        impl PacketConsumer for Collected {
            async fn append(&mut self, packet: Vec<u8>, _: &CancellationToken) -> Result<(), String> {
                self.0.extend(packet);
                Ok(())
            }

            async fn end_segment(&mut self, _: &CancellationToken) -> Result<(), String> {
                Ok(())
            }
        }
        for rate in [16_000, 24_000, 44_100, 48_000] {
            let reference = Bytes::from(wav(rate, 2, &sine(rate, rate as usize / 5, 440.0, 2)));
            let expected = decode(reference.clone(), &CancellationToken::new()).await.unwrap();
            let mut collected = Collected(Vec::new());
            decode_stream(reference, &CancellationToken::new(), &mut collected).await.unwrap();
            assert_eq!(collected.0, expected.pcm_24k_mono, "{rate}");
        }
    }

    #[tokio::test]
    async fn cancelling_a_stalled_consumer_releases_and_joins_the_stream_decoder() {
        struct Stalled(Option<tokio::sync::oneshot::Sender<()>>);
        impl PacketConsumer for Stalled {
            async fn append(&mut self, _: Vec<u8>, cancel: &CancellationToken) -> Result<(), String> {
                if let Some(entered) = self.0.take() { let _ = entered.send(()); }
                cancel.cancelled().await;
                Err(CANCELLED.into())
            }

            async fn end_segment(&mut self, _: &CancellationToken) -> Result<(), String> {
                Ok(())
            }
        }
        let reference = Bytes::from(wav(16_000, 1, &vec![0; 16_000 * 2 * 200]));
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let (entered, started) = tokio::sync::oneshot::channel();
        let operation = tokio::spawn(async move {
            decode_stream(reference, &worker_cancel, &mut Stalled(Some(entered))).await
        });
        started.await.unwrap();
        cancel.cancel();
        let result = tokio::time::timeout(std::time::Duration::from_secs(3), operation).await
            .expect("cancellation must unblock the bounded decoder queue").unwrap();
        assert_eq!(result.unwrap_err(), CANCELLED);
    }

    #[test]
    fn incremental_normalization_checks_limit_and_cancellation() {
        let cancel = CancellationToken::new();
        let mut discard = |_: &[u8]| Ok(());
        let mut output = PcmSink { max_bytes: Some(MAX_PCM_BYTES), frames: 0, write: &mut discard };
        let mut normalizer = MonoNormalizer::new(24_000).unwrap();
        normalizer.source_frames = (MAX_PCM_BYTES / 2) as u64;
        assert_eq!(normalizer.push(&[0.0], &cancel, &mut output).unwrap_err(), pcm_limit());
        let mut normalizer = MonoNormalizer::new(48_000).unwrap();
        cancel.cancel();
        assert_eq!(
            normalizer.push(&[0.0; 1024], &cancel, &mut output).unwrap_err(),
            DecodeError::Cancelled
        );
    }

    #[tokio::test]
    async fn cancelled_decode_is_not_started_and_file_size_is_bounded() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            decode(pcm_wav(&[0, 0]).unwrap().into(), &cancel)
                .await
                .unwrap_err(),
            DecodeError::Cancelled
        );
        assert!(decode(
            vec![0; MAX_AUDIO_FILE_BYTES + 1].into(),
            &CancellationToken::new()
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn float_wav_uses_decoder_instead_of_pcm16_fast_path() {
        let samples: Vec<_> = [-1.0_f32, -0.5, 0.5, 1.0]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        let mut bytes = pcm_wav(&samples).unwrap();
        bytes[20..22].copy_from_slice(&3_u16.to_le_bytes());
        bytes[28..32].copy_from_slice(&(OUTPUT_RATE * 4).to_le_bytes());
        bytes[32..34].copy_from_slice(&4_u16.to_le_bytes());
        bytes[34..36].copy_from_slice(&32_u16.to_le_bytes());
        let decoded = decode(bytes.into(), &CancellationToken::new())
            .await
            .unwrap();
        let expected: Vec<_> = [i16::MIN, -16_384, 16_384, i16::MAX]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect();
        assert_eq!(decoded.pcm_24k_mono, expected);
    }

    #[tokio::test]
    async fn pcm_fast_path_validates_all_chunks_and_output_limit_before_copy() {
        let mut invalid_tail = pcm_wav(&[1, 0]).unwrap();
        invalid_tail.extend_from_slice(b"JUNK\x08\0\0\0");
        let size = (invalid_tail.len() - 8) as u32;
        invalid_tail[4..8].copy_from_slice(&size.to_le_bytes());
        assert!(decode(invalid_tail.into(), &CancellationToken::new())
            .await
            .unwrap_err()
            .is_corrupt());

        let mut oversized = pcm_wav(&[0, 0]).unwrap();
        oversized.resize(44 + MAX_PCM_BYTES + 2, 0);
        oversized[4..8].copy_from_slice(&((36 + MAX_PCM_BYTES + 2) as u32).to_le_bytes());
        oversized[40..44].copy_from_slice(&((MAX_PCM_BYTES + 2) as u32).to_le_bytes());
        let error = decode(oversized.into(), &CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error, pcm_limit());
        assert!(!error.is_corrupt());
    }

    #[test]
    fn only_confirmed_media_damage_is_classified_as_corrupt() {
        assert!(media_error("decode", MediaError::DecodeError("bad frame")).is_corrupt());
        assert!(media_error(
            "decode",
            MediaError::IoError(std::io::ErrorKind::UnexpectedEof.into())
        )
        .is_corrupt());
        assert!(!media_error("decode", MediaError::Unsupported("codec")).is_corrupt());
        assert!(!media_error("decode", MediaError::LimitError("allocation")).is_corrupt());
        assert!(!media_error(
            "decode",
            MediaError::IoError(std::io::ErrorKind::Other.into())
        )
        .is_corrupt());
        assert!(!DecodeError::Worker("worker cancelled".into()).is_corrupt());
        assert!(!DecodeError::Cancelled.is_corrupt());
        assert_eq!(DecodeError::Cancelled.to_string(), CANCELLED);
    }
}
