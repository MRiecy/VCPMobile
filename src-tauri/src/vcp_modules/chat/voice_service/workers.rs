use super::input::{
    export_admission, fail_turn, record_transcript, update_input, ExportCompletion,
    PreparedSegment, Utterance, VoiceSession,
};
use super::segmentation::SegmentWindow;
use super::*;
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tauri::AppHandle;
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, OwnedSemaphorePermit};

const MAX_WAV_BYTES: u64 = 4 * 1024 * 1024;

pub(super) async fn prepare_audio(
    data: Vec<u8>,
    sample_count: u64,
    encoding: VoiceInputEncoding,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<InputAudio, String> {
    if cancel.is_cancelled() {
        return Err(network::CANCELLED.into());
    }
    // A long utterance may end exactly at its last exported boundary. Preserve
    // this empty final marker so ASR can commit the preceding transcript.
    if sample_count == 0 && data.is_empty() {
        return Ok(InputAudio {
            bytes: data,
            encoding,
        });
    }
    if sample_count == 0 || sample_count > 30 * 16_000 || data.len() as u64 > MAX_WAV_BYTES {
        return Err("导出的语音片段范围无效，请重试。".into());
    }
    let check_samples = |actual: usize| {
        if actual as u64 == sample_count {
            Ok(())
        } else {
            Err("语音片段样本数量不匹配。".to_string())
        }
    };
    let bytes = match encoding {
        VoiceInputEncoding::Wav => {
            check_samples(wav::pcm16_mono_sample_count(&data, 16000)?)?;
            data
        }
        VoiceInputEncoding::Mp3 => {
            let pcm = wav::pcm16_mono(&data, 16000)?;
            check_samples(pcm.len() / 2)?;
            drop(data);
            let cancellation = cancel.clone();
            // Keep awaiting the real worker on cancellation. The caller retains
            // its audio slot and utterance work lease until C releases its state.
            tokio::task::spawn_blocking(move || {
                vcp_voice_mp3::encode_pcm16_mono(&pcm, || cancellation.is_cancelled())
            })
            .await
            .map_err(|_| "语音 MP3 编码任务异常结束，请重新说话。".to_string())?
            .map_err(|error| match error {
                vcp_voice_mp3::EncodeError::Cancelled => network::CANCELLED.to_string(),
                _ => "语音 MP3 编码失败，请重新说话。".to_string(),
            })?
        }
    };
    if cancel.is_cancelled() {
        return Err(network::CANCELLED.into());
    }
    Ok(InputAudio { bytes, encoding })
}

async fn read_export(path: &str) -> Result<Vec<u8>, String> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| "无法读取本次语音片段。")?;
    let size = file
        .metadata()
        .await
        .map_err(|_| "无法读取语音片段信息。")?
        .len();
    if size > MAX_WAV_BYTES {
        return Err("导出的语音片段过大。".into());
    }
    let mut data = Vec::with_capacity(size as usize);
    file.take(MAX_WAV_BYTES + 1)
        .read_to_end(&mut data)
        .await
        .map_err(|_| "读取语音片段失败。")?;
    if data.len() as u64 > MAX_WAV_BYTES {
        return Err("导出的语音片段过大。".into());
    }
    Ok(data)
}

pub(super) async fn export(
    app: AppHandle,
    session: Arc<VoiceSession>,
    turn: Arc<Utterance>,
    window: SegmentWindow,
    slot: OwnedSemaphorePermit,
    completion: ExportCompletion,
) {
    if turn.cancel.is_cancelled() {
        return;
    }
    let result = if window.start == window.end {
        Ok(Vec::new())
    } else {
        // Never drop this future on cancellation. The native receiver must be
        // retained so even a late export path is claimed and deleted here.
        let exported = {
            let Some(_guard) = export_admission(&session, &turn).await else {
                return;
            };
            if turn.cancel.is_cancelled() {
                return;
            }
            tauri_plugin_vcp_mobile::voice::export_segment(
                app.clone(),
                session.id.clone(),
                window.epoch,
                window.start,
                window.end,
            )
            .await
        };
        match exported {
            Err(_) => Err("无法导出完整语音片段，请重新说话。".into()),
            Ok(segment) => {
                let result = if turn.cancel.is_cancelled() {
                    Err(network::CANCELLED.into())
                } else if segment.sample_rate != 16000
                    || segment.sample_count != window.end.saturating_sub(window.start)
                {
                    Err("导出的语音片段范围不完整，请重试。".into())
                } else {
                    read_export(&segment.path).await
                };
                if let Err(error) = tokio::fs::remove_file(&segment.path).await {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        log::warn!(
                            "[VoiceService] Temporary export cleanup failed: {}",
                            error.kind()
                        );
                    }
                }
                result
            }
        }
    };
    // The native snapshot and its file are no longer needed. Closing input can
    // finish sealing/releasing capture while accepted encoding and ASR continue.
    drop(completion);
    if turn.cancel.is_cancelled() {
        return;
    }
    let result = match result {
        Ok(data) => {
            prepare_audio(
                data,
                window.end.saturating_sub(window.start),
                session.config.input_encoding,
                &turn.cancel,
            )
            .await
        }
        Err(error) => Err(error),
    };
    if turn.cancel.is_cancelled() {
        return;
    }
    match result {
        Ok(audio) => {
            let prepared = PreparedSegment {
                sequence: window.sequence,
                final_segment: window.final_segment,
                audio,
                _slot: slot,
            };
            if turn.segments.try_send(prepared).is_err() && !turn.cancel.is_cancelled() {
                fail_turn(
                    &app,
                    &session,
                    &turn,
                    "asr",
                    "语音识别队列已满，已取消本次语音，请分段重试。".into(),
                );
            }
        }
        Err(error) => fail_turn(&app, &session, &turn, "asr", error),
    }
}

pub(super) async fn recognize(
    app: AppHandle,
    session: Arc<VoiceSession>,
    turn: Arc<Utterance>,
    mut receiver: mpsc::Receiver<PreparedSegment>,
) {
    let mut pending = BTreeMap::new();
    let mut sequence = 0u32;
    let mut transcript = String::new();
    loop {
        if turn.cancel.is_cancelled() {
            return;
        }
        let segment = if let Some(segment) = pending.remove(&sequence) {
            segment
        } else {
            let next = tokio::select! {
                _ = turn.cancel.cancelled() => return,
                segment = receiver.recv() => segment,
            };
            let Some(segment) = next else {
                return;
            };
            if segment.sequence != sequence {
                if segment.sequence < sequence
                    || pending.insert(segment.sequence, segment).is_some()
                {
                    fail_turn(
                        &app,
                        &session,
                        &turn,
                        "asr",
                        "语音片段顺序无效，已取消本次识别。".into(),
                    );
                    return;
                }
                continue;
            }
            segment
        };
        update_input(&app, &session, &turn, "recognizing", None);
        let mut last_partial = None::<Instant>;
        let result = if segment.audio.bytes.is_empty() {
            Ok(String::new())
        } else {
            network::recognize(&session.config, &segment.audio, &turn.cancel, |partial| {
                if last_partial
                    .is_none_or(|previous| previous.elapsed() >= Duration::from_millis(100))
                {
                    last_partial = Some(Instant::now());
                    if transcript.len().saturating_add(partial.len()) < network::MAX_TEXT_BYTES {
                        let text = if transcript.is_empty() {
                            partial.to_string()
                        } else {
                            format!("{transcript} {partial}")
                        };
                        update_input(&app, &session, &turn, "recognizing", Some(text));
                    }
                }
            })
            .await
        };
        if turn.cancel.is_cancelled() {
            return;
        }
        let text = match result {
            Ok(text) => text,
            Err(error) => {
                fail_turn(&app, &session, &turn, "asr", error);
                return;
            }
        };
        let text = text.trim();
        if transcript
            .len()
            .saturating_add(text.len())
            .saturating_add(1)
            > network::MAX_TEXT_BYTES
        {
            fail_turn(
                &app,
                &session,
                &turn,
                "asr",
                "连续语音转写结果过长，已取消本次语音，请分段重试。".into(),
            );
            return;
        }
        if !text.is_empty() {
            if !transcript.is_empty() {
                transcript.push(' ');
            }
            transcript.push_str(text);
        }
        if segment.final_segment {
            record_transcript(&app, &session, &turn, transcript);
            return;
        }
        if !transcript.is_empty() {
            update_input(&app, &session, &turn, "recording", Some(transcript.clone()));
        }
        sequence = sequence.saturating_add(1);
        // Dropping segment releases the shared audio slot only after recognition.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    fn exported_wav() -> Vec<u8> {
        let mut bytes = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0".to_vec();
        bytes.extend_from_slice(&16000u32.to_le_bytes());
        bytes.extend_from_slice(&32000u32.to_le_bytes());
        bytes.extend_from_slice(b"\x02\0\x10\0data\x04\0\0\0\x01\x02\x03\x04");
        let size = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&size.to_le_bytes());
        bytes
    }

    #[tokio::test]
    async fn both_upload_formats_preserve_samples_across_riff_chunks() {
        let mut data = exported_wav();
        data.extend_from_slice(b"JUNK\x03\0\0\0abc\0data\x04\0\0\0\x05\x06\x07\x08");
        let size = (data.len() - 8) as u32;
        data[4..8].copy_from_slice(&size.to_le_bytes());
        let cancel = CancellationToken::new();
        let wav = prepare_audio(data.clone(), 4, VoiceInputEncoding::Wav, &cancel)
            .await
            .unwrap();
        assert_eq!(wav.bytes, data);
        assert_eq!(wav.encoding, VoiceInputEncoding::Wav);
        let mp3 = prepare_audio(data.clone(), 4, VoiceInputEncoding::Mp3, &cancel)
            .await
            .unwrap();
        assert_eq!(mp3.encoding, VoiceInputEncoding::Mp3);
        assert_eq!(
            mp3.bytes,
            vcp_voice_mp3::encode_pcm16_mono(&[1, 2, 3, 4, 5, 6, 7, 8], || false).unwrap()
        );
        for encoding in [VoiceInputEncoding::Wav, VoiceInputEncoding::Mp3] {
            let mismatch = prepare_audio(data.clone(), 5, encoding, &cancel).await;
            assert!(matches!(mismatch, Err(error) if error.contains("样本数量不匹配")));
        }

        // An exact sample count does not excuse a malformed later RIFF chunk.
        data.extend_from_slice(b"JUNK\x08\0\0\0");
        let size = (data.len() - 8) as u32;
        data[4..8].copy_from_slice(&size.to_le_bytes());
        for encoding in [VoiceInputEncoding::Wav, VoiceInputEncoding::Mp3] {
            assert!(prepare_audio(data.clone(), 4, encoding, &cancel)
                .await
                .is_err());
        }
    }

    #[tokio::test]
    async fn both_upload_formats_preserve_the_empty_final_marker() {
        for encoding in [VoiceInputEncoding::Wav, VoiceInputEncoding::Mp3] {
            let audio = prepare_audio(Vec::new(), 0, encoding, &CancellationToken::new())
                .await
                .unwrap();
            assert!(audio.bytes.is_empty());
            assert_eq!(audio.encoding, encoding);
        }
    }

    #[tokio::test]
    async fn cancelled_upload_is_not_prepared_or_silently_changed_to_wav() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        for encoding in [VoiceInputEncoding::Wav, VoiceInputEncoding::Mp3] {
            let result = prepare_audio(exported_wav(), 2, encoding, &cancellation).await;
            assert!(matches!(result, Err(error) if error == network::CANCELLED));
        }
        let malformed = prepare_audio(
            vec![0; 44],
            2,
            VoiceInputEncoding::Mp3,
            &CancellationToken::new(),
        )
        .await;
        assert!(malformed.is_err());
    }
}
