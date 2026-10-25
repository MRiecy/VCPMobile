//! Bounded segment prefetch; native playback never backpressures an HTTP body.
use super::{
    audio_codec::{MAX_PCM_BYTES, PCM_PACKET_BYTES},
    network::CANCELLED,
};
use std::{
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::sync::{mpsc, Notify};
use tokio_util::sync::CancellationToken;

const PREFETCH_SEGMENTS: usize = 2;

#[derive(Default)]
struct SegmentState {
    pcm: Vec<u8>,
    cursor: usize,
    finished: bool,
}

/// One continuous allocation avoids an allocation/queue entry for every tiny
/// SSE delta. The cap measures the complete segment, including consumed bytes.
#[derive(Default)]
pub(super) struct SegmentBuffer {
    state: Mutex<SegmentState>,
    changed: Notify,
}

impl SegmentBuffer {
    pub(super) fn push(&self, pcm: Vec<u8>) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        if state.finished || pcm.len() % 2 != 0 {
            return Err("语音分段的 PCM 数据无效。".into());
        }
        if state.pcm.len().saturating_add(pcm.len()) > MAX_PCM_BYTES {
            return Err("语音分段超过音频大小限制。".into());
        }
        if state.pcm.is_empty() {
            state.pcm = pcm;
        } else {
            state.pcm.extend_from_slice(&pcm);
        }
        drop(state);
        self.changed.notify_one();
        Ok(())
    }

    pub(super) fn finish(&self) -> Result<(), String> {
        self.state
            .lock()
            .map_err(|error| error.to_string())?
            .finished = true;
        self.changed.notify_one();
        Ok(())
    }

    fn ensure_finished(&self) -> Result<(), String> {
        if self
            .state
            .lock()
            .map_err(|error| error.to_string())?
            .finished
        {
            Ok(())
        } else {
            Err("语音分段准备结束，但未确认音频完整。".into())
        }
    }

    /// The streaming collector itself is the cache source. Only the final WAV
    /// container is copied; network does not retain another complete PCM body.
    pub(super) async fn wav(
        self: &Arc<Self>,
        cancel: &CancellationToken,
    ) -> Result<Vec<u8>, String> {
        let segment = self.clone();
        let cancel = cancel.clone();
        tokio::task::spawn_blocking(move || {
            if cancel.is_cancelled() {
                return Err(CANCELLED.into());
            }
            let state = segment.state.lock().map_err(|error| error.to_string())?;
            if !state.finished {
                return Err("语音分段尚未完整接收。".into());
            }
            let wav = super::audio_codec::pcm_wav(&state.pcm)?;
            if cancel.is_cancelled() {
                return Err(CANCELLED.into());
            }
            Ok(wav)
        })
        .await
        .map_err(|_| "语音缓存封装任务失败。".to_string())?
    }

    async fn next(&self, cancel: &CancellationToken) -> Result<Option<Vec<u8>>, String> {
        loop {
            if cancel.is_cancelled() {
                return Err(CANCELLED.into());
            }
            let notified = self.changed.notified();
            {
                let mut state = self.state.lock().map_err(|error| error.to_string())?;
                if state.cursor < state.pcm.len() {
                    let end = (state.cursor + PCM_PACKET_BYTES).min(state.pcm.len());
                    let packet = state.pcm[state.cursor..end].to_vec();
                    state.cursor = end;
                    return Ok(Some(packet));
                }
                if state.finished {
                    return Ok(None);
                }
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(CANCELLED.into()),
                _ = notified => {},
            }
        }
    }
}

pub(super) trait SegmentSource {
    type Piece;
    fn next(
        &mut self,
        cancel: &CancellationToken,
    ) -> impl Future<Output = Result<Option<Self::Piece>, String>> + Send;
}

pub(super) trait PacketConsumer {
    fn append(
        &mut self,
        pcm: Vec<u8>,
        cancel: &CancellationToken,
    ) -> impl Future<Output = Result<(), String>> + Send;
}

/// There is one HTTP producer and one native consumer. A permit is acquired
/// before reading the next text piece or touching its cache/network, so K=2
/// includes a segment which is still being synthesized. The prepare owner must
/// finish its buffer once the complete audio has been received and validated.
pub(super) async fn run<S, P, F, C>(
    cancel: &CancellationToken,
    mut source: S,
    mut prepare: P,
    consumer: &mut C,
) -> Result<(), String>
where
    S: SegmentSource,
    P: FnMut(S::Piece, Arc<SegmentBuffer>, CancellationToken) -> F,
    F: Future<Output = Result<(), String>>,
    C: PacketConsumer,
{
    let local = cancel.child_token();
    let (sender, mut receiver) = mpsc::channel::<Arc<SegmentBuffer>>(PREFETCH_SEGMENTS);
    let produce = async {
        let result: Result<(), String> = async {
            loop {
                let permit = tokio::select! {
                    biased;
                    _ = local.cancelled() => return Err(CANCELLED.into()),
                    permit = sender.reserve() => permit.map_err(|_| CANCELLED.to_string())?,
                };
                let Some(piece) = source.next(&local).await? else {
                    return Ok(());
                };
                let segment = Arc::new(SegmentBuffer::default());
                permit.send(segment.clone());
                // Do not select/drop prepare: cache IO and decode workers own
                // their cancellation and must be awaited to actual completion.
                prepare(piece, segment.clone(), local.clone()).await?;
                segment.ensure_finished()?;
            }
        }
        .await;
        drop(sender);
        if result.is_err() {
            local.cancel();
        }
        result
    };
    let consume = async {
        let result: Result<(), String> = async {
            loop {
                let segment = tokio::select! {
                    biased;
                    _ = local.cancelled() => return Err(CANCELLED.into()),
                    segment = receiver.recv() => segment,
                };
                let Some(segment) = segment else {
                    return Ok(());
                };
                while let Some(pcm) = segment.next(&local).await? {
                    consumer.append(pcm, &local).await?;
                }
            }
        }
        .await;
        if result.is_err() {
            local.cancel();
        }
        result
    };
    // Both sides finish their cleanup even when one side fails. Internal failure
    // cancels only this child token; the caller still reports the original error.
    let (produced, consumed) = tokio::join!(produce, consume);
    for result in [&produced, &consumed] {
        if let Err(error) = result {
            if error != CANCELLED {
                return Err(error.clone());
            }
        }
    }
    produced.and(consumed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::VecDeque,
        sync::atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::Semaphore;

    struct Source(VecDeque<u8>);
    impl SegmentSource for Source {
        type Piece = u8;
        async fn next(&mut self, _: &CancellationToken) -> Result<Option<u8>, String> {
            Ok(self.0.pop_front())
        }
    }

    struct SlowConsumer {
        gate: Arc<Semaphore>,
        output: Vec<u8>,
    }
    impl PacketConsumer for SlowConsumer {
        async fn append(&mut self, pcm: Vec<u8>, cancel: &CancellationToken) -> Result<(), String> {
            if self.output.is_empty() {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(CANCELLED.into()),
                    permit = self.gate.acquire() => permit.unwrap().forget(),
                }
            }
            self.output.extend(pcm);
            Ok(())
        }
    }

    #[tokio::test]
    async fn slow_native_append_allows_two_complete_prefetches_but_not_a_third_request() {
        let cancel = CancellationToken::new();
        let gate = Arc::new(Semaphore::new(0));
        let release = gate.clone();
        let (requested, mut requests) = mpsc::channel(8);
        let worker = tokio::spawn(async move {
            let mut consumer = SlowConsumer {
                gate,
                output: vec![],
            };
            run(
                &cancel,
                Source((0..4).collect()),
                |piece, segment, _| {
                    let requested = requested.clone();
                    async move {
                        requested.send(piece).await.unwrap();
                        segment.push(vec![piece, 0])?;
                        segment.finish()
                    }
                },
                &mut consumer,
            )
            .await
            .unwrap();
            consumer.output
        });
        for expected in 0..3 {
            assert_eq!(requests.recv().await, Some(expected));
        }
        tokio::task::yield_now().await;
        assert!(requests.try_recv().is_err());
        release.add_permits(1);
        assert_eq!(requests.recv().await, Some(3));
        assert_eq!(worker.await.unwrap(), vec![0, 0, 1, 0, 2, 0, 3, 0]);
    }

    #[tokio::test(start_paused = true)]
    async fn failed_s2_is_requested_once_and_cancels_playback_without_requesting_s3() {
        let cancel = CancellationToken::new();
        let gate = Arc::new(Semaphore::new(0));
        let (requested, mut requests) = mpsc::channel(8);
        let started = tokio::time::Instant::now();
        let worker = tokio::spawn(async move {
            let mut consumer = SlowConsumer {
                gate,
                output: vec![],
            };
            let result = run(
                &cancel,
                Source((0..4).collect()),
                |piece, segment, _| {
                    let requested = requested.clone();
                    async move {
                        requested.send(piece).await.unwrap();
                        if piece == 2 {
                            segment.push(vec![99, 0, 99, 0])?;
                            return Err("transport interrupted".into());
                        }
                        segment.push(vec![piece, 0])?;
                        segment.finish()
                    }
                },
                &mut consumer,
            )
            .await;
            (result, consumer.output)
        });
        assert_eq!(requests.recv().await, Some(0));
        assert_eq!(requests.recv().await, Some(1));
        assert_eq!(requests.recv().await, Some(2));
        let (result, played) = worker.await.unwrap();
        assert_eq!(result.unwrap_err(), "transport interrupted");
        assert!(
            played.is_empty(),
            "native append must be cancelled without unblocking its gate"
        );
        assert_eq!(
            requests.recv().await,
            None,
            "no retry or S3 request may survive the error"
        );
        assert!(
            started.elapsed().is_zero(),
            "failure must not enter a backoff delay"
        );
    }

    #[tokio::test]
    async fn prepare_without_explicit_finish_is_an_error_instead_of_success_or_hanging() {
        let mut consumer = SlowConsumer {
            gate: Arc::new(Semaphore::new(1)),
            output: vec![],
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            run(
                &CancellationToken::new(),
                Source([0].into()),
                |_, segment, _| async move { segment.push(vec![1, 0]) },
                &mut consumer,
            ),
        )
        .await
        .expect("missing finish must not leave the consumer waiting forever");
        assert_eq!(result.unwrap_err(), "语音分段准备结束，但未确认音频完整。");
    }

    #[test]
    fn complete_pcm_buffer_is_taken_without_copying() {
        let segment = SegmentBuffer::default();
        let pcm = vec![0; PCM_PACKET_BYTES * 2];
        let allocation = pcm.as_ptr();
        segment.push(pcm).unwrap();
        assert_eq!(segment.state.lock().unwrap().pcm.as_ptr(), allocation);
    }

    #[tokio::test]
    async fn tiny_stream_deltas_are_limited_by_bytes_and_join_into_native_sized_packets() {
        let segment = SegmentBuffer::default();
        for _ in 0..100_000 {
            segment.push(vec![1, 0]).unwrap();
        }
        segment.finish().unwrap();
        let mut bytes = 0;
        let mut packets = 0;
        while let Some(pcm) = segment.next(&CancellationToken::new()).await.unwrap() {
            assert!(pcm.len() <= PCM_PACKET_BYTES);
            bytes += pcm.len();
            packets += 1;
        }
        assert_eq!(bytes, 200_000);
        assert_eq!(packets, 200_000usize.div_ceil(PCM_PACKET_BYTES));
        let segment = SegmentBuffer::default();
        segment.push(vec![0; MAX_PCM_BYTES]).unwrap();
        assert!(segment.push(vec![0, 0]).is_err());
    }

    #[tokio::test]
    async fn completed_stream_cache_uses_the_same_pcm_even_after_playback_consumes_it() {
        let segment = Arc::new(SegmentBuffer::default());
        let cancel = CancellationToken::new();
        segment.push(vec![1, 0, 2, 0]).unwrap();
        assert_eq!(segment.next(&cancel).await.unwrap(), Some(vec![1, 0, 2, 0]));
        segment.push(vec![3, 0]).unwrap();
        assert!(segment.wav(&cancel).await.is_err());
        segment.finish().unwrap();
        let wav = segment.wav(&cancel).await.unwrap();
        assert_eq!(
            super::super::wav::pcm16_mono(&wav, 24000).unwrap(),
            [1, 0, 2, 0, 3, 0]
        );
        cancel.cancel();
        assert_eq!(segment.wav(&cancel).await.unwrap_err(), CANCELLED);
    }

    #[tokio::test]
    async fn cancellation_wakes_empty_segment() {
        let segment = SegmentBuffer::default();
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let waiter = tokio::spawn(async move { segment.next(&cancel).await });
        stop.cancel();
        assert_eq!(waiter.await.unwrap().unwrap_err(), CANCELLED);
    }

    #[tokio::test]
    async fn cancellation_joins_file_or_decode_cleanup_instead_of_dropping_the_producer() {
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let entered = Arc::new(Notify::new());
        let started = entered.clone();
        let cleanup = Arc::new(Notify::new());
        let finish_cleanup = cleanup.clone();
        let worker = tokio::spawn(async move {
            let mut consumer = SlowConsumer {
                gate: Arc::new(Semaphore::new(0)),
                output: vec![],
            };
            run(
                &cancel,
                Source([0].into()),
                |_, segment, cancel| {
                    let started = started.clone();
                    let cleanup = cleanup.clone();
                    async move {
                        segment.push(vec![0, 0])?;
                        started.notify_one();
                        cancel.cancelled().await;
                        cleanup.notified().await;
                        Err(CANCELLED.into())
                    }
                },
                &mut consumer,
            )
            .await
        });
        entered.notified().await;
        stop.cancel();
        tokio::task::yield_now().await;
        assert!(!worker.is_finished());
        finish_cleanup.notify_one();
        assert_eq!(worker.await.unwrap().unwrap_err(), CANCELLED);
    }

    struct FailedConsumer;
    impl PacketConsumer for FailedConsumer {
        async fn append(&mut self, _: Vec<u8>, _: &CancellationToken) -> Result<(), String> {
            Err("native playback failure".into())
        }
    }

    #[tokio::test]
    async fn consumer_failure_cancels_producer_and_preserves_original_error_after_cleanup() {
        let cleaned = Arc::new(AtomicUsize::new(0));
        let finished = cleaned.clone();
        let result = run(
            &CancellationToken::new(),
            Source([0].into()),
            |_, segment, cancel| {
                let finished = finished.clone();
                async move {
                    segment.push(vec![0, 0])?;
                    cancel.cancelled().await;
                    finished.fetch_add(1, Ordering::SeqCst);
                    Err(CANCELLED.into())
                }
            },
            &mut FailedConsumer,
        )
        .await;
        assert_eq!(result.unwrap_err(), "native playback failure");
        assert_eq!(cleaned.load(Ordering::SeqCst), 1);
    }
}
