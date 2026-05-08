use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::{DbWriteQueue, DbWriteTask};
use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use crate::vcp_modules::message_repository::MessageRenderCompiler;
use tauri::{AppHandle, Manager, Runtime};

/// 规范化桌面端返回的消息 JSON，修复常见字段类型不匹配
fn normalize_desktop_message(val: &mut serde_json::Value) {
    if let Some(obj) = val.as_object_mut() {
        // isThinking: 数字 0/1 -> bool
        if let Some(v) = obj.get("isThinking").and_then(|v| v.as_i64()) {
            obj.insert("isThinking".to_string(), serde_json::json!(v != 0));
        }
        // isGroupMessage: 数字 0/1 -> bool
        if let Some(v) = obj.get("isGroupMessage").and_then(|v| v.as_i64()) {
            obj.insert("isGroupMessage".to_string(), serde_json::json!(v != 0));
        }
        // timestamp: 字符串数字 -> u64
        if let Some(v) = obj.get("timestamp") {
            if v.is_string() {
                if let Some(s) = v.as_str() {
                    if let Ok(n) = s.parse::<u64>() {
                        obj.insert("timestamp".to_string(), serde_json::json!(n));
                    }
                }
            }
        }
        // 附件 size: i64 -> u64
        if let Some(attachments) = obj.get_mut("attachments").and_then(|a| a.as_array_mut()) {
            for att in attachments {
                if let Some(att_obj) = att.as_object_mut() {
                    if let Some(v) = att_obj.get("size").and_then(|v| v.as_i64()) {
                        if v >= 0 {
                            att_obj.insert("size".to_string(), serde_json::json!(v as u64));
                        }
                    }
                }
            }
        }
    }
}

pub struct PullExecutor;

impl PullExecutor {
    pub async fn pull_agent<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        agent_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        let url = format!(
            "{}/api/mobile-sync/download-entity?id={}&type=agent",
            http_url, agent_id
        );
        let res = client
            .get(&url)
            .header("x-sync-token", sync_token)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !res.status().is_success() {
            return Err(format!("Pull agent failed: {}", res.status()));
        }

        let dto: AgentSyncDTO = res.json().await.map_err(|e| e.to_string())?;
        write_queue
            .submit(DbWriteTask::Agent {
                id: agent_id.to_string(),
                dto,
            })
            .await;

        Ok(())
    }

    pub async fn pull_group<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        group_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        let url = format!(
            "{}/api/mobile-sync/download-entity?id={}&type=group",
            http_url, group_id
        );
        let res = client
            .get(&url)
            .header("x-sync-token", sync_token)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !res.status().is_success() {
            return Err(format!("Pull group failed: {}", res.status()));
        }

        let dto: GroupSyncDTO = res.json().await.map_err(|e| e.to_string())?;
        write_queue
            .submit(DbWriteTask::Group {
                id: group_id.to_string(),
                dto,
            })
            .await;

        Ok(())
    }

    pub async fn pull_entities_batch<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        requests: Vec<serde_json::Value>,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        let url = format!("{}/api/mobile-sync/download-entities", http_url);
        let res = client
            .post(&url)
            .header("x-sync-token", sync_token)
            .json(&serde_json::json!({ "requests": requests }))
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !res.status().is_success() {
            return Err(format!("Pull entities batch failed: {}", res.status()));
        }

        let results: Vec<serde_json::Value> = res.json().await.map_err(|e| e.to_string())?;
        
        let mut agent_topics = Vec::new();
        let mut group_topics = Vec::new();

        for item in results {
            let id = item.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            let r#type = item
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let data = item.get("data").cloned().unwrap_or(serde_json::Value::Null);

            match r#type {
                "agent" => {
                    if let Ok(dto) = serde_json::from_value::<AgentSyncDTO>(data) {
                        write_queue
                            .submit(DbWriteTask::Agent {
                                id: id.to_string(),
                                dto,
                            })
                            .await;
                    }
                }
                "group" => {
                    if let Ok(dto) = serde_json::from_value::<GroupSyncDTO>(data) {
                        write_queue
                            .submit(DbWriteTask::Group {
                                id: id.to_string(),
                                dto,
                            })
                            .await;
                    }
                }
                "agent_topic" => {
                    if id == "default" {
                        continue;
                    }
                    if let Ok(dto) = serde_json::from_value::<AgentTopicSyncDTO>(data) {
                        agent_topics.push((id.to_string(), dto));
                    }
                }
                "group_topic" => {
                    if id == "default" {
                        continue;
                    }
                    if let Ok(dto) = serde_json::from_value::<GroupTopicSyncDTO>(data) {
                        group_topics.push((id.to_string(), dto));
                    }
                }
                _ => {}
            }
        }

        if !agent_topics.is_empty() {
            write_queue.submit(DbWriteTask::AgentTopicBatch { topics: agent_topics }).await;
        }
        if !group_topics.is_empty() {
            write_queue.submit(DbWriteTask::GroupTopicBatch { topics: group_topics }).await;
        }

        Ok(())
    }

    pub async fn pull_avatar<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        owner_type: &str,
        owner_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        let url = format!(
            "{}/api/mobile-sync/download-avatar?id={}&type={}",
            http_url, owner_id, owner_type
        );

        // 指数退避重试：avatar 下载受网络波动影响较大
        let mut retries = 0;
        let max_retries = 3;
        let mut delay_ms = 200u64;
        loop {
            match client
                .get(&url)
                .header("x-sync-token", sync_token)
                .send()
                .await
            {
                Ok(res) => {
                    if !res.status().is_success() {
                        return Err(format!("Pull avatar failed: {}", res.status()));
                    }
                    match res.bytes().await {
                        Ok(bytes) => {
                            write_queue
                                .submit(DbWriteTask::Avatar {
                                    owner_type: owner_type.to_string(),
                                    owner_id: owner_id.to_string(),
                                    bytes: bytes.to_vec(),
                                })
                                .await;
                            if retries > 0 {
                                println!(
                                    "[PullExecutor] Avatar {} {} succeeded after {} retries",
                                    owner_type, owner_id, retries
                                );
                            }
                            return Ok(());
                        }
                        Err(e) if retries < max_retries => {
                            retries += 1;
                            println!("[PullExecutor] Avatar {} {} decode failed (retry {}/{}): {}. Waiting {}ms", owner_type, owner_id, retries, max_retries, e, delay_ms);
                            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                            delay_ms *= 2;
                        }
                        Err(e) => {
                            return Err(format!(
                                "Pull avatar decode failed after {} retries: {}",
                                max_retries, e
                            ));
                        }
                    }
                }
                Err(e) if retries < max_retries => {
                    retries += 1;
                    println!("[PullExecutor] Avatar {} {} request failed (retry {}/{}): {}. Waiting {}ms", owner_type, owner_id, retries, max_retries, e, delay_ms);
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    delay_ms *= 2;
                }
                Err(e) => {
                    return Err(format!(
                        "Pull avatar request failed after {} retries: {}",
                        max_retries, e
                    ));
                }
            }
        }
    }

    pub async fn pull_agent_topic<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        topic_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        let url = format!(
            "{}/api/mobile-sync/download-entity?id={}&type=agent_topic",
            http_url, topic_id
        );
        let res = client
            .get(&url)
            .header("x-sync-token", sync_token)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if res.status() == reqwest::StatusCode::NOT_FOUND {
            // Topic not found on desktop, skip silently
            return Ok(());
        }

        if !res.status().is_success() {
            return Err(format!("Pull agent_topic failed: {}", res.status()));
        }

        let dto: AgentTopicSyncDTO = res.json().await.map_err(|e| e.to_string())?;
        write_queue
            .submit(DbWriteTask::AgentTopic {
                topic_id: topic_id.to_string(),
                dto,
            })
            .await;

        Ok(())
    }

    pub async fn pull_group_topic<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        topic_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        let url = format!(
            "{}/api/mobile-sync/download-entity?id={}&type=group_topic",
            http_url, topic_id
        );
        let res = client
            .get(&url)
            .header("x-sync-token", sync_token)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if res.status() == reqwest::StatusCode::NOT_FOUND {
            // Topic not found on desktop, skip silently
            return Ok(());
        }

        if !res.status().is_success() {
            return Err(format!("Pull group_topic failed: {}", res.status()));
        }

        let dto: GroupTopicSyncDTO = res.json().await.map_err(|e| e.to_string())?;
        write_queue
            .submit(DbWriteTask::GroupTopic {
                topic_id: topic_id.to_string(),
                dto,
            })
            .await;

        Ok(())
    }

    pub async fn pull_messages(
        app: &AppHandle,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        topic_id: &str,
        msg_ids: &[String],
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        let db = app.state::<DbState>();

        // 尝试获取 topic 信息，如果不存在则使用默认值
        // 消息数据会在 topic 后续同步时被正确关联
        let topic_row = sqlx::query("SELECT owner_id, owner_type FROM topics WHERE topic_id = ?")
            .bind(topic_id)
            .fetch_optional(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        let (_owner_id, _owner_type) = match topic_row {
            Some(r) => {
                use sqlx::Row;
                (r.get::<String, _>("owner_id"), r.get::<String, _>("owner_type"))
            },
            None => {
                // Topic 还未同步，使用占位值，后续 topic 同步时会更新
                println!(
                    "[PullExecutor] Topic {} not yet available, messages will be linked later",
                    topic_id
                );
                ("pending_owner".to_string(), "agent".to_string())
            }
        };

        let url = format!("{}/api/mobile-sync/download-messages", http_url);
        let res = client
            .post(&url)
            .header("x-sync-token", sync_token)
            .json(&serde_json::json!({ "topicId": topic_id, "msgIds": msg_ids }))
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !res.status().is_success() {
            return Err(format!("Pull messages failed: {}", res.status()));
        }

        let messages: Vec<serde_json::Value> = res.json().await.map_err(|e| e.to_string())?;
        let mut parsed_messages = Vec::new();

        // 1. 批量收集所有附件 hash，一次性查询本地路径（替代 N+1 查询）
        let mut all_hashes = Vec::new();
        for m_val in &messages {
            if let Some(obj) = m_val.as_object() {
                if let Some(attachments) = obj.get("attachments").and_then(|a| a.as_array()) {
                    for att in attachments {
                        if let Some(att_obj) = att.as_object() {
                            if let Some(hash) = att_obj.get("hash").and_then(|h| h.as_str()) {
                                if !hash.is_empty() {
                                    all_hashes.push(hash.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        let mut path_map = std::collections::HashMap::new();
        if !all_hashes.is_empty() {
            let placeholders = all_hashes
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", ");
            let query = format!(
                "SELECT hash, internal_path FROM attachments WHERE hash IN ({})",
                placeholders
            );
            let mut q = sqlx::query(&query);
            for h in &all_hashes {
                q = q.bind(h);
            }
            if let Ok(rows) = q.fetch_all(&db.pool).await {
                use sqlx::Row;
                for row in rows {
                    if let (Ok(hash), Ok(path)) = (
                        row.try_get::<String, _>("hash"),
                        row.try_get::<String, _>("internal_path"),
                    ) {
                        path_map.insert(hash, path);
                    }
                }
            }
        }

        // 2. 遍历消息，用缓存的 path_map 填充附件路径，并规范化桌面端字段
        let mut failed_count = 0usize;
        for mut m_val in messages {
            normalize_desktop_message(&mut m_val);

            if let Some(obj) = m_val.as_object_mut() {
                if let Some(attachments) = obj.get_mut("attachments").and_then(|a| a.as_array_mut())
                {
                    for att in attachments {
                        if let Some(att_obj) = att.as_object_mut() {
                            if let Some(hash) = att_obj.get("hash").and_then(|h| h.as_str()) {
                                if !hash.is_empty() {
                                    if let Some(path) = path_map.get(hash) {
                                        att_obj
                                            .entry("internalPath".to_string())
                                            .or_insert(serde_json::json!(path));
                                        att_obj.entry("src".to_string()).or_insert(
                                            serde_json::json!(format!("file://{}", path)),
                                        );
                                    } else {
                                        let default_path = format!("file://attachments/{}", hash);
                                        att_obj.entry("internalPath".to_string()).or_insert(
                                            serde_json::json!(
                                                default_path.trim_start_matches("file://")
                                            ),
                                        );
                                        att_obj
                                            .entry("src".to_string())
                                            .or_insert(serde_json::json!(default_path));
                                    }
                                }
                            }
                            att_obj
                                .entry("status".to_string())
                                .or_insert(serde_json::json!("ready"));
                        }
                    }
                }
                obj.remove("avatarUrl");
                obj.remove("avatarColor");
            }

            match serde_json::from_value::<crate::vcp_modules::chat_manager::ChatMessage>(
                m_val.clone(),
            ) {
                Ok(msg) => {
                    parsed_messages.push(msg);
                }
                Err(e) => {
                    failed_count += 1;
                    if let Some(obj) = m_val.as_object() {
                        println!(
                            "[PullExecutor] Parse fail diagnostic for topic {} msg id={:?}:",
                            topic_id,
                            obj.get("id").or_else(|| obj.get("msgId"))
                        );
                        println!("  role={:?} timestamp={:?} isThinking={:?} isGroupMessage={:?} attachments={:?}",
                            obj.get("role"), obj.get("timestamp"), obj.get("isThinking"), obj.get("isGroupMessage"), obj.get("attachments").map(|v| v.is_array()));
                    }
                    println!(
                        "[PullExecutor] Failed to parse message in topic {}: {}. Raw value: {}",
                        topic_id, e, m_val
                    );
                }
            }
        }
        if failed_count > 0 {
            println!("[PullExecutor] Topic {} message parse summary: total_requested={}, success={}, failed={}", topic_id, parsed_messages.len() + failed_count, parsed_messages.len(), failed_count);
        }

        // 判断是否需要冒泡更新（如果 topic 不存在则跳过）
        let topic_exists: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM topics WHERE topic_id = ? AND deleted_at IS NULL",
        )
        .bind(topic_id)
        .fetch_one(&db.pool)
        .await
        .unwrap_or(false);

        let skip_bubble = !topic_exists;

        if !parsed_messages.is_empty() {
            // 3. 并发预渲染 (Parallel Pre-render on CPU)
            let mut render_bytes_list = Vec::with_capacity(parsed_messages.len());
            for msg in &parsed_messages {
                let content = msg.content.clone();
                let topic_id_log = topic_id.to_string();
                let msg_id_log = msg.id.clone();
                
                let result = std::panic::catch_unwind(|| {
                    let blocks =
                        MessageRenderCompiler::compile(
                            &content,
                        );
                    MessageRenderCompiler::serialize(&blocks)
                });
                
                match result {
                    Ok(Ok(bytes)) => render_bytes_list.push(bytes),
                    Ok(Err(e)) => {
                        println!("[PullExecutor] Serialize failed for msg {} (topic {}): {}", msg_id_log, topic_id_log, e);
                        render_bytes_list.push(Vec::new());
                    }
                    Err(_) => {
                        println!("[PullExecutor] Compile panicked for msg {} (topic {})", msg_id_log, topic_id_log);
                        render_bytes_list.push(Vec::new());
                    }
                }
            }

            // 4. 提交到写入队列 (Batched Consumption on DB Worker)
            write_queue.submit(DbWriteTask::TopicMessages {
                topic_id: topic_id.to_string(),
                messages: parsed_messages,
                render_bytes: render_bytes_list,
                skip_bubble,
            }).await;
        }

        Ok(())
    }
}
