use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_executor::{BatchPullResult, PullExecutor, PushExecutor};
use crate::vcp_modules::sync_logger::{LogLevel, SyncLogger};
use crate::vcp_modules::sync_service::{emit_sync_log, Phase3Tracker, SyncCommand};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc;

pub struct BatchDiffHandler;

impl BatchDiffHandler {
    #[allow(clippy::too_many_arguments)]
    pub async fn handle_diff_batch(
        app_handle: &AppHandle,
        payload: &Value,
        http_client: &reqwest::Client,
        base_url: &str,
        token: &str,
        tracker: &Arc<Phase3Tracker>,
        tx_internal: &mpsc::UnboundedSender<SyncCommand>,
        logger: &Arc<Mutex<SyncLogger>>,
        write_queue: &Arc<DbWriteQueue>,
        pending_diff_batches: &Arc<
            tokio::sync::Mutex<
                std::collections::VecDeque<serde_json::Map<String, serde_json::Value>>,
            >,
        >,
        prerender_enabled: bool,
    ) -> Result<(), String> {
        if let Some(results) = payload["results"].as_object() {
            // 分类 topics: push_only, push_pull, pull_only
            let mut push_topic_ids: Vec<String> = Vec::new();
            let mut pull_batch: Vec<(String, Vec<String>)> = Vec::new();

            for (topic_id, result) in results {
                let to_pull_ids: Vec<String> = result["toPull"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                let to_push = result["toPush"].as_bool().unwrap_or(false);

                if !to_push && to_pull_ids.is_empty() {
                    // 无需操作，直接标记完成
                    tracker
                        .mark_completed(topic_id, logger, tx_internal, app_handle, true)
                        .await;
                    continue;
                }

                if to_push {
                    push_topic_ids.push(topic_id.clone());
                }
                if !to_pull_ids.is_empty() {
                    pull_batch.push((topic_id.clone(), to_pull_ids));
                }
            }

            let has_push = !push_topic_ids.is_empty();
            let has_pull = !pull_batch.is_empty();

            if has_push || has_pull {
                let h_in = app_handle.clone();
                let c_in = http_client.clone();
                let b_in = base_url.to_string();
                let token = token.to_string();
                let tracker_clone = tracker.clone();
                let tx_internal_msg = tx_internal.clone();
                let sync_logger_msg = logger.clone();
                let wq_in = write_queue.clone();

                let sync_state =
                    app_handle.state::<crate::vcp_modules::sync::sync_service::SyncState>();
                let uploaded_hashes = sync_state.uploaded_hashes.clone();

                // 收集所有涉及的 topic ID（去重）
                let mut all_topic_ids: HashSet<String> = HashSet::new();
                for tid in &push_topic_ids {
                    all_topic_ids.insert(tid.clone());
                }
                for (tid, _) in &pull_batch {
                    all_topic_ids.insert(tid.clone());
                }

                tauri::async_runtime::spawn(async move {
                    // 1. Push 批量（先执行，确保 push_pull 的 topic 推送完再拉取）
                    if has_push {
                        match PushExecutor::push_messages_batch(
                            &h_in,
                            &c_in,
                            &b_in,
                            &token,
                            &push_topic_ids,
                            uploaded_hashes.clone(),
                        )
                        .await
                        {
                            Ok(results) => {
                                for r in &results {
                                    if r.success {
                                        tracker_clone.mark_modified(&r.topic_id).await;
                                    } else {
                                        let err = r.error.as_deref().unwrap_or("unknown");
                                        if let Ok(mut l) = sync_logger_msg.lock() {
                                            l.log_operation(
                                                "messages",
                                                "topic",
                                                &r.topic_id,
                                                false,
                                                Some(err),
                                            );
                                        }
                                        emit_sync_log(
                                            &h_in,
                                            "error",
                                            &format!("Push failed for {}: {}", r.topic_id, err),
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                let err_msg = format!("Batch push messages failed: {}", e);
                                if let Ok(mut l) = sync_logger_msg.lock() {
                                    l.log(LogLevel::Error, "messages", &err_msg);
                                }
                                emit_sync_log(&h_in, "error", &err_msg);
                            }
                        }
                    }

                    // 2. Pull 批量（push 完成后再 pull，确保 push_pull 的 topic 数据已合并）
                    if has_pull {
                        match PullExecutor::pull_messages_batch(
                            &h_in,
                            &c_in,
                            &b_in,
                            &token,
                            &pull_batch,
                            &wq_in,
                            prerender_enabled,
                        )
                        .await
                        {
                            Ok(results) => {
                                let result_map: std::collections::HashMap<&str, &BatchPullResult> =
                                    results.iter().map(|r| (r.topic_id.as_str(), r)).collect();
                                for (tid, _) in &pull_batch {
                                    if let Some(r) = result_map.get(tid.as_str()) {
                                        if r.success {
                                            tracker_clone.mark_modified(tid).await;
                                        } else {
                                            let err = r.error.as_deref().unwrap_or("unknown");
                                            if let Ok(mut l) = sync_logger_msg.lock() {
                                                l.log_operation(
                                                    "messages",
                                                    "topic",
                                                    tid,
                                                    false,
                                                    Some(err),
                                                );
                                            }
                                            emit_sync_log(
                                                &h_in,
                                                "error",
                                                &format!("Pull failed for {}: {}", tid, err),
                                            );
                                        }
                                    } else {
                                        if let Ok(mut l) = sync_logger_msg.lock() {
                                            l.log_operation(
                                                "messages",
                                                "topic",
                                                tid,
                                                false,
                                                Some("not in batch response"),
                                            );
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                let err_msg = format!("Batch pull messages failed: {}", e);
                                if let Ok(mut l) = sync_logger_msg.lock() {
                                    l.log(LogLevel::Error, "messages", &err_msg);
                                }
                                emit_sync_log(&h_in, "error", &err_msg);
                            }
                        }
                    }

                    // 3. 所有 topic 标记完成
                    for tid in &all_topic_ids {
                        tracker_clone
                            .mark_completed(tid, &sync_logger_msg, &tx_internal_msg, &h_in, false)
                            .await;
                    }

                    println!(
                        "[SyncService] Phase 3 batch done: push={} pull={}",
                        push_topic_ids.len(),
                        pull_batch.len()
                    );
                });
            }

            // 当前批次处理完毕，发送下一批（如果还有）
            let mut pending = pending_diff_batches.lock().await;
            if let Some(next_batch) = pending.pop_front() {
                println!(
                    "[SyncService] Sending next diff batch, {} remaining",
                    pending.len()
                );
                let msg = json!({
                    "type": "SYNC_MESSAGE_DIFF_BATCH",
                    "topics": next_batch,
                });
                let _ = tx_internal.send(SyncCommand::SendWsMessage(msg));
            }
        }
        Ok(())
    }
}
