use dashmap::DashMap;
use futures_util::StreamExt;
use futures_util::TryStreamExt;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Error as IoError;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::oneshot;
use tokio_util::codec::{FramedRead, LinesCodec};
use tokio_util::io::StreamReader;
use url::Url;

/// =================================================================
/// vcp_modules/vcp_client.rs - 统一的 VCP 请求处理模块 (Rust 重写版)
/// =================================================================
/// 该模块对应原项目的 modules/vcpClient.js，负责处理所有与 VCP 服务器的通信。
/// 包含动态路由、上下文注入（音乐、UI 规范）、流式 SSE 解析以及请求中止机制。
/// 请求参数结构体
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcpRequestPayload {
    pub vcp_url: String,                // VCP服务器URL
    pub vcp_api_key: String,            // API密钥
    pub messages: Vec<Value>,           // 消息数组
    pub model_config: Value,            // 模型配置 (包含 model, stream, temperature 等)
    pub message_id: String,             // 消息ID (用于跟踪和中止)
    pub context: Option<Value>,         // 上下文信息 (agentId, topicId等)
    pub stream_channel: Option<String>, // 流式数据频道名称 (默认为 vcp-stream-event)
}

/// 流式事件结构体，用于向前端发送数据
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StreamEvent {
    pub r#type: String,         // 事件类型: "data", "end", "error"
    pub chunk: Option<Value>,   // 数据块 (仅 type="data" 时有效)
    pub message_id: String,     // 消息ID
    pub context: Option<Value>, // 透传的上下文信息
    pub error: Option<String>,  // 错误信息 (仅 type="error" 时有效)
}

/// 全局活跃请求管理器，使用 DashMap 存储中止信号发送端
/// messageId -> oneshot::Sender
pub struct ActiveRequests(pub Arc<DashMap<String, oneshot::Sender<()>>>);

impl Default for ActiveRequests {
    fn default() -> Self {
        println!("[VCPClient] Initialized successfully.");
        Self(Arc::new(DashMap::new()))
    }
}

/// 内部辅助函数：获取应用程序数据目录
async fn get_app_data_path<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("AppData"))
}

/// 核心请求函数：sendToVCP
/// 对应 JS 版的 sendToVCP。处理逻辑：
/// 1. 数据验证与规范化 (通过 Rust 类型系统自动处理部分)
/// 2. 动态路由切换 (根据设置注入 /v1/chatvcp/completions)
/// 3. 上下文注入 (音乐信息、UI 规范要求)
/// 4. 发起 HTTP 请求 (支持流式和非流式)
/// 5. 注册 AbortController 实现中止机制
#[tauri::command]
#[allow(non_snake_case)]
pub async fn sendToVCP<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, ActiveRequests>,
    payload: VcpRequestPayload,
) -> Result<Value, String> {
    let (res, _is_aborted) = perform_vcp_request(&app, state.0.clone(), payload).await?;
    Ok(res)
}

/// 核心请求实现函数，可供 Tauri Command 或 内部 Rust 模块(如 GroupOrchestrator) 调用
/// 返回 Result<(全量内容/响应体, 是否被中止), 错误信息>
pub async fn perform_vcp_request<R: Runtime>(
    app: &AppHandle<R>,
    active_requests: Arc<DashMap<String, oneshot::Sender<()>>>,
    payload: VcpRequestPayload,
) -> Result<(Value, bool), String> {
    println!(
        "[VCPClient] perform_vcp_request called for messageId: {}, context: {:?}",
        payload.message_id, payload.context
    );
    let app_data_path = get_app_data_path(app).await;
    let stream_channel = payload
        .stream_channel
        .clone()
        .unwrap_or_else(|| "vcp-stream".to_string());

    // === 0. 数据验证和规范化 ===
    // ... (rest of data normalization unchanged)
    let mut messages: Vec<Value> = payload
        .messages
        .into_iter()
        .map(|mut msg| {
            if !msg.is_object() {
                return json!({"role": "system", "content": "[Invalid message]"});
            }
            let content = msg.get("content").cloned().unwrap_or(Value::Null);
            if content.is_object() {
                if let Some(text) = content.get("text") {
                    msg["content"] = text.clone();
                } else {
                    msg["content"] = json!(content.to_string());
                }
            } else if !content.is_array() && !content.is_string() && !content.is_null() {
                msg["content"] = json!(content.to_string());
            }
            msg
        })
        .collect();

    // === 1. 读取设置与动态路由切换 ===
    let settings_path = app_data_path.join("settings.json");
    let mut enable_vcp_tool_injection = false;
    let mut agent_music_control = false;
    let mut enable_agent_bubble_theme = false;

    if let Ok(content) = tokio::fs::read_to_string(&settings_path).await {
        if let Ok(settings) = serde_json::from_str::<Value>(&content) {
            enable_vcp_tool_injection = settings["enableVcpToolInjection"]
                .as_bool()
                .unwrap_or(false);
            agent_music_control = settings["agentMusicControl"].as_bool().unwrap_or(false);
            enable_agent_bubble_theme = settings["enableAgentBubbleTheme"]
                .as_bool()
                .unwrap_or(false);
        }
    }

    let mut final_url = payload.vcp_url.clone();
    if enable_vcp_tool_injection {
        if let Ok(mut url) = Url::parse(&final_url) {
            url.set_path("/v1/chatvcp/completions");
            final_url = url.to_string();
        }
    }

    // === 2. 上下文注入 ===
    let has_system = messages.iter().any(|m| m["role"] == "system");
    if !has_system {
        messages.insert(0, json!({"role": "system", "content": ""}));
    }

    let mut top_parts = Vec::new();
    let mut bottom_parts = Vec::new();

    // 3.1 音乐状态注入
    let music_state_path = app_data_path.join("music_state.json");
    if let Ok(content) = tokio::fs::read_to_string(&music_state_path).await {
        if let Ok(m_state) = serde_json::from_str::<Value>(&content) {
            if let (Some(title), Some(artist)) =
                (m_state["title"].as_str(), m_state["artist"].as_str())
            {
                let album = m_state["album"].as_str().unwrap_or("未知专辑");
                bottom_parts.push(format!(
                    "[当前播放音乐：{} - {} ({})]",
                    title, artist, album
                ));
            }
        }
    }

    // 3.2 播放列表与点歌台注入
    if agent_music_control {
        let songlist_path = app_data_path.join("songlist.json");
        if let Ok(content) = tokio::fs::read_to_string(&songlist_path).await {
            if let Ok(songlist) = serde_json::from_str::<Value>(&content) {
                if let Some(songs) = songlist.as_array() {
                    let titles: Vec<&str> =
                        songs.iter().filter_map(|s| s["title"].as_str()).collect();
                    if !titles.is_empty() {
                        top_parts.push(format!("[播放列表——\n{}\n]", titles.join("\n")));
                    }
                }
            }
        }
        bottom_parts.push("点歌台{{VCPMusicController}}".to_string());
    }

    // 3.3 UI 规范要求注入
    if enable_agent_bubble_theme {
        bottom_parts.push("输出规范要求：{{VarDivRender}}".to_string());
    }

    // 应用注入到 System Message
    if !top_parts.is_empty() || !bottom_parts.is_empty() {
        for m in messages.iter_mut() {
            if m["role"] == "system" {
                let original_content = m["content"].as_str().unwrap_or("");
                let mut final_parts = Vec::new();
                if !top_parts.is_empty() {
                    final_parts.push(top_parts.join("\n"));
                }
                if !original_content.is_empty() {
                    final_parts.push(original_content.to_string());
                }
                if !bottom_parts.is_empty() {
                    final_parts.push(bottom_parts.join("\n"));
                }
                m["content"] = json!(final_parts.join("\n\n").trim());
                break;
            }
        }
    }

    // === 4. 准备请求体 ===
    let is_stream = payload.model_config["stream"].as_bool().unwrap_or(false);
    let mut request_body = payload.model_config.clone();
    if let Some(obj) = request_body.as_object_mut() {
        obj.insert("messages".to_string(), json!(messages));
        obj.insert("requestId".to_string(), json!(payload.message_id));
        obj.insert("stream".to_string(), json!(is_stream));
    }

    // === 5. 配置网络请求 ===
    let client = Client::builder()
        // 移除硬超时限制，对齐桌面端，让长思考模型有充足时间生成，连接管理交给 VCP 服务器
        .build()
        .map_err(|e| e.to_string())?;

    // 创建并注册中止信号
    let (abort_tx, abort_rx) = oneshot::channel();
    active_requests.insert(payload.message_id.clone(), abort_tx);

    let message_id = payload.message_id.clone();
    let context = payload.context.clone();
    let api_key = payload.vcp_api_key.clone();

    if is_stream {
        // === 6. 流式处理模式 (同步等待，以便串行调用) ===
        let app_handle = app.clone();
        let message_id_inner = message_id.clone();
        let context_inner = context.clone();
        let active_requests_inner = active_requests.clone();

        let mut full_content = String::new();
        let mut is_aborted = false;
        let mut abort_rx = abort_rx; // 取得所有权进入循环

        let res_future = client
            .post(&final_url)
            .header(AUTHORIZATION, format!("Bearer {}", api_key))
            .header(CONTENT_TYPE, "application/json")
            .json(&request_body)
            .send();

        tokio::select! {
            _ = &mut abort_rx => {
                let _ = app_handle.emit(&stream_channel, StreamEvent {
                    r#type: "error".to_string(),
                    chunk: None,
                    message_id: message_id_inner.clone(),
                    context: context_inner.clone(),
                    error: Some("请求已中止".to_string()),
                });
                active_requests_inner.remove(&message_id_inner);
                return Ok((json!({ "fullContent": "", "streamingStarted": false }), true));
            }
            response_res = res_future => {
                match response_res {
                    Ok(resp) if resp.status().is_success() => {
                        let stream = resp.bytes_stream().map_err(IoError::other);
                        let reader = StreamReader::new(stream);
                        let mut lines = FramedRead::new(reader, LinesCodec::new());

                        while let Some(line_res) = lines.next().await {
                            // 每读取一行前，检查是否收到中止信号 (Deep Polling)
                            if abort_rx.try_recv().is_ok() {
                                is_aborted = true;
                                println!("[VCPClient] Stream deep-polling detected abort for message: {}", message_id_inner);
                                // 发送结束符告知前端
                                let _ = app_handle.emit(&stream_channel, StreamEvent {
                                    r#type: "end".to_string(),
                                    chunk: None,
                                    message_id: message_id_inner.clone(),
                                    context: context_inner.clone(),
                                    error: None,
                                });
                                break;
                            }

                            if let Ok(line) = line_res {
                                if line.trim().is_empty() { continue; }
                                if line.starts_with("data: ") {
                                    let data = line.trim_start_matches("data: ").trim();
                                    if data == "[DONE]" {
                                        let _ = app_handle.emit(&stream_channel, StreamEvent {
                                            r#type: "end".to_string(),
                                            chunk: None,
                                            message_id: message_id_inner.clone(),
                                            context: context_inner.clone(),
                                            error: None,
                                        });
                                        break;
                                    }
                                    if let Ok(chunk) = serde_json::from_str::<Value>(data) {
                                        // 累加全量内容
                                        if let Some(text) = chunk["choices"][0]["delta"]["content"].as_str() {
                                            full_content.push_str(text);
                                        }
                                        let _ = app_handle.emit(&stream_channel, StreamEvent {
                                            r#type: "data".to_string(),
                                            chunk: Some(chunk),
                                            message_id: message_id_inner.clone(),
                                            context: context_inner.clone(),
                                            error: None,
                                        });
                                    }
                                }
                            }
                        }
                    }
                    Ok(resp) => {
                        let status = resp.status();
                        let text = resp.text().await.unwrap_or_default();
                        let _ = app_handle.emit(&stream_channel, StreamEvent {
                            r#type: "error".to_string(),
                            chunk: None,
                            message_id: message_id_inner.clone(),
                            context: context_inner.clone(),
                            error: Some(format!("VCP服务器错误: {} - {}", status, text)),
                        });
                        active_requests_inner.remove(&message_id_inner);
                        return Err(format!("VCP Error: {}", status));
                    }
                    Err(e) => {
                        let _ = app_handle.emit(&stream_channel, StreamEvent {
                            r#type: "error".to_string(),
                            chunk: None,
                            message_id: message_id_inner.clone(),
                            context: context_inner.clone(),
                            error: Some(format!("网络请求异常: {}", e)),
                        });
                        active_requests_inner.remove(&message_id_inner);
                        return Err(e.to_string());
                    }
                }
            }
        }

        active_requests_inner.remove(&message_id_inner);
        Ok((
            json!({ "fullContent": full_content, "streamingStarted": true }),
            is_aborted,
        ))
    } else {
        // === 7. 非流式响应模式 ===
        let response = client
            .post(&final_url)
            .header(AUTHORIZATION, format!("Bearer {}", api_key))
            .header(CONTENT_TYPE, "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| format!("VCP请求失败: {}", e))?;

        active_requests.remove(&message_id);

        if !response.status().is_success() {
            let status = response.status();
            return Err(format!("VCP响应错误: {}", status));
        }

        let vcp_response = response
            .json::<Value>()
            .await
            .map_err(|e| format!("JSON解析失败: {}", e))?;
        Ok((json!({"response": vcp_response, "context": context}), false))
    }
}

/// 中止请求 Command: interruptRequest
/// 通过 messageId 立即触发对应的 oneshot 信号
#[tauri::command]
#[allow(non_snake_case)]
pub fn interruptRequest(
    state: tauri::State<'_, ActiveRequests>,
    message_id: String,
) -> Result<Value, String> {
    println!(
        "[VCPClient] interruptRequest called for messageId: {}. Active requests: {}",
        message_id,
        state.0.len()
    );
    if let Some((_, sender)) = state.0.remove(&message_id) {
        println!(
            "[VCPClient] Found AbortController for messageId: {}, aborting...",
            message_id
        );
        let _ = sender.send(());
        println!(
            "[VCPClient] Request interrupted for messageId: {}. Remaining active requests: {}",
            message_id,
            state.0.len()
        );
        Ok(json!({"success": true, "message": format!("Request {} interrupted", message_id)}))
    } else {
        println!(
            "[VCPClient] No active request found for messageId: {}",
            message_id
        );
        Err(format!("Request {} not found", message_id))
    }
}

/// 测试 VCP 后端连接状态并获取模型列表 (对齐桌面端 main.js fetchAndCacheModels 逻辑)
#[tauri::command]
pub async fn test_vcp_connection(vcp_url: String, vcp_api_key: String) -> Result<Value, String> {
    println!(
        "[VCPClient] test_vcp_connection called for URL: {}",
        vcp_url
    );

    // 对齐桌面端原汁原味的逻辑：
    // const urlObject = new URL(vcpServerUrl);
    // const baseUrl = `${urlObject.protocol}//${urlObject.host}`;
    // const modelsUrl = new URL('/v1/models', baseUrl).toString();

    let url_object = match Url::parse(&vcp_url) {
        Ok(url) => url,
        Err(e) => return Err(format!("URL 解析失败: {}", e)),
    };

    // 对齐 JS 的 urlObject.host (包含端口号)
    let port_str = match url_object.port() {
        Some(p) => format!(":{}", p),
        None => "".to_string(),
    };
    let host_with_port = format!("{}{}", url_object.host_str().unwrap_or(""), port_str);
    let base_url = format!("{}://{}", url_object.scheme(), host_with_port);

    let models_url = if base_url.ends_with('/') {
        format!("{}v1/models", base_url)
    } else {
        format!("{}/v1/models", base_url)
    };

    println!(
        "[VCPClient] Testing connection to (Original Logic): {}",
        models_url
    );

    let client = Client::builder()
        .timeout(Duration::from_secs(10)) // 测试连接 10s 超时即可
        .build()
        .map_err(|e| e.to_string())?;

    let res = client
        .get(&models_url)
        .header(AUTHORIZATION, format!("Bearer {}", vcp_api_key))
        .send()
        .await
        .map_err(|e| format!("网络请求失败: {}", e))?;

    let status = res.status();
    if status.is_success() {
        let json_res: Value = res
            .json()
            .await
            .map_err(|e| format!("JSON解析失败: {}", e))?;

        // 尝试提取模型数量，对齐桌面端 `cachedModels = data.data || []`
        let model_count = json_res
            .get("data")
            .and_then(|data| data.as_array())
            .map(|arr| arr.len())
            .unwrap_or(0);

        Ok(json!({
            "success": true,
            "status": status.as_u16(),
            "modelCount": model_count,
            "models": json_res
        }))
    } else {
        let text = res.text().await.unwrap_or_default();
        Err(format!("验证失败 ({}): {}", status.as_u16(), text))
    }
}
