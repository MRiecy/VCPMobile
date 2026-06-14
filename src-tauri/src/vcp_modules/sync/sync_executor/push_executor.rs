use crate::vcp_modules::agent_service;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_service;
use crate::vcp_modules::sync_dto::{
    AgentMessageSyncDTO, AgentSyncDTO, GroupMessageSyncDTO, GroupSyncDTO, UserMessageSyncDTO,
};
use sqlx::Row;
use std::collections::HashSet;
use std::sync::Arc;
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::RwLock;

async fn query_avatar_color(pool: &sqlx::SqlitePool, agent_id: &str) -> Option<String> {
    if agent_id.is_empty() {
        return None;
    }

    sqlx::query_scalar::<sqlx::Sqlite, Option<String>>(
        "SELECT dominant_color FROM avatars WHERE owner_id = ? AND owner_type = 'agent' AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .flatten()
}

/// 批量 Push 单 topic 处理结果
pub struct PushBatchResult {
    pub topic_id: String,
    pub success: bool,
    pub error: Option<String>,
}

pub struct PushExecutor;

impl PushExecutor {
    pub async fn push_agent<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        agent_id: &str,
    ) -> Result<(), String> {
        let config =
            agent_service::read_agent_config_internal(app, &app.state(), agent_id, None).await?;
        let dto = AgentSyncDTO::from(&config);

        let idempotency_key = generate_idempotency_key("push", "agent", agent_id);
        let url = format!("{}/api/mobile-sync/upload-entity", http_url);

        let _ = client
            .post(&url)
            .header("x-sync-token", sync_token)
            .header("Authorization", format!("Bearer {}", sync_token))
            .header("x-idempotency-key", idempotency_key)
            .json(&serde_json::json!({ "id": agent_id, "type": "agent", "data": dto }))
            .send()
            .await;

        Ok(())
    }

    pub async fn push_group<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        group_id: &str,
    ) -> Result<(), String> {
        let config =
            group_service::read_group_config(app.clone(), app.state(), group_id.to_string())
                .await?;
        let dto = GroupSyncDTO::from(&config);

        let idempotency_key = generate_idempotency_key("push", "group", group_id);
        let url = format!("{}/api/mobile-sync/upload-entity", http_url);

        let _ = client
            .post(&url)
            .header("x-sync-token", sync_token)
            .header("Authorization", format!("Bearer {}", sync_token))
            .header("x-idempotency-key", idempotency_key)
            .json(&serde_json::json!({ "id": group_id, "type": "group", "data": dto }))
            .send()
            .await;

        Ok(())
    }

    /// 批量 Push 实体 (Agent/Group/Topic)
    pub async fn push_entities_batch<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        items: Vec<serde_json::Value>, // 预先构建好的 [{id, type, data}]
    ) -> Result<(), String> {
        if items.is_empty() {
            return Ok(());
        }

        let url = format!("{}/api/mobile-sync/upload-entities-batch", http_url);
        let response = client
            .post(&url)
            .header("x-sync-token", sync_token)
            .header("Authorization", format!("Bearer {}", sync_token))
            .json(&serde_json::json!({ "items": items }))
            .send()
            .await
            .map_err(|e| format!("Batch push request failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let err_body = response.text().await.unwrap_or_default();
            return Err(format!(
                "Batch push entities failed: HTTP {} body={}",
                status, err_body
            ));
        }

        Ok(())
    }

    pub async fn push_avatar<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        owner_type: &str,
        owner_id: &str,
    ) -> Result<(), String> {
        let db = app.state::<DbState>();

        let row = sqlx::query(
            "SELECT image_data, mime_type FROM avatars WHERE owner_id = ? AND owner_type = ?",
        )
        .bind(owner_id)
        .bind(owner_type)
        .fetch_optional(&db.pool)
        .await
        .map_err(|e| e.to_string())?;

        if let Some(r) = row {
            let image_data: Vec<u8> = r.get("image_data");
            let mime_type: String = r.get("mime_type");

            let url = format!(
                "{}/api/mobile-sync/upload-avatar?id={}&type={}",
                http_url, owner_id, owner_type
            );
            let _ = client
                .post(&url)
                .header("x-sync-token", sync_token)
                .header("Authorization", format!("Bearer {}", sync_token))
                .header("Content-Type", mime_type)
                .body(image_data)
                .send()
                .await;
        }

        Ok(())
    }

    /// 批量 Push — 一次 HTTP 请求推送多个 topic 的消息
    ///
    /// 手机端批量加载消息 → POST /upload-messages-batch (NDJSON)
    /// → 解析响应收集 neededAttachmentHashes → 去重上传附件
    ///
    /// 返回每个 topic 的处理结果。
    pub async fn push_messages_batch<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        topic_ids: &[String],
        uploaded_hashes: Arc<RwLock<HashSet<String>>>,
    ) -> Result<Vec<PushBatchResult>, String> {
        if topic_ids.is_empty() {
            return Ok(Vec::new());
        }

        let db = app.state::<DbState>();

        // 1. 批量查询 topic 的 owner 信息
        let placeholders = topic_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let topic_query = format!(
            "SELECT topic_id, owner_id, owner_type FROM topics WHERE topic_id IN ({})",
            placeholders
        );
        let mut q = sqlx::query(&topic_query);
        for id in topic_ids {
            q = q.bind(id);
        }
        let topic_rows = q.fetch_all(&db.pool).await.map_err(|e| e.to_string())?;

        let mut owner_map: std::collections::HashMap<String, (String, String)> =
            std::collections::HashMap::new();
        for row in &topic_rows {
            let tid: String = row.get("topic_id");
            let oid: String = row.get("owner_id");
            let otype: String = row.get("owner_type");
            owner_map.insert(tid, (oid, otype));
        }

        // 2. 批量加载所有 topic 的消息（一次 SQL）
        let messages_by_topic =
            crate::vcp_modules::message_service::load_multi_topic_messages(&db.pool, topic_ids)
                .await?;

        // 3. 构建批量上传请求 (全流式 NDJSON)
        let mut ndjson_body = String::new();
        for tid in topic_ids {
            let history = messages_by_topic.get(tid).cloned().unwrap_or_default();
            if let Some((_owner_id, owner_type)) = owner_map.get(tid) {
                let dto_messages = build_message_dtos(app, &history, owner_type).await;
                let line = serde_json::json!({
                    "topicId": tid,
                    "messages": dto_messages,
                });
                ndjson_body.push_str(&line.to_string());
                ndjson_body.push('\n');
            }
        }

        let url = format!("{}/api/mobile-sync/upload-messages-batch", http_url);
        let response = client
            .post(&url)
            .header("x-sync-token", sync_token)
            .header("Authorization", format!("Bearer {}", sync_token))
            .header("Content-Type", "application/x-ndjson")
            .body(ndjson_body) // reqwest 接受 String 作为 Body
            .send()
            .await
            .map_err(|e| format!("Batch push request failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let err_body = response.text().await.unwrap_or_default();
            return Err(format!(
                "Batch push messages failed: HTTP {} body={}",
                status, err_body
            ));
        }

        // 4. 解析 NDJSON 响应
        let body = response.text().await.map_err(|e| e.to_string())?;
        let mut results = Vec::new();
        let mut all_needed_hashes: Vec<String> = Vec::new();

        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let data: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(e) => {
                    log::error!(
                        "[PushExecutor] Batch push: NDJSON parse error on line: {:.100}... ({})",
                        line,
                        e
                    );
                    continue;
                }
            };
            let tid = data["topicId"].as_str().unwrap_or("").to_string();
            if tid.is_empty() {
                log::error!(
                    "[PushExecutor] Batch push: NDJSON line missing topicId: {:.100}...",
                    line
                );
                continue;
            }

            let success = data["success"].as_bool().unwrap_or(false);
            let error = data["error"].as_str().map(|s| s.to_string());

            if success {
                // 收集此 topic 需要的附件 hash
                if let Some(needed) = data["neededAttachmentHashes"].as_array() {
                    for h in needed {
                        if let Some(hash) = h.as_str() {
                            all_needed_hashes.push(hash.to_string());
                        }
                    }
                }
            }

            results.push(PushBatchResult {
                topic_id: tid,
                success,
                error,
            });
        }

        // 5. 去重后上传附件（复用现有 3 并发上传逻辑）
        if !all_needed_hashes.is_empty() {
            use std::collections::HashSet;
            let unique_hashes: Vec<String> = {
                let mut seen = HashSet::new();
                all_needed_hashes
                    .into_iter()
                    .filter(|h| seen.insert(h.clone()))
                    .collect()
            };

            // 筛选出尚未上传的 hash
            let hashes_to_upload: Vec<String> = {
                let tracker_guard = uploaded_hashes.read().await;
                unique_hashes
                    .into_iter()
                    .filter(|h| !tracker_guard.contains(h))
                    .collect()
            };

            const MAX_CONCURRENT_UPLOADS: usize = 3;
            for chunk in hashes_to_upload.chunks(MAX_CONCURRENT_UPLOADS) {
                let futures: Vec<_> = chunk
                    .iter()
                    .map(|hash| upload_attachment(app, client, http_url, sync_token, hash))
                    .collect();
                let upload_results = futures_util::future::join_all(futures).await;
                let mut tracker_guard = uploaded_hashes.write().await;
                for (hash, result) in chunk.iter().zip(upload_results) {
                    if result.is_ok() {
                        tracker_guard.insert(hash.clone());
                    }
                }
            }
        }

        let ok_count = results.iter().filter(|r| r.success).count();
        log::info!(
            "[PushExecutor] Batch push completed: {}/{} topics",
            ok_count,
            topic_ids.len()
        );
        Ok(results)
    }
}

fn generate_idempotency_key(action: &str, entity_type: &str, id: &str) -> String {
    let now = chrono::Utc::now().timestamp() / 60;
    let now_str = now.to_string();
    crate::vcp_modules::infra::utils::calculate_sha256_slices(&[
        action.as_bytes(),
        entity_type.as_bytes(),
        id.as_bytes(),
        now_str.as_bytes(),
    ])
}

async fn build_message_dtos<R: Runtime>(
    app: &AppHandle<R>,
    history: &[crate::vcp_modules::chat_manager::ChatMessage],
    owner_type: &str,
) -> Vec<serde_json::Value> {
    let db = app.state::<DbState>();
    let mut results = Vec::new();

    for msg in history {
        let msg_value = if msg.role == "user" {
            let dto = UserMessageSyncDTO::from(msg);
            serde_json::to_value(dto).ok()
        } else if owner_type == "group" {
            let avatar_color =
                query_avatar_color(&db.pool, &msg.agent_id.clone().unwrap_or_default())
                    .await
                    .unwrap_or("#6B7280".to_string());
            let dto = GroupMessageSyncDTO::from_message(msg, avatar_color);
            serde_json::to_value(dto).ok()
        } else {
            let avatar_color =
                query_avatar_color(&db.pool, &msg.agent_id.clone().unwrap_or_default())
                    .await
                    .unwrap_or("#6B7280".to_string());
            let dto = AgentMessageSyncDTO::from_message(msg, avatar_color);
            serde_json::to_value(dto).ok()
        };

        if let Some(v) = msg_value {
            results.push(v);
        }
    }

    results
}

async fn upload_attachment<R: Runtime>(
    app: &AppHandle<R>,
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    hash: &str,
) -> Result<(), String> {
    let db = app.state::<DbState>();

    let row = sqlx::query("SELECT mime_type, internal_path FROM attachments WHERE hash = ?")
        .bind(hash)
        .fetch_optional(&db.pool)
        .await
        .map_err(|e| e.to_string())?;

    if let Some(att_row) = row {
        let mime_type: String = att_row.get("mime_type");
        let internal_path: String = att_row.get("internal_path");

        let name_row =
            sqlx::query("SELECT display_name FROM message_attachments WHERE hash = ? LIMIT 1")
                .bind(hash)
                .fetch_optional(&db.pool)
                .await
                .unwrap_or(None);
        let display_name = name_row
            .map(|r| r.get::<String, _>("display_name"))
            .unwrap_or_else(|| "unnamed".to_string());

        let file_path = internal_path.trim_start_matches("file://");
        let file_data = tokio::fs::read(file_path)
            .await
            .map_err(|e| format!("读取附件失败: {}", e))?;

        let url = format!(
            "{}/api/mobile-sync/upload-attachment?hash={}&type={}&name={}",
            http_url,
            hash,
            urlencoding::encode(&mime_type),
            urlencoding::encode(&display_name)
        );

        let response = client
            .post(&url)
            .header("x-sync-token", sync_token)
            .header("Authorization", format!("Bearer {}", sync_token))
            .header("Content-Type", "application/octet-stream")
            .body(file_data)
            .send()
            .await
            .map_err(|e| format!("上传附件失败: {}", e))?;

        if response.status().is_success() {
            log::debug!("[PushExecutor] Attachment uploaded: {}", hash);
        }
    }

    Ok(())
}
