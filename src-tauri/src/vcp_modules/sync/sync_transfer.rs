//! Application payload throughput for one manual sync session. Activity is the union of
//! requests. A streaming response stays active between reads because the transport can keep
//! receiving into its buffers. Local work outside requests and finalization do not run the clock.
use bytes::Bytes;
use futures_util::{stream::BoxStream, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TransferSnapshot {
    uploaded_bytes: u64,
    downloaded_bytes: u64,
    active_duration_ms: f64,
}

#[derive(Debug, Default)]
struct TransferState {
    uploaded: u64,
    downloaded: u64,
    active: usize,
    started: Option<Instant>,
    elapsed: Duration,
    frozen: bool,
}

impl TransferState {
    fn begin(&mut self, now: Instant) {
        if self.frozen {
            return;
        }
        if self.active == 0 {
            self.started = Some(now);
        }
        self.active += 1;
    }

    fn end(&mut self, now: Instant) {
        if self.frozen || self.active == 0 {
            return;
        }
        self.active -= 1;
        if self.active == 0 {
            if let Some(started) = self.started.take() {
                self.elapsed += now.saturating_duration_since(started);
            }
        }
    }

    fn snapshot(&self, now: Instant) -> TransferSnapshot {
        let elapsed = self.elapsed
            + self
                .started
                .map_or(Duration::ZERO, |start| now.saturating_duration_since(start));
        TransferSnapshot {
            uploaded_bytes: self.uploaded,
            downloaded_bytes: self.downloaded,
            active_duration_ms: elapsed.as_secs_f64() * 1000.0,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct TransferMeter(Arc<Mutex<TransferState>>);

impl TransferMeter {
    // A telemetry lock must never turn a recoverable sync into a failure.
    fn state(&self) -> std::sync::MutexGuard<'_, TransferState> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(super) fn begin(&self) -> TransferActivity {
        self.state().begin(Instant::now());
        TransferActivity(self.clone())
    }

    fn uploaded(&self, bytes: usize) {
        let mut state = self.state();
        if !state.frozen {
            state.uploaded = state.uploaded.saturating_add(bytes as u64);
        }
    }

    fn downloaded(&self, bytes: usize) {
        let mut state = self.state();
        if !state.frozen {
            state.downloaded = state.downloaded.saturating_add(bytes as u64);
        }
    }

    pub(super) fn snapshot(&self) -> TransferSnapshot {
        self.state().snapshot(Instant::now())
    }

    pub(super) fn freeze(&self) -> TransferSnapshot {
        let mut state = self.state();
        let now = Instant::now();
        if let Some(started) = state.started.take() {
            state.elapsed += now.saturating_duration_since(started);
        }
        state.frozen = true;
        state.active = 0;
        state.snapshot(now)
    }

    pub(super) fn is_frozen(&self) -> bool {
        self.state().frozen
    }
}

#[derive(Debug)]
pub(super) struct TransferActivity(TransferMeter);

impl Drop for TransferActivity {
    fn drop(&mut self) {
        self.0.state().end(Instant::now());
    }
}

const UPLOAD_CHUNK_BYTES: usize = 64 * 1024;

struct UploadStream {
    remaining: Bytes,
    meter: TransferMeter,
    activity: Option<TransferActivity>,
}

impl Stream for UploadStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.remaining.is_empty() {
            self.activity.take();
            return Poll::Ready(None);
        }
        // A peer can respond before its request body has finished. Keep upload activity
        // independently alive from the first consumed chunk through EOF/drop.
        if self.activity.is_none() {
            self.activity = Some(self.meter.begin());
        }
        let length = self.remaining.len().min(UPLOAD_CHUNK_BYTES);
        let chunk = self.remaining.split_to(length);
        self.meter.uploaded(length);
        if self.remaining.is_empty() {
            self.activity.take();
        }
        Poll::Ready(Some(Ok(chunk)))
    }
}

/// Only the sync transport uses this wrapper; request construction and validation stay in
/// the existing executors. Known body lengths are preserved when installing the stream.
#[derive(Clone)]
pub struct SyncHttpClient {
    inner: reqwest::Client,
    meter: TransferMeter,
}

impl SyncHttpClient {
    pub(super) fn new(inner: reqwest::Client, meter: TransferMeter) -> Self {
        Self { inner, meter }
    }

    pub fn post(&self, url: impl reqwest::IntoUrl) -> SyncRequestBuilder {
        SyncRequestBuilder {
            inner: self.inner.post(url),
            meter: self.meter.clone(),
        }
    }

    pub fn get(&self, url: impl reqwest::IntoUrl) -> SyncRequestBuilder {
        SyncRequestBuilder {
            inner: self.inner.get(url),
            meter: self.meter.clone(),
        }
    }
}

pub struct SyncRequestBuilder {
    inner: reqwest::RequestBuilder,
    meter: TransferMeter,
}

impl SyncRequestBuilder {
    pub fn header(mut self, name: &'static str, value: impl AsRef<str>) -> Self {
        self.inner = self.inner.header(name, value.as_ref());
        self
    }

    pub fn body(mut self, body: Vec<u8>) -> Self {
        let length = body.len();
        self.inner = self
            .inner
            .header(reqwest::header::CONTENT_LENGTH, length)
            .body(reqwest::Body::wrap_stream(UploadStream {
                remaining: Bytes::from(body),
                meter: self.meter.clone(),
                activity: None,
            }));
        self
    }

    pub async fn send(self) -> Result<SyncResponse, reqwest::Error> {
        let activity = self.meter.begin();
        let response = self.inner.send().await?;
        Ok(SyncResponse {
            inner: response,
            meter: self.meter,
            activity,
        })
    }
}

pub struct SyncResponse {
    inner: reqwest::Response,
    meter: TransferMeter,
    activity: TransferActivity,
}

impl SyncResponse {
    pub fn status(&self) -> reqwest::StatusCode {
        self.inner.status()
    }
    pub fn content_length(&self) -> Option<u64> {
        self.inner.content_length()
    }
    pub fn headers(&self) -> &reqwest::header::HeaderMap {
        self.inner.headers()
    }

    pub fn bytes_stream(self) -> DownloadStream {
        let remaining = self.inner.content_length();
        DownloadStream {
            inner: self.inner.bytes_stream().boxed(),
            meter: self.meter,
            activity: (remaining != Some(0)).then_some(self.activity),
            remaining,
        }
    }
}

pub struct DownloadStream {
    inner: BoxStream<'static, Result<Bytes, reqwest::Error>>,
    meter: TransferMeter,
    activity: Option<TransferActivity>,
    remaining: Option<u64>,
}

impl Stream for DownloadStream {
    type Item = Result<Bytes, reqwest::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let result = self.inner.as_mut().poll_next(cx);
        if let Poll::Ready(chunk) = &result {
            if let Some(Ok(bytes)) = chunk {
                self.meter.downloaded(bytes.len());
                if let Some(remaining) = &mut self.remaining {
                    *remaining = remaining.saturating_sub(bytes.len() as u64);
                }
                // Exclude parsing after a known-length body's last chunk. Decompressed or
                // chunked bodies keep their activity until the transport reports EOF.
                if self.remaining == Some(0) {
                    self.activity.take();
                }
            } else {
                self.activity.take();
            }
        }
        result
    }
}

type RawWebSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransferFrame {
    #[serde(rename = "type")]
    kind: String,
    nonce: Option<String>,
    session_id: Option<u64>,
    attempt_id: Option<u64>,
    phase: Option<String>,
}

impl TransferFrame {
    fn expected_response(&self) -> Option<String> {
        match self.kind.as_str() {
            "VERSION_CHECK" => Some("VERSION_ACK".into()),
            "SYNC_MANIFEST_REQUEST" => Some("SYNC_MANIFEST_RESULT".into()),
            "SYNC_TOPIC_DIFF_REQUEST" => Some("SYNC_TOPIC_DIFF_RESULT".into()),
            "SYNC_MESSAGE_DIFF_REQUEST" => Some("SYNC_MESSAGE_DIFF_RESULT".into()),
            "PHASE_COMPLETED" => self.final_ack_key(),
            _ => None,
        }
    }

    fn final_ack_key(&self) -> Option<String> {
        Some(format!(
            "PHASE_ACK:{}:{}:{}:{}",
            self.session_id?,
            self.attempt_id?,
            self.phase.as_deref()?,
            self.nonce.as_deref()?
        ))
    }

    fn response_key(&self) -> Option<String> {
        match self.kind.as_str() {
            "PHASE_ACK" => self.final_ack_key(),
            _ => Some(self.kind.clone()),
        }
    }
}

#[derive(Debug)]
pub(super) struct SyncWebSocket {
    inner: RawWebSocket,
    meter: TransferMeter,
    pending: HashMap<String, TransferActivity>,
}

async fn send_websocket_payload<S>(
    socket: &mut S,
    message: Message,
    meter: &TransferMeter,
) -> Result<Option<(String, TransferActivity)>, S::Error>
where
    S: futures_util::Sink<Message> + Unpin,
{
    use futures_util::SinkExt;
    let transfer = match &message {
        Message::Text(text) => Some((
            text.len(),
            serde_json::from_str::<TransferFrame>(text)
                .ok()
                .and_then(|frame| frame.expected_response()),
        )),
        _ => None,
    };
    let activity = transfer.as_ref().map(|_| meter.begin());
    socket.feed(message).await?;
    // feed has handed this frame to the transport. A subsequent flush failure must
    // retain those accepted bytes, just as an HTTP body remains counted after a failure.
    if let Some((length, _)) = &transfer {
        meter.uploaded(*length);
    }
    socket.flush().await?;
    Ok(match (transfer, activity) {
        (Some((_, Some(response))), Some(activity)) => Some((response, activity)),
        _ => None,
    })
}

impl SyncWebSocket {
    pub(super) fn new(inner: RawWebSocket, meter: TransferMeter) -> Self {
        Self {
            inner,
            meter,
            pending: HashMap::new(),
        }
    }

    pub(super) async fn send(
        &mut self,
        message: Message,
    ) -> Result<(), tokio_tungstenite::tungstenite::Error> {
        if let Some((response, activity)) =
            send_websocket_payload(&mut self.inner, message, &self.meter).await?
        {
            self.pending.insert(response, activity);
        }
        Ok(())
    }

    pub(super) async fn next(
        &mut self,
    ) -> Option<Result<Message, tokio_tungstenite::tungstenite::Error>> {
        let message = self.inner.next().await;
        match &message {
            Some(Ok(Message::Text(text))) => {
                if let Ok(frame) = serde_json::from_str::<TransferFrame>(text) {
                    if frame.kind == "SYNC_ERROR" && !self.pending.is_empty() {
                        self.meter.downloaded(text.len());
                        self.pending.clear();
                    } else if let Some(key) = frame.response_key() {
                        if self.pending.contains_key(&key) {
                            self.meter.downloaded(text.len());
                            self.pending.remove(&key);
                        }
                    }
                }
                // Diagnostics, unsolicited phase notifications, stale and malformed replies
                // have no owned request interval. Forward them unchanged without counting them.
            }
            None | Some(Err(_)) | Some(Ok(Message::Close(_))) => self.pending.clear(),
            _ => {}
        }
        message
    }

    pub(super) fn clear_pending_transfer(&mut self) {
        self.pending.clear();
    }

    pub(super) async fn close(
        &mut self,
        frame: Option<tokio_tungstenite::tungstenite::protocol::CloseFrame>,
    ) -> Result<(), tokio_tungstenite::tungstenite::Error> {
        self.pending.clear();
        self.inner.close(frame).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn overlapping_network_operations_use_union_and_exclude_idle_time() {
        let now = Instant::now();
        let mut state = TransferState::default();
        state.begin(now);
        state.begin(now + Duration::from_millis(20));
        state.end(now + Duration::from_millis(70));
        state.end(now + Duration::from_millis(100));
        state.begin(now + Duration::from_millis(500));
        state.end(now + Duration::from_millis(550));
        assert_eq!(
            state
                .snapshot(now + Duration::from_secs(5))
                .active_duration_ms,
            150.0
        );
    }

    #[tokio::test]
    async fn upload_counts_only_consumed_chunks_and_does_not_copy_the_body() {
        let meter = TransferMeter::default();
        let bytes = Bytes::from(vec![7; UPLOAD_CHUNK_BYTES * 3]);
        let original_pointer = bytes.as_ptr();
        let mut stream = UploadStream {
            remaining: bytes,
            meter: meter.clone(),
            activity: None,
        };
        assert_eq!(meter.snapshot().uploaded_bytes, 0);
        let chunk = stream
            .next()
            .await
            .expect("first chunk")
            .expect("upload bytes");
        assert_eq!(chunk.as_ptr(), original_pointer);
        assert_eq!(chunk.len(), UPLOAD_CHUNK_BYTES);
        assert_eq!(meter.state().active, 1); // Also covers a peer responding before upload EOF.
        drop(stream); // Request cancellation must not count the unread remainder.
        assert_eq!(meter.snapshot().uploaded_bytes, UPLOAD_CHUNK_BYTES as u64);
        assert_eq!(meter.state().active, 0);
    }

    #[tokio::test]
    async fn download_activity_includes_buffering_between_chunks_and_ends_on_cancel() {
        let meter = TransferMeter::default();
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut stream = DownloadStream {
            inner: futures_util::stream::poll_fn(move |cx| receiver.poll_recv(cx)).boxed(),
            meter: meter.clone(),
            activity: Some(meter.begin()),
            remaining: None,
        };
        assert!(futures_util::poll!(stream.next()).is_pending());
        assert_eq!(meter.state().active, 1);
        sender
            .send(Ok(Bytes::from_static(b"received")))
            .expect("deliver bytes");
        assert!(stream.next().await.expect("chunk").is_ok());
        // The transport may receive into its buffers during parsing/DB backpressure.
        assert_eq!(meter.state().active, 1);
        assert_eq!(meter.snapshot().downloaded_bytes, 8);
        assert!(futures_util::poll!(stream.next()).is_pending());
        drop(stream);
        assert_eq!(meter.state().active, 0);
    }

    #[tokio::test]
    async fn completed_response_releases_activity_at_known_length_or_eof() {
        for remaining in [Some(8), None] {
            let meter = TransferMeter::default();
            let mut stream = DownloadStream {
                inner: futures_util::stream::iter([Ok(Bytes::from_static(b"received"))]).boxed(),
                meter: meter.clone(),
                activity: Some(meter.begin()),
                remaining,
            };
            stream.next().await.expect("chunk").expect("bytes");
            assert_eq!(meter.state().active, usize::from(remaining.is_none()));
            assert!(stream.next().await.is_none());
            assert_eq!(meter.state().active, 0);
        }
    }

    #[test]
    fn terminal_snapshot_stays_frozen_when_inflight_work_finishes() {
        let meter = TransferMeter::default();
        let activity = meter.begin();
        meter.uploaded(100);
        meter.downloaded(20);
        let final_snapshot = meter.freeze();
        meter.uploaded(500);
        meter.downloaded(500);
        drop(activity);
        drop(meter.begin());
        assert_eq!(meter.snapshot(), final_snapshot);
        assert_eq!(final_snapshot.uploaded_bytes, 100);
        assert_eq!(final_snapshot.downloaded_bytes, 20);
        assert_eq!(
            TransferMeter::default().snapshot(),
            TransferSnapshot::default()
        );
    }

    #[tokio::test]
    async fn actual_http_preserves_body_headers_and_counts_partial_failed_response() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind HTTP");
        let address = listener.local_addr().expect("HTTP address");
        let body = vec![b'x'; UPLOAD_CHUNK_BYTES * 2 + 17];
        let body_length = body.len();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept HTTP");
            let mut received = Vec::new();
            let header_end = loop {
                let mut chunk = [0u8; 8192];
                let read = stream.read(&mut chunk).await.expect("read HTTP request");
                assert!(read > 0);
                received.extend_from_slice(&chunk[..read]);
                if let Some(position) = received.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    break position + 4;
                }
            };
            let headers = String::from_utf8_lossy(&received[..header_end]).to_ascii_lowercase();
            assert!(headers.contains(&format!("content-length: {body_length}\r\n")));
            assert!(headers.contains("content-type: application/x-ndjson\r\n"));
            assert!(!headers.contains("transfer-encoding:"));
            while received.len() - header_end < body_length {
                let mut chunk = [0u8; 8192];
                let read = stream.read(&mut chunk).await.expect("read HTTP body");
                assert!(read > 0);
                received.extend_from_slice(&chunk[..read]);
            }
            assert!(received[header_end..].iter().all(|byte| *byte == b'x'));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\nConnection: close\r\n\r\nshort",
                )
                .await
                .expect("send partial response");
            stream.shutdown().await.expect("close partial response");
        });
        let meter = TransferMeter::default();
        let client = SyncHttpClient::new(reqwest::Client::new(), meter.clone());
        let request = client
            .post(format!("http://{address}/sync"))
            .header("Content-Type", "application/x-ndjson")
            .body(body);
        assert_eq!(meter.snapshot().uploaded_bytes, 0);
        let response = request.send().await.expect("response headers");
        let mut stream = response.bytes_stream();
        let mut failed = false;
        while let Some(chunk) = stream.next().await {
            if chunk.is_err() {
                failed = true;
                break;
            }
        }
        assert!(failed, "truncated body must remain a transport failure");
        server.await.expect("HTTP server finished");
        assert_eq!(meter.snapshot().uploaded_bytes, body_length as u64);
        assert_eq!(meter.snapshot().downloaded_bytes, 5);
        assert_eq!(meter.state().active, 0);
    }

    #[tokio::test]
    async fn websocket_counts_accepted_frames_even_when_flush_fails() {
        struct FailingSink {
            reject_before_accept: bool,
        }
        impl futures_util::Sink<Message> for FailingSink {
            type Error = &'static str;
            fn poll_ready(
                self: Pin<&mut Self>,
                _: &mut Context<'_>,
            ) -> Poll<Result<(), Self::Error>> {
                Poll::Ready(if self.reject_before_accept {
                    Err("not accepted")
                } else {
                    Ok(())
                })
            }
            fn start_send(self: Pin<&mut Self>, _: Message) -> Result<(), Self::Error> {
                Ok(())
            }
            fn poll_flush(
                self: Pin<&mut Self>,
                _: &mut Context<'_>,
            ) -> Poll<Result<(), Self::Error>> {
                Poll::Ready(Err("accepted but flush failed"))
            }
            fn poll_close(
                self: Pin<&mut Self>,
                _: &mut Context<'_>,
            ) -> Poll<Result<(), Self::Error>> {
                Poll::Ready(Ok(()))
            }
        }
        let text = r#"{"type":"VERSION_CHECK"}"#;
        for reject_before_accept in [false, true] {
            let meter = TransferMeter::default();
            let result = send_websocket_payload(
                &mut FailingSink {
                    reject_before_accept,
                },
                Message::Text(text.into()),
                &meter,
            )
            .await;
            assert!(result.is_err());
            assert_eq!(
                meter.snapshot().uploaded_bytes,
                if reject_before_accept {
                    0
                } else {
                    text.len() as u64
                }
            );
            assert_eq!(meter.state().active, 0);
        }
    }

    #[tokio::test]
    async fn websocket_waits_for_matching_response_and_excludes_heartbeat_and_idle() {
        use futures_util::SinkExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind WS");
        let address = listener.local_addr().expect("WS address");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept WS");
            tokio_tungstenite::accept_async(stream)
                .await
                .expect("upgrade WS")
        });
        let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}"))
            .await
            .expect("connect WS");
        let mut server = server.await.expect("server socket");
        let meter = TransferMeter::default();
        let mut socket = SyncWebSocket::new(socket, meter.clone());
        let request = r#"{"type":"SYNC_MANIFEST_REQUEST"}"#;
        socket
            .send(Message::Text(request.into()))
            .await
            .expect("send request");
        server
            .next()
            .await
            .expect("request frame")
            .expect("read request");
        assert_eq!(meter.state().active, 1);
        server
            .send(Message::Text(r#"{"type":"SYNC_LOG_EVENT"}"#.into()))
            .await
            .expect("send log");
        socket.next().await.expect("log frame").expect("read log");
        assert_eq!(meter.state().active, 1);
        assert_eq!(meter.snapshot().downloaded_bytes, 0);
        server
            .send(Message::Text(r#"{"type":"SYNC_MANIFEST_RESULT"}"#.into()))
            .await
            .expect("send response");
        socket
            .next()
            .await
            .expect("response frame")
            .expect("read response");
        assert_eq!(meter.state().active, 0);
        let before_ping = meter.snapshot();
        for frame in [
            r#"{"type":"SYNC_LOG_EVENT"}"#,
            r#"{"type":"DESKTOP_PHASE_COMPLETE"}"#,
            r#"{"type":"SYNC_MANIFEST_RESULT"}"#,
        ] {
            server
                .send(Message::Text(frame.into()))
                .await
                .expect("send unowned notification");
            socket
                .next()
                .await
                .expect("notification frame")
                .expect("read notification");
            assert_eq!(meter.snapshot(), before_ping);
        }
        socket
            .send(Message::Ping(vec![1, 2].into()))
            .await
            .expect("send heartbeat");
        assert_eq!(meter.snapshot(), before_ping);
        socket
            .send(Message::Text(r#"{"type":"PHASE_START"}"#.into()))
            .await
            .expect("send notification");
        assert_eq!(meter.state().active, 0);
        socket.send(Message::Text(r#"{"type":"PHASE_COMPLETED","sessionId":1,"attemptId":2,"phase":"messages","nonce":"current"}"#.into())).await.expect("send completion");
        assert_eq!(meter.state().active, 1);
        server.send(Message::Text(r#"{"type":"PHASE_ACK","sessionId":1,"attemptId":1,"phase":"messages","nonce":"current"}"#.into())).await.expect("send stale ack");
        socket.next().await.expect("ack frame").expect("read ack");
        assert_eq!(meter.state().active, 1);
        server.send(Message::Text(r#"{"type":"PHASE_ACK","sessionId":1,"attemptId":2,"phase":"messages","nonce":"current"}"#.into())).await.expect("send matching ack");
        socket
            .next()
            .await
            .expect("matching ack frame")
            .expect("read matching ack");
        assert_eq!(meter.state().active, 0);
        socket
            .send(Message::Text(request.into()))
            .await
            .expect("send another request");
        assert_eq!(meter.state().active, 1);
        socket.clear_pending_transfer();
        assert_eq!(meter.state().active, 0);
    }
}
