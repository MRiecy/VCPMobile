use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::message_service;
use futures_util::StreamExt;
use serde::Deserialize;
use tokio::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum SyncMessage {
    #[serde(rename = "SYNC_DELTA")]
    SyncDelta {
        #[serde(rename = "topicId")]
        topic_id: String,
        #[serde(rename = "agentId")]
        agent_id: String,
        delta: DeltaContent,
    },
}

#[derive(Debug, Deserialize)]
struct DeltaContent {
    added: Vec<ChatMessage>,
    updated: Vec<ChatMessage>,
    deleted: Vec<String>,
}

pub struct SyncDaemonState {
    pub is_running: Mutex<bool>,
    pub stop_signal: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl SyncDaemonState {
    pub fn new() -> Self {
        Self {
            is_running: Mutex::new(false),
            stop_signal: Mutex::new(None),
        }
    }
}

/// 启动同步守护进程
pub async fn start_daemon(
    app_handle: AppHandle,
    ws_url: String, // 例如 ws://192.168.1.100:5975?token=xxx
) -> Result<(), String> {
    let state = app_handle.state::<SyncDaemonState>();
    let mut running = state.is_running.lock().await;
    
    if *running {
        return Err("Sync daemon is already running".to_string());
    }

    let (tx, mut rx) = tokio::sync::oneshot::channel();
    *state.stop_signal.lock().await = Some(tx);
    *running = true;

    let app_clone = app_handle.clone();
    
    tokio::spawn(async move {
        let mut retry_delay = Duration::from_secs(2);
        let max_retry_delay = Duration::from_secs(60);

        loop {
            // 检查停止信号
            if rx.try_recv().is_ok() {
                println!("[SyncDaemon] Stopping daemon...");
                break;
            }

            println!("[SyncDaemon] Attempting to connect to {}", ws_url);
            
            match connect_async(&ws_url).await {
                Ok((mut ws_stream, _)) => {
                    println!("[SyncDaemon] Connected to desktop sync node.");
                    retry_delay = Duration::from_secs(2); // 重置重试延迟

                    loop {
                        tokio::select! {
                            msg = ws_stream.next() => {
                                match msg {
                                    Some(Ok(Message::Text(text))) => {
                                        if let Ok(sync_msg) = serde_json::from_str::<SyncMessage>(&text) {
                                            handle_sync_message(&app_clone, sync_msg).await;
                                        }
                                    }
                                    Some(Ok(Message::Close(_))) | None => {
                                        println!("[SyncDaemon] Connection closed by server.");
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                            _ = &mut rx => {
                                println!("[SyncDaemon] Stopping daemon from select...");
                                return;
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[SyncDaemon] Connection error: {}. Retrying in {:?}...", e, retry_delay);
                }
            }

            tokio::time::sleep(retry_delay).await;
            retry_delay = std::cmp::min(retry_delay * 2, max_retry_delay);
        }

        let binding = app_clone.state::<SyncDaemonState>();
        let mut running = binding.is_running.lock().await;
        *running = false;
    });

    Ok(())
}

async fn handle_sync_message(app_handle: &AppHandle, msg: SyncMessage) {
    match msg {
        SyncMessage::SyncDelta { topic_id, agent_id, delta } => {
            println!("[SyncDaemon] Received delta for topic {}: +{} ~{} -{}", 
                topic_id, delta.added.length(), delta.updated.length(), delta.deleted.length());
            
            let db_state = app_handle.state::<DbState>();
            
            let res = message_service::apply_sync_delta(
                app_handle,
                &db_state.pool,
                &agent_id,
                &topic_id,
                delta.added,
                delta.updated,
                delta.deleted,
            ).await;

            if let Err(e) = res {
                eprintln!("[SyncDaemon] Failed to apply sync delta: {}", e);
            } else {
                // 触发 UI 刷新事件
                // 注意：由于我们可能同时收到多条消息变动，直接触发 topic-index-updated 
                // 让前端 TopicStore 感知到最新的 msg_count 和 updated_at
                // 前端已经有监听 topic-index-updated 的逻辑
                println!("[SyncDaemon] Sync delta applied successfully for {}", topic_id);
            }
        }
    }
}

// 辅助 trait 用于 Vec
trait VecExt {
    fn length(&self) -> usize;
}
impl<T> VecExt for Vec<T> {
    fn length(&self) -> usize { self.len() }
}
