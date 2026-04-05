use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::path_topology_service::resolve_astbin_path;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use tauri::{Manager, Runtime};
use tauri::http::{Response, header, Request};
use url::Url;
use crate::vcp_modules::lifecycle_manager::LifecycleState;
use crate::vcp_modules::lifecycle_manager::CoreStatus;

pub fn handle_vcp_request<R: Runtime>(ctx: tauri::UriSchemeContext<'_, R>, request: Request<Vec<u8>>) -> Response<std::borrow::Cow<'static, [u8]>> {
    let handle = ctx.app_handle().clone();
    let uri_str = request.uri().to_string();
    let url_res = Url::parse(&uri_str);
    if url_res.is_err() {
        return Response::builder().status(400).body(Vec::new().into()).unwrap();
    }
    let url = url_res.unwrap();
    
    // Path check: /api/messages
    if url.path() != "/api/messages" {
        return Response::builder().status(404).body(Vec::new().into()).unwrap();
    }

    // Query params
    let query_pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    let topic_id = query_pairs.get("topic_id").cloned();
    let item_id = query_pairs.get("item_id").cloned();
    let limit: usize = query_pairs.get("limit").and_then(|s| s.parse().ok()).unwrap_or(20);
    let offset: usize = query_pairs.get("offset").and_then(|s| s.parse().ok()).unwrap_or(0);

    if topic_id.is_none() || item_id.is_none() {
        return Response::builder().status(400).body(Vec::new().into()).unwrap();
    }

    let topic_id = topic_id.unwrap();
    let item_id = item_id.unwrap();

    // Check if core is ready
    let lifecycle = handle.state::<LifecycleState>();
    let is_ready = tauri::async_runtime::block_on(async {
        *lifecycle.status.read().await == CoreStatus::Ready
    });

    if !is_ready {
        return Response::builder().status(503).body(Vec::new().into()).unwrap();
    }

    let db_state = handle.state::<DbState>();
    let pool = db_state.pool.clone();
    let handle_clone = handle.clone();

    // Since we need to return synchronously but DB is async, 
    // block_on is used here. In real scenarios consider asynchronous protocol.
    let response = tauri::async_runtime::block_on(async move {
        // Query message indices
        // We want the most recent 'limit' messages starting from 'offset' back in time.
        // Then we return them in chronological (ASC) order.
        let pointers_res: Result<Vec<sqlx::sqlite::SqliteRow>, sqlx::Error> = sqlx::query(
                "SELECT render_byte_offset, render_byte_length 
                FROM message_index 
                WHERE topic_id = ? AND is_deleted = 0 
                ORDER BY created_at DESC 
                LIMIT ? OFFSET ?"
            )
            .bind(&topic_id)
            .bind(limit as i32)
            .bind(offset as i32)
            .fetch_all(&pool)
            .await;

        match pointers_res {
            Ok(mut rows) => {
                // Reverse to get ASC order
                rows.reverse();

                let astbin_path = resolve_astbin_path(&handle_clone, &item_id, &topic_id);
                if !astbin_path.exists() {
                    return Response::builder()
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(b"[]".to_vec().into())
                        .unwrap();
                }

                let mut file = match File::open(&astbin_path) {
                    Ok(f) => f,
                    Err(_) => {
                        return Response::builder().status(500).body(Vec::new().into()).unwrap();
                    }
                };

                let mut result_json = Vec::new();
                result_json.push(b'[');

                for (i, row) in rows.iter().enumerate() {
                    use sqlx::Row;
                    let r_offset: Option<i32> = row.try_get("render_byte_offset").ok();
                    let r_length: Option<i32> = row.try_get("render_byte_length").ok();

                    if let (Some(offset), Some(length)) = (r_offset, r_length) {
                        if i > 0 {
                            result_json.push(b',');
                        }
                        
                        let mut buffer = vec![0u8; length as usize];
                        if file.seek(SeekFrom::Start(offset as u64)).is_ok() && file.read_exact(&mut buffer).is_ok() {
                            result_json.extend_from_slice(&buffer);
                        } else {
                            result_json.extend_from_slice(b"[]");
                        }
                    }
                }
                result_json.push(b']');

                Response::builder()
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                    .body(result_json.into())
                    .unwrap()
            }
            Err(e) => {
                eprintln!("[VCPProtocol] DB error: {}", e);
                Response::builder().status(500).body(Vec::new().into()).unwrap()
            }
        }
    });

    response
}
