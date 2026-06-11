use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::{watch, RwLock};
use tokio::time::{sleep, Duration};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use url::Url;

lazy_static::lazy_static! {
    static ref INFO_CONNECTION_ACTIVE: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    static ref METADATA_LIST: Arc<RwLock<VecDeque<Value>>> = Arc::new(RwLock::new(VecDeque::new()));
    static ref COMPRESSED_PAYLOADS: Arc<RwLock<HashMap<String, Vec<u8>>>> = Arc::new(RwLock::new(HashMap::new()));
    static ref WS_INFO_URL_CHANNEL: (watch::Sender<Option<Url>>, watch::Receiver<Option<Url>>) = watch::channel(None);
    static ref CURRENT_INFO_STATUS: Arc<RwLock<String>> = Arc::new(RwLock::new("closed".to_string()));
}

fn next_id_counter() -> u64 {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn parse_info_url(url: &str, key: &str) -> Result<Url, String> {
    let base_url_trimmed = url.trim().trim_end_matches('/');
    let mut ws_url = Url::parse(base_url_trimmed).map_err(|e| format!("Invalid URL: {}", e))?;

    // 将路径替换为 /vcpinfo
    ws_url.set_path("/vcpinfo");
    let url_str = ws_url.to_string();

    let url_with_key = if url_str.contains("VCP_Key=") {
        url_str
    } else {
        format!("{}/VCP_Key={}", url_str.trim_end_matches('/'), key)
    };

    Url::parse(&url_with_key).map_err(|e| format!("Invalid URL with Key: {}", e))
}

fn compress_payload(payload_str: &str) -> Result<Vec<u8>, String> {
    zstd::encode_all(payload_str.as_bytes(), 3).map_err(|e| format!("Zstd compress failed: {}", e))
}

#[tauri::command]
pub async fn get_vcp_info_connection_status() -> Result<String, String> {
    Ok(CURRENT_INFO_STATUS.read().await.clone())
}

#[tauri::command]
pub async fn get_vcp_info_metadata_list() -> Result<Vec<Value>, String> {
    let list = METADATA_LIST.read().await;
    Ok(list.iter().cloned().collect())
}

#[tauri::command]
pub async fn get_vcp_info_payload(_app: AppHandle, id: String) -> Result<String, String> {
    let compressed_map = COMPRESSED_PAYLOADS.read().await;
    if let Some(compressed) = compressed_map.get(&id) {
        let decompressed_bytes = zstd::decode_all(&compressed[..])
            .map_err(|e| format!("Zstd decompress failed: {}", e))?;
        let decompressed_str = String::from_utf8(decompressed_bytes)
            .map_err(|e| format!("Invalid UTF-8 after decompress: {}", e))?;
        Ok(decompressed_str)
    } else {
        Err("Payload not found in memory cache".to_string())
    }
}

#[tauri::command]
pub async fn clear_vcp_info(app: AppHandle) -> Result<(), String> {
    // 1. 清空内存
    {
        let mut list = METADATA_LIST.write().await;
        list.clear();
    }
    {
        let mut compressed_map = COMPRESSED_PAYLOADS.write().await;
        compressed_map.clear();
    }
    // 2. 广播给前端
    let _ = app.emit(
        "vcp-info-event",
        serde_json::json!({
            "type": "vcp-info-clear"
        }),
    );
    Ok(())
}

#[tauri::command]
pub async fn init_vcp_info_connection(
    app: AppHandle,
    url: String,
    key: String,
) -> Result<(), String> {
    init_vcp_info_connection_internal(app, url, key).await
}

pub async fn init_vcp_info_connection_internal<R: tauri::Runtime>(
    app: AppHandle<R>,
    url: String,
    key: String,
) -> Result<(), String> {
    if url.trim().is_empty() || key.trim().is_empty() {
        let _ = WS_INFO_URL_CHANNEL.0.send(None);
        return Ok(());
    }

    let ws_url = parse_info_url(&url, &key)?;

    // 存入 watch channel 供监听器线程消费
    let _ = WS_INFO_URL_CHANNEL.0.send(Some(ws_url));

    if INFO_CONNECTION_ACTIVE.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        start_vcp_info_listener(h).await;
    });

    Ok(())
}

async fn start_vcp_info_listener<R: tauri::Runtime>(app_handle: AppHandle<R>) {
    let mut url_rx = WS_INFO_URL_CHANNEL.0.subscribe();
    let mut retry_delay = Duration::from_millis(1000);

    loop {
        let ws_url = {
            let val = url_rx.borrow().clone();
            match val {
                Some(u) => u,
                None => {
                    if url_rx.changed().await.is_err() {
                        break;
                    }
                    continue;
                }
            }
        };

        let masked_url = if ws_url.as_str().contains("VCP_Key=") {
            let parts: Vec<&str> = ws_url.as_str().split("VCP_Key=").collect();
            format!("{}VCP_Key=********", parts[0])
        } else {
            ws_url.to_string()
        };
        log::info!("[VCPInfo] Attempting to connect to {}...", masked_url);

        {
            *CURRENT_INFO_STATUS.write().await = "connecting".to_string();
        }

        let _ = app_handle.emit(
            "vcp-info-event",
            serde_json::json!({
                "type": "vcp-info-status",
                "status": "connecting",
                "message": "连接中...",
                "source": "VCPInfo"
            }),
        );

        let mut request = match ws_url.as_str().into_client_request() {
            Ok(req) => req,
            Err(e) => {
                {
                    *CURRENT_INFO_STATUS.write().await = "error".to_string();
                }
                log::error!(
                    "[VCPInfo] Failed to build request: {}. Retrying in 5 seconds...",
                    e
                );
                let _ = app_handle.emit(
                    "vcp-info-event",
                    serde_json::json!({
                        "type": "vcp-info-status",
                        "status": "error",
                        "message": "连接错误",
                        "source": "VCPInfo"
                    }),
                );

                tokio::select! {
                    _ = url_rx.changed() => {},
                    _ = sleep(retry_delay) => {},
                }
                retry_delay = (retry_delay * 2).min(Duration::from_secs(60));
                continue;
            }
        };

        if let Some(host) = ws_url.host_str() {
            let host_with_port = if let Some(port) = ws_url.port() {
                format!("{}:{}", host, port)
            } else {
                host.to_string()
            };
            if let Ok(val) = host_with_port.parse() {
                request.headers_mut().insert("Host", val);
            }

            let origin_scheme = match ws_url.scheme() {
                "wss" => "https",
                _ => "http",
            };
            let origin = if let Some(port) = ws_url.port() {
                format!("{}://{}:{}", origin_scheme, host, port)
            } else {
                format!("{}://{}", origin_scheme, host)
            };
            if let Ok(val) = origin.parse() {
                request.headers_mut().insert("Origin", val);
            }
        }

        request.headers_mut().insert(
            "User-Agent",
            "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36".parse().unwrap()
        );

        match tokio::time::timeout(Duration::from_secs(10), connect_async(request)).await {
            Ok(connection_result) => match connection_result {
                Ok((ws_stream, _)) => {
                    retry_delay = Duration::from_millis(1000);
                    {
                        *CURRENT_INFO_STATUS.write().await = "connected".to_string();
                    }
                    log::info!("[VCPInfo] Connected successfully to {}", masked_url);

                    let (mut ws_write, mut ws_read) = ws_stream.split();

                    let _ = app_handle.emit(
                        "vcp-info-event",
                        serde_json::json!({
                            "type": "vcp-info-status",
                            "status": "connected",
                            "message": "已连接",
                            "source": "VCPInfo"
                        }),
                    );

                    let mut heartbeat_timer = Box::pin(sleep(Duration::from_secs(15)));

                    loop {
                        tokio::select! {
                            // 监听 URL 变更
                            _ = url_rx.changed() => {
                                log::info!("[VCPInfo] URL changed, closing current connection.");
                                break;
                            }
                            // 心跳周期触发
                            _ = &mut heartbeat_timer => {
                                if let Err(e) = ws_write.send(Message::Ping(vec![].into())).await {
                                    log::error!("[VCPInfo] Failed to send Ping: {}", e);
                                    break;
                                }
                                heartbeat_timer.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(15));
                            }
                            // 处理接收到的消息
                            msg_result = ws_read.next() => {
                                match msg_result {
                                    Some(Ok(msg)) => {
                                        if msg.is_text() {
                                            let text = msg.to_text().unwrap_or_default();
                                            if let Ok(payload) = serde_json::from_str::<Value>(text) {
                                                // 提取、缓存并推送消息
                                                process_incoming_vcp_info(&app_handle, payload, text).await;
                                            }
                                        }
                                    }
                                    Some(Err(e)) => {
                                        log::error!("[VCPInfo] WebSocket error during read: {}", e);
                                        break;
                                    }
                                    None => {
                                        log::warn!("[VCPInfo] Connection closed by server.");
                                        break;
                                    }
                                }
                            }
                        }
                    }

                    log::info!("[VCPInfo] Disconnected from {}.", ws_url);
                    {
                        *CURRENT_INFO_STATUS.write().await = "closed".to_string();
                    }
                    let _ = app_handle.emit(
                        "vcp-info-event",
                        serde_json::json!({
                            "type": "vcp-info-status",
                            "status": "closed",
                            "message": "连接已断开",
                            "source": "VCPInfo"
                        }),
                    );
                }
                Err(e) => {
                    {
                        *CURRENT_INFO_STATUS.write().await = "error".to_string();
                    }
                    log::error!("[VCPInfo] Connection Error: {}", e);
                    let _ = app_handle.emit(
                        "vcp-info-event",
                        serde_json::json!({
                            "type": "vcp-info-status",
                            "status": "error",
                            "message": "连接错误",
                            "source": "VCPInfo"
                        }),
                    );
                }
            },
            Err(_) => {
                {
                    *CURRENT_INFO_STATUS.write().await = "error".to_string();
                }
                log::error!("[VCPInfo] Connection timed out after 10 seconds.");
                let _ = app_handle.emit(
                    "vcp-info-event",
                    serde_json::json!({
                        "type": "vcp-info-status",
                        "status": "error",
                        "message": "连接超时",
                        "source": "VCPInfo"
                    }),
                );
            }
        }

        tokio::select! {
            _ = url_rx.changed() => log::info!("[VCPInfo] URL changed during retry wait."),
            _ = sleep(retry_delay) => {},
        }
        retry_delay = (retry_delay * 2).min(Duration::from_secs(60));
    }
}

async fn process_incoming_vcp_info<R: tauri::Runtime>(
    app_handle: &AppHandle<R>,
    payload: Value,
    raw_text: &str,
) {
    let timestamp_ms = chrono::Utc::now().timestamp_millis();
    let msg_id = format!("vcp_info_{}_{}", timestamp_ms, next_id_counter());

    // 1. 尝试提取 Metadata 索引，过滤掉无用消息
    if let Some(metadata) = extract_metadata(&msg_id, &payload) {
        // 2. 将原始 payload 使用 Zstd 压缩
        match compress_payload(raw_text) {
            Ok(compressed_data) => {
                // 3. 压入内存列表与哈希表
                let mut list = METADATA_LIST.write().await;
                let mut compressed_map = COMPRESSED_PAYLOADS.write().await;

                compressed_map.insert(msg_id.clone(), compressed_data);
                list.push_front(metadata.clone());

                // 4. 超出 500 条进行 FIFO 淘汰
                while list.len() > 500 {
                    if let Some(popped) = list.pop_back() {
                        if let Some(popped_id) = popped.get("id").and_then(|id| id.as_str()) {
                            compressed_map.remove(popped_id);
                        }
                    } else {
                        break;
                    }
                }

                // 5. 发送前端广播事件
                let _ = app_handle.emit(
                    "vcp-info-event",
                    serde_json::json!({
                        "type": "vcp-info-message",
                        "data": metadata
                    }),
                );
            }
            Err(e) => {
                log::error!("[VCPInfo] Zstd compression failed: {}", e);
            }
        }
    }
}

fn extract_metadata(msg_id: &str, val: &Value) -> Option<Value> {
    let msg_type = val.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let timestamp = val
        .get("timestamp")
        .and_then(|t| t.as_str())
        .unwrap_or(&chrono::Utc::now().to_rfc3339())
        .to_string();

    let (title, subtitle, summary, has_details) = match msg_type {
        "AGENT_PRIVATE_CHAT_PREVIEW" => {
            let agent_name = val
                .get("agentName")
                .and_then(|a| a.as_str())
                .unwrap_or("Unknown");
            let session_id = val.get("sessionId").and_then(|s| s.as_str()).unwrap_or("");
            let query = val.get("query").and_then(|q| q.as_str()).unwrap_or("");
            let response = val.get("response").and_then(|r| r.as_str()).unwrap_or("");

            let sub = if !session_id.is_empty() {
                Some(format!("Session: {}", session_id))
            } else {
                None
            };

            let mut sum = format!("💬 [USER]: {} | [AI]: {}", query, response);
            if sum.chars().count() > 50 {
                sum = sum.chars().take(50).collect::<String>() + "...";
            }
            (format!("Agent 私聊: {}", agent_name), sub, sum, true)
        }
        "META_THINKING_CHAIN" => {
            let chain_name = val
                .get("chainName")
                .and_then(|c| c.as_str())
                .unwrap_or("未知");
            let query = val.get("query").and_then(|q| q.as_str()).unwrap_or("");
            let total_stages = val.get("totalStages").and_then(|s| s.as_u64()).unwrap_or(0);
            let k_seq = val
                .get("kSequence")
                .and_then(|k| k.as_array())
                .map(|arr| {
                    format!(
                        "{:?}",
                        arr.iter()
                            .map(|v| v.as_u64().unwrap_or(0))
                            .collect::<Vec<_>>()
                    )
                })
                .unwrap_or_else(|| "[]".to_string());
            let activated = val
                .get("activatedGroups")
                .and_then(|g| g.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();

            let sub = Some(format!("阶段: {} | K序列: {}", total_stages, k_seq));
            let mut sum = if activated.is_empty() {
                query.to_string()
            } else {
                format!("[激活分组: {}] {}", activated, query)
            };
            if sum.chars().count() > 50 {
                sum = sum.chars().take(50).collect::<String>() + "...";
            }
            (format!("元思考链: {}", chain_name), sub, sum, true)
        }
        "AI_MEMO_RETRIEVAL" => {
            let diary_count = val.get("diaryCount").and_then(|c| c.as_u64()).unwrap_or(0);
            let file_count = val.get("fileCount").and_then(|f| f.as_u64()).unwrap_or(0);
            let mode = val
                .get("mode")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown");
            let chunk_count = val
                .get("tagMemoChunkCount")
                .and_then(|c| c.as_u64())
                .unwrap_or(0);
            let memo = val
                .get("extractedMemories")
                .and_then(|m| m.as_str())
                .unwrap_or("");
            let error = val.get("error").and_then(|e| e.as_str());

            let sub = Some(format!("模式: {} | 扫描: {}文件", mode, file_count));

            let mut sum = if let Some(err_msg) = error {
                format!("[Error] {}", err_msg)
            } else {
                let prefix = if chunk_count > 0 {
                    format!("[TagMemo 召回 {} Chunks] ", chunk_count)
                } else {
                    "".to_string()
                };
                format!("{}{}", prefix, memo)
            };
            if sum.chars().count() > 50 {
                sum = sum.chars().take(50).collect::<String>() + "...";
            }
            (format!("记忆回溯 ({})", diary_count), sub, sum, true)
        }
        "DailyNote" => {
            let db_name = val.get("dbName").and_then(|d| d.as_str()).unwrap_or("未知");
            let action = val
                .get("action")
                .and_then(|a| a.as_str())
                .unwrap_or("DirectRecall");
            let summary = val
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();
            (
                format!("日记直接召回: {}", db_name),
                Some(format!("模式: {}", action)),
                summary,
                false,
            )
        }
        t if t.starts_with("AGENT_DREAM_") => {
            let agent_name = val
                .get("agentName")
                .and_then(|a| a.as_str())
                .unwrap_or("Unknown");
            let title = format!("Agent梦境: {}", agent_name);
            let summary;
            let subtitle;
            let has_details;

            match t {
                "AGENT_DREAM_START" => {
                    subtitle = Some("[入梦开始]".to_string());
                    summary = val
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("")
                        .to_string();
                    has_details = false;
                }
                "AGENT_DREAM_ASSOCIATIONS" => {
                    let seed_count = val.get("seedCount").and_then(|c| c.as_u64()).unwrap_or(0);
                    let assoc_count = val
                        .get("associationCount")
                        .and_then(|c| c.as_u64())
                        .unwrap_or(0);
                    subtitle = Some(format!(
                        "[共鸣联想] 种子数: {} | 联想数: {}",
                        seed_count, assoc_count
                    ));

                    let recent = val
                        .get("recentSeedsCount")
                        .and_then(|c| c.as_u64())
                        .unwrap_or(0);
                    let mid = val
                        .get("midSeedsCount")
                        .and_then(|c| c.as_u64())
                        .unwrap_or(0);
                    let deep = val
                        .get("deepRecallsCount")
                        .and_then(|c| c.as_u64())
                        .unwrap_or(0);
                    summary = format!(
                        "种子: {} (近:{} | 中:{} | 深:{}) ➜ 联想: {}",
                        seed_count, recent, mid, deep, assoc_count
                    );
                    has_details = true;
                }
                "AGENT_DREAM_NARRATIVE" => {
                    let full_length = val
                        .get("fullLength")
                        .and_then(|l| l.as_u64())
                        .or_else(|| {
                            val.get("narrative")
                                .and_then(|n| n.as_str())
                                .map(|s| s.chars().count() as u64)
                        })
                        .unwrap_or(0);
                    subtitle = Some(format!("[梦叙事] 字数: {}", full_length));

                    let narrative = val.get("narrative").and_then(|n| n.as_str()).unwrap_or("");
                    let mut s = narrative.to_string();
                    if s.chars().count() > 50 {
                        s = s.chars().take(50).collect::<String>() + "...";
                    }
                    summary = s;
                    has_details = true;
                }
                "AGENT_DREAM_OPERATIONS" => {
                    let operation_count = val
                        .get("operationCount")
                        .and_then(|c| c.as_u64())
                        .unwrap_or(0);
                    let log_file = val
                        .get("logFile")
                        .and_then(|l| l.as_str())
                        .unwrap_or("None");
                    subtitle = Some(format!(
                        "[梦操作] 数量: {} | 日志: {}",
                        operation_count, log_file
                    ));

                    let mut merge = 0;
                    let mut delete = 0;
                    let mut insight = 0;
                    if let Some(ops) = val.get("operations").and_then(|o| o.as_array()) {
                        for op in ops {
                            match op.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                                "merge" => merge += 1,
                                "delete" => delete += 1,
                                "insight" => insight += 1,
                                _ => {}
                            }
                        }
                    }
                    summary = format!(
                        "[操作 {} 项] 待审核: {}合并, {}删除, {}感悟",
                        val.get("operations")
                            .and_then(|o| o.as_array())
                            .map(|a| a.len())
                            .unwrap_or(0),
                        merge,
                        delete,
                        insight
                    );
                    has_details = true;
                }
                "AGENT_DREAM_END" => {
                    let status = val
                        .get("status")
                        .and_then(|s| s.as_str())
                        .unwrap_or("unknown");
                    subtitle = Some(format!("[出梦 ({})]", status));
                    summary = val
                        .get("message")
                        .or_else(|| val.get("error"))
                        .and_then(|m| m.as_str())
                        .unwrap_or("")
                        .to_string();
                    has_details = false;
                }
                _ => {
                    subtitle = Some(t.replace("AGENT_DREAM_", ""));
                    summary = val
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("")
                        .to_string();
                    has_details = true;
                }
            }
            (title, subtitle, summary, has_details)
        }
        "AGENT_DREAM_SCHEDULE" => {
            let message = val.get("message").and_then(|m| m.as_str()).unwrap_or("");
            let hour = val.get("currentHour").and_then(|h| h.as_u64()).unwrap_or(0);
            let agents = val
                .get("agents")
                .and_then(|a| a.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();

            (
                "梦境自动调度".to_string(),
                Some(format!("时间: {}点", hour)),
                format!("准备入梦: {} | {}", agents, message),
                false,
            )
        }
        _ => {
            // 兜底处理 RAG_RETRIEVAL_DETAILS 以及未匹配的自定义 RAG 事件
            if val.get("dbName").is_some() && val.get("results").is_some() {
                let db_name = val.get("dbName").and_then(|d| d.as_str()).unwrap_or("未知");
                let k = val.get("k").and_then(|k| k.as_u64()).unwrap_or(0);

                // 解析是否启用时间过滤 (布尔值)
                let use_time = val
                    .get("useTime")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                // 解析策略标签
                let mut strategies: Vec<String> = Vec::new();
                if use_time {
                    strategies.push("Time".to_string());
                }
                if val
                    .get("useRerankPlus")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    strategies.push("Rerank+".to_string());
                } else if val
                    .get("useRerank")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    strategies.push("Rerank".to_string());
                }
                if val
                    .get("useTagMemo")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    let weight = val.get("tagWeight").and_then(|w| w.as_f64()).unwrap_or(0.0);
                    strategies.push(format!("TagMemo({:.2})", weight));
                }
                if val
                    .get("useGeodesicRerank")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    strategies.push("GeoRerank".to_string());
                }
                if val
                    .get("useAssociate")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    strategies.push("Associate".to_string());
                }
                if val
                    .get("useGroup")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    strategies.push("Group".to_string());
                }

                let sub = if strategies.is_empty() {
                    Some(format!("K: {}", k))
                } else {
                    Some(format!("K: {} | [{}]", k, strategies.join(" | ")))
                };

                let results_len = val
                    .get("results")
                    .and_then(|r| r.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let query = val.get("query").and_then(|q| q.as_str()).unwrap_or("");
                let mut sum = format!("[召回 {} 项] {}", results_len, query);
                if sum.chars().count() > 50 {
                    sum = sum.chars().take(50).collect::<String>() + "...";
                }
                (format!("RAG知识库: {}", db_name), sub, sum, true)
            } else {
                // 不属于 AI 认知相关事件，直接过滤
                return None;
            }
        }
    };

    Some(serde_json::json!({
        "id": msg_id,
        "type": msg_type,
        "title": title,
        "subtitle": subtitle,
        "summary": summary,
        "timestamp": timestamp,
        "hasDetails": has_details
    }))
}
