use super::*;
use axum::body::Body;
use axum::extract::{Query, State as AxumState};
use axum::http::{HeaderMap, Response as HttpResponse};
use axum::response::Redirect;
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::stream;
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;

async fn spawn_test_server(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let address = listener.local_addr().expect("test listener address");
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("serve test router");
    });
    (format!("http://{address}"), task)
}

fn test_settings(base: &str) -> Settings {
    Settings {
        vcp_server_url: format!("{base}/proxy/v1/chat/completions"),
        vcp_api_key: "test-bearer".to_string(),
        admin_username: "user".to_string(),
        admin_password: "pass".to_string(),
        ..Settings::default()
    }
}

#[test]
fn normalizes_chat_completion_url_without_losing_proxy_prefix() {
    let base =
        normalize_server_base("https://example.test/proxy/v1/chat/completions?secret=no#fragment")
            .expect("valid URL");
    assert_eq!(base.as_str(), "https://example.test/proxy/");

    let mut admin = base;
    append_url_segments(&mut admin, &["admin_api", "dailynotes", "folders"]).expect("append path");
    assert_eq!(
        admin.as_str(),
        "https://example.test/proxy/admin_api/dailynotes/folders"
    );

    let encoded = normalize_server_base("https://example.test/proxy%20space/v1/chat/completions")
        .expect("encoded proxy prefix");
    assert_eq!(encoded.as_str(), "https://example.test/proxy%20space/");
}

#[test]
fn rejects_embedded_credentials_and_unsafe_segments() {
    assert!(normalize_server_base("https://user:pass@example.test/v1/chat/completions").is_err());
    for value in ["", ".", "..", "a/b", "a\\b", "C:secret", "bad\nname"] {
        assert!(validate_path_segment(value, "test").is_err(), "{value}");
    }
    assert!(validate_path_segment("Nova 的知识", "test").is_ok());
}

#[test]
fn enforces_checked_body_budget() {
    assert_eq!(checked_body_size(4, 5, 9).expect("within limit"), 9);
    assert_eq!(
        checked_body_size(4, 6, 9).expect_err("over limit").code,
        DiaryErrorCode::ResponseTooLarge
    );
    assert!(checked_body_size(usize::MAX, 1, usize::MAX).is_err());
}

#[test]
fn maps_all_documented_http_statuses_to_stable_codes() {
    let cases = [
        (400, DiaryErrorCode::InvalidRequest),
        (408, DiaryErrorCode::Timeout),
        (409, DiaryErrorCode::Conflict),
        (413, DiaryErrorCode::ResponseTooLarge),
        (422, DiaryErrorCode::InvalidRequest),
        (401, DiaryErrorCode::AuthRequired),
        (403, DiaryErrorCode::Forbidden),
        (404, DiaryErrorCode::NotFound),
        (429, DiaryErrorCode::RateLimited),
        (499, DiaryErrorCode::Cancelled),
        (500, DiaryErrorCode::ServerError),
        (503, DiaryErrorCode::ServiceUnavailable),
        (504, DiaryErrorCode::Timeout),
    ];
    for (status, expected) in cases {
        let status = StatusCode::from_u16(status).expect("valid fixture status");
        let error = map_http_status(status, Some(br#"{"error":"bounded fixture"}"#));
        assert_eq!(error.code, expected);
        assert_eq!(error.message, "bounded fixture");
    }
}

#[test]
fn serializes_public_dtos_and_error_codes_as_the_frontend_contract() {
    let key = DiaryNoteKey {
        folder: "F".to_string(),
        file: "a.txt".to_string(),
    };
    assert_eq!(
        serde_json::to_value(&DiarySemanticSearchRequest {
            request_id: "request-1".to_string(),
            query: "memory".to_string(),
            folder: None,
            search_all: true,
            k: 5,
        })
        .expect("serialize semantic request"),
        json!({
            "requestId": "request-1",
            "query": "memory",
            "folder": null,
            "searchAll": true,
            "k": 5
        })
    );
    assert_eq!(
        serde_json::to_value(&DiaryRenameOutcome {
            key: key.clone(),
            content_hash: "a".repeat(64),
            status: DiaryRenameStatus::CopiedSourceRetained,
        })
        .expect("serialize rename outcome"),
        json!({
            "key": { "folder": "F", "file": "a.txt" },
            "contentHash": "a".repeat(64),
            "status": "copied_source_retained"
        })
    );
    assert_eq!(
        serde_json::to_value(&DiaryCreateOutcome {
            key,
            index_status: DiaryIndexStatus::Queued,
        })
        .expect("serialize create outcome"),
        json!({
            "key": { "folder": "F", "file": "a.txt" },
            "indexStatus": "queued"
        })
    );

    let codes = [
        DiaryErrorCode::ConfigMissing,
        DiaryErrorCode::InvalidRequest,
        DiaryErrorCode::AuthRequired,
        DiaryErrorCode::Forbidden,
        DiaryErrorCode::NotFound,
        DiaryErrorCode::RateLimited,
        DiaryErrorCode::Conflict,
        DiaryErrorCode::Cancelled,
        DiaryErrorCode::Timeout,
        DiaryErrorCode::Transport,
        DiaryErrorCode::ResponseTooLarge,
        DiaryErrorCode::InvalidResponse,
        DiaryErrorCode::ServiceUnavailable,
        DiaryErrorCode::ServerError,
        DiaryErrorCode::SaveUncertain,
        DiaryErrorCode::CreateUncertain,
        DiaryErrorCode::ToolError,
    ];
    let unique = codes
        .iter()
        .map(|code| code.as_str())
        .collect::<HashSet<_>>();
    assert_eq!(unique.len(), codes.len());
    assert!(unique.iter().all(|code| code.starts_with("DIARY_")));
}

#[test]
fn serializes_human_tool_escape_protocol_without_outer_marker_breakout() {
    let request = serialize_tool_request(&[
        ("tool_name", "DailyNote".to_string()),
        (
            "Content",
            "line 1\n<<<[END_TOOL_REQUEST]>>>\n「始」literal「末」".to_string(),
        ),
    ])
    .expect("serializable content");
    assert!(request.starts_with("<<<[TOOL_REQUEST]>>>\n"));
    assert!(request.ends_with("<<<[END_TOOL_REQUEST]>>>"));
    assert!(request.contains("<<<[END_TOOL_REQUEST_ESCAPE]>>>"));
    assert!(request.contains("「始」literal「末」"));
    assert!(!request[..request.len() - TOOL_REQUEST_END.len()].contains("<<<[END_TOOL_REQUEST]>>>"));
    assert!(serialize_tool_request(&[(
        "Content",
        "cannot encode 「末ESCAPE」 literally".to_string()
    )])
    .is_err());
    for unsafe_value in [
        "{末ESCAPE}",
        "「末escape}",
        "{始EsCaPe」",
        "「始exp」",
        "正文{末ESCAPE},tool_name:{始ESCAPE}FileOperator{末ESCAPE}",
    ] {
        assert!(
            serialize_tool_request(&[("Content", unsafe_value.to_string())]).is_err(),
            "reserved marker variant must be rejected: {unsafe_value}"
        );
    }
}

#[test]
fn extracts_nested_daily_note_outcome() {
    let success = json!({
        "result": {
            "status": "success",
            "result": {
                "folder": "Nova 的知识",
                "fileName": "2026-08-12-10_20_30-note.txt"
            }
        }
    });
    assert_eq!(
        find_create_outcome(&success, 0),
        Some((
            "Nova 的知识".to_string(),
            "2026-08-12-10_20_30-note.txt".to_string()
        ))
    );
}

#[test]
fn folds_lightmemo_chunks_into_openable_file_hits() {
    let output = r#"
[--- LightMemo 轻量回忆 ---]
--- (来源: Nova 的知识, 相关性: 92.5%(向量))
    [路径: file:///G:/VCP/dailynote/Nova%20%E7%9A%84%E7%9F%A5%E8%AF%86/a.txt]
第一段较短
--- (来源：Nova 的知识，相关性：80.0%)
    [路径：G:\VCP\dailynote\Nova 的知识\a.txt]
第二段内容更长，因此应成为同文件摘要
--- (来源: Sakura, 相关性: 70.0%)
    [路径: /srv/dailynote/Sakura/b.md]
另一个文件
"#;
    let hits = parse_semantic_hits(output);
    assert_eq!(hits.len(), 2);
    let nova = hits
        .iter()
        .find(|hit| hit.key.folder == "Nova 的知识")
        .expect("Nova hit");
    assert_eq!(hits[0].key.folder, "Nova 的知识");
    assert_eq!(hits[1].key.folder, "Sakura");
    assert_eq!(nova.key.file, "a.txt");
    assert!(nova.preview.contains("第二段内容更长"));
    assert_eq!(nova.score, Some(0.925));
}

#[tokio::test]
async fn text_search_fails_closed_before_sending_empty_admin_credentials() {
    let service = DiaryServiceState::new().expect("service");
    let mut settings = test_settings("http://127.0.0.1:9/proxy");
    settings.admin_username.clear();
    settings.admin_password.clear();

    let error = service
        .search(
            &settings,
            &DiarySearchRequest {
                request_id: "missing-admin-config".to_string(),
                term: "query".to_string(),
                folder: Some("F".to_string()),
            },
        )
        .await
        .expect_err("missing credentials must not reach the network");
    assert_eq!(error.code, DiaryErrorCode::ConfigMissing);
}

#[test]
fn normalizes_partial_batch_results_without_promoting_http_success() {
    let a = DiaryNoteKey {
        folder: "A".to_string(),
        file: "one.txt".to_string(),
    };
    let b = DiaryNoteKey {
        folder: "A".to_string(),
        file: "two.txt".to_string(),
    };
    let outcome = normalize_move_outcome(
        &[a.clone(), b.clone()],
        MoveEnvelope {
            moved: vec!["A/one.txt to B/one.txt".to_string()],
            errors: vec![RawBatchError {
                note: "A/two.txt".to_string(),
                error: "File already exists at destination".to_string(),
            }],
        },
    );
    assert_eq!(outcome.succeeded, vec![a]);
    assert_eq!(outcome.errors.len(), 1);
    assert_eq!(outcome.errors[0].key, b);
}

#[tokio::test]
async fn stale_search_owner_cannot_clear_new_owner() {
    let lifecycle = Mutex::new(());
    let slot = Mutex::new(SearchOwnerSlot::default());
    let first = DiaryServiceState::begin_search(&lifecycle, &slot, "first")
        .await
        .expect("first owner");
    let first_finished = first.finished.clone();
    let first_task = tokio::spawn(async move {
        first.token.cancelled().await;
        first_finished.cancel();
    });
    let second = DiaryServiceState::begin_search(&lifecycle, &slot, "second")
        .await
        .expect("second owner");
    first_task.await.expect("first task joined");
    assert!(DiaryServiceState::complete_search(&slot, &second).await);
    assert!(slot.lock().await.active.is_none());
}

#[test]
fn rename_preserves_extension() {
    assert_eq!(
        normalize_rename_target("old.txt", "new").expect("append extension"),
        "new.txt"
    );
    assert_eq!(
        normalize_rename_target("old.md", "new.md").expect("same extension"),
        "new.md"
    );
    assert!(normalize_rename_target("old.md", "new.txt").is_err());
}

#[tokio::test]
async fn admin_requests_preserve_proxy_prefix_and_send_basic_auth() {
    let router = Router::new().route(
        "/proxy/admin_api/dailynotes/folders",
        get(|headers: HeaderMap| async move {
            let authorized = headers
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                == Some("Basic dXNlcjpwYXNz");
            if authorized {
                (
                    StatusCode::OK,
                    Json(json!({"folders": ["Nova 的知识", "项目簇"]})),
                )
            } else {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"error": "missing auth"})),
                )
            }
        }),
    );
    let (base, server) = spawn_test_server(router).await;
    let service = DiaryServiceState::new().expect("service");
    let folders = service
        .list_folders(&test_settings(&base))
        .await
        .expect("authorized folders");
    assert_eq!(folders.folders, vec!["Nova 的知识", "项目簇"]);
    server.abort();
}

#[tokio::test]
async fn human_tool_uses_bearer_and_official_escape_fields() {
    let captured_body = Arc::new(StdMutex::new(String::new()));
    let body_for_route = captured_body.clone();
    let router = Router::new().route(
            "/proxy/v1/human/tool",
            post(move |headers: HeaderMap, body: String| {
                let captured_body = body_for_route.clone();
                async move {
                    *captured_body.lock().expect("capture body") = body;
                    let authorized = headers
                        .get(reqwest::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        == Some("Bearer test-bearer");
                    if authorized {
                        (
                            StatusCode::OK,
                            Json(json!({
                                "result": {
                                    "content": "--- (来源: Nova 的知识, 相关性: 90%)\n[路径: file:///tmp/Nova%20%E7%9A%84%E7%9F%A5%E8%AF%86/a.txt]\n命中正文"
                                }
                            })),
                        )
                    } else {
                        (
                            StatusCode::UNAUTHORIZED,
                            Json(json!({"error": "missing bearer"})),
                        )
                    }
                }
            }),
        );
    let (base, server) = spawn_test_server(router).await;
    let service = DiaryServiceState::new().expect("service");
    let response = service
        .semantic_search(
            &test_settings(&base),
            &DiarySemanticSearchRequest {
                request_id: "semantic-test".to_string(),
                query: "温柔".to_string(),
                folder: Some("Nova 的知识".to_string()),
                search_all: false,
                k: 5,
            },
        )
        .await
        .expect("semantic response");
    assert_eq!(response.hits.len(), 1);
    assert_eq!(response.hits[0].key.file, "a.txt");
    let request_body = captured_body.lock().expect("captured body");
    assert!(request_body.starts_with(TOOL_REQUEST_START));
    assert!(request_body.contains("tool_name:「始ESCAPE」LightMemo「末ESCAPE」"));
    assert!(request_body.contains("folder:「始ESCAPE」Nova 的知识「末ESCAPE」"));
    server.abort();
}

#[tokio::test]
async fn refuses_redirects_even_when_the_target_would_succeed() {
    let followed = Arc::new(AtomicUsize::new(0));
    let followed_route = followed.clone();
    let router = Router::new()
        .route(
            "/proxy/admin_api/dailynotes/folders",
            get(|| async { Redirect::temporary("/proxy/redirect-target") }),
        )
        .route(
            "/proxy/redirect-target",
            get(move || {
                let followed = followed_route.clone();
                async move {
                    followed.fetch_add(1, AtomicOrdering::SeqCst);
                    Json(json!({"folders": []}))
                }
            }),
        );
    let (base, server) = spawn_test_server(router).await;
    let service = DiaryServiceState::new().expect("service");
    let error = service
        .list_folders(&test_settings(&base))
        .await
        .expect_err("redirect must fail closed");
    assert_eq!(error.code, DiaryErrorCode::Forbidden);
    assert_eq!(followed.load(AtomicOrdering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn enforces_actual_stream_size_without_content_length() {
    let router = Router::new().route(
        "/chunked",
        get(|| async {
            let chunks = stream::iter([
                Ok::<Bytes, Infallible>(Bytes::from_static(b"12345678901234567890")),
                Ok::<Bytes, Infallible>(Bytes::from_static(b"12345678901234567890")),
            ]);
            HttpResponse::builder()
                .status(StatusCode::OK)
                .body(Body::from_stream(chunks))
                .expect("chunked response")
        }),
    );
    let (base, server) = spawn_test_server(router).await;
    let service = DiaryServiceState::new().expect("service");
    let request = service.client.get(format!("{base}/chunked"));
    let error = service
        .request_bytes(request, 32, Duration::from_secs(2), None)
        .await
        .expect_err("stream must exceed budget");
    assert_eq!(error.code, DiaryErrorCode::ResponseTooLarge);
    server.abort();
}

#[tokio::test]
async fn rejects_declared_oversize_malformed_json_and_total_timeout() {
    let router = Router::new()
        .route("/malformed", get(|| async { (StatusCode::OK, "not-json") }))
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Json(json!({"ok": true}))
            }),
        );
    let (base, server) = spawn_test_server(router).await;
    let service = DiaryServiceState::new().expect("service");

    let declared_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind declared fixture");
    let declared_address = declared_listener
        .local_addr()
        .expect("declared fixture address");
    let declared_server = tokio::spawn(async move {
        let (mut socket, _) = declared_listener.accept().await.expect("accept request");
        let mut request = [0_u8; 1024];
        let _ = socket.read(&mut request).await.expect("read request");
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n")
            .await
            .expect("write declared response");
    });

    let declared = service
        .request_bytes(
            service
                .client
                .get(format!("http://{declared_address}/declared")),
            32,
            Duration::from_secs(1),
            None,
        )
        .await
        .expect_err("declared body over budget");
    assert_eq!(declared.code, DiaryErrorCode::ResponseTooLarge);
    declared_server.await.expect("declared fixture task");

    let malformed = service
        .request_json::<Value>(
            service.client.get(format!("{base}/malformed")),
            1024,
            Duration::from_secs(1),
            None,
        )
        .await
        .expect_err("malformed JSON");
    assert_eq!(malformed.code, DiaryErrorCode::InvalidResponse);

    let timeout = service
        .request_bytes(
            service.client.get(format!("{base}/slow")),
            1024,
            Duration::from_millis(20),
            None,
        )
        .await
        .expect_err("total timeout");
    assert_eq!(timeout.code, DiaryErrorCode::Timeout);
    server.abort();
}

#[derive(Clone)]
struct SaveFixture {
    content: Arc<StdMutex<String>>,
    post_count: Arc<AtomicUsize>,
}

#[derive(Clone, Default)]
struct RenameFixture {
    target_content: Arc<StdMutex<Option<String>>>,
}

#[derive(Clone)]
struct AmbiguousWriteFixture {
    content: Arc<StdMutex<String>>,
    mode: Arc<AtomicUsize>,
}

#[tokio::test]
async fn rename_reports_source_retained_when_delete_request_fails() {
    let fixture = RenameFixture::default();
    let router = Router::new()
            .route(
                "/proxy/admin_api/dailynotes/note/F/old.txt",
                get(|| async { Json(json!({"content": "source"})) }),
            )
            .route(
                "/proxy/admin_api/dailynotes/note/F/new.txt",
                get(|AxumState(state): AxumState<RenameFixture>| async move {
                    match state
                        .target_content
                        .lock()
                        .expect("read target")
                        .clone()
                    {
                        Some(content) => (StatusCode::OK, Json(json!({"content": content}))),
                        None => (
                            StatusCode::NOT_FOUND,
                            Json(json!({"error": "not found"})),
                        ),
                    }
                })
                .post(
                    |AxumState(state): AxumState<RenameFixture>, Json(payload): Json<Value>| async move {
                        let content = payload
                            .get("content")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        *state.target_content.lock().expect("write target") = Some(content);
                        Json(json!({"success": true}))
                    },
                ),
            )
            .route(
                "/proxy/admin_api/dailynotes/delete-batch",
                post(|| async {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": "delete unavailable"})),
                    )
                }),
            )
            .with_state(fixture);
    let (base, server) = spawn_test_server(router).await;
    let service = DiaryServiceState::new().expect("service");
    let outcome = service
        .rename_note(
            &test_settings(&base),
            &DiaryRenameRequest {
                source: DiaryNoteKey {
                    folder: "F".to_string(),
                    file: "old.txt".to_string(),
                },
                target_file: "new.txt".to_string(),
                baseline_hash: content_hash("source"),
            },
        )
        .await
        .expect("verified target is a partial rename outcome");

    assert_eq!(outcome.status, DiaryRenameStatus::CopiedSourceRetained);
    assert_eq!(outcome.key.file, "new.txt");
    server.abort();
}

#[tokio::test]
async fn rename_completes_after_verified_copy_and_rejects_known_target() {
    let fixture = RenameFixture::default();
    let router = Router::new()
        .route(
            "/proxy/admin_api/dailynotes/note/F/old.txt",
            get(|| async { Json(json!({"content": "source"})) }),
        )
        .route(
            "/proxy/admin_api/dailynotes/note/F/new.txt",
            get(|AxumState(state): AxumState<RenameFixture>| async move {
                match state
                    .target_content
                    .lock()
                    .expect("read target")
                    .clone()
                {
                    Some(content) => (StatusCode::OK, Json(json!({"content": content}))),
                    None => (
                        StatusCode::NOT_FOUND,
                        Json(json!({"error": "not found"})),
                    ),
                }
            })
            .post(
                |AxumState(state): AxumState<RenameFixture>, Json(payload): Json<Value>| async move {
                    let content = payload
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    *state.target_content.lock().expect("write target") = Some(content);
                    Json(json!({"success": true}))
                },
            ),
        )
        .route(
            "/proxy/admin_api/dailynotes/delete-batch",
            post(|| async { Json(json!({"deleted": ["F/old.txt"], "errors": []})) }),
        )
        .with_state(fixture);
    let (base, server) = spawn_test_server(router).await;
    let service = DiaryServiceState::new().expect("service");
    let request = DiaryRenameRequest {
        source: DiaryNoteKey {
            folder: "F".to_string(),
            file: "old.txt".to_string(),
        },
        target_file: "new.txt".to_string(),
        baseline_hash: content_hash("source"),
    };

    let outcome = service
        .rename_note(&test_settings(&base), &request)
        .await
        .expect("complete rename");
    assert_eq!(outcome.status, DiaryRenameStatus::Renamed);

    let conflict = service
        .rename_note(&test_settings(&base), &request)
        .await
        .expect_err("verified target now exists");
    assert_eq!(conflict.code, DiaryErrorCode::Conflict);
    server.abort();
}

#[tokio::test]
async fn classifies_ambiguous_write_readback_without_retrying() {
    let fixture = AmbiguousWriteFixture {
        content: Arc::new(StdMutex::new("baseline".to_string())),
        mode: Arc::new(AtomicUsize::new(0)),
    };
    let router =
        Router::new()
            .route(
                "/proxy/admin_api/dailynotes/note/F/a.txt",
                get(
                    |AxumState(state): AxumState<AmbiguousWriteFixture>| async move {
                        if state.mode.load(AtomicOrdering::SeqCst) == 3 {
                            return (
                                StatusCode::SERVICE_UNAVAILABLE,
                                Json(json!({"error": "readback unavailable"})),
                            );
                        }
                        let content = state.content.lock().expect("read content").clone();
                        (StatusCode::OK, Json(json!({"content": content})))
                    },
                )
                .post(
                    |AxumState(state): AxumState<AmbiguousWriteFixture>,
                     Json(payload): Json<Value>| async move {
                        let mode = state.mode.load(AtomicOrdering::SeqCst);
                        let candidate = payload
                            .get("content")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let (status, body) = match mode {
                            0 => {
                                *state.content.lock().expect("candidate") = candidate.to_string();
                                (
                                    StatusCode::INTERNAL_SERVER_ERROR,
                                    json!({"error": "ambiguous write"}).to_string(),
                                )
                            }
                            2 => {
                                *state.content.lock().expect("third version") = "third".to_string();
                                (
                                    StatusCode::INTERNAL_SERVER_ERROR,
                                    json!({"error": "ambiguous write"}).to_string(),
                                )
                            }
                            4 => {
                                *state.content.lock().expect("candidate") = candidate.to_string();
                                (StatusCode::OK, "not-json".to_string())
                            }
                            _ => (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                json!({"error": "ambiguous write"}).to_string(),
                            ),
                        };
                        (status, body)
                    },
                ),
            )
            .with_state(fixture.clone());
    let (base, server) = spawn_test_server(router).await;
    let service = DiaryServiceState::new().expect("service");
    let settings = test_settings(&base);
    let key = DiaryNoteKey {
        folder: "F".to_string(),
        file: "a.txt".to_string(),
    };
    let baseline_hash = content_hash("baseline");
    let candidate_hash = content_hash("candidate");

    let verified = service
        .write_and_verify(
            &settings,
            &key,
            "candidate",
            &candidate_hash,
            Some(&baseline_hash),
        )
        .await
        .expect("candidate readback proves success");
    assert!(verified.verified);

    fixture.mode.store(4, AtomicOrdering::SeqCst);
    *fixture.content.lock().expect("reset baseline") = "baseline".to_string();
    let malformed_success = service
        .write_and_verify(
            &settings,
            &key,
            "candidate",
            &candidate_hash,
            Some(&baseline_hash),
        )
        .await
        .expect("malformed 2xx still requires and accepts candidate readback");
    assert!(malformed_success.verified);

    fixture.mode.store(1, AtomicOrdering::SeqCst);
    *fixture.content.lock().expect("reset baseline") = "baseline".to_string();
    let unchanged = service
        .write_and_verify(
            &settings,
            &key,
            "candidate",
            &candidate_hash,
            Some(&baseline_hash),
        )
        .await
        .expect_err("baseline readback is uncertain");
    assert_eq!(unchanged.code, DiaryErrorCode::SaveUncertain);

    fixture.mode.store(2, AtomicOrdering::SeqCst);
    *fixture.content.lock().expect("reset baseline") = "baseline".to_string();
    let third = service
        .write_and_verify(
            &settings,
            &key,
            "candidate",
            &candidate_hash,
            Some(&baseline_hash),
        )
        .await
        .expect_err("third version is conflict");
    assert_eq!(third.code, DiaryErrorCode::Conflict);

    fixture.mode.store(3, AtomicOrdering::SeqCst);
    let unavailable = service
        .write_and_verify(
            &settings,
            &key,
            "candidate",
            &candidate_hash,
            Some(&baseline_hash),
        )
        .await
        .expect_err("unreadable result is uncertain");
    assert_eq!(unavailable.code, DiaryErrorCode::SaveUncertain);
    server.abort();
}

#[tokio::test]
async fn create_success_without_a_final_key_is_uncertain() {
    let router = Router::new().route(
        "/proxy/v1/human/tool",
        post(|| async { Json(json!({"status": "success", "result": {"message": "created"}})) }),
    );
    let (base, server) = spawn_test_server(router).await;
    let service = DiaryServiceState::new().expect("service");
    let error = service
        .create_note(
            &test_settings(&base),
            &DiaryCreateRequest {
                maid: "Nova".to_string(),
                date: "2026-08-12".to_string(),
                folder: Some("F".to_string()),
                file_name_suffix: Some("a".to_string()),
                tag: None,
                content: "content".to_string(),
            },
        )
        .await
        .expect_err("a missing final key cannot prove which file was created");
    assert_eq!(error.code, DiaryErrorCode::CreateUncertain);
    server.abort();
}

#[tokio::test]
async fn save_checks_baseline_and_reads_back_the_written_content() {
    let fixture = SaveFixture {
        content: Arc::new(StdMutex::new("baseline".to_string())),
        post_count: Arc::new(AtomicUsize::new(0)),
    };
    let router = Router::new()
        .route(
            "/proxy/admin_api/dailynotes/note/F/a.txt",
            get(|AxumState(state): AxumState<SaveFixture>| async move {
                let content = state.content.lock().expect("read content").clone();
                Json(json!({"content": content}))
            })
            .post(
                |AxumState(state): AxumState<SaveFixture>, Json(payload): Json<Value>| async move {
                    let content = payload
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    *state.content.lock().expect("write content") = content;
                    state.post_count.fetch_add(1, AtomicOrdering::SeqCst);
                    Json(json!({"success": true}))
                },
            ),
        )
        .with_state(fixture.clone());
    let (base, server) = spawn_test_server(router).await;
    let service = DiaryServiceState::new().expect("service");
    let settings = test_settings(&base);
    let key = DiaryNoteKey {
        folder: "F".to_string(),
        file: "a.txt".to_string(),
    };
    let outcome = service
        .save_note(
            &settings,
            &DiarySaveRequest {
                key: key.clone(),
                content: "candidate".to_string(),
                baseline_hash: content_hash("baseline"),
                force: false,
            },
        )
        .await
        .expect("verified save");
    assert!(outcome.verified);
    assert_eq!(outcome.content_hash, content_hash("candidate"));
    assert_eq!(fixture.post_count.load(AtomicOrdering::SeqCst), 1);

    *fixture.content.lock().expect("change remote") = "remote change".to_string();
    let conflict = service
        .save_note(
            &settings,
            &DiarySaveRequest {
                key,
                content: "should not write".to_string(),
                baseline_hash: content_hash("candidate"),
                force: false,
            },
        )
        .await
        .expect_err("stale baseline conflict");
    assert_eq!(conflict.code, DiaryErrorCode::Conflict);
    assert_eq!(fixture.post_count.load(AtomicOrdering::SeqCst), 1);

    let forced = service
        .save_note(
            &settings,
            &DiarySaveRequest {
                key: DiaryNoteKey {
                    folder: "F".to_string(),
                    file: "a.txt".to_string(),
                },
                content: "forced".to_string(),
                baseline_hash: content_hash("candidate"),
                force: true,
            },
        )
        .await
        .expect("explicit force bypasses stale baseline precheck");
    assert_eq!(forced.content_hash, content_hash("forced"));
    assert_eq!(fixture.post_count.load(AtomicOrdering::SeqCst), 2);
    server.abort();
}

#[tokio::test]
async fn create_move_delete_and_folder_delete_keep_wire_contracts_and_partial_results() {
    let router = Router::new()
        .route(
            "/proxy/v1/human/tool",
            post(|headers: HeaderMap, body: String| async move {
                assert_eq!(
                    headers
                        .get(reqwest::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer test-bearer")
                );
                assert!(body.contains("tool_name:「始ESCAPE」DailyNote「末ESCAPE」"));
                Json(json!({
                    "result": {
                        "folder": "F",
                        "fileName": "created.txt"
                    }
                }))
            }),
        )
        .route(
            "/proxy/admin_api/dailynotes/move",
            post(|Json(payload): Json<Value>| async move {
                assert_eq!(payload["targetFolder"], "B");
                Json(json!({
                    "moved": ["A/one.txt to B/one.txt"],
                    "errors": [{"note": "A/two.txt", "error": "already exists"}]
                }))
            }),
        )
        .route(
            "/proxy/admin_api/dailynotes/delete-batch",
            post(|Json(payload): Json<Value>| async move {
                assert_eq!(payload["notesToDelete"].as_array().map(Vec::len), Some(2));
                Json(json!({
                    "deleted": ["A/one.txt"],
                    "errors": [{"note": "A/two.txt", "error": "not found"}]
                }))
            }),
        )
        .route(
            "/proxy/admin_api/dailynotes/folder/delete",
            post(|Json(payload): Json<Value>| async move {
                assert_eq!(payload["folderName"], "Empty");
                Json(json!({"success": true}))
            }),
        );
    let (base, server) = spawn_test_server(router).await;
    let service = DiaryServiceState::new().expect("service");
    let settings = test_settings(&base);
    let created = service
        .create_note(
            &settings,
            &DiaryCreateRequest {
                maid: "Nova".to_string(),
                date: "2026-08-12".to_string(),
                folder: Some("F".to_string()),
                file_name_suffix: Some("created".to_string()),
                tag: Some("test".to_string()),
                content: "content".to_string(),
            },
        )
        .await
        .expect("create outcome");
    assert_eq!(created.key.file, "created.txt");

    let sources = vec![
        DiaryNoteKey {
            folder: "A".to_string(),
            file: "one.txt".to_string(),
        },
        DiaryNoteKey {
            folder: "A".to_string(),
            file: "two.txt".to_string(),
        },
    ];
    let moved = service
        .move_notes(
            &settings,
            &DiaryMoveRequest {
                sources: sources.clone(),
                target_folder: "B".to_string(),
            },
        )
        .await
        .expect("move partial outcome");
    assert_eq!(moved.succeeded, vec![sources[0].clone()]);
    assert_eq!(moved.errors[0].key, sources[1]);

    let deleted = service
        .delete_notes(
            &settings,
            &DiaryDeleteRequest {
                sources: sources.clone(),
            },
        )
        .await
        .expect("delete partial outcome");
    assert_eq!(deleted.succeeded, vec![sources[0].clone()]);
    assert_eq!(deleted.errors[0].key, sources[1]);

    service
        .delete_empty_folder(&settings, "Empty")
        .await
        .expect("empty folder delete");
    server.abort();
}

#[tokio::test]
async fn a_new_text_search_cancels_the_previous_request_owner() {
    let slow_started = Arc::new(Notify::new());
    let route_notify = slow_started.clone();
    let router = Router::new().route(
        "/proxy/admin_api/dailynotes/search",
        get(move |Query(query): Query<HashMap<String, String>>| {
            let slow_started = route_notify.clone();
            async move {
                if query.get("term").is_some_and(|term| term == "slow") {
                    slow_started.notify_one();
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
                Json(json!({
                    "notes": [{
                        "name": "latest.txt",
                        "lastModified": "2026-08-12T10:00:00.000Z",
                        "preview": "latest"
                    }],
                    "total": 1,
                    "limited": false
                }))
            }
        }),
    );
    let (base, server) = spawn_test_server(router).await;
    let service = Arc::new(DiaryServiceState::new().expect("service"));
    let settings = test_settings(&base);
    let first_service = service.clone();
    let first_settings = settings.clone();
    let first = tokio::spawn(async move {
        first_service
            .search(
                &first_settings,
                &DiarySearchRequest {
                    request_id: "first".to_string(),
                    term: "slow".to_string(),
                    folder: Some("F".to_string()),
                },
            )
            .await
    });
    slow_started.notified().await;

    let second = service
        .search(
            &settings,
            &DiarySearchRequest {
                request_id: "second".to_string(),
                term: "latest".to_string(),
                folder: Some("F".to_string()),
            },
        )
        .await
        .expect("latest search");
    let first_error = first
        .await
        .expect("first task")
        .expect_err("first search cancelled");
    assert_eq!(first_error.code, DiaryErrorCode::Cancelled);
    assert_eq!(second.notes[0].file, "latest.txt");
    server.abort();
}

#[tokio::test]
async fn a_new_semantic_search_cancels_the_previous_semantic_owner() {
    let slow_started = Arc::new(Notify::new());
    let route_notify = slow_started.clone();
    let router = Router::new().route(
        "/proxy/v1/human/tool",
        post(move |body: String| {
            let slow_started = route_notify.clone();
            async move {
                if body.contains("query:「始ESCAPE」slow「末ESCAPE」") {
                    slow_started.notify_one();
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
                Json(json!({
                    "result": {
                        "content": "--- (来源: F, 相关性: 90%)\n[路径: /srv/F/latest.txt]\nlatest"
                    }
                }))
            }
        }),
    );
    let (base, server) = spawn_test_server(router).await;
    let service = Arc::new(DiaryServiceState::new().expect("service"));
    let settings = test_settings(&base);
    let first_service = service.clone();
    let first_settings = settings.clone();
    let first = tokio::spawn(async move {
        first_service
            .semantic_search(
                &first_settings,
                &DiarySemanticSearchRequest {
                    request_id: "semantic-first".to_string(),
                    query: "slow".to_string(),
                    folder: Some("F".to_string()),
                    search_all: false,
                    k: 5,
                },
            )
            .await
    });
    slow_started.notified().await;

    let second = service
        .semantic_search(
            &settings,
            &DiarySemanticSearchRequest {
                request_id: "semantic-second".to_string(),
                query: "latest".to_string(),
                folder: Some("F".to_string()),
                search_all: false,
                k: 5,
            },
        )
        .await
        .expect("latest semantic search");
    let first_error = first
        .await
        .expect("first semantic task")
        .expect_err("first semantic search cancelled");
    assert_eq!(first_error.code, DiaryErrorCode::Cancelled);
    assert_eq!(second.hits[0].key.file, "latest.txt");
    server.abort();
}
