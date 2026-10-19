use super::{
    samples::PreparedCloneSample, sse::SseDecoder, wav, InputAudio, VoiceServiceConfig, VoiceTtsMode,
};
use crate::vcp_modules::infra::http_clients::{client, HttpProfile};
use crate::vcp_modules::settings_manager::VOICE_GUIDANCE_MAX_CHARS;
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::{future::Future, time::Duration};
use tokio_util::sync::CancellationToken;

pub(super) const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_JSON_BYTES: usize = 8 * 1024 * 1024;
const MAX_STREAM_BYTES: usize = 32 * 1024 * 1024;
const MAX_PCM_BYTES: usize = 8 * 1024 * 1024;
pub(super) const PCM_PACKET_BYTES: usize = 32768;
pub(super) const CANCELLED: &str = "voice_cancelled";

async fn request(
    config: &VoiceServiceConfig,
    body: &Value,
    cancel: &CancellationToken,
) -> Result<reqwest::Response, String> {
    let request = client(HttpProfile::ChatStream)
        .post(config.endpoint()?)
        .bearer_auth(&config.api_key)
        .timeout(Duration::from_secs(120))
        .json(body)
        .send();
    let response = tokio::select! {
        _ = cancel.cancelled() => return Err(CANCELLED.into()),
        result = request => result.map_err(|error| if error.is_timeout() {
            "语音服务请求超时，请重试。".to_string()
        } else { "无法连接语音服务，请检查网络和配置。".to_string() })?,
    };
    if !response.status().is_success() {
        // Do not echo provider bodies: they can contain credentials or the
        // submitted audio data. There is deliberately no automatic retry.
        return Err(format!(
            "语音服务返回 HTTP {}，请检查连接和 API 配置。",
            response.status().as_u16()
        ));
    }
    Ok(response)
}

async fn read_json(
    response: reqwest::Response,
    cancel: &CancellationToken,
) -> Result<Value, String> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => return Err(CANCELLED.into()),
            next = stream.next() => next,
        };
        let Some(chunk) = next else {
            break;
        };
        let chunk = chunk.map_err(|_| "语音服务响应传输失败。")?;
        if body.len().saturating_add(chunk.len()) > MAX_JSON_BYTES {
            return Err("语音服务响应过大。".into());
        }
        body.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&body).map_err(|_| "语音服务返回了无效 JSON。")?;
    check_provider_error(&value)?;
    Ok(value)
}

fn check_provider_error(value: &Value) -> Result<(), String> {
    if value.get("error").is_some_and(|error| !error.is_null()) {
        Err("语音服务报告处理失败，请检查配置后重试。".into())
    } else {
        Ok(())
    }
}

async fn read_events<F, Fut>(
    response: reqwest::Response,
    cancel: &CancellationToken,
    mut consume: F,
) -> Result<(), String>
where
    F: FnMut(Value) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    let mut stream = response.bytes_stream();
    let mut decoder = SseDecoder::default();
    let mut total = 0usize;
    let mut complete = false;
    loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => return Err(CANCELLED.into()),
            next = stream.next() => next,
        };
        let Some(chunk) = next else {
            break;
        };
        let chunk = chunk.map_err(|_| "语音服务流式响应中断。")?;
        total = total.saturating_add(chunk.len());
        if total > MAX_STREAM_BYTES {
            return Err("语音服务流式响应超过大小限制。".into());
        }
        for data in decoder.push(&chunk)? {
            if cancel.is_cancelled() {
                return Err(CANCELLED.into());
            }
            if data.trim() == "[DONE]" {
                return Ok(());
            }
            if data.trim().is_empty() {
                continue;
            }
            let value: Value =
                serde_json::from_str(&data).map_err(|_| "语音服务返回了无效流式 JSON。")?;
            check_provider_error(&value)?;
            complete |= value
                .pointer("/choices/0/finish_reason")
                .is_some_and(|reason| !reason.is_null());
            consume(value).await?;
        }
    }
    decoder.finish()?;
    if complete {
        Ok(())
    } else {
        Err("语音服务在完成响应前断开连接。".into())
    }
}

pub(super) async fn recognize<F: FnMut(&str)>(
    config: &VoiceServiceConfig,
    audio: &InputAudio,
    cancel: &CancellationToken,
    mut partial: F,
) -> Result<String, String> {
    let mime = match audio.encoding {
        super::VoiceInputEncoding::Wav => "audio/wav",
        super::VoiceInputEncoding::Mp3 => "audio/mpeg",
    };
    let body = json!({
        "model": "mimo-v2.5-asr", "stream": config.asr_stream,
        "messages": [{ "role": "user", "content": [{ "type": "input_audio", "input_audio": {
            "data": format!("data:{mime};base64,{}", STANDARD.encode(&audio.bytes))
        }}]}],
        "asr_options": { "language": config.language },
    });
    let response = request(config, &body, cancel).await?;
    if !config.asr_stream {
        let value = read_json(response, cancel).await?;
        let text = value
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .ok_or_else(|| "语音识别服务未返回转写文本。".to_string())?;
        if text.len() > MAX_TEXT_BYTES {
            return Err("语音识别结果过长。".into());
        }
        return Ok(text.to_string());
    }
    let mut text = String::new();
    read_events(response, cancel, |value| {
        let result = if let Some(delta) = value
            .pointer("/choices/0/delta/content")
            .and_then(Value::as_str)
        {
            if text.len().saturating_add(delta.len()) > MAX_TEXT_BYTES {
                Err("语音识别结果过长。".into())
            } else {
                text.push_str(delta);
                partial(&text);
                Ok(())
            }
        } else {
            Ok(())
        };
        std::future::ready(result)
    })
    .await?;
    Ok(text)
}

#[derive(Default)]
struct PcmPackets {
    trailing: Option<u8>,
    total: usize,
}

impl PcmPackets {
    fn push(&mut self, audio: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        self.total = self.total.saturating_add(audio.len());
        if self.total > MAX_PCM_BYTES {
            return Err("单段语音合成结果过大。".into());
        }
        let mut bytes = Vec::with_capacity(audio.len() + 1);
        if let Some(byte) = self.trailing.take() {
            bytes.push(byte);
        }
        bytes.extend_from_slice(audio);
        if bytes.len() % 2 == 1 {
            self.trailing = bytes.pop();
        }
        Ok(bytes.chunks(PCM_PACKET_BYTES).map(<[u8]>::to_vec).collect())
    }
    fn finish(&self) -> Result<(), String> {
        if self.trailing.is_some() {
            Err("语音合成返回了不完整的 PCM 样本。".into())
        } else if self.total == 0 {
            Err("语音合成未返回音频。".into())
        } else {
            Ok(())
        }
    }
}

fn synthesis_request(
    config: &VoiceServiceConfig,
    text: &str,
    guidance: Option<&str>,
    sample: Option<&PreparedCloneSample>,
) -> Result<Value, String> {
    let stream = config.tts_streaming();
    let mut audio = json!({"format": if stream { "pcm16" } else { "wav" }});
    let model = match config.tts_mode {
        VoiceTtsMode::Builtin => {
            audio["voice"] = json!(config.voice);
            "mimo-v2.5-tts"
        }
        VoiceTtsMode::Clone => {
            let sample = sample.ok_or("音色复刻参考音频未准备完成。")?;
            audio["voice"] = json!(sample.data_url.as_ref());
            "mimo-v2.5-tts-voiceclone"
        }
    };
    let prompt = guidance
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&config.tts_style_prompt)
        .trim();
    if text.len() > MAX_TEXT_BYTES || prompt.chars().count() > VOICE_GUIDANCE_MAX_CHARS {
        return Err("单段语音正文或指导过长。".into());
    }
    let mut messages = Vec::with_capacity(2);
    if !prompt.is_empty() {
        messages.push(json!({"role": "user", "content": prompt}));
    }
    messages.push(json!({"role": "assistant", "content": text}));
    Ok(json!({ "model": model, "stream": stream, "messages": messages, "audio": audio }))
}

pub(super) async fn synthesize<F, Fut>(
    config: &VoiceServiceConfig,
    text: &str,
    guidance: Option<&str>,
    sample: Option<&PreparedCloneSample>,
    cancel: &CancellationToken,
    mut output: F,
) -> Result<(), String>
where
    F: FnMut(Vec<u8>) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    if cancel.is_cancelled() {
        return Err(CANCELLED.into());
    }
    let body = synthesis_request(config, text, guidance, sample)?;
    let response = request(config, &body, cancel).await?;
    drop(body);
    let mut packets = PcmPackets::default();
    if !config.tts_streaming() {
        let value = read_json(response, cancel).await?;
        let encoded = value
            .pointer("/choices/0/message/audio/data")
            .and_then(Value::as_str)
            .ok_or_else(|| "语音合成未返回音频。".to_string())?;
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|_| "语音合成返回了无效的音频编码。")?;
        let pcm = wav::pcm16_mono(&bytes, 24000)?;
        for packet in packets.push(&pcm)? {
            output(packet).await?;
        }
    } else {
        // Events are bounded individually; each audio.data is an independent
        // base64 payload. PCM sample alignment is retained across those events.
        read_events(response, cancel, |value| {
            let chunks = value
                .pointer("/choices/0/delta/audio/data")
                .and_then(Value::as_str)
                .map(|encoded| {
                    STANDARD
                        .decode(encoded)
                        .map_err(|_| "语音合成返回了无效的音频编码。".to_string())
                })
                .transpose()
                .and_then(|audio| {
                    audio.map_or_else(|| Ok(Vec::new()), |audio| packets.push(&audio))
                });
            // Consume one bounded event before reading the next network chunk.
            let outputs =
                chunks.map(|chunks| chunks.into_iter().map(&mut output).collect::<Vec<_>>());
            async move {
                for result in outputs? {
                    result.await?;
                }
                Ok(())
            }
        })
        .await?;
    }
    packets.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::task::JoinHandle;
    use tokio::time::timeout;

    const FAKE_KEY: &str = "fixture-only-mimo-key";
    const PROVIDER_SECRET: &str = "provider-echo-must-stay-private";

    struct CapturedRequest {
        headers: String,
        body: Value,
    }

    struct HttpFixture {
        config: VoiceServiceConfig,
        server: JoinHandle<CapturedRequest>,
    }

    async fn capture_request(socket: &mut TcpStream) -> CapturedRequest {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 1024];
        let header_end = loop {
            if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                break index + 4;
            }
            let read = timeout(Duration::from_secs(3), socket.read(&mut buffer))
                .await
                .expect("request headers arrive")
                .expect("read request headers");
            assert_ne!(read, 0, "request ended before its headers");
            bytes.extend_from_slice(&buffer[..read]);
            assert!(
                bytes.len() <= 64 * 1024,
                "fixture request exceeds its bound"
            );
        };
        let headers =
            String::from_utf8(bytes[..header_end].to_vec()).expect("HTTP headers are UTF-8");
        let length = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .map(|(_, value)| value.trim().parse::<usize>().expect("valid Content-Length"))
            .expect("JSON request declares its length");
        assert!(length <= 64 * 1024);
        while bytes.len() - header_end < length {
            let read = timeout(Duration::from_secs(3), socket.read(&mut buffer))
                .await
                .expect("request body arrives")
                .expect("read request body");
            assert_ne!(read, 0, "request ended before its JSON body");
            bytes.extend_from_slice(&buffer[..read]);
        }
        let body = serde_json::from_slice(&bytes[header_end..header_end + length])
            .expect("request body is JSON");
        CapturedRequest { headers, body }
    }

    /// A real loopback HTTP server. It never consults the application's stored
    /// configuration or contacts a provider. In stalled mode only cancellation
    /// can finish reading: the server deliberately withholds HTTP/SSE completion.
    async fn http_fixture(
        status: u16,
        content_type: &'static str,
        fragments: Vec<Vec<u8>>,
        stalled: bool,
    ) -> HttpFixture {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback fixture");
        let address = listener.local_addr().expect("fixture address");
        let config = VoiceServiceConfig {
            base_url: format!("http://{address}/v1/"),
            api_key: FAKE_KEY.into(),
            asr_stream: true,
            input_encoding: Default::default(),
            tts_stream: true,
            language: "zh".into(),
            voice: "白桦".into(),
            tts_mode: VoiceTtsMode::Builtin,
            tts_style_prompt: String::new(),
            clone_sample_id: String::new(),
        };
        let server = tokio::spawn(async move {
            let (mut socket, _) = timeout(Duration::from_secs(3), listener.accept())
                .await
                .expect("one client connects")
                .expect("accept fixture request");
            socket
                .set_nodelay(true)
                .expect("disable Nagle for split payloads");
            let captured = capture_request(&mut socket).await;
            let headers = format!(
                "HTTP/1.1 {status} Fixture\r\nContent-Type: {content_type}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
            );
            socket
                .write_all(headers.as_bytes())
                .await
                .expect("write response headers");
            for fragment in fragments {
                assert!(!fragment.is_empty());
                // HTTP chunk boundaries deliberately split UTF-8 and base64 JSON.
                let mut wire = format!("{:x}\r\n", fragment.len()).into_bytes();
                wire.extend_from_slice(&fragment);
                wire.extend_from_slice(b"\r\n");
                if socket.write_all(&wire).await.is_err() {
                    // A 401 or SSE error may legitimately make the client close
                    // before the provider's final chunk has been written.
                    break;
                }
                tokio::task::yield_now().await;
            }
            if stalled {
                let mut byte = [0u8; 1];
                let closed = timeout(Duration::from_secs(3), socket.read(&mut byte))
                    .await
                    .expect("cancellation closes the unfinished response");
                assert!(
                    matches!(closed, Ok(0) | Err(_)),
                    "client unexpectedly wrote another request"
                );
            } else {
                let _ = socket.write_all(b"0\r\n\r\n").await;
                let _ = socket.shutdown().await;
            }
            drop(socket);
            assert!(
                timeout(Duration::from_millis(120), listener.accept())
                    .await
                    .is_err(),
                "voice network code retried the provider request"
            );
            captured
        });
        HttpFixture { config, server }
    }

    fn json_fragments(body: Value) -> Vec<Vec<u8>> {
        serde_json::to_vec(&body)
            .expect("fixture JSON")
            .chunks(7)
            .map(<[u8]>::to_vec)
            .collect()
    }

    fn sse(value: Value) -> Vec<u8> {
        format!("data: {value}\r\n\r\n").into_bytes()
    }

    fn wav_with_padding(sample_rate: u32, pcm: &[u8]) -> Vec<u8> {
        assert_eq!(pcm.len() % 2, 0);
        let mut bytes = b"RIFF\0\0\0\0WAVEJUNK\x03\0\0\0odd\0fmt \x10\0\0\0".to_vec();
        bytes.extend_from_slice(&[1, 0, 1, 0]);
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        bytes.extend_from_slice(&[2, 0, 16, 0]);
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
        bytes.extend_from_slice(pcm);
        let size = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&size.to_le_bytes());
        bytes
    }

    fn assert_request_headers(request: &CapturedRequest) {
        assert_eq!(
            request.headers.lines().next(),
            Some("POST /v1/chat/completions HTTP/1.1")
        );
        let headers = request.headers.to_ascii_lowercase();
        assert!(headers.contains(&format!("authorization: bearer {FAKE_KEY}")));
        assert!(headers.contains("content-type: application/json"));
    }

    fn assert_asr_request(request: &CapturedRequest, streaming: bool, mime: &str, audio: &[u8]) {
        assert_request_headers(request);
        assert_eq!(request.body["model"], "mimo-v2.5-asr");
        assert_eq!(request.body["stream"], streaming);
        assert_eq!(request.body["asr_options"]["language"], "zh");
        let messages = request.body["messages"].as_array().expect("ASR messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        let parts = messages[0]["content"]
            .as_array()
            .expect("ASR audio content");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["type"], "input_audio");
        let data = parts[0]["input_audio"]["data"]
            .as_str()
            .expect("ASR Data URL");
        let encoded = data
            .strip_prefix(&format!("data:{mime};base64,"))
            .expect("actual audio Data URL prefix");
        assert_eq!(STANDARD.decode(encoded).expect("audio base64"), audio);
    }

    fn assert_tts_request(request: &CapturedRequest, streaming: bool, text: &str) {
        assert_request_headers(request);
        assert_eq!(request.body["model"], "mimo-v2.5-tts");
        assert_eq!(request.body["stream"], streaming);
        assert_eq!(
            request.body["audio"]["format"],
            if streaming { "pcm16" } else { "wav" }
        );
        assert_eq!(request.body["audio"]["voice"], "白桦");
        let messages = request.body["messages"].as_array().expect("TTS messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "assistant");
        assert_eq!(messages[0]["content"], text);
    }

    #[tokio::test]
    async fn asr_stream_request_and_split_chinese_sse_use_the_real_http_path() {
        let mut response = b": keepalive\r\n\r\n".to_vec();
        response.extend(sse(
            json!({"choices":[{"delta":{"content":"你好，"},"finish_reason":null}]}),
        ));
        response.extend(sse(
            json!({"choices":[{"delta":{"content":"世界。"},"finish_reason":null}]}),
        ));
        response.extend(sse(
            json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
        ));
        response.extend_from_slice(b"data: [DONE]\r\n\r\n");
        let HttpFixture { config, server } = http_fixture(
            200,
            "text/event-stream",
            response.chunks(1).map(<[u8]>::to_vec).collect(),
            false,
        )
        .await;
        let audio = InputAudio {
            bytes: wav_with_padding(16000, &[1, 2, 3, 4]),
            encoding: super::super::VoiceInputEncoding::Wav,
        };
        let mut partials = Vec::new();
        let text = timeout(
            Duration::from_secs(4),
            recognize(&config, &audio, &CancellationToken::new(), |part| {
                partials.push(part.to_string())
            }),
        )
        .await
        .expect("ASR finishes")
        .expect("ASR succeeds");
        assert_eq!(text, "你好，世界。");
        assert_eq!(partials, ["你好，", "你好，世界。"]);
        assert_asr_request(
            &server.await.expect("fixture completed"),
            true,
            "audio/wav",
            &audio.bytes,
        );
    }

    #[tokio::test]
    async fn asr_nonstream_request_returns_message_content_without_partial_callbacks() {
        let HttpFixture { mut config, server } = http_fixture(200, "application/json",
            json_fragments(json!({"choices":[{"message":{"role":"assistant","content":"完整识别结果。"},"finish_reason":"stop"}]})), false).await;
        config.asr_stream = false;
        let audio = InputAudio {
            bytes: wav_with_padding(16000, &[5, 6, 7, 8]),
            encoding: super::super::VoiceInputEncoding::Wav,
        };
        let mut partials = Vec::new();
        let text = timeout(
            Duration::from_secs(4),
            recognize(&config, &audio, &CancellationToken::new(), |part| {
                partials.push(part.to_string())
            }),
        )
        .await
        .expect("ASR finishes")
        .expect("ASR succeeds");
        assert_eq!(text, "完整识别结果。");
        assert!(partials.is_empty());
        assert_asr_request(
            &server.await.expect("fixture completed"),
            false,
            "audio/wav",
            &audio.bytes,
        );
    }

    #[tokio::test]
    async fn encoded_mp3_is_uploaded_with_its_actual_mime_in_both_asr_modes() {
        use super::super::{workers::prepare_audio, VoiceInputEncoding};
        let pcm: Vec<u8> = (0..16000)
            .flat_map(|sample| (((sample % 160) as i16 - 80) * 150).to_le_bytes())
            .collect();
        let cancellation = CancellationToken::new();
        let audio = prepare_audio(
            wav_with_padding(16000, &pcm),
            16000,
            VoiceInputEncoding::Mp3,
            &cancellation,
        )
        .await
        .expect("real native LAME encoding succeeds");
        assert_eq!(&audio.bytes[..2], &[0xff, 0xf3]);
        assert!(audio.bytes.len() < pcm.len() / 2);
        for streaming in [false, true] {
            let fragments = if streaming {
                let mut response = sse(
                    json!({"choices":[{"delta":{"content":"识别完成。"},"finish_reason":"stop"}]}),
                );
                response.extend_from_slice(b"data: [DONE]\n\n");
                response.chunks(7).map(<[u8]>::to_vec).collect()
            } else {
                json_fragments(json!({"choices":[{"message":{"content":"识别完成。"}}]}))
            };
            let HttpFixture { mut config, server } = http_fixture(
                200,
                if streaming {
                    "text/event-stream"
                } else {
                    "application/json"
                },
                fragments,
                false,
            )
            .await;
            config.asr_stream = streaming;
            // MIME follows the already encoded payload, even if other settings
            // describe a different input format by the time networking begins.
            config.input_encoding = VoiceInputEncoding::Wav;
            let text = timeout(
                Duration::from_secs(4),
                recognize(&config, &audio, &cancellation, |_| {}),
            )
            .await
            .expect("ASR finishes")
            .expect("ASR succeeds");
            assert_eq!(text, "识别完成。");
            assert_asr_request(
                &server.await.expect("fixture completed"),
                streaming,
                "audio/mpeg",
                &audio.bytes,
            );
        }
    }

    #[tokio::test]
    async fn tts_stream_keeps_pcm16_sample_order_across_sse_payload_and_transport_boundaries() {
        let pcm: Vec<u8> = [-32768i16, -1234, 0, 2345, 32767]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect();
        let mut response = Vec::new();
        for part in [&pcm[..1], &pcm[1..6], &pcm[6..9], &pcm[9..]] {
            response.extend(sse(json!({"choices":[{"delta":{"audio":{"data":STANDARD.encode(part)}},"finish_reason":null}]})));
        }
        response.extend_from_slice(b"data: [DONE]\n\n");
        let HttpFixture { config, server } = http_fixture(
            200,
            "text/event-stream",
            response.chunks(5).map(<[u8]>::to_vec).collect(),
            false,
        )
        .await;
        let mut output = Vec::new();
        timeout(
            Duration::from_secs(4),
            synthesize(
                &config,
                "按原顺序朗读。",
                None,
                None,
                &CancellationToken::new(),
                |part| {
                    output.push(part);
                    std::future::ready(Ok(()))
                },
            ),
        )
        .await
        .expect("TTS finishes")
        .expect("TTS succeeds");
        assert_eq!(output.concat(), pcm);
        assert!(output
            .iter()
            .all(|part| !part.is_empty() && part.len() % 2 == 0 && part.len() <= PCM_PACKET_BYTES));
        assert_tts_request(
            &server.await.expect("fixture completed"),
            true,
            "按原顺序朗读。",
        );
    }

    #[tokio::test]
    async fn tts_nonstream_decodes_24khz_riff_with_unknown_chunk_padding_into_ordered_packets() {
        let pcm: Vec<u8> = (0..20_000)
            .flat_map(|index| (index as i16).to_le_bytes())
            .collect();
        let wav = wav_with_padding(24000, &pcm);
        let HttpFixture { mut config, server } = http_fixture(
            200,
            "application/json",
            vec![serde_json::to_vec(
                &json!({"choices":[{"message":{"audio":{"data":STANDARD.encode(wav)}}}]}),
            )
            .unwrap()],
            false,
        )
        .await;
        config.tts_stream = false;
        let mut output = Vec::new();
        timeout(
            Duration::from_secs(4),
            synthesize(
                &config,
                "完整合成。",
                None,
                None,
                &CancellationToken::new(),
                |part| {
                    output.push(part);
                    std::future::ready(Ok(()))
                },
            ),
        )
        .await
        .expect("TTS finishes")
        .expect("TTS succeeds");
        assert_eq!(output.len(), 2);
        assert_eq!(output[0].len(), PCM_PACKET_BYTES);
        assert_eq!(output.concat(), pcm);
        assert_tts_request(
            &server.await.expect("fixture completed"),
            false,
            "完整合成。",
        );
    }

    #[tokio::test]
    async fn tts_nonstream_rejects_a_wav_sample_rate_other_than_the_24khz_playback_contract() {
        let wav = wav_with_padding(16000, &[1, 2, 3, 4]);
        let HttpFixture { mut config, server } = http_fixture(
            200,
            "application/json",
            json_fragments(
                json!({"choices":[{"message":{"audio":{"data":STANDARD.encode(wav)}}}]}),
            ),
            false,
        )
        .await;
        config.tts_stream = false;
        let mut output = Vec::new();
        let result = timeout(
            Duration::from_secs(4),
            synthesize(
                &config,
                "不能错误变速。",
                None,
                None,
                &CancellationToken::new(),
                |part| {
                    output.push(part);
                    std::future::ready(Ok(()))
                },
            ),
        )
        .await
        .expect("invalid TTS ends");
        assert!(result.is_err());
        assert!(output.is_empty());
        server.await.expect("fixture completed without retries");
    }

    #[tokio::test]
    async fn builtin_and_clone_submit_automatic_style_with_their_distinct_voices() {
        for mode in [VoiceTtsMode::Builtin, VoiceTtsMode::Clone] {
            let pcm = vec![1, 2, 3, 4];
            let HttpFixture { mut config, server } = http_fixture(
                200,
                "application/json",
                json_fragments(json!({"choices":[{"message":{"audio":{"data":STANDARD.encode(wav_with_padding(24000, &pcm))}}}]})),
                false,
            ).await;
            config.tts_mode = mode;
            // A saved builtin preference cannot turn clone compatibility SSE on.
            config.tts_stream = mode == VoiceTtsMode::Clone;
            config.tts_style_prompt = "轻声慢说".into();
            let sample = PreparedCloneSample {
                data_url: std::sync::Arc::from("data:audio/wav;base64,AQIDBA=="),
            };
            let mut output = Vec::new();
            synthesize(&config, "[轻笑]你好。", Some("本条自动指导"), Some(&sample),
                &CancellationToken::new(), |part| {
                    output.extend(part);
                    std::future::ready(Ok(()))
                }).await.expect("synthetic provider WAV is decoded");
            assert_eq!(output, pcm);
            let captured = server.await.expect("one request without retry");
            assert_request_headers(&captured);
            assert_eq!(captured.body["stream"], false);
            assert_eq!(captured.body["audio"]["format"], "wav");
            assert_eq!(captured.body["messages"][1], json!({"role":"assistant","content":"[轻笑]你好。"}));
            assert_eq!(captured.body["messages"][0], json!({"role":"user","content":"本条自动指导"}));
            assert!(captured.body["audio"].get("optimize_text_preview").is_none());
            if mode == VoiceTtsMode::Builtin {
                assert_eq!(captured.body["model"], "mimo-v2.5-tts");
                assert_eq!(captured.body["audio"]["voice"], config.voice);
            } else {
                assert_eq!(captured.body["model"], "mimo-v2.5-tts-voiceclone");
                assert_eq!(captured.body["audio"]["voice"], sample.data_url.as_ref());
            }
        }
    }

    #[test]
    fn automatic_style_overrides_optional_fallback_in_both_modes() {
        let sample = PreparedCloneSample {
            data_url: std::sync::Arc::from("data:audio/wav;base64,AQIDBA=="),
        };
        for mode in [VoiceTtsMode::Builtin, VoiceTtsMode::Clone] {
            let mut config = VoiceServiceConfig::from_settings(
                crate::vcp_modules::settings_manager::Settings::default(),
            );
            config.tts_mode = mode;
            config.tts_style_prompt = "轻声慢说".into();
            for guidance in [None, Some(" \n")] {
                let body = synthesis_request(&config, "正文", guidance, Some(&sample)).unwrap();
                assert_eq!(body["messages"][0]["content"], "轻声慢说");
            }
            let body = synthesis_request(&config, "正文", Some("本条指导"), Some(&sample)).unwrap();
            assert_eq!(body["messages"][0]["content"], "本条指导");
            config.tts_style_prompt.clear();
            let body = synthesis_request(&config, "正文", None, Some(&sample)).unwrap();
            assert_eq!(body["messages"], json!([{"role":"assistant", "content":"正文"}]));
            assert!(synthesis_request(&config, "正文", Some("本条指导"), Some(&sample)).is_ok());
            if mode == VoiceTtsMode::Clone {
                assert!(synthesis_request(&config, "正文", Some("本条指导"), None).is_err());
            }
        }
    }

    #[tokio::test]
    async fn http_and_provider_errors_never_echo_credentials_or_retry() {
        for (status, streaming) in [(401, false), (200, false), (200, true)] {
            let provider =
                json!({"error":{"message":format!("bad key {FAKE_KEY}; {PROVIDER_SECRET}")}});
            let fragments = if streaming {
                vec![sse(provider)]
            } else {
                json_fragments(provider)
            };
            let HttpFixture { mut config, server } = http_fixture(
                status,
                if streaming {
                    "text/event-stream"
                } else {
                    "application/json"
                },
                fragments,
                false,
            )
            .await;
            config.asr_stream = streaming;
            let error = timeout(
                Duration::from_secs(4),
                recognize(
                    &config,
                    &InputAudio {
                        bytes: vec![0, 1],
                        encoding: Default::default(),
                    },
                    &CancellationToken::new(),
                    |_| {},
                ),
            )
            .await
            .expect("error completes")
            .expect_err("provider failure is returned");
            if status == 401 {
                assert!(error.contains("401"));
            }
            assert!(!error.contains(FAKE_KEY));
            assert!(!error.contains(PROVIDER_SECRET));
            server.await.expect("one request, no retry");
        }
    }

    #[tokio::test]
    async fn cancelling_an_in_progress_stream_closes_reading_and_never_retries() {
        let HttpFixture { config, server } = http_fixture(
            200,
            "text/event-stream",
            vec![sse(
                json!({"choices":[{"delta":{"content":"已到达"},"finish_reason":null}]}),
            )],
            true,
        )
        .await;
        let cancellation = CancellationToken::new();
        let request_cancellation = cancellation.clone();
        let (progress, first_partial) = tokio::sync::oneshot::channel();
        let operation = tokio::spawn(async move {
            let mut progress = Some(progress);
            recognize(
                &config,
                &InputAudio {
                    bytes: vec![0, 1],
                    encoding: Default::default(),
                },
                &request_cancellation,
                |text| {
                    if let Some(progress) = progress.take() {
                        let _ = progress.send(text.to_string());
                    }
                },
            )
            .await
        });
        assert_eq!(
            timeout(Duration::from_secs(3), first_partial)
                .await
                .expect("read has started")
                .unwrap(),
            "已到达"
        );
        cancellation.cancel();
        let result = timeout(Duration::from_secs(2), operation)
            .await
            .expect("cancelled read settles")
            .expect("request task joins");
        assert_eq!(result.unwrap_err(), CANCELLED);
        server
            .await
            .expect("response connection closed without retries");
    }

    #[test]
    fn pcm_packets_join_odd_network_payloads_without_reordering_or_oversized_appends() {
        let mut packets = PcmPackets::default();
        assert!(packets.push(&[1]).unwrap().is_empty());
        assert_eq!(packets.push(&[2, 3, 4]).unwrap(), [vec![1, 2, 3, 4]]);
        let data = vec![9; PCM_PACKET_BYTES * 2 + 2];
        let output = packets.push(&data).unwrap();
        assert!(output
            .iter()
            .all(|chunk| chunk.len() <= PCM_PACKET_BYTES && chunk.len() % 2 == 0));
        assert_eq!(output.concat(), data);
        packets.finish().unwrap();
    }
    #[test]
    fn leftover_half_sample_is_an_error() {
        let mut packets = PcmPackets::default();
        packets.push(&[1]).unwrap();
        assert!(packets.finish().is_err());
    }
}
