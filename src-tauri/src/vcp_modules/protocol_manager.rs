use crate::vcp_modules::avatar_service::extract_dominant_color_from_bytes;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::message_stream_protocol::handle_vcp_request;
use sha2::{Digest, Sha256};
use std::fs;
use tauri::http::{Request, Response};
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::vcp_host::native_portal::PortalState;

/// 协议指挥部：统一管理所有 VCP 私有协议
pub fn register_vcp_protocols<R: Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder
        // 1. 同步协议（已迁移为异步处理）：vcp://api/messages
        .register_asynchronous_uri_scheme_protocol("vcp", |ctx, request, responder| {
            handle_vcp_request(ctx, request, responder);
        })
        // 2. 异步头像协议：vcp-avatar://agent/{id}
        .register_asynchronous_uri_scheme_protocol("vcp-avatar", |ctx, request, responder| {
            let app_handle = ctx.app_handle().clone();
            handle_avatar_protocol(
                app_handle,
                request,
                Box::new(move |res| responder.respond(res)),
            );
        })
        // 3. 全屏门户协议：vcp-portal://render?id={id}
        .register_asynchronous_uri_scheme_protocol("vcp-portal", |ctx, request, responder| {
            let app_handle = ctx.app_handle().clone();
            handle_vcp_portal_protocol(
                app_handle,
                request,
                Box::new(move |res| responder.respond(res)),
            );
        })
}

fn handle_vcp_portal_protocol<R: Runtime>(
    app_handle: AppHandle<R>,
    request: Request<Vec<u8>>,
    responder: Box<dyn FnOnce(Response<Vec<u8>>) + Send>,
) {
    let uri = request.uri().to_string();
    log::info!("[PortalProtocol] Incoming URI: {}", uri);

    // 解析出 id 参数
    let parsed_url =
        url::Url::parse(&uri).unwrap_or_else(|_| url::Url::parse("vcp-portal://error").unwrap());
    let id = parsed_url
        .query_pairs()
        .find(|(k, _)| k == "id")
        .map(|(_, v)| v.into_owned());

    if let Some(id_val) = id {
        let portal_state = app_handle.state::<PortalState>();
        // 从缓存中取出（取出即删除，保证仅用一次，防止内存泄漏）
        if let Some((_, html_content)) = portal_state.contents.remove(&id_val) {
            log::info!(
                "[PortalProtocol] Found content for id: {}, size: {}",
                id_val,
                html_content.len()
            );
            let response = Response::builder()
                .header("Content-Type", "text/html; charset=utf-8")
                .header("Access-Control-Allow-Origin", "*")
                .body(html_content.into_bytes())
                .unwrap();
            responder(response);
            return;
        } else {
            log::warn!("[PortalProtocol] Content not found for id: {}", id_val);
        }
    } else {
        log::warn!("[PortalProtocol] No id parameter found in URI: {}", uri);
    }

    let error_html =
        "<html><body><h1>Error: Content not found or expired</h1></body></html>".to_string();
    let response = Response::builder()
        .status(404)
        .header("Content-Type", "text/html; charset=utf-8")
        .body(error_html.into_bytes())
        .unwrap();
    responder(response);
}

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
#[tauri::command]
pub async fn prepare_vcp_upload<R: Runtime>(
    app_handle: AppHandle<R>,
    db_state: State<'_, DbState>,
    metadata: UploadMetadata,
) -> Result<UploadEndpoint, String> {
    // 1. 监听本地随机端口
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let port = listener.local_addr().unwrap().port();
    let token = uuid::Uuid::new_v4().to_string();

    let url = format!("http://127.0.0.1:{}", port);
    let token_clone = token.clone();
    let pool = db_state.pool.clone();

    tauri::async_runtime::spawn(async move {
        let mut upload_finished = false;
        let timeout = std::time::Duration::from_secs(20);
        let start_time = std::time::Instant::now();

        let mut temp_dir = app_handle.path().app_cache_dir().unwrap();
        temp_dir.push("uploads");
        if !temp_dir.exists() {
            let _ = fs::create_dir_all(&temp_dir);
        }

        while !upload_finished && start_time.elapsed() < timeout {
            let accept_res =
                tokio::time::timeout(std::time::Duration::from_millis(500), listener.accept())
                    .await;

            let (mut socket, _addr) = match accept_res {
                Ok(Ok(conn)) => conn,
                _ => continue,
            };

            let mut buffer = [0u8; 65536];
            let mut body_started = false;
            let mut header_data = Vec::with_capacity(4096);

            let session_id = uuid::Uuid::new_v4().to_string();
            let temp_file_path = temp_dir.join(format!("{}.tmp", session_id));

            let mut bytes_count = 0u64;
            let mut hasher = Sha256::new();
            let mut is_options = false;
            let mut file: Option<tokio::fs::File> = None;

            loop {
                let n = match socket.read(&mut buffer).await {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => break,
                };

                let data = if !body_started {
                    header_data.extend_from_slice(&buffer[..n]);
                    if let Some(pos) = header_data.windows(4).position(|w| w == b"\r\n\r\n") {
                        body_started = true;
                        let header_str = String::from_utf8_lossy(&header_data[..pos]);

                        if header_str.starts_with("OPTIONS") {
                            is_options = true;
                            let resp = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\nAccess-Control-Allow-Headers: *\r\nAccess-Control-Max-Age: 86400\r\nConnection: close\r\n\r\n";
                            socket.write_all(resp.as_bytes()).await.ok();
                            break;
                        }

                        file = tokio::fs::File::create(&temp_file_path).await.ok();

                        let header_len_in_current_buffer = if header_data.len() > n {
                            let consumed_before = header_data.len() - n;
                            (pos + 4).saturating_sub(consumed_before)
                        } else {
                            0
                        };

                        if header_len_in_current_buffer < n {
                            &buffer[header_len_in_current_buffer..n]
                        } else {
                            &[]
                        }
                    } else {
                        &[]
                    }
                } else {
                    &buffer[..n]
                };

                if !data.is_empty() && !is_options {
                    if let Some(ref mut f) = file {
                        let _ = f.write_all(data).await;
                    }
                    hasher.update(data);
                    bytes_count += data.len() as u64;

                    if bytes_count >= metadata.size {
                        break;
                    }
                }
            }

            if let Some(mut f) = file.take() {
                let _ = f.flush().await;
                drop(f);
            }

            if !is_options && bytes_count > 0 {
                if bytes_count < metadata.size {
                    let _ = fs::remove_file(&temp_file_path);
                    let response =
                        "HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\nIncomplete Data";
                    let _ = socket.write_all(response.as_bytes()).await;
                } else {
                    let hash = hex::encode(hasher.finalize());
                    let final_data_res = finalize_high_speed_upload(
                        &app_handle,
                        &pool,
                        &temp_file_path,
                        &metadata,
                        hash,
                        bytes_count,
                    )
                    .await;

                    let (status, body) = match final_data_res {
                        Ok(data) => (200, serde_json::to_vec(&data).unwrap_or_default()),
                        Err(e) => (500, e.into_bytes()),
                    };

                    let response = format!(
                        "HTTP/1.1 {} OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        status, body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.write_all(&body).await;

                    upload_finished = true;
                }
            }
        }
    });

    Ok(UploadEndpoint {
        url,
        token: token_clone,
    })
}

async fn finalize_high_speed_upload<R: Runtime>(
    app_handle: &AppHandle<R>,
    pool: &sqlx::SqlitePool,
    temp_path: &std::path::PathBuf,
    metadata: &UploadMetadata,
    hash: String,
    size: u64,
) -> Result<crate::vcp_modules::file_manager::AttachmentData, String> {
    let ext = std::path::Path::new(&metadata.name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let internal_name = if ext.is_empty() {
        hash.clone()
    } else {
        format!("{}.{}", hash, ext)
    };

    let mut dest = app_handle.path().app_config_dir().unwrap();
    dest.push("data");
    dest.push("attachments");
    if !dest.exists() {
        fs::create_dir_all(&dest).ok();
    }
    let dest_path = dest.join(internal_name);

    if !dest_path.exists() {
        fs::rename(temp_path, &dest_path).map_err(|e| e.to_string())?;
    } else {
        fs::remove_file(temp_path).ok();
    }

    crate::vcp_modules::file_manager::register_attachment_internal(
        app_handle,
        pool,
        hash,
        metadata.name.clone(),
        metadata.mime.clone(),
        size,
        dest_path.to_str().unwrap().to_string(),
    )
    .await
}

/// 处理头像读取协议 (vcp-avatar://agent/{id})
fn handle_avatar_protocol<R: Runtime>(
    app_handle: AppHandle<R>,
    request: Request<Vec<u8>>,
    responder: Box<dyn FnOnce(Response<Vec<u8>>) + Send>,
) {
    let uri = request.uri().to_string();

    // 添加更详细的调试日志
    log::info!("=== [AvatarProtocol] DEBUG START ===");
    log::info!("[AvatarProtocol] Incoming URI: {}", uri);
    log::info!("[AvatarProtocol] Request method: {:?}", request.method());
    log::info!("[AvatarProtocol] Request headers: {:?}", request.headers());

    tauri::async_runtime::spawn(async move {
        let db_state = app_handle.state::<DbState>();
        let pool = &db_state.pool;

        // 使用 log::info! 以确保日志在移动端控制台可见
        log::info!("[AvatarProtocol] Database pool acquired");

        // 更加鲁棒的路径提取：
        // 1. 原始格式: vcp-avatar://user/avatar
        // 2. Android 映射格式: https://vcp-avatar.tauri.localhost/user/avatar
        // 3. 某些 Webview 可能只传路径: /user/avatar
        let path_part = if uri.starts_with("vcp-avatar://") {
            uri.strip_prefix("vcp-avatar://").unwrap_or("")
        } else if uri.contains("vcp-avatar.tauri.localhost/") {
            uri.split("vcp-avatar.tauri.localhost/")
                .last()
                .unwrap_or("")
        } else if uri.contains("/vcp-avatar/") {
            uri.split("/vcp-avatar/").last().unwrap_or("")
        } else {
            uri.as_str()
        };

        // 移除查询参数并处理可能的双斜杠或前缀斜杠
        let path = path_part
            .split('?')
            .next()
            .unwrap_or("")
            .trim_start_matches('/');
        log::info!("[AvatarProtocol] Cleaned path: {}", path);
        let parts: Vec<&str> = path.split('/').collect();

        if parts.len() >= 2 {
            let owner_type = parts[0];
            let owner_id = parts[1];

            log::info!(
                "[AvatarProtocol] Parsed: type={}, id={}",
                owner_type,
                owner_id
            );

            // 首先检查数据库中是否有记录
            let count_res: Result<Option<sqlx::sqlite::SqliteRow>, sqlx::Error> = sqlx::query(
                "SELECT COUNT(*) as count FROM avatars WHERE owner_type = ? AND owner_id = ?",
            )
            .bind(owner_type)
            .bind(owner_id)
            .fetch_optional(pool)
            .await;

            match count_res {
                Ok(Some(count_row)) => {
                    use sqlx::Row;
                    let count: i64 = count_row.get("count");
                    log::info!(
                        "[AvatarProtocol] Found {} avatar records for {}/{}",
                        count,
                        owner_type,
                        owner_id
                    );
                }
                Ok(None) => {
                    log::info!("[AvatarProtocol] No count query result");
                }
                Err(e) => {
                    log::error!("[AvatarProtocol] Count query error: {}", e);
                }
            }

            let row_res: Result<Option<sqlx::sqlite::SqliteRow>, sqlx::Error> = sqlx::query(
                "SELECT mime_type, image_data, dominant_color FROM avatars WHERE owner_type = ? AND owner_id = ?"
            )
            .bind(owner_type)
            .bind(owner_id)
            .fetch_optional(pool)
            .await;

            match row_res {
                Ok(Some(row)) => {
                    use sqlx::Row;
                    let mime: String = row.get("mime_type");
                    let data: Vec<u8> = row.get("image_data");
                    let color: Option<String> = row.get("dominant_color");

                    log::info!(
                        "[AvatarProtocol] Found data: size={}, mime={}",
                        data.len(),
                        mime
                    );

                    let final_color = match color {
                        Some(c) => c,
                        None => {
                            let calculated = extract_dominant_color_from_bytes(&data)
                                .unwrap_or_else(|_| "#808080".to_string());
                            let pool_clone = pool.clone();
                            let owner_type_clone = owner_type.to_string();
                            let owner_id_clone = owner_id.to_string();
                            let calculated_clone = calculated.clone();
                            tauri::async_runtime::spawn(async move {
                                let _ = sqlx::query("UPDATE avatars SET dominant_color = ? WHERE owner_type = ? AND owner_id = ?")
                                    .bind(calculated_clone)
                                    .bind(owner_type_clone)
                                    .bind(owner_id_clone)
                                    .execute(&pool_clone)
                                    .await;
                            });
                            calculated
                        }
                    };

                    log::info!(
                        "[AvatarProtocol] Responding with: content-type={}, color={}",
                        mime,
                        final_color
                    );

                    let response = Response::builder()
                        .header("Content-Type", mime)
                        .header("Access-Control-Allow-Origin", "*")
                        .header("Cache-Control", "no-cache") // 调试期间禁用协议级缓存
                        .header("X-Avatar-Color", final_color)
                        .body(data)
                        .unwrap();
                    responder(response);
                    log::info!("[AvatarProtocol] Response sent successfully");
                    return;
                }
                Ok(None) => {
                    log::warn!(
                        "[AvatarProtocol] No record found in DB for {}/{}",
                        owner_type,
                        owner_id
                    );
                }
                Err(e) => {
                    log::error!("[AvatarProtocol] DB Error: {}", e);
                }
            }
        } else {
            log::warn!("[AvatarProtocol] Invalid path format: {:?}", parts);
        }

        log::warn!("[AvatarProtocol] 404 for URI: {}", uri);
        log::info!("=== [AvatarProtocol] DEBUG END ===");
        responder(Response::builder().status(404).body(Vec::new()).unwrap());
    });
}
