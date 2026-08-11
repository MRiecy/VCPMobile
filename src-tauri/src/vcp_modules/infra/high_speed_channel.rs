use crate::vcp_modules::db_manager::DbState;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{Duration, Instant};

const MAX_UPLOAD_BYTES: u64 = 200 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const ENDPOINT_ACCEPT_LIFETIME: Duration = Duration::from_secs(30);
const CONNECTION_LIFETIME: Duration = Duration::from_secs(10 * 60);
const IO_IDLE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(serde::Deserialize)]
pub struct UploadMetadata {
    pub name: String,
    pub mime: String,
    pub size: u64,
}

#[derive(serde::Serialize)]
pub struct UploadEndpoint {
    pub url: String,
    pub token: String,
}

/// 准备高速上传链路：启动临时本地服务器并返回端口
///
/// 【适用场景】非 Android 端的大文件 (≥2MB) 上传。前端通过 XHR 向本地临时 TCP
/// 端口发送流式数据，Rust 直接写入磁盘，绕过 Tauri IPC 的内存限制。
///
/// Android 端不走此链路：Android 通过原生插件 `pick_file` 在 Kotlin 层完成流式
/// 拷贝与哈希，直接调用 `register_local_file` 零拷贝注册，无需 WebView 参与传输。
#[tauri::command]
pub async fn prepare_vcp_upload<R: Runtime>(
    app_handle: AppHandle<R>,
    db_state: State<'_, DbState>,
    metadata: UploadMetadata,
) -> Result<UploadEndpoint, String> {
    validate_upload_metadata(&metadata)?;

    let mut temp_dir = app_handle
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?;
    temp_dir.push("uploads");
    tokio::fs::create_dir_all(&temp_dir)
        .await
        .map_err(|e| e.to_string())?;

    // 1. 监听本地随机端口
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let token = uuid::Uuid::new_v4().to_string();

    let url = format!("http://127.0.0.1:{}", port);
    let token_clone = token.clone();
    let pool = db_state.pool.clone();

    tauri::async_runtime::spawn(async move {
        let accept_deadline = Instant::now() + ENDPOINT_ACCEPT_LIFETIME;
        while Instant::now() < accept_deadline {
            let remaining = accept_deadline.saturating_duration_since(Instant::now());
            let accept_res =
                tokio::time::timeout(remaining.min(Duration::from_millis(500)), listener.accept())
                    .await;

            let (socket, _addr) = match accept_res {
                Ok(Ok(conn)) => conn,
                _ => continue,
            };

            match handle_upload_connection(socket, &app_handle, &pool, &temp_dir, &metadata, &token)
                .await
            {
                Ok(true) => break,
                Ok(false) => {}
                Err(error) => log::warn!("[HighSpeedUpload] connection failed: {error}"),
            }
        }
    });

    Ok(UploadEndpoint {
        url,
        token: token_clone,
    })
}

#[derive(Debug)]
struct ProtocolError {
    status: &'static str,
    message: String,
}

impl ProtocolError {
    fn new(status: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

#[derive(Debug)]
enum UploadRequestKind {
    Preflight,
    Upload,
}

fn validate_upload_metadata(metadata: &UploadMetadata) -> Result<(), String> {
    if metadata.size == 0 || metadata.size > MAX_UPLOAD_BYTES {
        return Err(format!(
            "附件大小必须在 1 字节到 {} MB 之间",
            MAX_UPLOAD_BYTES / 1024 / 1024
        ));
    }
    if metadata.name.trim().is_empty() || metadata.name.len() > 512 {
        return Err("附件名称为空或过长".to_string());
    }
    if metadata.mime.len() > 255 {
        return Err("附件 MIME 类型过长".to_string());
    }
    Ok(())
}

fn parse_upload_request_head(
    head: &[u8],
    expected_token: &str,
    expected_size: u64,
) -> Result<UploadRequestKind, ProtocolError> {
    let text = std::str::from_utf8(head)
        .map_err(|_| ProtocolError::new("400 Bad Request", "请求头不是有效 UTF-8"))?;
    let mut lines = text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| ProtocolError::new("400 Bad Request", "缺少请求行"))?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap_or_default();
    let path = request_parts.next().unwrap_or_default();
    let version = request_parts.next().unwrap_or_default();
    if request_parts.next().is_some()
        || path != "/"
        || !(version == "HTTP/1.1" || version == "HTTP/1.0")
    {
        return Err(ProtocolError::new("400 Bad Request", "无效请求行"));
    }
    if method == "OPTIONS" {
        return Ok(UploadRequestKind::Preflight);
    }
    if method != "POST" {
        return Err(ProtocolError::new(
            "405 Method Not Allowed",
            "仅允许 POST 上传",
        ));
    }

    let mut supplied_token: Option<&str> = None;
    let mut content_length: Option<u64> = None;
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| ProtocolError::new("400 Bad Request", "请求头格式错误"))?;
        let value = value.trim();
        if name.trim().eq_ignore_ascii_case("x-upload-token") {
            if supplied_token.replace(value).is_some() {
                return Err(ProtocolError::new("400 Bad Request", "重复上传 token"));
            }
        } else if name.trim().eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(ProtocolError::new("400 Bad Request", "重复 Content-Length"));
            }
            content_length = Some(
                value
                    .parse::<u64>()
                    .map_err(|_| ProtocolError::new("400 Bad Request", "无效 Content-Length"))?,
            );
        }
    }

    if supplied_token != Some(expected_token) {
        return Err(ProtocolError::new("401 Unauthorized", "上传 token 无效"));
    }
    if content_length != Some(expected_size) {
        return Err(ProtocolError::new(
            "413 Payload Too Large",
            "正文长度与已声明附件大小不一致",
        ));
    }
    Ok(UploadRequestKind::Upload)
}

async fn write_http_response(
    socket: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, X-Upload-Token\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = tokio::time::timeout(IO_IDLE_TIMEOUT, socket.write_all(head.as_bytes())).await;
    if !body.is_empty() {
        let _ = tokio::time::timeout(IO_IDLE_TIMEOUT, socket.write_all(body)).await;
    }
}

async fn read_request_head(socket: &mut TcpStream) -> Result<(Vec<u8>, Vec<u8>), ProtocolError> {
    let mut header_data = Vec::with_capacity(4096);
    let mut buffer = [0u8; 8192];
    loop {
        let n = tokio::time::timeout(IO_IDLE_TIMEOUT, socket.read(&mut buffer))
            .await
            .map_err(|_| ProtocolError::new("408 Request Timeout", "读取请求头超时"))?
            .map_err(|e| ProtocolError::new("400 Bad Request", e.to_string()))?;
        if n == 0 {
            return Err(ProtocolError::new("400 Bad Request", "请求头未完成"));
        }
        header_data.extend_from_slice(&buffer[..n]);
        if let Some(pos) = header_data
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
        {
            if pos + 4 > MAX_HEADER_BYTES {
                return Err(ProtocolError::new(
                    "431 Request Header Fields Too Large",
                    "请求头过大",
                ));
            }
            let body_prefix = header_data.split_off(pos + 4);
            header_data.truncate(pos);
            return Ok((header_data, body_prefix));
        }
        if header_data.len() > MAX_HEADER_BYTES {
            return Err(ProtocolError::new(
                "431 Request Header Fields Too Large",
                "请求头过大",
            ));
        }
    }
}

async fn receive_upload_body(
    socket: &mut TcpStream,
    temp_path: &std::path::Path,
    initial_body: &[u8],
    expected_size: u64,
) -> Result<(String, u64), ProtocolError> {
    if initial_body.len() as u64 > expected_size {
        return Err(ProtocolError::new(
            "413 Payload Too Large",
            "正文超过声明大小",
        ));
    }
    let mut file = tokio::fs::File::create(temp_path)
        .await
        .map_err(|e| ProtocolError::new("500 Internal Server Error", e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut received = 0u64;

    if !initial_body.is_empty() {
        tokio::time::timeout(IO_IDLE_TIMEOUT, file.write_all(initial_body))
            .await
            .map_err(|_| ProtocolError::new("408 Request Timeout", "写入临时文件超时"))?
            .map_err(|e| ProtocolError::new("500 Internal Server Error", e.to_string()))?;
        hasher.update(initial_body);
        received = initial_body.len() as u64;
    }

    let mut buffer = [0u8; 64 * 1024];
    while received < expected_size {
        let remaining = (expected_size - received) as usize;
        let read_len = remaining.min(buffer.len());
        let n = tokio::time::timeout(IO_IDLE_TIMEOUT, socket.read(&mut buffer[..read_len]))
            .await
            .map_err(|_| ProtocolError::new("408 Request Timeout", "上传读取空闲超时"))?
            .map_err(|e| ProtocolError::new("400 Bad Request", e.to_string()))?;
        if n == 0 {
            return Err(ProtocolError::new("400 Bad Request", "上传正文不完整"));
        }
        tokio::time::timeout(IO_IDLE_TIMEOUT, file.write_all(&buffer[..n]))
            .await
            .map_err(|_| ProtocolError::new("408 Request Timeout", "写入临时文件超时"))?
            .map_err(|e| ProtocolError::new("500 Internal Server Error", e.to_string()))?;
        hasher.update(&buffer[..n]);
        received += n as u64;
    }
    tokio::time::timeout(IO_IDLE_TIMEOUT, file.flush())
        .await
        .map_err(|_| ProtocolError::new("408 Request Timeout", "刷新临时文件超时"))?
        .map_err(|e| ProtocolError::new("500 Internal Server Error", e.to_string()))?;

    Ok((hex::encode(hasher.finalize()), received))
}

async fn handle_upload_connection<R: Runtime>(
    mut socket: TcpStream,
    app_handle: &AppHandle<R>,
    pool: &sqlx::SqlitePool,
    temp_dir: &std::path::Path,
    metadata: &UploadMetadata,
    expected_token: &str,
) -> Result<bool, String> {
    let temp_file_path = temp_dir.join(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = tokio::time::timeout(CONNECTION_LIFETIME, async {
        let (head, initial_body) = match read_request_head(&mut socket).await {
            Ok(value) => value,
            Err(error) => {
                write_http_response(
                    &mut socket,
                    error.status,
                    "text/plain; charset=utf-8",
                    error.message.as_bytes(),
                )
                .await;
                return Ok(false);
            }
        };

        match parse_upload_request_head(&head, expected_token, metadata.size) {
            Ok(UploadRequestKind::Preflight) => {
                write_http_response(&mut socket, "204 No Content", "text/plain", &[]).await;
                return Ok(false);
            }
            Ok(UploadRequestKind::Upload) => {}
            Err(error) => {
                write_http_response(
                    &mut socket,
                    error.status,
                    "text/plain; charset=utf-8",
                    error.message.as_bytes(),
                )
                .await;
                return Ok(false);
            }
        }

        let (hash, bytes_count) =
            match receive_upload_body(&mut socket, &temp_file_path, &initial_body, metadata.size)
                .await
            {
                Ok(value) => value,
                Err(error) => {
                    let _ = tokio::fs::remove_file(&temp_file_path).await;
                    write_http_response(
                        &mut socket,
                        error.status,
                        "text/plain; charset=utf-8",
                        error.message.as_bytes(),
                    )
                    .await;
                    return Ok(false);
                }
            };

        match finalize_high_speed_upload(
            app_handle,
            pool,
            &temp_file_path,
            metadata,
            hash,
            bytes_count,
        )
        .await
        {
            Ok(data) => {
                let body = serde_json::to_vec(&data).map_err(|e| e.to_string())?;
                write_http_response(&mut socket, "200 OK", "application/json", &body).await;
                Ok(true)
            }
            Err(error) => {
                let _ = tokio::fs::remove_file(&temp_file_path).await;
                write_http_response(
                    &mut socket,
                    "500 Internal Server Error",
                    "text/plain; charset=utf-8",
                    error.as_bytes(),
                )
                .await;
                Ok(false)
            }
        }
    })
    .await;

    match result {
        Ok(value) => value,
        Err(_) => {
            let _ = tokio::fs::remove_file(&temp_file_path).await;
            write_http_response(
                &mut socket,
                "408 Request Timeout",
                "text/plain; charset=utf-8",
                b"upload session timed out",
            )
            .await;
            Ok(false)
        }
    }
}

async fn finalize_high_speed_upload<R: Runtime>(
    app_handle: &AppHandle<R>,
    pool: &sqlx::SqlitePool,
    temp_path: &std::path::Path,
    metadata: &UploadMetadata,
    hash: String,
    size: u64,
) -> Result<crate::vcp_modules::file_manager::AttachmentData, String> {
    let internal_name = crate::vcp_modules::file_manager::safe_storage_extension(&metadata.name)
        .map(|extension| format!("{}.{}", hash, extension))
        .unwrap_or_else(|| hash.clone());

    let dest = crate::vcp_modules::file_manager::get_attachments_root_dir(app_handle)?;
    tokio::fs::create_dir_all(&dest)
        .await
        .map_err(|e| format!("创建附件目录失败: {e}"))?;
    let dest_path = dest.join(internal_name);

    install_verified_upload(temp_path, &dest_path, &hash, size).await?;

    let internal_path = dest_path
        .to_str()
        .ok_or_else(|| "附件目标路径不是有效 UTF-8".to_string())?
        .to_string();

    crate::vcp_modules::file_manager::register_attachment_internal(
        app_handle,
        pool,
        hash,
        metadata.name.clone(),
        metadata.mime.clone(),
        size,
        internal_path,
    )
    .await
}

async fn install_verified_upload(
    temp_path: &std::path::Path,
    dest_path: &std::path::Path,
    expected_hash: &str,
    expected_size: u64,
) -> Result<(), String> {
    if !dest_path.exists() {
        let source = temp_path.to_path_buf();
        let destination = dest_path.to_path_buf();
        let move_result = tokio::task::spawn_blocking(move || {
            crate::vcp_modules::file_manager::safe_rename(source, destination)
        })
        .await
        .map_err(|error| format!("附件移动任务失败: {error}"))?;

        if let Err(error) = move_result {
            // Windows 等平台的无覆盖 rename 可能输给同哈希并发提交；只接受已完整验证的胜者。
            if !dest_path.exists() {
                return Err(format!("附件移动失败: {error}"));
            }
        }
    }

    crate::vcp_modules::file_manager::verify_existing_cas(dest_path, expected_hash, expected_size)
        .await?;

    if temp_path.exists() {
        tokio::fs::remove_file(temp_path)
            .await
            .map_err(|error| format!("清理已验证的重复附件临时文件失败: {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post_head(token: &str, size: u64) -> Vec<u8> {
        format!(
            "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Upload-Token: {token}\r\nContent-Length: {size}\r\nContent-Type: application/octet-stream"
        )
        .into_bytes()
    }
    #[test]
    fn upload_head_requires_matching_token_and_exact_length() {
        assert!(matches!(
            parse_upload_request_head(&post_head("secret", 42), "secret", 42),
            Ok(UploadRequestKind::Upload)
        ));

        let wrong_token = parse_upload_request_head(&post_head("wrong", 42), "secret", 42)
            .expect_err("wrong token must be rejected");
        assert_eq!(wrong_token.status, "401 Unauthorized");

        let wrong_length = parse_upload_request_head(&post_head("secret", 41), "secret", 42)
            .expect_err("mismatched length must be rejected");
        assert_eq!(wrong_length.status, "413 Payload Too Large");
    }

    #[test]
    fn upload_head_rejects_ambiguous_duplicates_but_allows_preflight() {
        let duplicate = b"POST / HTTP/1.1\r\nX-Upload-Token: secret\r\nX-Upload-Token: secret\r\nContent-Length: 42";
        let error = parse_upload_request_head(duplicate, "secret", 42)
            .expect_err("duplicate security headers must be rejected");
        assert_eq!(error.status, "400 Bad Request");

        assert!(matches!(
            parse_upload_request_head(
                b"OPTIONS / HTTP/1.1\r\nOrigin: asset://localhost",
                "secret",
                42
            ),
            Ok(UploadRequestKind::Preflight)
        ));
    }

    #[test]
    fn upload_metadata_has_a_hard_size_boundary() {
        let valid = UploadMetadata {
            name: "sample.bin".to_string(),
            mime: "application/octet-stream".to_string(),
            size: MAX_UPLOAD_BYTES,
        };
        assert!(validate_upload_metadata(&valid).is_ok());

        let oversized = UploadMetadata {
            size: MAX_UPLOAD_BYTES + 1,
            ..valid
        };
        assert!(validate_upload_metadata(&oversized).is_err());
    }

    #[tokio::test]
    async fn existing_cas_must_be_verified_before_upload_temp_is_discarded() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().join("upload.tmp");
        let destination = temp.path().join("cas.bin");
        let expected = b"expected";
        let hash = crate::vcp_modules::infra::utils::calculate_sha256(expected);

        std::fs::write(&source, expected).expect("source");
        std::fs::write(&destination, b"corrupt!").expect("corrupt destination");
        assert!(
            install_verified_upload(&source, &destination, &hash, expected.len() as u64)
                .await
                .is_err()
        );
        assert!(
            source.exists(),
            "unverified upload temp must remain retryable"
        );

        std::fs::write(&destination, expected).expect("valid destination");
        install_verified_upload(&source, &destination, &hash, expected.len() as u64)
            .await
            .expect("matching CAS should be reusable");
        assert!(
            !source.exists(),
            "verified duplicate temp should be removed"
        );
    }

    #[tokio::test]
    async fn newly_installed_upload_is_verified_after_atomic_move() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().join("upload.tmp");
        let destination = temp.path().join("cas.bin");
        let expected = b"new-cas";
        let hash = crate::vcp_modules::infra::utils::calculate_sha256(expected);
        std::fs::write(&source, expected).expect("source");

        install_verified_upload(&source, &destination, &hash, expected.len() as u64)
            .await
            .expect("new CAS should install");
        assert!(!source.exists());
        assert_eq!(std::fs::read(destination).expect("destination"), expected);
    }
}
