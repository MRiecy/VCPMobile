use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::lifecycle_manager::CoreStatus;
use crate::vcp_modules::lifecycle_manager::LifecycleState;
use tauri::http::{header, Request, Response};
use tauri::{Manager, Runtime};
use url::Url;

pub fn handle_vcp_request<R: Runtime>(
    ctx: tauri::UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
) -> Response<std::borrow::Cow<'static, [u8]>> {
    let handle = ctx.app_handle().clone();
    let uri_str = request.uri().to_string();
    let url_res = Url::parse(&uri_str);
    if url_res.is_err() {
        return Response::builder()
            .status(400)
            .body(Vec::new().into())
            .unwrap();
    }
    let url = url_res.unwrap();

    // Path check: /api/messages
    if url.path() != "/api/messages" {
        return Response::builder()
            .status(404)
            .body(Vec::new().into())
            .unwrap();
    }

    // Query params
    let query_pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    let topic_id = query_pairs.get("topic_id").cloned();
    let owner_id = query_pairs.get("owner_id").cloned();
    let owner_type = query_pairs.get("owner_type").cloned();
    let limit: usize = query_pairs
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);
    let offset: usize = query_pairs
        .get("offset")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    if topic_id.is_none() || owner_id.is_none() || owner_type.is_none() {
        return Response::builder()
            .status(400)
            .body(Vec::new().into())
            .unwrap();
    }

    let topic_id = topic_id.unwrap();
    let _owner_id = owner_id.unwrap();
    let _owner_type = owner_type.unwrap();

    // Check if core is ready
    let lifecycle = handle.state::<LifecycleState>();
    let is_ready = tauri::async_runtime::block_on(async {
        *lifecycle.status.read().await == CoreStatus::Ready
    });

    if !is_ready {
        return Response::builder()
            .status(503)
            .body(Vec::new().into())
            .unwrap();
    }

    let db_state = handle.state::<DbState>();
    let pool = db_state.pool.clone();
    let _handle_clone = handle.clone();

    // Since we need to return synchronously but DB is async,
    // block_on is used here. In real scenarios consider asynchronous protocol.
    let response = tauri::async_runtime::block_on(async move {
        // Query message indices and render content directly from DB
        let rows_res: Result<Vec<sqlx::sqlite::SqliteRow>, sqlx::Error> = sqlx::query(
            "SELECT render_content 
                FROM messages 
                WHERE topic_id = ? AND deleted_at IS NULL 
                ORDER BY timestamp DESC 
                LIMIT ? OFFSET ?",
        )
        .bind(&topic_id)
        .bind(limit as i32)
        .bind(offset as i32)
        .fetch_all(&pool)
        .await;

        match rows_res {
            Ok(mut rows) => {
                // Reverse to get ASC order (chronological)
                rows.reverse();

                let mut result_json = Vec::new();
                result_json.push(b'[');

                use sqlx::Row;
                for (i, row) in rows.iter().enumerate() {
                    let render_content: Option<Vec<u8>> = row.get("render_content");

                    if let Some(content) = render_content {
                        if i > 0 {
                            result_json.push(b',');
                        }
                        result_json.extend_from_slice(&content);
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
                Response::builder()
                    .status(500)
                    .body(Vec::new().into())
                    .unwrap()
            }
        }
    });

    response
}
