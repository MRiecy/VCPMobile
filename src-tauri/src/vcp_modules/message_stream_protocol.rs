use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::lifecycle_manager::CoreStatus;
use crate::vcp_modules::lifecycle_manager::LifecycleState;
use sqlx::Row;
use tauri::http::{header, Request, Response};
use tauri::{Manager, Runtime};
use url::Url;

pub fn handle_vcp_request<R: Runtime>(
    ctx: tauri::UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    let handle = ctx.app_handle().clone();
    let uri_str = request.uri().to_string();

    tauri::async_runtime::spawn(async move {
        let url_res = Url::parse(&uri_str);
        if url_res.is_err() {
            responder.respond(Response::builder().status(400).body(Vec::new()).unwrap());
            return;
        }
        let url = url_res.unwrap();

        // 更加鲁棒的路径提取：兼容 vcp://api/messages 和 https://vcp.tauri.localhost/api/messages
        let path = url.path();
        let is_valid_path =
            path == "/api/messages" || (path == "/messages" && url.host_str() == Some("api"));

        if !is_valid_path {
            responder.respond(Response::builder().status(404).body(Vec::new()).unwrap());
            return;
        }

        // Query params
        let query_pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        let topic_id = query_pairs.get("topic_id").cloned();
        let _owner_id = query_pairs.get("owner_id").cloned();
        let _owner_type = query_pairs.get("owner_type").cloned();
        let msg_id = query_pairs.get("msg_id").cloned();
        let fetch_raw = query_pairs
            .get("fetch_raw")
            .map(|s| s == "true")
            .unwrap_or(false);
        let limit: usize = query_pairs
            .get("limit")
            .and_then(|s| s.parse().ok())
            .unwrap_or(20);
        let offset: usize = query_pairs
            .get("offset")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        if (fetch_raw && msg_id.is_none()) || (!fetch_raw && topic_id.is_none()) {
            responder.respond(Response::builder().status(400).body(Vec::new()).unwrap());
            return;
        }

        // Check if core is ready
        let lifecycle = handle.state::<LifecycleState>();
        let is_ready = *lifecycle.status.read().await == CoreStatus::Ready;

        if !is_ready {
            responder.respond(Response::builder().status(503).body(Vec::new()).unwrap());
            return;
        }

        let db_state = handle.state::<DbState>();
        let pool = &db_state.pool;

        if fetch_raw {
            if let Some(mid) = msg_id {
                let row_res = sqlx::query("SELECT content FROM messages WHERE msg_id = ?")
                    .bind(&mid)
                    .fetch_optional(pool)
                    .await;
                match row_res {
                    Ok(Some(row)) => {
                        let content: String = row.get(0);
                        responder.respond(
                            Response::builder()
                                .status(200)
                                .header("Content-Type", "text/plain")
                                .body(content.into_bytes())
                                .unwrap(),
                        );
                    }
                    Ok(None) => {
                        responder.respond(Response::builder().status(404).body(Vec::new()).unwrap())
                    }
                    Err(_) => {
                        responder.respond(Response::builder().status(500).body(Vec::new()).unwrap())
                    }
                }
            }
        } else {
            let topic_id = topic_id.unwrap();
            // Query message indices and render content directly from DB
            let rows_res = sqlx::query(
                "SELECT msg_id, role, name, agent_id, timestamp, is_thinking, is_group_message, group_id, render_content 
                 FROM messages 
                 WHERE topic_id = ? AND deleted_at IS NULL 
                 ORDER BY timestamp DESC 
                 LIMIT ? OFFSET ?",
            )
            .bind(&topic_id)
            .bind(limit as i32)
            .bind(offset as i32)
            .fetch_all(pool)
            .await;

            match rows_res {
                Ok(mut rows) => {
                    // Reverse to get ASC order (chronological)
                    rows.reverse();

                    let mut messages = Vec::new();
                    use sqlx::Row;
                    for row in rows.iter() {
                        let mut msg_map = serde_json::Map::new();

                        let msg_id: String = row.get("msg_id");
                        msg_map.insert("id".to_string(), serde_json::Value::String(msg_id));

                        let role: String = row.get("role");
                        msg_map.insert("role".to_string(), serde_json::Value::String(role));

                        let name: Option<String> = row.get("name");
                        if let Some(n) = name {
                            msg_map.insert("name".to_string(), serde_json::Value::String(n));
                        }

                        let agent_id: Option<String> = row.get("agent_id");
                        if let Some(aid) = agent_id {
                            msg_map.insert("agent_id".to_string(), serde_json::Value::String(aid));
                        }

                        let timestamp: i64 = row.get("timestamp");
                        msg_map.insert(
                            "timestamp".to_string(),
                            serde_json::Value::Number(timestamp.into()),
                        );

                        let is_thinking: Option<i32> = row.get("is_thinking");
                        if let Some(t) = is_thinking {
                            msg_map
                                .insert("is_thinking".to_string(), serde_json::Value::Bool(t != 0));
                        }

                        let is_group_message: i32 = row.get("is_group_message");
                        msg_map.insert(
                            "is_group_message".to_string(),
                            serde_json::Value::Bool(is_group_message != 0),
                        );

                        let group_id: Option<String> = row.get("group_id");
                        if let Some(gid) = group_id {
                            msg_map.insert("group_id".to_string(), serde_json::Value::String(gid));
                        }

                        let render_content: Option<Vec<u8>> = row.get("render_content");
                        if let Some(content_bytes) = render_content {
                            if let Ok(blocks_val) =
                                serde_json::from_slice::<serde_json::Value>(&content_bytes)
                            {
                                msg_map.insert("blocks".to_string(), blocks_val);
                            }
                        }

                        messages.push(serde_json::Value::Object(msg_map));
                    }

                    let res_bytes = serde_json::to_vec(&messages).unwrap();

                    responder.respond(
                        Response::builder()
                            .header(header::CONTENT_TYPE, "application/json")
                            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                            .body(res_bytes)
                            .unwrap(),
                    );
                }
                Err(e) => {
                    eprintln!("[VCPProtocol] DB error: {}", e);
                    responder.respond(Response::builder().status(500).body(Vec::new()).unwrap());
                }
            }
        }
    });
}
