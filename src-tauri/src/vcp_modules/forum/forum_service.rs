//! VCP 论坛远端服务：`/admin_api/forum/*` 的认证代理 + `/v1/human/tool` 发帖通道。
//!
//! 职责边界：认证、超时、响应大小上限、uid/字段长度校验、TOOL_REQUEST 文本协议拼装
//! （ESCAPE 定界符，对齐服务端 modules/vcpLoop/toolCallParser.js 的还原语义）。
//! 帖子 JSON 以 Value 透传（契约解释权在前端 Store 的归一化层）。
//!
//! - 浏览、回帖与 REST 发帖走 admin Basic Auth；仅 REST 发帖返回 404 时
//!   回退 `/v1/human/tool`（Bearer 主 API Key + TOOL_REQUEST 私有文本协议）；
//! - 发帖/回帖内容用 `「始ESCAPE」…「末ESCAPE」` 定界——服务端解析器只认
//!   `「末ESCAPE」` 为字段结束，plain `「末」` 在内容中安全；
//! - 内容中的字面 ESCAPE 标记会被服务端 `_restoreEscapedLiterals` 折叠还原，
//!   发送前需先把它们改写为带空格的变体保护。

use crate::vcp_modules::infra::admin_api;
use crate::vcp_modules::infra::http_clients::{client, HttpProfile};
use crate::vcp_modules::infra::vcp_http_endpoint::{append_url_segments, resolve_service_base};
use crate::vcp_modules::settings_manager::{read_config, Settings, SettingsState};
use reqwest::{Method, RequestBuilder, StatusCode};
use serde_json::Value;
use std::time::Duration;
use tauri::{AppHandle, Runtime, State};

/// 常规读/写总超时。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// GET /posts 无分页、全量读盘，给足余量。
const LIST_TIMEOUT: Duration = Duration::from_secs(60);
/// 列表响应体上限（数百帖的元数据）。
const MAX_LIST_BYTES: u64 = 8 * 1024 * 1024;
/// 详情响应体上限（后端单帖上限 2MB，加 envelope 余量）。
const MAX_DETAIL_BYTES: u64 = 3 * 1024 * 1024;

// ---- 上游约束（routes/forumApi.js:10-19 + Plugin/VCPForum/VCPForum.js） ----
const MAX_CONTENT_CHARS: usize = 50_000;
const MAX_MAID_CHARS: usize = 50;
const MAX_TITLE_CHARS: usize = 100;

fn build_url(settings: &Settings, suffix: &[&str]) -> Result<String, String> {
    Ok(admin_api::admin_url(settings, suffix)?.to_string())
}

/// uid 校验：后端限定 `[a-zA-Z0-9_-]`，≤64。
fn validate_uid(uid: &str) -> Result<&str, String> {
    let trimmed = uid.trim();
    if trimmed.is_empty()
        || trimmed.len() > 64
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("帖子 uid 格式无效".to_string());
    }
    Ok(trimmed)
}

fn validate_length(label: &str, value: &str, max_chars: usize) -> Result<(), String> {
    if value.chars().count() > max_chars {
        return Err(format!("{label}超过 {max_chars} 字符上限"));
    }
    Ok(())
}

async fn send_json(
    request: RequestBuilder,
    timeout: Duration,
    max_bytes: u64,
) -> Result<Value, String> {
    send_json_with_status(request, timeout, max_bytes)
        .await
        .map(|(_, body)| body)
}

/// 带状态码的发送（供需要按状态码回退的调用方，如发帖的 REST→human/tool 降级）。
async fn send_json_with_status(
    request: RequestBuilder,
    timeout: Duration,
    max_bytes: u64,
) -> Result<(StatusCode, Value), String> {
    let resp = request
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| format!("论坛请求失败: {e}"))?;

    let status = resp.status();
    if let Some(len) = resp.content_length() {
        if len > max_bytes {
            return Err(format!("论坛响应体异常庞大（{len} 字节），已拒绝读取"));
        }
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取论坛响应失败: {e}"))?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!(
            "论坛响应体超过上限（{} 字节），已拒绝解析",
            bytes.len()
        ));
    }

    let body: Value =
        serde_json::from_slice(&bytes).map_err(|_| "服务器返回了不符合契约的 JSON".to_string())?;

    if !status.is_success() && status != StatusCode::NOT_FOUND {
        let detail = body
            .get("error")
            .and_then(|e| e.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                "管理员凭据校验失败，请检查 设置 中的管理员账号与密码".to_string()
            }
            StatusCode::TOO_MANY_REQUESTS => {
                "请求过于频繁，已被服务器临时限制，请稍后再试".to_string()
            }
            _ => format!("论坛操作失败: {detail}"),
        });
    }
    Ok((status, body))
}

async fn settings_of<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
) -> Result<std::sync::Arc<Settings>, String> {
    let settings = read_config(app_handle, settings_state).await?;
    admin_api::ensure_admin_config(&settings, "VCP 论坛")?;
    Ok(settings)
}

/// 帖子列表（PostMeta[]，mtime 降序；无分页，全量）。
#[tauri::command]
pub async fn forum_list_posts<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let url = build_url(&settings, &["forum", "posts"])?;
    send_json(
        admin_api::client_get(&settings, &url)?,
        LIST_TIMEOUT,
        MAX_LIST_BYTES,
    )
    .await
}

/// 帖子详情（整篇原始 Markdown，含元信息头与全部楼层）。
#[tauri::command]
pub async fn forum_get_post<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    uid: String,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let uid = validate_uid(&uid)?;
    let url = build_url(&settings, &["forum", "post", uid])?;
    send_json(
        admin_api::client_get(&settings, &url)?,
        DEFAULT_TIMEOUT,
        MAX_DETAIL_BYTES,
    )
    .await
}

/// 回帖（自动追加楼层；500 楼上限由后端把关）。
#[tauri::command]
pub async fn forum_reply<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    uid: String,
    maid: String,
    content: String,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let uid = validate_uid(&uid)?;
    let maid = maid.trim();
    if maid.is_empty() {
        return Err("署名不能为空".to_string());
    }
    validate_length("署名", maid, MAX_MAID_CHARS)?;
    if content.trim().is_empty() {
        return Err("回复内容不能为空".to_string());
    }
    validate_length("回复内容", &content, MAX_CONTENT_CHARS)?;

    let url = build_url(&settings, &["forum", "reply", uid])?;
    send_json(
        admin_api::client_post_json(
            &settings,
            &url,
            &serde_json::json!({ "maid": maid, "content": content }),
        )?,
        DEFAULT_TIMEOUT,
        MAX_DETAIL_BYTES,
    )
    .await
}

/// 删除整个帖子或单个楼层（上游 `DELETE /forum/post/:uid`，body `{floor?}`；
/// floor 省略 = 删整帖。后端会重排剩余楼层号）。
#[tauri::command]
pub async fn forum_delete<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    uid: String,
    floor: Option<u32>,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let uid = validate_uid(&uid)?;
    if let Some(floor) = floor {
        if floor == 0 || floor > 500 {
            return Err("楼层号无效".to_string());
        }
    }

    let url = build_url(&settings, &["forum", "post", uid])?;
    let body = match floor {
        Some(floor) => serde_json::json!({ "floor": floor }),
        None => serde_json::json!({}),
    };
    send_json(
        admin_api::client_request(&settings, Method::DELETE, &url)?.json(&body),
        DEFAULT_TIMEOUT,
        MAX_DETAIL_BYTES,
    )
    .await
}

// ---------- 发帖：/v1/human/tool + TOOL_REQUEST 文本协议 ----------

/// ESCAPE 定界保护：内容中的字面 ESCAPE 标记会被服务端还原折叠，
/// 发送前改写为带空格变体（病态场景，正常内容不受影响）。
fn shield_escape_literals(raw: &str) -> String {
    raw.replace("「始ESCAPE」", "「始 ESCAPE」")
        .replace("「末ESCAPE」", "「末 ESCAPE」")
        .replace("「始escape」", "「始 escape」")
        .replace("「末escape」", "「末 escape」")
}

/// 以 ESCAPE 定界符拼装一个协议字段。
fn escape_field(key: &str, value: &str) -> String {
    format!(
        "{key}:「始ESCAPE」{}「末ESCAPE」",
        shield_escape_literals(value)
    )
}

/// 拼装 VCPForum CreatePost 的 TOOL_REQUEST 纯文本请求体。
fn build_create_post_body(maid: &str, board: &str, title: &str, content: &str) -> String {
    let fields = [
        escape_field("tool_name", "VCPForum"),
        escape_field("command", "CreatePost"),
        escape_field("maid", maid),
        escape_field("board", board),
        escape_field("title", title),
        escape_field("content", content),
    ];
    format!(
        "<<<[TOOL_REQUEST]>>>\n{}\n<<<[END_TOOL_REQUEST]>>>",
        fields.join(",\n")
    )
}

/// 发帖。优先走补丁新增的 REST 端点 `POST /admin_api/forum/posts`（admin Basic）；
/// 旧版服务器无此端点（404）时回退 `/v1/human/tool`（Bearer 主 API Key +
/// TOOL_REQUEST 文本协议）。
#[tauri::command]
pub async fn forum_create_post<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    maid: String,
    board: String,
    title: String,
    content: String,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    create_post(&settings, &maid, &board, &title, &content).await
}

async fn create_post(
    settings: &Settings,
    maid: &str,
    board: &str,
    title: &str,
    content: &str,
) -> Result<Value, String> {
    let maid = maid.trim();
    let board = board.trim();
    let title = title.trim();
    if maid.is_empty() || board.is_empty() || title.is_empty() {
        return Err("署名、板块、标题均不能为空".to_string());
    }
    validate_length("署名", maid, MAX_MAID_CHARS)?;
    validate_length("标题", title, MAX_TITLE_CHARS)?;
    if content.trim().is_empty() {
        return Err("正文不能为空".to_string());
    }
    validate_length("正文", content, MAX_CONTENT_CHARS)?;

    // 1) REST 优先
    let rest_url = build_url(settings, &["forum", "posts"])?;
    let (status, body) = send_json_with_status(
        admin_api::client_post_json(
            settings,
            &rest_url,
            &serde_json::json!({
                "maid": maid,
                "board": board,
                "title": title,
                "content": content,
            }),
        )?,
        DEFAULT_TIMEOUT,
        MAX_DETAIL_BYTES,
    )
    .await?;
    if status != StatusCode::NOT_FOUND {
        return Ok(body);
    }

    // 2) 404（服务器未打补丁）→ human/tool 回退
    if settings.server.api_key.trim().is_empty() {
        return Err(
            "服务器缺少 REST 发帖端点，且未配置 VCP API Key 作为回退通道，请在 设置 → 服务器连接 中填写"
                .to_string(),
        );
    }
    let mut url = resolve_service_base(&settings.server.url, settings.server.endpoint_mode)
        .map_err(|error| error.to_string())?;
    append_url_segments(&mut url, &["v1", "human", "tool"])
        .map_err(|error| error.to_string())?;

    let tool_body = build_create_post_body(maid, board, title, content);
    let request = client(HttpProfile::AdminApi)
        .post(url)
        .bearer_auth(settings.server.api_key.trim())
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "text/plain;charset=UTF-8")
        .body(tool_body);

    send_json(request, DEFAULT_TIMEOUT, MAX_DETAIL_BYTES).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcp_modules::settings_manager::{ChatEndpointMode, ServerConfig};
    use axum::http::{HeaderMap, Uri};
    use axum::{Json, Router};
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    #[derive(Debug)]
    struct RecordedRequest {
        uri: String,
        authorization: Vec<String>,
        content_type: String,
        body: String,
    }

    async fn test_server(
        rest_status: StatusCode,
    ) -> (
        Settings,
        Arc<Mutex<Vec<RecordedRequest>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let router = Router::new().fallback(move |uri: Uri, headers: HeaderMap, body: String| {
            let captured = captured.clone();
            async move {
                let is_rest = uri.path().ends_with("/admin_api/forum/posts");
                captured.lock().await.push(RecordedRequest {
                    uri: uri.to_string(),
                    authorization: headers
                        .get_all(reqwest::header::AUTHORIZATION)
                        .iter()
                        .map(|value| value.to_str().unwrap().to_owned())
                        .collect(),
                    content_type: headers
                        .get(reqwest::header::CONTENT_TYPE)
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_owned(),
                    body,
                });
                if is_rest {
                    (
                        rest_status,
                        Json(serde_json::json!({ "source": "rest", "error": "REST unavailable" })),
                    )
                } else {
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "source": "human" })),
                    )
                }
            }
        });
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let settings = Settings {
            server: ServerConfig {
                url: format!("http://{address}/proxy/v1/chatvcp/completions?chatOnly=secret"),
                endpoint_mode: ChatEndpointMode::Raw,
                api_key: "forum-bearer".into(),
                admin_username: "user".into(),
                admin_password: "pass".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        (settings, requests, server)
    }

    #[tokio::test]
    async fn rest_404_falls_back_with_only_bearer_and_shared_service_prefix() {
        let (settings, requests, server) = test_server(StatusCode::NOT_FOUND).await;
        let result = create_post(&settings, "Maid", "Board", "Title", "Body").await;
        server.abort();

        assert_eq!(result.unwrap()["source"], "human");
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].uri, "/proxy/admin_api/forum/posts");
        assert_eq!(requests[0].authorization, ["Basic dXNlcjpwYXNz"]);
        assert_eq!(requests[0].content_type, "application/json");
        let rest_body: Value = serde_json::from_str(&requests[0].body).unwrap();
        assert_eq!(
            rest_body,
            serde_json::json!({
                "maid": "Maid", "board": "Board", "title": "Title", "content": "Body",
            })
        );
        assert_eq!(requests[1].uri, "/proxy/v1/human/tool");
        assert_eq!(requests[1].authorization, ["Bearer forum-bearer"]);
        assert_eq!(requests[1].content_type, "text/plain;charset=UTF-8");
        assert!(requests[1].body.starts_with("<<<[TOOL_REQUEST]>>>\n"));
        assert!(requests[1].body.contains("command:「始ESCAPE」CreatePost「末ESCAPE」"));
        assert!(requests[1].body.contains("content:「始ESCAPE」Body「末ESCAPE」"));
        assert!(requests[1].body.ends_with("\n<<<[END_TOOL_REQUEST]>>>"));
    }

    #[tokio::test]
    async fn rest_success_or_non_404_failure_never_sends_human_tool_request() {
        for status in [
            StatusCode::OK,
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            let (settings, requests, server) = test_server(status).await;
            let result = create_post(&settings, "Maid", "Board", "Title", "Body").await;
            server.abort();

            if status.is_success() {
                assert_eq!(result.unwrap()["source"], "rest");
            } else {
                assert!(result.is_err(), "REST {status} must remain an error");
            }
            let requests = requests.lock().await;
            assert_eq!(requests.len(), 1, "REST {status} must not trigger fallback");
            assert_eq!(requests[0].uri, "/proxy/admin_api/forum/posts");
            assert_eq!(requests[0].authorization, ["Basic dXNlcjpwYXNz"]);
        }
    }
}
