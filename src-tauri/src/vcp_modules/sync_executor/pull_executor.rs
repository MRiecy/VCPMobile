use crate::vcp_modules::db_write_queue::{DbWriteQueue, DbWriteTask};
use crate::vcp_modules::message_repository::MessageRenderCompiler;
use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use std::sync::Arc;
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::{mpsc, Semaphore};

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

/// 共享消息处理管线：附件路径批量查询 → 规范化 → 解析 → 预渲染 → 写入队列
/// 被 `pull_messages_batch` 内各并发任务复用。
/// 返回 `(parsed_count, failed_count)`。
async fn process_topic_messages<R: Runtime>(
    app: &AppHandle<R>,
    topic_id: &str,
    messages: Vec<serde_json::Value>,
    write_queue: &DbWriteQueue,
) -> Result<(usize, usize), String> {
    use crate::vcp_modules::db_manager::DbState;
    use sqlx::Row;
    let db = app.state::<DbState>();

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
    let mut parsed_messages = Vec::new();
    let mut failed_count = 0usize;
    for mut m_val in messages {
        normalize_desktop_message(&mut m_val);

        if let Some(obj) = m_val.as_object_mut() {
            if let Some(attachments) = obj.get_mut("attachments").and_then(|a| a.as_array_mut()) {
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
                }
                println!(
                    "[PullExecutor] Failed to parse message in topic {}: {}. Raw value: {}",
                    topic_id, e, m_val
                );
            }
        }
    }
    if failed_count > 0 {
        println!(
            "[PullExecutor] Topic {} message parse summary: total_received={}, success={}, failed={}",
            topic_id,
            parsed_messages.len() + failed_count,
            parsed_messages.len(),
            failed_count
        );
    }

    // Sync 期间强制跳过即时冒泡
    let skip_bubble = true;
    let parsed_count = parsed_messages.len();

    if !parsed_messages.is_empty() {
        // 3. 并发预渲染 (Parallel Pre-render on CPU)
        let mut render_bytes_list = Vec::with_capacity(parsed_count);
        for msg in &parsed_messages {
            let content = msg.content.clone();
            let topic_id_log = topic_id.to_string();
            let msg_id_log = msg.id.clone();

            let result = std::panic::catch_unwind(|| {
                let blocks = MessageRenderCompiler::compile(&content);
                MessageRenderCompiler::serialize(&blocks)
            });

            match result {
                Ok(Ok(bytes)) => render_bytes_list.push(bytes),
                Ok(Err(e)) => {
                    println!(
                        "[PullExecutor] Serialize failed for msg {} (topic {}): {}",
                        msg_id_log, topic_id_log, e
                    );
                    render_bytes_list.push(Vec::new());
                }
                Err(_) => {
                    println!(
                        "[PullExecutor] Compile panicked for msg {} (topic {})",
                        msg_id_log, topic_id_log
                    );
                    render_bytes_list.push(Vec::new());
                }
            }
        }

        // 4. 提交到写入队列
        write_queue
            .submit(DbWriteTask::TopicMessages {
                topic_id: topic_id.to_string(),
                messages: parsed_messages,
                render_bytes: render_bytes_list,
                skip_bubble,
            })
            .await;
    }

    Ok((parsed_count, failed_count))
}

/// 批量 Pull 单 topic 处理结果
#[allow(dead_code)]
pub struct BatchPullResult {
    pub topic_id: String,
    pub success: bool,
    pub parsed_count: usize,
    pub failed_count: usize,
    pub error: Option<String>,
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
            .header("Authorization", format!("Bearer {}", sync_token))
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
            .header("Authorization", format!("Bearer {}", sync_token))
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
            .header("Authorization", format!("Bearer {}", sync_token))
            .json(&serde_json::json!({ "requests": requests }))
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !res.status().is_success() {
            return Err(format!("Pull entities batch failed: {}", res.status()));
        }

        let results: Vec<serde_json::Value> = res.json().await.map_err(|e| e.to_string())?;
        println!("[PullExecutor] Received {} entities from server", results.len());

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
            println!("[PullExecutor] Submitting {} agent topics to write queue", agent_topics.len());
            write_queue
                .submit(DbWriteTask::AgentTopicBatch {
                    topics: agent_topics,
                })
                .await;
        }
        if !group_topics.is_empty() {
            println!("[PullExecutor] Submitting {} group topics to write queue", group_topics.len());
            write_queue
                .submit(DbWriteTask::GroupTopicBatch {
                    topics: group_topics,
                })
                .await;
        }

        println!("[PullExecutor] Batch pull completed");
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
                .header("Authorization", format!("Bearer {}", sync_token))
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
            .header("Authorization", format!("Bearer {}", sync_token))
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
            .header("Authorization", format!("Bearer {}", sync_token))
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

    /// 流式批量 Pull — 一次 HTTP 请求拉取多个 topic 的消息
    ///
    /// 桌面端以 NDJSON 逐 topic 分帧返回，手机端逐行消费，
    /// 不等待整个响应结束。单 topic 失败不中断流。
    ///
    /// **并发控制**: Semaphore(20) + tokio spawn 并行处理 topic 消息，
    /// mpsc channel 实时推送进度日志。NDJSON 解析与并发处理完全分离。
    ///
    /// 返回每个 topic 的处理结果。
    pub async fn pull_messages_batch<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        requests: &[(String, Vec<String>)], // (topic_id, msg_ids), 空 vec = 拉全部消息
        write_queue: &DbWriteQueue,
    ) -> Result<Vec<BatchPullResult>, String> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }

        let url = format!("{}/api/mobile-sync/download-messages-stream", http_url);
        let req_body: Vec<serde_json::Value> = requests
            .iter()
            .map(|(tid, ids)| {
                serde_json::json!({ "topicId": tid, "msgIds": ids })
            })
            .collect();

        let res = client
            .post(&url)
            .header("x-sync-token", sync_token)
            .header("Authorization", format!("Bearer {}", sync_token))
            .json(&serde_json::json!({ "requests": req_body }))
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !res.status().is_success() {
            let status = res.status();
            let err_body = res.text().await.unwrap_or_default();
            return Err(format!(
                "Batch pull messages failed: HTTP {} body={}",
                status, err_body
            ));
        }

        // ── 并发基础设施 ──
        let sem = Arc::new(Semaphore::new(20));
        let (tx, mut rx) = mpsc::unbounded_channel::<BatchPullResult>();
        let mut spawn_handles = Vec::new();
        let total = requests.len();

        // 启动接收协程：实时消费 channel 输出进度日志
        let receiver_handle = tokio::spawn(async move {
            let mut results = Vec::new();
            let mut completed = 0usize;
            while let Some(result) = rx.recv().await {
                completed += 1;
                if result.success {
                    println!(
                        "[PullExecutor] Batch pull: topic {} completed ({}/{})",
                        result.topic_id, completed, total
                    );
                } else {
                    let err = result.error.as_deref().unwrap_or("unknown");
                    eprintln!(
                        "[PullExecutor] Batch pull: topic {} FAILED ({}/{}): {}",
                        result.topic_id, completed, total, err
                    );
                }
                results.push(result);
            }
            results
        });

        // ── NDJSON 解析协程 ──
        use futures_util::StreamExt;
        let mut stream = res.bytes_stream();
        let mut buffer: Vec<u8> = Vec::new();

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| format!("Stream read error: {}", e))?;

            // 检测流级错误帧
            if chunk.starts_with(b"{\"_stream_error\"") || chunk.starts_with(br#"{"_stream_error""#) {
                if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&chunk) {
                    let msg = val["_stream_error"].as_str().unwrap_or("unknown stream error");
                    return Err(format!("Desktop stream error: {}", msg));
                }
            }

            buffer.extend_from_slice(&chunk);

            // 逐行解析 NDJSON（支持 chunk 边界跨越）
            while let Some(line_end) = buffer.iter().position(|&b| b == b'\n') {
                let line = buffer.drain(..=line_end).collect::<Vec<_>>();
                if line.len() <= 1 {
                    continue;
                }

                let topic_data: serde_json::Value =
                    serde_json::from_slice(&line).unwrap_or(serde_json::Value::Null);

                let topic_id = topic_data["topicId"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();

                if topic_id.is_empty() {
                    eprintln!("[PullExecutor] Batch pull: malformed NDJSON line, skipping");
                    continue;
                }

                // 检查单 topic 错误帧
                if let Some(topic_err) = topic_data["_error"].as_str() {
                    let _ = tx.send(BatchPullResult {
                        topic_id,
                        success: false,
                        parsed_count: 0,
                        failed_count: 0,
                        error: Some(format!("Desktop error: {}", topic_err)),
                    });
                    continue;
                }

                let messages: Vec<serde_json::Value> = topic_data["messages"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();

                if messages.is_empty() {
                    let _ = tx.send(BatchPullResult {
                        topic_id,
                        success: true,
                        parsed_count: 0,
                        failed_count: 0,
                        error: None,
                    });
                    continue;
                }

                // 并发处理：Semaphore 控制并发度，spawn 异步任务
                let permit = sem.clone().acquire_owned().await.map_err(|e| e.to_string())?;
                let app_clone = app.clone();
                let wq_clone = write_queue.clone();
                let tx_clone = tx.clone();
                let handle = tokio::spawn(async move {
                    let _permit = permit; // 持有 permit 直到任务完成
                    match process_topic_messages(&app_clone, &topic_id, messages, &wq_clone).await {
                        Ok((parsed, failed)) => {
                            let _ = tx_clone.send(BatchPullResult {
                                topic_id,
                                success: true,
                                parsed_count: parsed,
                                failed_count: failed,
                                error: None,
                            });
                        }
                        Err(e) => {
                            let _ = tx_clone.send(BatchPullResult {
                                topic_id,
                                success: false,
                                parsed_count: 0,
                                failed_count: 0,
                                error: Some(e),
                            });
                        }
                    }
                });
                spawn_handles.push(handle);
            }
        }

        // 处理流结束后 buffer 中残留的非换行数据（兜底）
        if !buffer.is_empty() {
            if let Ok(topic_data) = serde_json::from_slice::<serde_json::Value>(&buffer) {
                let topic_id = topic_data["topicId"].as_str().unwrap_or("").to_string();
                if !topic_id.is_empty() {
                    let messages: Vec<serde_json::Value> = topic_data["messages"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    if !messages.is_empty() {
                        let permit = sem.acquire_owned().await.map_err(|e| e.to_string())?;
                        let app_clone = app.clone();
                        let wq_clone = write_queue.clone();
                        let tx_clone = tx.clone();
                        let handle = tokio::spawn(async move {
                            let _permit = permit;
                            match process_topic_messages(&app_clone, &topic_id, messages, &wq_clone).await {
                                Ok((parsed, failed)) => {
                                    let _ = tx_clone.send(BatchPullResult {
                                        topic_id,
                                        success: true,
                                        parsed_count: parsed,
                                        failed_count: failed,
                                        error: None,
                                    });
                                }
                                Err(e) => {
                                    let _ = tx_clone.send(BatchPullResult {
                                        topic_id,
                                        success: false,
                                        parsed_count: 0,
                                        failed_count: 0,
                                        error: Some(e),
                                    });
                                }
                            }
                        });
                        spawn_handles.push(handle);
                    } else {
                        let _ = tx.send(BatchPullResult {
                            topic_id,
                            success: true,
                            parsed_count: 0,
                            failed_count: 0,
                            error: None,
                        });
                    }
                }
            }
        }

        // ── 等待所有任务完成 ──
        drop(tx); // 关闭 channel，通知 receiver 不再有新消息
        let _ = futures_util::future::join_all(spawn_handles).await;
        let results = receiver_handle.await.unwrap_or_default();

        let ok_count = results.iter().filter(|r| r.success).count();
        let err_count = results.iter().filter(|r| !r.success).count();
        println!(
            "[PullExecutor] Batch pull completed: {}/{} topics processed, {} errors",
            ok_count,
            total,
            err_count
        );
        Ok(results)
    }
}
