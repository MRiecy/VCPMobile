use super::input::{
    emit_current, export_admission, fail_turn, record_transcript, ExportCompletion,
    PreparedSegment, Utterance, VoiceSession,
};
use super::segmentation::SegmentWindow;
use super::*;
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, OwnedSemaphorePermit};

const MAX_WAV_BYTES: u64 = 1024 * 1024;

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

pub(super) async fn export<R: Runtime>(
    app: AppHandle<R>,
    session: Arc<VoiceSession>,
    turn: Arc<Utterance>,
    window: SegmentWindow,
    slot: OwnedSemaphorePermit,
    _completion: ExportCompletion<R>,
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
                    read_export(&segment.path).await.and_then(|data| {
                        let pcm = wav::pcm16_mono(&data, 16000)?;
                        if pcm.len() as u64 / 2 != segment.sample_count {
                            return Err("语音片段样本数量不匹配。".into());
                        }
                        Ok(data)
                    })
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
    if turn.cancel.is_cancelled() {
        return;
    }
    match result {
        Ok(wav) => {
            let prepared = PreparedSegment {
                sequence: window.sequence,
                final_segment: window.final_segment,
                wav,
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

pub(super) async fn recognize<R: Runtime>(
    app: AppHandle<R>,
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
        emit_current(&app, &session, &turn, "status", Some("recognizing"), None);
        let mut last_partial = None::<Instant>;
        let result = if segment.wav.is_empty() {
            Ok(String::new())
        } else {
            network::recognize(&session.config, &segment.wav, &turn.cancel, |partial| {
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
                        emit_current(
                            &app,
                            &session,
                            &turn,
                            "asr-partial",
                            Some("recognizing"),
                            Some(text),
                        );
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
            emit_current(
                &app,
                &session,
                &turn,
                "asr-partial",
                Some("recording"),
                Some(transcript.clone()),
            );
        }
        sequence = sequence.saturating_add(1);
        // Dropping segment releases the shared audio slot only after recognition.
    }
}
