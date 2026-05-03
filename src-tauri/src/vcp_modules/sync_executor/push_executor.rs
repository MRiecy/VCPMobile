use crate::vcp_modules::agent_service;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_service;
use crate::vcp_modules::sync_dto::{
    AgentMessageSyncDTO, AgentSyncDTO, AgentTopicSyncDTO, GroupMessageSyncDTO, GroupSyncDTO,
    GroupTopicSyncDTO, UserMessageSyncDTO,
};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::collections::HashSet;
use std::sync::Arc;
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::RwLock;

async fn query_avatar_color(pool: &sqlx::SqlitePool, agent_id: &str) -> String {
    if agent_id.is_empty() {
        return "rgb(128, 128, 128)".to_string();
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
    .unwrap_or_else(|| "rgb(128, 128, 128)".to_string())
}

async fn query_avatar_color_cached(
    pool: &sqlx::SqlitePool,
    cache: &dashmap::DashMap<String, String>,
    agent_id: &str,
) -> String {
    if agent_id.is_empty() {
        return "rgb(128, 128, 128)".to_string();
    }
    if let Some(cached) = cache.get(agent_id) {
        return cached.clone();
    }
    let color = query_avatar_color(pool, agent_id).await;
    // 防止缓存无界增长：超过 256 条目时清空
    const AVATAR_COLOR_CACHE_MAX: usize = 256;
    if cache.len() >= AVATAR_COLOR_CACHE_MAX {
        cache.clear();
    }
    cache.insert(agent_id.to_string(), color.clone());
    color
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
            agent_service::read_agent_config(app.clone(), app.state(), agent_id.to_string(), None)
                .await?;
        let dto = AgentSyncDTO::from(&config);

        let idempotency_key = generate_idempotency_key("push", "agent", agent_id);
        let url = format!("{}/api/mobile-sync/upload-entity", http_url);

        let _ = client
            .post(&url)
            .header("x-sync-token", sync_token)
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
            .header("x-idempotency-key", idempotency_key)
            .json(&serde_json::json!({ "id": group_id, "type": "group", "data": dto }))
            .send()
            .await;

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
                .header("Content-Type", mime_type)
                .body(image_data)
                .send()
                .await;
        }

        Ok(())
    }

    pub async fn push_agent_topic<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        topic_id: &str,
    ) -> Result<(), String> {
        let db = app.state::<DbState>();

        let row = sqlx::query("SELECT topic_id, title, created_at, locked, unread, owner_id FROM topics WHERE topic_id = ?")
            .bind(topic_id)
            .fetch_optional(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        if let Some(r) = row {
            let dto = AgentTopicSyncDTO {
                id: r.get("topic_id"),
                name: r.get("title"),
                created_at: r.get("created_at"),
                locked: r.get::<i64, _>("locked") != 0,
                unread: r.get::<i64, _>("unread") != 0,
                owner_id: r.get("owner_id"),
            };

            let idempotency_key = generate_idempotency_key("push", "agent_topic", topic_id);
            let url = format!("{}/api/mobile-sync/upload-entity", http_url);

            let _ = client
                .post(&url)
                .header("x-sync-token", sync_token)
                .header("x-idempotency-key", idempotency_key)
                .json(&serde_json::json!({ "id": topic_id, "type": "agent_topic", "data": dto }))
                .send()
                .await;
        }

        Ok(())
    }

    pub async fn push_group_topic<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        topic_id: &str,
    ) -> Result<(), String> {
        let db = app.state::<DbState>();

        let row = sqlx::query(
            "SELECT topic_id, title, created_at, owner_id FROM topics WHERE topic_id = ?",
        )
        .bind(topic_id)
        .fetch_optional(&db.pool)
        .await
        .map_err(|e| e.to_string())?;

        if let Some(r) = row {
            let dto = GroupTopicSyncDTO {
                id: r.get("topic_id"),
                name: r.get("title"),
                created_at: r.get("created_at"),
                owner_id: r.get("owner_id"),
            };

            let idempotency_key = generate_idempotency_key("push", "group_topic", topic_id);
            let url = format!("{}/api/mobile-sync/upload-entity", http_url);

            let _ = client
                .post(&url)
                .header("x-sync-token", sync_token)
                .header("x-idempotency-key", idempotency_key)
                .json(&serde_json::json!({ "id": topic_id, "type": "group_topic", "data": dto }))
                .send()
                .await;
        }

        Ok(())
    }

    pub async fn push_messages(
        app: &AppHandle,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        topic_id: &str,
        uploaded_hashes: Option<Arc<RwLock<HashSet<String>>>>,
    ) -> Result<(), String> {
        let db = app.state::<DbState>();

        let topic_row = sqlx::query("SELECT owner_id, owner_type FROM topics WHERE topic_id = ?")
            .bind(topic_id)
            .fetch_optional(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        if let Some(r) = topic_row {
            let owner_id: String = r.get("owner_id");
            let owner_type: String = r.get("owner_type");

            // 推送时必须加载该 topic 的全部消息，否则会导致桌面端 hash 不一致
            let msg_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM messages WHERE topic_id = ? AND deleted_at IS NULL",
            )
            .bind(topic_id)
            .fetch_one(&db.pool)
            .await
            .unwrap_or(0);
            if msg_count > 9999 {
                println!("[PushExecutor] WARN: topic {} has {} messages, exceeding single-batch load limit. Consider incremental push protocol.", topic_id, msg_count);
            }

            let history = crate::vcp_modules::message_service::load_chat_history_internal(
                app,
                &owner_id,
                &owner_type,
                topic_id,
                None,
                None,
                true,
            )
            .await?;

            let dto_messages = build_message_dtos(app, &history, &owner_type).await;

            let url = format!("{}/api/mobile-sync/upload-messages", http_url);
            let response = client
                .post(&url)
                .header("x-sync-token", sync_token)
                .json(&serde_json::json!({ "topicId": topic_id, "messages": dto_messages }))
                .send()
                .await;

            if let Ok(resp) = response {
                if resp.status().is_success() {
                    if let Ok(result) = resp.json::<serde_json::Value>().await {
                        if let Some(needed_hashes) = result
                            .get("neededAttachmentHashes")
                            .and_then(|v| v.as_array())
                        {
                            // 1. 筛选出真正需要上传的附件 hash
                            let mut hashes_to_upload = Vec::new();
                            for hash_value in needed_hashes {
                                if let Some(hash) = hash_value.as_str() {
                                    let should_upload = if let Some(ref tracker) = uploaded_hashes {
                                        let tracker_guard = tracker.read().await;
                                        !tracker_guard.contains(hash)
                                    } else {
                                        true
                                    };
                                    if should_upload {
                                        hashes_to_upload.push(hash.to_string());
                                    } else {
                                        println!("[PushExecutor] Skipping already uploaded attachment: {}", hash);
                                    }
                                }
                            }

                            // 2. 限流并发上传（最多 3 个并发）
                            const MAX_CONCURRENT_UPLOADS: usize = 3;
                            for chunk in hashes_to_upload.chunks(MAX_CONCURRENT_UPLOADS) {
                                let futures: Vec<_> = chunk
                                    .iter()
                                    .map(|hash| {
                                        upload_attachment(app, client, http_url, sync_token, hash)
                                    })
                                    .collect();
                                let results = futures_util::future::join_all(futures).await;
                                // 3. 批量标记已上传
                                if let Some(ref tracker) = uploaded_hashes {
                                    let mut tracker_guard = tracker.write().await;
                                    for (hash, result) in chunk.iter().zip(results) {
                                        if result.is_ok() {
                                            tracker_guard.insert(hash.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

fn generate_idempotency_key(action: &str, entity_type: &str, id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(action.as_bytes());
    hasher.update(entity_type.as_bytes());
    hasher.update(id.as_bytes());
    let now = chrono::Utc::now().timestamp() / 60;
    hasher.update(now.to_string().as_bytes());
    format!("{:x}", hasher.finalize())
}

async fn build_message_dtos<R: Runtime>(
    app: &AppHandle<R>,
    history: &[crate::vcp_modules::chat_manager::ChatMessage],
    owner_type: &str,
) -> Vec<serde_json::Value> {
    let db = app.state::<DbState>();
    let sync_state = app.state::<crate::vcp_modules::sync_service::SyncState>();
    let cache = &sync_state.avatar_color_cache;
    let mut results = Vec::new();

    for msg in history {
        let msg_value = if msg.role == "user" {
            let dto = UserMessageSyncDTO::from(msg);
            serde_json::to_value(dto).ok()
        } else if owner_type == "group" {
            let avatar_color = query_avatar_color_cached(
                &db.pool,
                cache,
                &msg.agent_id.clone().unwrap_or_default(),
            )
            .await;
            let dto = GroupMessageSyncDTO::from_message(msg, avatar_color);
            serde_json::to_value(dto).ok()
        } else {
            let avatar_color = query_avatar_color_cached(
                &db.pool,
                cache,
                &msg.agent_id.clone().unwrap_or_default(),
            )
            .await;
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
