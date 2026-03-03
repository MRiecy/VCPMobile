use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Runtime, Emitter};
use std::sync::Arc;
use dashmap::DashMap;
use tokio::sync::oneshot;
use futures_util::StreamExt;
use reqwest::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use std::time::Duration;
use tokio_util::codec::{FramedRead, LinesCodec};
use tokio_util::io::StreamReader;
use std::io::Error as IoError;
use futures_util::TryStreamExt;
use std::path::PathBuf;
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
    pub vcp_url: String,            // VCP服务器URL
    pub vcp_api_key: String,        // API密钥
    pub messages: Vec<Value>,       // 消息数组
    pub model_config: Value,        // 模型配置 (包含 model, stream, temperature 等)
    pub message_id: String,         // 消息ID (用于跟踪和中止)
    pub context: Option<Value>,     // 上下文信息 (agentId, topicId等)
    pub stream_channel: Option<String>, // 流式数据频道名称 (默认为 vcp-stream-event)
}

/// 流式事件结构体，用于向前端发送数据
#[derive(Debug, Serialize, Clone)]
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
    app.path().app_data_dir().unwrap_or_else(|_| PathBuf::from("AppData"))
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
    println!("[VCPClient] sendToVCP called for messageId: {}, context: {:?}", payload.message_id, payload.context);
    let app_data_path = get_app_data_path(&app).await;
    let stream_channel = payload.stream_channel.clone().unwrap_or_else(|| "vcp-stream-event".to_string());
    
    // === 0. 数据验证和规范化 (Data Validation and Normalization) ===
    let mut messages: Vec<Value> = payload.messages.into_iter().map(|mut msg| {
        if !msg.is_object() {
            println!("[VCPClient] Invalid message object: {:?}", msg);
            return json!({"role": "system", "content": "[Invalid message]"});
        }
        
        let content = msg.get("content").cloned().unwrap_or(Value::Null);
        
        if content.is_object() {
            if let Some(text) = content.get("text") {
                // 如果 content.text 存在，提取它
                msg["content"] = text.clone();
            } else {
                // 否则序列化整个对象
                msg["content"] = json!(content.to_string());
                println!("[VCPClient] Message content is object without text field, stringifying: {:?}", content);
            }
        } else if content.is_array() {
            // 保持数组形式 (多模态)
        } else if !content.is_string() && !content.is_null() {
            // 转换非字符串/非空值为字符串
            msg["content"] = json!(content.to_string());
            println!("[VCPClient] Converting non-string content to string: {:?}", content);
        }
        msg
    }).collect();

    // === 1. 读取设置与动态路由切换 (URL Switching) ===
    let settings_path = app_data_path.join("settings.json");
    let mut enable_vcp_tool_injection = false;
    let mut agent_music_control = false;
    let mut enable_agent_bubble_theme = false;

    if settings_path.exists() {
        match std::fs::read_to_string(&settings_path) {
            Ok(content) => {
                if let Ok(settings) = serde_json::from_str::<Value>(&content) {
                    enable_vcp_tool_injection = settings["enableVcpToolInjection"].as_bool().unwrap_or(false);
                    agent_music_control = settings["agentMusicControl"].as_bool().unwrap_or(false);
                    enable_agent_bubble_theme = settings["enableAgentBubbleTheme"].as_bool().unwrap_or(false);
                } else {
                    println!("[VCPClient] Error parsing settings.json. Proceeding with defaults.");
                }
            }
            Err(e) => {
                println!("[VCPClient] Error reading settings or switching URL: {}. Proceeding with original URL.", e);
            }
        }
    }

    let mut final_url = payload.vcp_url.clone();
    if enable_vcp_tool_injection {
        if let Ok(mut url) = Url::parse(&final_url) {
            url.set_path("/v1/chatvcp/completions");
            final_url = url.to_string();
            println!("[VCPClient] VCP tool injection is ON. URL switched to: {}", final_url);
        }
    } else {
        println!("[VCPClient] VCP tool injection is OFF. Using original URL: {}", final_url);
    }

    // === 2. 上下文注入 (Context Injection) ===
    
    // 确保消息列表中存在 System 消息
    let has_system = messages.iter().any(|m| m["role"] == "system");
    if !has_system {
        messages.insert(0, json!({"role": "system", "content": ""}));
    }

    let mut top_parts = Vec::new();
    let mut bottom_parts = Vec::new();
    
    // 3.1 音乐状态注入
    // 尝试读取当前播放状态 (在移动端重构中，未来应通过 MusicManager 模块获取)
    let music_state_path = app_data_path.join("music_state.json");
    if music_state_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&music_state_path) {
            if let Ok(m_state) = serde_json::from_str::<Value>(&content) {
                if let (Some(title), Some(artist)) = (m_state["title"].as_str(), m_state["artist"].as_str()) {
                    let album = m_state["album"].as_str().unwrap_or("未知专辑");
                    bottom_parts.push(format!("[当前播放音乐：{} - {} ({})]", title, artist, album));
                }
            } else {
                println!("[VCPClient] Failed to inject music info: Invalid music_state.json format");
            }
        } else {
            println!("[VCPClient] Failed to inject music info: Error reading music_state.json");
        }
    }

    // 3.2 播放列表与点歌台注入
    if agent_music_control {
        let songlist_path = app_data_path.join("songlist.json");
        if songlist_path.exists() {
            match std::fs::read_to_string(&songlist_path) {
                Ok(content) => {
                    if let Ok(songlist) = serde_json::from_str::<Value>(&content) {
                        if let Some(songs) = songlist.as_array() {
                            let titles: Vec<&str> = songs.iter()
                                .filter_map(|s| s["title"].as_str())
                                .collect();
                            if !titles.is_empty() {
                                top_parts.push(format!("[播放列表——\n{}\n]", titles.join("\n")));
                            }
                        }
                    } else {
                        println!("[VCPClient] Failed to inject music info: Invalid songlist.json format");
                    }
                }
                Err(e) => {
                    println!("[VCPClient] Failed to inject music info: Error reading songlist.json: {}", e);
                }
            }
        }
        bottom_parts.push("点歌台{{VCPMusicController}}".to_string());
    }

    // 3.3 UI 规范要求注入 (Agent Bubble Theme)
    if enable_agent_bubble_theme {
        bottom_parts.push("输出规范要求：{{VarDivRender}}".to_string());
    }

    // 应用注入到 System Message
    if !top_parts.is_empty() || !bottom_parts.is_empty() {
        for m in messages.iter_mut() {
            if m["role"] == "system" {
                let original_content = m["content"].as_str().unwrap_or("");
                let mut final_parts = Vec::new();
                if !top_parts.is_empty() { final_parts.push(top_parts.join("\n")); }
                if !original_content.is_empty() { final_parts.push(original_content.to_string()); }
                if !bottom_parts.is_empty() { final_parts.push(bottom_parts.join("\n")); }
                
                m["content"] = json!(final_parts.join("\n\n").trim());
                break;
            }
        }
    }

    // === 4. 准备请求体 (Request Body) ===
    let is_stream = payload.model_config["stream"].as_bool().unwrap_or(false);
    let mut request_body = payload.model_config.clone();
    if let Some(obj) = request_body.as_object_mut() {
        obj.insert("messages".to_string(), json!(messages));
        obj.insert("requestId".to_string(), json!(payload.message_id));
        obj.insert("stream".to_string(), json!(is_stream));
    }

    if let Ok(serialized) = serde_json::to_string(&request_body) {
        let preview = if serialized.len() > 100 { &serialized[..100] } else { &serialized };
        println!("[VCPClient] Request body preview: {}...", preview);
    } else {
        println!("[VCPClient] Failed to serialize request body");
    }

    // === 5. 配置网络请求 (HTTP Client) ===
    let client = Client::builder()
        .timeout(Duration::from_secs(30)) // 30秒超时限制
        .build()
        .map_err(|e| e.to_string())?;

    // 创建并注册中止信号
    let (abort_tx, abort_rx) = oneshot::channel();
    state.0.insert(payload.message_id.clone(), abort_tx);
    println!("[VCPClient] Registered AbortController for messageId: {}. Active requests: {}", payload.message_id, state.0.len());

    let message_id = payload.message_id.clone();
    let context = payload.context.clone();
    let api_key = payload.vcp_api_key.clone();

    println!("[VCPClient] Sending request to: {}", final_url);

    if is_stream {
        // === 6. 流式处理模式 (SSE Parsing) ===
        let app_handle = app.clone();
        let message_id_inner = message_id.clone();
        let context_inner = context.clone();
        let state_inner = state.0.clone();

        tokio::spawn(async move {
            println!("[VCPClient] Starting stream processing for messageId: {}", message_id_inner);
            let res_future = client.post(&final_url)
                .header(AUTHORIZATION, format!("Bearer {}", api_key))
                .header(CONTENT_TYPE, "application/json")
                .json(&request_body)
                .send();

            tokio::select! {
                // 处理任务中止信号
                _ = abort_rx => {
                    println!("[VCPClient] Request aborted for messageId: {}", message_id_inner);
                    let _ = app_handle.emit(&stream_channel, StreamEvent {
                        r#type: "error".to_string(),
                        chunk: None,
                        message_id: message_id_inner.clone(),
                        context: context_inner.clone(),
                        error: Some("请求已中止".to_string()),
                    });
                }
                // 处理响应流
                response_res = res_future => {
                    match response_res {
                        Ok(resp) if resp.status().is_success() => {
                            // 使用 StreamReader 和 LinesCodec 建立行读取器，处理 SSE 格式
                            let stream = resp.bytes_stream().map_err(|e| IoError::new(std::io::ErrorKind::Other, e));
                            let reader = StreamReader::new(stream);
                            let mut lines = FramedRead::new(reader, LinesCodec::new());

                            while let Some(line_res) = lines.next().await {
                                if let Ok(line) = line_res {
                                    let line_str: String = line;
                                    if line_str.trim().is_empty() { continue; }
                                    
                                    if line_str.starts_with("data: ") {
                                        let data = line_str.trim_start_matches("data: ").trim();
                                        
                                        // 处理 [DONE] 信号
                                        if data == "[DONE]" {
                                            println!("[VCPClient] Stream [DONE] for messageId: {}", message_id_inner);
                                            let _ = app_handle.emit(&stream_channel, StreamEvent {
                                                r#type: "end".to_string(),
                                                chunk: None,
                                                message_id: message_id_inner.clone(),
                                                context: context_inner.clone(),
                                                error: None,
                                            });
                                            break;
                                        }
                                        
                                        // 解析 JSON Chunk 并推送到前端
                                        match serde_json::from_str::<Value>(data) {
                                            Ok(chunk) => {
                                                let _ = app_handle.emit(&stream_channel, StreamEvent {
                                                    r#type: "data".to_string(),
                                                    chunk: Some(chunk),
                                                    message_id: message_id_inner.clone(),
                                                    context: context_inner.clone(),
                                                    error: None,
                                                });
                                            }
                                            Err(e) => {
                                                println!("[VCPClient] Failed to parse stream chunk for messageId: {}: {}, 原始数据: {}", message_id_inner, e, data);
                                            }
                                        }
                                    }
                                }
                            }
                            println!("[VCPClient] Stream ended for messageId: {}", message_id_inner);
                            println!("[VCPClient] Stream lock released for messageId: {}", message_id_inner);
                        }
                        Ok(resp) => {
                            // HTTP 错误响应处理
                            let status = resp.status();
                            let text = resp.text().await.unwrap_or_default();
                            println!("[VCPClient] VCP request failed. Status: {}, Response Text: {}", status, text);
                            let _ = app_handle.emit(&stream_channel, StreamEvent {
                                r#type: "error".to_string(),
                                chunk: None,
                                message_id: message_id_inner.clone(),
                                context: context_inner.clone(),
                                error: Some(format!("VCP服务器错误: {} - {}", status, text)),
                            });
                        }
                        Err(e) => {
                            // 网络请求异常处理
                            if e.is_timeout() {
                                println!("[VCPClient] Timeout triggered for messageId: {}", message_id_inner);
                            }
                            println!("[VCPClient] Request error: {}", e);
                            let _ = app_handle.emit(&stream_channel, StreamEvent {
                                r#type: "error".to_string(),
                                chunk: None,
                                message_id: message_id_inner.clone(),
                                context: context_inner.clone(),
                                error: Some(format!("网络请求异常: {}", e)),
                            });
                            println!("[VCPClient] Stream processing error for messageId: {}: {}", message_id_inner, e);
                        }
                    }
                }
            }
            // 最终清理活跃记录
            state_inner.remove(&message_id_inner);
            println!("[VCPClient] Cleaned up AbortController for messageId: {}. Active requests: {}", message_id_inner, state_inner.len());
            println!("[VCPClient] Stream processing completed for messageId: {}", message_id_inner);
        });

        Ok(json!({"streamingStarted": true}))
    } else {
        // === 7. 非流式响应模式 ===
        println!("[VCPClient] Processing non-streaming response");
        let response = client.post(&final_url)
            .header(AUTHORIZATION, format!("Bearer {}", api_key))
            .header(CONTENT_TYPE, "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| {
                println!("[VCPClient] Request error: {}", e);
                format!("VCP请求失败: {}", e)
            })?;

        state.0.remove(&message_id);
        println!("[VCPClient] Cleaned up AbortController for messageId: {}. Active requests: {}", message_id, state.0.len());

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            println!("[VCPClient] VCP request failed. Status: {}, Response Text: {}", status, text);
            return Err(format!("VCP响应错误: {} - {}", status, text));
        }

        let vcp_response = response.json::<Value>().await.map_err(|e| format!("JSON解析失败: {}", e))?;
        Ok(json!({"response": vcp_response, "context": context}))
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
    println!("[VCPClient] interruptRequest called for messageId: {}. Active requests: {}", message_id, state.0.len());
    if let Some((_, sender)) = state.0.remove(&message_id) {
        println!("[VCPClient] Found AbortController for messageId: {}, aborting...", message_id);
        let _ = sender.send(());
        println!("[VCPClient] Request interrupted for messageId: {}. Remaining active requests: {}", message_id, state.0.len());
        Ok(json!({"success": true, "message": format!("Request {} interrupted", message_id)}))
    } else {
        println!("[VCPClient] No active request found for messageId: {}", message_id);
        Err(format!("Request {} not found", message_id))
    }
}
