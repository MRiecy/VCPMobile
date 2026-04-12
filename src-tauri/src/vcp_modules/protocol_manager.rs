use tauri::{AppHandle, Manager, Runtime, State};
use tauri::http::{Response, Request};
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::message_stream_protocol::handle_vcp_request;
use tokio::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use sha2::{Digest, Sha256};
use std::time::UNIX_EPOCH;
use std::fs;
use std::io::Write;

/// 协议指挥部：统一管理所有 VCP 私有协议
pub fn register_vcp_protocols<R: Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder
        // 1. 同步协议：vcp://api/messages
        .register_uri_scheme_protocol("vcp", |ctx, request| {
            handle_vcp_request(ctx, request)
        })
        // 2. 异步头像协议：vcp-avatar://agent/{id}
        .register_asynchronous_uri_scheme_protocol("vcp-avatar", |ctx, request, responder| {
            let app_handle = ctx.app_handle().clone();
            handle_avatar_protocol(app_handle, request, Box::new(move |res| responder.respond(res)));
        })
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
    // 回到 127.0.0.1 绑定，降低被系统误判为恶意后台服务的风险
    let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|e| e.to_string())?;
    let port = listener.local_addr().unwrap().port();
    let token = uuid::Uuid::new_v4().to_string();
    
    // 依然使用 127.0.0.1 返回给前端
    let url = format!("http://127.0.0.1:{}", port);
    let token_clone = token.clone();
    let pool = db_state.pool.clone();

    // 2. 异步启动链路监听任务 (处理 OPTIONS 和 POST)
    log::info!("[ProtocolManager] High-speed link listening on port {}", port);
    tauri::async_runtime::spawn(async move {
        log::info!("[ProtocolManager] Spawned upload task for {}", metadata.name);
        let mut upload_finished = false;
        // 调大全局超时到 20 秒，给用户充分的选择和思考时间
        let timeout = std::time::Duration::from_secs(20); 
        let start_time = std::time::Instant::now();

        // 预创建目录
        let mut temp_dir = app_handle.path().app_cache_dir().unwrap();
        temp_dir.push("uploads");
        if !temp_dir.exists() { let _ = fs::create_dir_all(&temp_dir); }

        while !upload_finished && start_time.elapsed() < timeout {
            let accept_res = tokio::time::timeout(
                std::time::Duration::from_millis(500), 
                listener.accept()
            ).await;

            let (mut socket, addr) = match accept_res {
                Ok(Ok(conn)) => conn,
                _ => continue,
            };

            log::info!("[ProtocolManager] Accepted connection from {}", addr);
            let mut buffer = [0u8; 65536]; // 提升缓冲区到 64KB
            let mut body_started = false;
            let mut header_data = Vec::with_capacity(4096);
            
            let session_id = uuid::Uuid::new_v4().to_string();
            let temp_file_path = temp_dir.join(format!("{}.tmp", session_id));

            let mut bytes_count = 0u64;
            let mut hasher = Sha256::new();
            let mut is_options = false;
            
            // 使用 tokio 的异步文件处理，防止阻塞执行线程
            let mut file: Option<tokio::fs::File> = None;

            // 简单处理 HTTP 报文
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
                            log::info!("[ProtocolManager] Detected CORS Preflight (OPTIONS)");
                            is_options = true;
                            let resp = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\nAccess-Control-Allow-Headers: *\r\nAccess-Control-Max-Age: 86400\r\nConnection: close\r\n\r\n";
                            socket.write_all(resp.as_bytes()).await.ok();
                            break; 
                        }
                        
                        // POST 请求：在此处创建文件
                        log::info!("[ProtocolManager] Data stream started, creating temp file: {:?}", temp_file_path);
                        file = tokio::fs::File::create(&temp_file_path).await.ok();

                        let header_len_in_current_buffer = if header_data.len() > n {
                             let consumed_before = header_data.len() - n;
                             if pos + 4 > consumed_before { pos + 4 - consumed_before } else { 0 }
                        } else {
                            pos + 4
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

                    // 核心修复：一旦字节收够，立即退出，防止被 Keep-Alive 挂起
                    if bytes_count >= metadata.size {
                        log::info!("[ProtocolManager] Stream complete: {}/{}", bytes_count, metadata.size);
                        break;
                    }
                }
            }

            // 必须在 drop(file) 之前，确保文件已刷盘
            if let Some(mut f) = file.take() { 
                let _ = f.flush().await;
                drop(f); 
            }

            if !is_options && bytes_count > 0 {
                // 必须校验数据完整性，防止保存中断产生的残缺文件
                if bytes_count < metadata.size {
                    log::warn!("[ProtocolManager] Upload interrupted or incomplete: {}/{} bytes", bytes_count, metadata.size);
                    let _ = fs::remove_file(&temp_file_path);
                    
                    let response = "HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\nIncomplete Data";
                    let _ = socket.write_all(response.as_bytes()).await;
                } else {
                    log::info!("[ProtocolManager] Finalizing upload, total: {} bytes", bytes_count);
                    let hash = hex::encode(hasher.finalize());
                    let final_data_res = finalize_high_speed_upload(&app_handle, &pool, &temp_file_path, &metadata, hash, bytes_count).await;
                    
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
                    log::info!("[ProtocolManager] Upload task finished successfully");
                }
            }
        }
    });

    Ok(UploadEndpoint { url, token: token_clone })
}

async fn finalize_high_speed_upload<R: Runtime>(
    app_handle: &AppHandle<R>,
    pool: &sqlx::SqlitePool,
    temp_path: &std::path::PathBuf,
    metadata: &UploadMetadata,
    hash: String,
    size: u64,
) -> Result<crate::vcp_modules::file_manager::AttachmentData, String> {
    let ext = std::path::Path::new(&metadata.name).extension().and_then(|e| e.to_str()).unwrap_or("");
    let internal_name = if ext.is_empty() { hash.clone() } else { format!("{}.{}", hash, ext) };
    
    let mut dest = app_handle.path().app_config_dir().unwrap();
    dest.push("data"); dest.push("attachments");
    if !dest.exists() { fs::create_dir_all(&dest).ok(); }
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
    ).await
}

/// 处理头像读取协议 (vcp-avatar://agent/{id})
fn handle_avatar_protocol<R: Runtime>(
    app_handle: AppHandle<R>,
    request: Request<Vec<u8>>,
    responder: Box<dyn FnOnce(Response<Vec<u8>>) + Send>,
) {
    let uri = request.uri().to_string();

    tauri::async_runtime::spawn(async move {
        let db_state = app_handle.state::<DbState>();
        let pool = &db_state.pool;

        let path = uri.strip_prefix("vcp-avatar://").unwrap_or("");
        let parts: Vec<&str> = path.split('/').collect();

        if parts.len() >= 2 {
            let owner_type = parts[0];
            let owner_id = if owner_type == "user" && parts[1] == "default" {
                "default_user"
            } else {
                parts[1]
            };

            let row_res: Result<Option<sqlx::sqlite::SqliteRow>, sqlx::Error> = sqlx::query(
                "SELECT mime_type, image_data, dominant_color FROM avatars WHERE owner_type = ? AND owner_id = ?"
            )
            .bind(owner_type)
            .bind(owner_id)
            .fetch_optional(pool)
            .await;

            if let Ok(Some(row)) = row_res {
                use sqlx::Row;
                let mime: String = row.get("mime_type");
                let data: Vec<u8> = row.get("image_data");
                let color: Option<String> = row.get("dominant_color");

                let mut builder = Response::builder()
                    .header("Content-Type", mime)
                    .header("Access-Control-Allow-Origin", "*")
                    .header("Cache-Control", "max-age=3600");

                if let Some(c) = color {
                    builder = builder.header("X-Avatar-Color", c);
                }

                let response = builder.body(data).unwrap();
                responder(response);
                return;
            }
        }

        // Fallback: 404
        responder(Response::builder().status(404).body(Vec::new()).unwrap());
    });
}

fn handle_upload_trigger<R: Runtime>(_app_handle: AppHandle<R>, _request: Request<Vec<u8>>, responder: Box<dyn FnOnce(Response<Vec<u8>>) + Send>) {
    responder(Response::builder().status(200).body("Ready".as_bytes().to_vec()).unwrap());
}
