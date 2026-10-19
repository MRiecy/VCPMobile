//! Shared, cancellable JEV transport. It does not own group queues or messages.

use super::jev_protocol::{JevDecisionRequest, JevDecisionResponse};
use crate::vcp_modules::infra::settings_manager::types::{JevConfig, JevProvider};
use bytes::Bytes;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER};
use reqwest::{Client, Url};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

// Decision responses are small numeric maps. Bound decompressed response and
// serialized request memory independently of the group's message count window.
const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_RETRY_DELAY: Duration = Duration::from_secs(300);
const MAX_KEY_POOLS: usize = 16;

#[derive(Debug)]
pub struct JevClientError {
    pub code: &'static str,
    pub status: Option<u16>,
    pub retryable: bool,
    message: String,
}

impl JevClientError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            status: None,
            retryable: false,
            message: message.into(),
        }
    }

    fn cancelled() -> Self {
        Self::new("JEV_CANCELLED", "Jev 仲裁已取消。")
    }

    pub fn is_cancelled(&self) -> bool {
        self.code == "JEV_CANCELLED"
    }
}

impl std::fmt::Display for JevClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for JevClientError {}

struct CachedTransport {
    proxy_url: String,
    client: Client,
}

struct KeyPoolCursor {
    // Never retain or log the Key text in the process-wide rotation registry.
    fingerprint: [u8; 32],
    next: usize,
}

#[derive(Default)]
pub struct JevClient {
    transport: Mutex<Option<CachedTransport>>,
    key_pools: Mutex<VecDeque<KeyPoolCursor>>,
}

static CLIENT: OnceLock<JevClient> = OnceLock::new();

pub async fn decide(
    config: &JevConfig,
    request: &JevDecisionRequest,
    cancellation: CancellationToken,
) -> Result<JevDecisionResponse, JevClientError> {
    CLIENT
        .get_or_init(JevClient::default)
        .decide(config, request, cancellation)
        .await
}

struct AttemptFailure {
    error: JevClientError,
    retry_after: Option<Duration>,
}

impl From<JevClientError> for AttemptFailure {
    fn from(error: JevClientError) -> Self {
        Self {
            error,
            retry_after: None,
        }
    }
}

impl JevClient {
    fn transport(&self, proxy_url: &str) -> Result<Client, JevClientError> {
        let mut cached = self
            .transport
            .lock()
            .map_err(|_| JevClientError::new("JEV_INTERNAL_ERROR", "Jev 网络状态不可用。"))?;
        if let Some(cached) = cached
            .as_ref()
            .filter(|cached| cached.proxy_url == proxy_url)
        {
            return Ok(cached.client.clone());
        }
        // An explicit configurable proxy is a client-level policy, so this
        // bounded subsystem cache cannot use the fixed AdminApi profile. In-
        // flight clones keep their existing pool when the proxy setting changes.
        let mut builder = Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(10))
            .pool_idle_timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none());
        if !proxy_url.is_empty() {
            let parsed = Url::parse(proxy_url).map_err(|_| {
                JevClientError::new(
                    "JEV_INVALID_PROXY_URL",
                    "Jev 代理地址必须是完整的 HTTP(S) URL。",
                )
            })?;
            if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
                return Err(JevClientError::new(
                    "JEV_INVALID_PROXY_URL",
                    "Jev 代理地址必须是完整的 HTTP(S) URL。",
                ));
            }
            builder = builder.proxy(reqwest::Proxy::all(proxy_url).map_err(|_| {
                JevClientError::new("JEV_INVALID_PROXY_URL", "无法建立 Jev 代理连接配置。")
            })?);
        }
        let client = builder
            .build()
            .map_err(|_| JevClientError::new("JEV_CLIENT_ERROR", "无法初始化 Jev 网络客户端。"))?;
        *cached = Some(CachedTransport {
            proxy_url: proxy_url.to_owned(),
            client: client.clone(),
        });
        Ok(client)
    }

    fn select_key<'a>(&self, keys: &[&'a str]) -> Result<&'a str, JevClientError> {
        let mut hash = Sha256::new();
        for key in keys {
            hash.update(key.len().to_be_bytes());
            hash.update(key.as_bytes());
        }
        let fingerprint: [u8; 32] = hash.finalize().into();
        let mut pools = self
            .key_pools
            .lock()
            .map_err(|_| JevClientError::new("JEV_INTERNAL_ERROR", "Jev 密钥轮询状态不可用。"))?;
        if let Some(pool) = pools
            .iter_mut()
            .find(|pool| pool.fingerprint == fingerprint)
        {
            let index = pool.next % keys.len();
            pool.next = (index + 1) % keys.len();
            return Ok(keys[index]);
        }
        if pools.len() == MAX_KEY_POOLS {
            pools.pop_front();
        }
        pools.push_back(KeyPoolCursor {
            fingerprint,
            next: 1 % keys.len(),
        });
        Ok(keys[0])
    }

    pub async fn decide(
        &self,
        config: &JevConfig,
        request: &JevDecisionRequest,
        cancellation: CancellationToken,
    ) -> Result<JevDecisionResponse, JevClientError> {
        if cancellation.is_cancelled() {
            return Err(JevClientError::cancelled());
        }
        if !config.enabled {
            return Err(JevClientError::new(
                "JEV_DISABLED",
                "请先在设置中启用 Jev 服务。",
            ));
        }
        request
            .validate()
            .map_err(|message| JevClientError::new("JEV_INVALID_REQUEST", message))?;
        if !(1000..=300000).contains(&config.timeout_ms)
            || config.max_retries > 10
            || !(1..=30000).contains(&config.retry_base_delay_ms)
        {
            return Err(JevClientError::new(
                "JEV_INVALID_CONFIG",
                "Jev 超时或重试设置超出有效范围。",
            ));
        }
        let provider = config.active_provider();
        let url = endpoint(&provider.api_url)?;
        let model = provider.model.trim();
        let keys: Vec<_> = provider
            .api_key
            .split([',', '，', '|'])
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .collect();
        if model.is_empty() || keys.is_empty() {
            return Err(JevClientError::new(
                "JEV_NOT_CONFIGURED",
                "请配置当前 Jev 服务的 API Key 和模型。",
            ));
        }
        let body = Bytes::from(encode_body(model, request)?);
        let client = self.transport(config.proxy_url.trim())?;
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if config.provider == JevProvider::Openrouter {
            for (name, value) in [
                ("http-referer", config.http_referer.trim()),
                ("x-openrouter-title", config.app_title.trim()),
            ] {
                if !value.is_empty() {
                    headers.insert(
                        name,
                        HeaderValue::from_str(value).map_err(|_| {
                            JevClientError::new(
                                "JEV_INVALID_CONFIG",
                                "Jev 应用标识包含无效的 HTTP header 字符。",
                            )
                        })?,
                    );
                }
            }
        }
        // Select exactly once per logical decision; all its retries use this Key.
        let key = self.select_key(&keys)?;
        let mut authorization = HeaderValue::from_str(&format!("Bearer {key}"))
            .map_err(|_| JevClientError::new("JEV_INVALID_CONFIG", "Jev API Key 包含无效字符。"))?;
        authorization.set_sensitive(true);
        headers.insert(AUTHORIZATION, authorization);
        let timeout = Duration::from_millis(u64::from(config.timeout_ms));

        for attempt in 0..=config.max_retries {
            let result = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(JevClientError::cancelled()),
                result = tokio::time::timeout(timeout, perform_attempt(&client, &url, &headers, &body, request)) => {
                    result.unwrap_or_else(|_| Err(AttemptFailure::from(JevClientError {
                        code: "JEV_TIMEOUT", status: None, retryable: true,
                        message: "Jev 请求超时。".into(),
                    })))
                }
            };
            match result {
                Ok(response) => return Ok(response),
                Err(failure) => {
                    if !failure.error.retryable || attempt == config.max_retries {
                        return Err(failure.error);
                    }
                    let backoff_ms = u64::from(config.retry_base_delay_ms)
                        .saturating_mul(1u64 << attempt)
                        .min(30000);
                    let delay = failure
                        .retry_after
                        .unwrap_or(Duration::from_millis(backoff_ms));
                    // Do not retry earlier than the server requests. A very long
                    // Retry-After ends this finite attempt instead of parking a
                    // mobile group session indefinitely.
                    if delay > MAX_RETRY_DELAY {
                        return Err(failure.error);
                    }
                    tokio::select! {
                        biased;
                        _ = cancellation.cancelled() => return Err(JevClientError::cancelled()),
                        _ = tokio::time::sleep(delay) => {}
                    }
                }
            }
        }
        Err(JevClientError::new(
            "JEV_INTERNAL_ERROR",
            "Jev 请求未取得结果。",
        ))
    }
}

fn endpoint(value: &str) -> Result<Url, JevClientError> {
    let error = || {
        JevClientError::new(
            "JEV_INVALID_URL",
            "Jev API 地址必须是完整的 HTTP(S) URL，不能内嵌用户名、密码或 fragment。",
        )
    };
    let url = Url::parse(value.trim()).map_err(|_| error())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(error());
    }
    Ok(url)
}

fn encode_body(model: &str, request: &JevDecisionRequest) -> Result<Vec<u8>, JevClientError> {
    #[derive(Serialize)]
    struct Body<'a> {
        model: &'a str,
        #[serde(flatten)]
        request: &'a JevDecisionRequest,
    }
    struct BoundedBody(Vec<u8>);
    impl Write for BoundedBody {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_REQUEST_BYTES.saturating_sub(self.0.len()) {
                return Err(std::io::Error::other("JEV request body too large"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut body = BoundedBody(Vec::new());
    serde_json::to_writer(&mut body, &Body { model, request }).map_err(|_| {
        JevClientError::new(
            "JEV_INVALID_REQUEST",
            "Jev 请求无法编码或超过 4 MiB，请缩小仲裁上下文。",
        )
    })?;
    Ok(body.0)
}

fn network_error(error: reqwest::Error) -> AttemptFailure {
    // reqwest's Display can include credentials in a URL. Neither URLs, provider
    // response bodies, nor header values enter a returned/loggable error.
    let timeout = error.is_timeout();
    AttemptFailure::from(JevClientError {
        code: if timeout {
            "JEV_TIMEOUT"
        } else {
            "JEV_NETWORK_ERROR"
        },
        status: error.status().map(|status| status.as_u16()),
        retryable: !error.is_builder(),
        message: if timeout {
            "Jev 请求超时。"
        } else {
            "Jev 网络请求失败，请检查连接或代理设置。"
        }
        .into(),
    })
}

fn retry_after(value: Option<&HeaderValue>) -> Option<Duration> {
    let value = value?.to_str().ok()?.trim();
    let seconds = value
        .parse::<f64>()
        .ok()
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .or_else(|| {
            chrono::DateTime::parse_from_rfc2822(value)
                .ok()
                .map(|date| {
                    (date.with_timezone(&chrono::Utc) - chrono::Utc::now())
                        .num_milliseconds()
                        .max(0) as f64
                        / 1000.0
                })
        })?;
    Some(Duration::from_secs_f64(
        seconds.min(MAX_RETRY_DELAY.as_secs_f64() + 1.0),
    ))
}

async fn perform_attempt(
    client: &Client,
    url: &Url,
    headers: &HeaderMap,
    body: &Bytes,
    request: &JevDecisionRequest,
) -> Result<JevDecisionResponse, AttemptFailure> {
    let mut response = client
        .post(url.clone())
        .headers(headers.clone())
        .body(body.clone())
        .send()
        .await
        .map_err(network_error)?;
    let status = response.status();
    if !status.is_success() {
        return Err(AttemptFailure {
            error: JevClientError {
                code: "JEV_HTTP_ERROR",
                status: Some(status.as_u16()),
                retryable: matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504 | 529),
                message: format!("Jev 请求失败（HTTP {}）。", status.as_u16()),
            },
            retry_after: retry_after(response.headers().get(RETRY_AFTER)),
        });
    }
    let oversize = || {
        AttemptFailure::from(JevClientError::new(
            "JEV_INVALID_RESPONSE",
            "Jev 响应超过 1 MiB。",
        ))
    };
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(oversize());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network_error)? {
        if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
            return Err(oversize());
        }
        bytes.extend_from_slice(&chunk);
    }
    let response: JevDecisionResponse = serde_json::from_slice(&bytes).map_err(|_| {
        JevClientError::new("JEV_INVALID_RESPONSE", "Jev 响应不是有效的决策 JSON。")
    })?;
    response
        .validate_for(request)
        .map_err(|message| JevClientError::new("JEV_INVALID_RESPONSE", message))?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::super::jev_protocol::JevQuestion;
    use super::*;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    struct Reply {
        status: StatusCode,
        body: Value,
        retry_after: Option<&'static str>,
        delay: Duration,
    }

    impl Reply {
        fn ok() -> Self {
            Self {
                status: StatusCode::OK,
                body: json!({"id":"fixture-response", "answers":{
                    "speaker_routing":{"probabilities":{"agent_0":0.8,"end_conversation":0.2}},
                    "continue_discussion":{"noul":0.9}
                }}),
                retry_after: None,
                delay: Duration::ZERO,
            }
        }
    }

    #[derive(Default)]
    struct FixtureState {
        replies: Mutex<VecDeque<Reply>>,
        requests: Mutex<Vec<(HeaderMap, Value)>>,
        arrived: tokio::sync::Notify,
    }

    struct Fixture {
        url: String,
        state: Arc<FixtureState>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn handler(
        State(state): State<Arc<FixtureState>>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> impl IntoResponse {
        state.requests.lock().unwrap().push((headers, body));
        let reply = state.replies.lock().unwrap().pop_front().unwrap();
        state.arrived.notify_one();
        tokio::time::sleep(reply.delay).await;
        let mut headers = HeaderMap::new();
        if let Some(value) = reply.retry_after {
            headers.insert(RETRY_AFTER, HeaderValue::from_static(value));
        }
        (reply.status, headers, Json(reply.body))
    }

    async fn fixture(replies: Vec<Reply>) -> Fixture {
        let state = Arc::new(FixtureState::default());
        state.replies.lock().unwrap().extend(replies);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/alpha/decisions", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/alpha/decisions", post(handler))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Fixture { url, state, task }
    }

    fn config(url: &str) -> JevConfig {
        let mut config = JevConfig::default();
        config.enabled = true;
        config.typesafe.api_url = url.into();
        config.typesafe.api_key = "fixture-first， fixture-second|".into();
        config.typesafe.model = "fixture-model".into();
        config.retry_base_delay_ms = 1;
        config
    }

    fn request() -> JevDecisionRequest {
        JevDecisionRequest {
            state: json!({"group":{"id":"fixture"}}),
            questions: BTreeMap::from([
                (
                    "speaker_routing".into(),
                    JevQuestion::Choice {
                        instructions: "选择发言者".into(),
                        criteria: BTreeMap::from([
                            ("agent_0".into(), "成员".into()),
                            ("end_conversation".into(), "结束".into()),
                        ]),
                    },
                ),
                (
                    "continue_discussion".into(),
                    JevQuestion::Noul {
                        instructions: "继续？".into(),
                    },
                ),
            ]),
        }
    }

    #[tokio::test]
    async fn exact_endpoint_and_body_retry_same_key_then_rotate_next_decision() {
        let mut busy = Reply::ok();
        busy.status = StatusCode::TOO_MANY_REQUESTS;
        busy.retry_after = Some("0");
        let fixture = fixture(vec![busy, Reply::ok(), Reply::ok()]).await;
        let config = config(&fixture.url);
        let client = JevClient::default();
        client
            .decide(&config, &request(), CancellationToken::new())
            .await
            .unwrap();
        client
            .decide(&config, &request(), CancellationToken::new())
            .await
            .unwrap();
        let requests = fixture.state.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].0[AUTHORIZATION], "Bearer fixture-first");
        assert_eq!(requests[1].0[AUTHORIZATION], "Bearer fixture-first");
        assert_eq!(requests[2].0[AUTHORIZATION], "Bearer fixture-second");
        assert_eq!(requests[0].1["model"], "fixture-model");
        assert_eq!(requests[0].1["state"], request().state);
        assert_eq!(requests[0].1.as_object().unwrap().len(), 3);
        assert_eq!(
            requests[0].1["questions"]["speaker_routing"]["type"],
            "choice"
        );
        assert!(!requests[0].0.contains_key("x-openrouter-title"));
    }

    #[tokio::test]
    async fn malformed_success_and_auth_failure_do_not_retry_or_expose_response_body() {
        for (status, body, code) in [
            (
                StatusCode::OK,
                json!({"answers":{}}),
                "JEV_INVALID_RESPONSE",
            ),
            (
                StatusCode::UNAUTHORIZED,
                json!({"error":"fixture-first secret echoed by upstream"}),
                "JEV_HTTP_ERROR",
            ),
        ] {
            let mut reply = Reply::ok();
            reply.status = status;
            reply.body = body;
            let fixture = fixture(vec![reply]).await;
            let error = JevClient::default()
                .decide(&config(&fixture.url), &request(), CancellationToken::new())
                .await
                .unwrap_err();
            assert_eq!(error.code, code);
            assert_eq!(fixture.state.requests.lock().unwrap().len(), 1);
            assert!(!format!("{error:?} {error}").contains("fixture-first"));
        }
    }

    #[tokio::test]
    async fn cancellation_interrupts_http_wait_and_retry_after_without_another_request() {
        for waiting_in_retry in [false, true] {
            let mut reply = Reply::ok();
            if waiting_in_retry {
                reply.status = StatusCode::SERVICE_UNAVAILABLE;
                reply.retry_after = Some("300");
            } else {
                reply.delay = Duration::from_secs(60);
            }
            let fixture = fixture(vec![reply]).await;
            let config = config(&fixture.url);
            let cancellation = CancellationToken::new();
            let worker_token = cancellation.clone();
            let task = tokio::spawn(async move {
                JevClient::default()
                    .decide(&config, &request(), worker_token)
                    .await
            });
            tokio::time::timeout(Duration::from_secs(2), fixture.state.arrived.notified())
                .await
                .unwrap();
            // Let the local 503 reach the client before cancelling its long wait.
            if waiting_in_retry {
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
            cancellation.cancel();
            let error = tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err();
            assert!(error.is_cancelled());
            assert_eq!(fixture.state.requests.lock().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn openrouter_metadata_is_sent_and_retries_remain_finite() {
        let replies = (0..3)
            .map(|_| {
                let mut reply = Reply::ok();
                reply.status = StatusCode::BAD_GATEWAY;
                reply
            })
            .collect();
        let fixture = fixture(replies).await;
        let mut config = config(&fixture.url);
        config.provider = JevProvider::Openrouter;
        config.openrouter = config.typesafe.clone();
        config.http_referer = "https://example.invalid/vcpmobile".into();
        config.app_title = "VCPMobile fixture".into();
        config.max_retries = 2;
        let error = JevClient::default()
            .decide(&config, &request(), CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.status, Some(502));
        let requests = fixture.state.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(
            requests[0].0["http-referer"],
            "https://example.invalid/vcpmobile"
        );
        assert_eq!(requests[0].0["x-openrouter-title"], "VCPMobile fixture");
    }

    #[tokio::test]
    async fn request_timeout_bounds_a_stalled_server_without_retrying_when_disabled() {
        let mut reply = Reply::ok();
        reply.delay = Duration::from_secs(60);
        let fixture = fixture(vec![reply]).await;
        let mut config = config(&fixture.url);
        config.timeout_ms = 1000;
        config.max_retries = 0;
        let error = tokio::time::timeout(
            Duration::from_secs(3),
            JevClient::default().decide(&config, &request(), CancellationToken::new()),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(error.code, "JEV_TIMEOUT");
        assert_eq!(fixture.state.requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn oversized_response_is_a_terminal_protocol_failure() {
        let mut reply = Reply::ok();
        reply.body["extra"] = json!("x".repeat(MAX_RESPONSE_BYTES));
        let fixture = fixture(vec![reply]).await;
        let error = JevClient::default()
            .decide(&config(&fixture.url), &request(), CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.code, "JEV_INVALID_RESPONSE");
        assert_eq!(fixture.state.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn request_encoding_is_bounded_and_endpoint_is_not_a_chat_path_template() {
        let mut request = request();
        request.state = json!("x".repeat(MAX_REQUEST_BYTES));
        assert_eq!(
            encode_body("model", &request).unwrap_err().code,
            "JEV_INVALID_REQUEST"
        );
        assert!(endpoint("/alpha/decisions").is_err());
        assert_eq!(
            endpoint("https://example.invalid/custom/decision?version=1")
                .unwrap()
                .as_str(),
            "https://example.invalid/custom/decision?version=1"
        );
    }
}
