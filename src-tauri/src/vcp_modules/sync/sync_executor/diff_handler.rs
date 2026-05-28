use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_executor::{PullExecutor, PushExecutor};
use crate::vcp_modules::sync_logger::SyncLogger;
use crate::vcp_modules::sync_service::{emit_sync_log, SyncCommand};
use crate::vcp_modules::sync_types::SyncDataType;
use futures_util::StreamExt;
use serde_json::{json, Value};
use sqlx::Row;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

pub struct DiffHandler;

impl DiffHandler {
    #[allow(clippy::too_many_arguments)]
    pub async fn handle_diff(
        app_handle: &AppHandle,
        payload: &Value,
        data_type: SyncDataType,
        http_client: &reqwest::Client,
        base_url: &str,
        token: &str,
        write_queue: &Arc<DbWriteQueue>,
        pending_tasks: &Arc<AtomicU32>,
        total_tasks: &Arc<AtomicU32>,
        manifest_responses_received: &Arc<AtomicU32>,
        expected_manifest_count: &Arc<AtomicU32>,
        manifest_phase: &Arc<AtomicU8>,
        tx_internal: &mpsc::UnboundedSender<SyncCommand>,
        changed_owners: &Arc<tokio::sync::Mutex<HashSet<String>>>,
        logger: &Arc<Mutex<SyncLogger>>,
    ) -> Result<(), String> {
        if let Some(items) = payload["data"].as_array() {
            let items_clone: Vec<serde_json::Value> = items.clone();

            // 统计有效操作数（排除 SKIP）
            let pull_count = items_clone.iter().filter(|i| i["action"] == "PULL").count() as u32;
            let push_count = items_clone.iter().filter(|i| i["action"] == "PUSH").count() as u32;
            let delete_count = items_clone
                .iter()
                .filter(|i| i["action"] == "DELETE")
                .count() as u32;
            let push_delete_count = items_clone
                .iter()
                .filter(|i| i["action"] == "PUSH_DELETE")
                .count() as u32;
            let total_ops = pull_count + push_count + delete_count + push_delete_count;

            if total_ops > 0 {
                let phase_tag = match data_type {
                    SyncDataType::Agent | SyncDataType::Group | SyncDataType::Avatar => {
                        "owner_metadata"
                    }
                    SyncDataType::Topic => "topic_metadata",
                    SyncDataType::Message => "messages",
                };
                let msg = format!(
                    "[{}] Diff: pull={} push={} delete={} push_delete={}",
                    data_type, pull_count, push_count, delete_count, push_delete_count
                );
                println!("[Sync] [{}] {}", phase_tag, msg);
                emit_sync_log(app_handle, "info", &msg);

                if let Ok(mut l) = logger.lock() {
                    l.log_operation(
                        phase_tag,
                        &data_type.to_string(),
                        "manifest",
                        true,
                        Some(&format!(
                            "pull={} push={} delete={} push_delete={}",
                            pull_count, push_count, delete_count, push_delete_count
                        )),
                    );
                }
            }
            pending_tasks.fetch_add(total_ops, Ordering::SeqCst);
            total_tasks.fetch_add(total_ops, Ordering::SeqCst);

            let received = manifest_responses_received.fetch_add(1, Ordering::SeqCst) + 1;
            let expected = expected_manifest_count.load(Ordering::SeqCst);
            let current_phase = manifest_phase.load(Ordering::SeqCst);
            let msg_phase = payload["phase"].as_u64().unwrap_or(0) as u8;

            if received == expected && (msg_phase == current_phase || msg_phase == 0) {
                let current_pending = pending_tasks.load(Ordering::SeqCst);
                println!(
                    "[SyncService] All manifests received for Phase {}: dataType={}, pending={}",
                    current_phase, data_type, current_pending
                );

                if current_pending == 0 {
                    if current_phase == 1 {
                        let _ = tx_internal.send(SyncCommand::StartTopicMetadata);
                    } else if current_phase == 2 {
                        let _ = tx_internal.send(SyncCommand::StartTopicValidation);
                    }
                } else {
                    let tx_internal_wd = tx_internal.clone();
                    let current_phase_wd = current_phase;
                    let manifest_phase_wd = manifest_phase.clone();
                    let pending_wd = pending_tasks.clone();
                    let handle_clone_wd = app_handle.clone();

                    tauri::async_runtime::spawn(async move {
                        let mut last_pending = pending_wd.load(Ordering::SeqCst);
                        let mut stuck_count = 0;
                        loop {
                            tokio::time::sleep(Duration::from_secs(10)).await;
                            if manifest_phase_wd.load(Ordering::SeqCst) != current_phase_wd {
                                break;
                            }
                            let current_pending = pending_wd.load(Ordering::SeqCst);
                            if current_pending == 0 {
                                break;
                            }

                            if current_pending == last_pending {
                                stuck_count += 1;
                                println!(
                                    "[SyncService] WATCHDOG: Phase {} pending count stuck at {} ({} ticks)",
                                    current_phase_wd, current_pending, stuck_count
                                );
                            } else {
                                stuck_count = 0;
                                last_pending = current_pending;
                            }

                            if stuck_count >= 6 {
                                println!("[SyncService] WATCHDOG FATAL: Phase {} DEADLOCK detected. Forcing transition...", current_phase_wd);
                                emit_sync_log(
                                    &handle_clone_wd,
                                    "error",
                                    &format!("[TIMEOUT WARNING] 检测到同步流程异常停滞超过 60 秒 (Phase {})。看门狗机制介入强制过渡以恢复正常通信流水线。部分未决 Topic 状态将推迟到下次同步时补齐。", current_phase_wd)
                                );
                                if current_phase_wd == 1 {
                                    let _ = tx_internal_wd.send(SyncCommand::StartTopicMetadata);
                                } else if current_phase_wd == 2 {
                                    let _ = tx_internal_wd.send(SyncCommand::StartTopicValidation);
                                }
                                break;
                            } else if stuck_count >= 1 {
                                emit_sync_log(
                                    &handle_clone_wd,
                                    "warn",
                                    &format!(
                                        "同步进度缓慢 (Phase {})，剩余任务: {}...",
                                        current_phase_wd, current_pending
                                    ),
                                );
                            }
                        }
                    });
                }
            }

            // 归类任务
            let mut batch_pull_requests = Vec::new();
            let mut push_topics_to_fetch = Vec::new();
            let mut other_items = Vec::new();

            for item in items_clone {
                let id = item["id"].as_str().unwrap_or_default().to_string();
                let action = item["action"].as_str().unwrap_or_default().to_string();

                // V2: Populate changed_owners for Phase 2 Topic Sync
                if data_type == SyncDataType::Agent || data_type == SyncDataType::Group {
                    let is_mismatched = item["mismatchedContent"].as_bool().unwrap_or(false);
                    if action == "PUSH" || action == "PULL" || is_mismatched {
                        let mut owners = changed_owners.lock().await;
                        owners.insert(id.clone());
                    }
                }

                if action == "PULL"
                    && (data_type == SyncDataType::Topic
                        || data_type == SyncDataType::Agent
                        || data_type == SyncDataType::Group)
                {
                    let type_str = match data_type {
                        SyncDataType::Topic => {
                            if item["ownerType"] == "group" {
                                "group_topic"
                            } else {
                                "agent_topic"
                            }
                        }
                        SyncDataType::Agent => "agent",
                        SyncDataType::Group => "group",
                        _ => unreachable!(),
                    };
                    batch_pull_requests.push(json!({ "id": id, "type": type_str }));
                } else if action == "PUSH" && data_type == SyncDataType::Topic {
                    let owner_id = item["ownerId"].as_str().unwrap_or_default().to_string();
                    let owner_type = item["ownerType"].as_str().unwrap_or("agent").to_string();
                    push_topics_to_fetch.push((id, owner_id, owner_type));
                } else {
                    other_items.push(item);
                }
            }

            // 派发任务
            if !batch_pull_requests.is_empty() {
                let h_in = app_handle.clone();
                let c_in = http_client.clone();
                let b_in = base_url.to_string();
                let token = token.to_string();
                let wq_in = write_queue.clone();
                let pending = pending_tasks.clone();
                let total_tasks_in = total_tasks.clone();
                let tx_internal_in = tx_internal.clone();
                let manifest_received_in = manifest_responses_received.clone();
                let manifest_expected_in = expected_manifest_count.clone();
                let manifest_phase_in = manifest_phase.clone();
                let data_type_inner = data_type.clone();

                tauri::async_runtime::spawn(async move {
                    let chunk_size = match data_type_inner {
                        SyncDataType::Agent | SyncDataType::Group => 50,
                        SyncDataType::Topic => 1000,
                        _ => 100,
                    };
                    for chunk in batch_pull_requests.chunks(chunk_size) {
                        let sub_batch = chunk.to_vec();
                        let sub_count = sub_batch.len() as u32;
                        let _ = PullExecutor::pull_entities_batch(
                            &h_in, &c_in, &b_in, &token, sub_batch, &wq_in,
                        )
                        .await;
                        pending.fetch_sub(sub_count, Ordering::SeqCst);
                        let current_pending = pending.load(Ordering::SeqCst);
                        let total = total_tasks_in.load(Ordering::SeqCst);
                        let done = total.saturating_sub(current_pending);
                        let _ = h_in.emit(
                            "vcp-sync-progress",
                            json!({
                                "phase": if manifest_phase_in.load(Ordering::SeqCst) == 1 {
                                    "owner_metadata"
                                } else {
                                    "topic_metadata"
                                },
                                "total": total,
                                "completed": done,
                                "message": format!("Syncing: {}/{}", done, total)
                            }),
                        );
                        if current_pending == 0
                            && manifest_received_in.load(Ordering::SeqCst)
                                == manifest_expected_in.load(Ordering::SeqCst)
                        {
                            let phase = manifest_phase_in.load(Ordering::SeqCst);
                            if phase == 1 {
                                let _ = tx_internal_in.send(SyncCommand::StartTopicMetadata);
                            } else if phase == 2 {
                                let _ = tx_internal_in.send(SyncCommand::StartTopicValidation);
                            }
                        }
                    }
                });
            }

            if !push_topics_to_fetch.is_empty() {
                let h_in = app_handle.clone();
                let c_in = http_client.clone();
                let token = token.to_string();
                let pending = pending_tasks.clone();
                let total_tasks_in = total_tasks.clone();
                let tx_internal_in = tx_internal.clone();
                let manifest_received_in = manifest_responses_received.clone();
                let manifest_expected_in = expected_manifest_count.clone();
                let manifest_phase_in = manifest_phase.clone();
                let http_url = base_url.to_string();

                tauri::async_runtime::spawn(async move {
                    let db = h_in.state::<DbState>();
                    let mut batch_push_requests = Vec::new();

                    // 异步批量查询 Topic 元数据
                    for (id, _diff_owner_id, owner_type) in push_topics_to_fetch {
                        println!("[SyncDebug] Fetching metadata for topic: {}", id);
                        let row_res = sqlx::query("SELECT topic_id, title, created_at, locked, unread, owner_id FROM topics WHERE topic_id = ?")
                            .bind(&id)
                            .fetch_optional(&db.pool)
                            .await;

                        match row_res {
                            Ok(Some(r)) => {
                                let db_owner_id: String = r.get("owner_id");
                                let tid: String = r.get("topic_id");
                                println!(
                                    "[SyncDebug] Found topic {} (owner: {})",
                                    tid, db_owner_id
                                );

                                let type_str = if owner_type == "group" {
                                    "group_topic"
                                } else {
                                    "agent_topic"
                                };
                                let dto = if owner_type == "group" {
                                    json!({ "id": tid, "name": r.get::<String, _>("title"), "createdAt": r.get::<i64, _>("created_at"), "ownerId": db_owner_id })
                                } else {
                                    json!({ "id": tid, "name": r.get::<String, _>("title"), "createdAt": r.get::<i64, _>("created_at"), "locked": r.get::<i64, _>("locked") != 0, "unread": r.get::<i64, _>("unread") != 0, "ownerId": db_owner_id })
                                };
                                batch_push_requests
                                    .push(json!({ "id": id, "type": type_str, "data": dto }));
                            }
                            Ok(None) => {
                                println!("[SyncDebug] Topic NOT FOUND in database: {}", id);
                                pending.fetch_sub(1, Ordering::SeqCst);
                            }
                            Err(e) => {
                                println!("[SyncDebug] SQL ERROR fetching topic {}: {}", id, e);
                                pending.fetch_sub(1, Ordering::SeqCst);
                            }
                        }
                    }

                    println!(
                        "[SyncDebug] Prepared {} metadata push requests",
                        batch_push_requests.len()
                    );

                    // 分块发送
                    for chunk in batch_push_requests.chunks(1000) {
                        let sub_batch = chunk.to_vec();
                        let sub_count = sub_batch.len() as u32;
                        println!(
                            "[SyncDebug] Sending batch of {} topics to desktop",
                            sub_count
                        );

                        let push_res = PushExecutor::push_entities_batch(
                            &h_in, &c_in, &http_url, &token, sub_batch,
                        )
                        .await;
                        match push_res {
                            Ok(_) => println!(
                                "[SyncDebug] Successfully pushed metadata batch to desktop"
                            ),
                            Err(e) => {
                                println!("[SyncDebug] FAILED to push metadata batch: {}", e)
                            }
                        }

                        pending.fetch_sub(sub_count, Ordering::SeqCst);

                        let current_pending = pending.load(Ordering::SeqCst);
                        let total = total_tasks_in.load(Ordering::SeqCst);
                        let done = total.saturating_sub(current_pending);
                        let _ = h_in.emit(
                            "vcp-sync-progress",
                            json!({ "phase": "topic_metadata", "total": total, "completed": done, "message": format!("Syncing: {}/{}", done, total) }),
                        );
                    }

                    // 信号外移：确保只要 pending 归零且 manifest 已收齐，就触发下一阶段
                    let current_pending = pending.load(Ordering::SeqCst);
                    if current_pending == 0
                        && manifest_received_in.load(Ordering::SeqCst)
                            == manifest_expected_in.load(Ordering::SeqCst)
                    {
                        let phase = manifest_phase_in.load(Ordering::SeqCst);
                        if phase == 1 {
                            let _ = tx_internal_in.send(SyncCommand::StartTopicMetadata);
                        } else if phase == 2 {
                            let _ = tx_internal_in.send(SyncCommand::StartTopicValidation);
                        }
                    }
                });
            }

            if !other_items.is_empty() {
                let h_in = app_handle.clone();
                let c_in = http_client.clone();
                let b_in = base_url.to_string();
                let token = token.to_string();
                let wq_in = write_queue.clone();
                let pending = pending_tasks.clone();
                let total_tasks_in = total_tasks.clone();
                let tx_internal_in = tx_internal.clone();
                let manifest_received_in = manifest_responses_received.clone();
                let manifest_expected_in = expected_manifest_count.clone();
                let manifest_phase_in = manifest_phase.clone();
                let data_type_base = data_type.clone();

                tauri::async_runtime::spawn(async move {
                    futures_util::stream::iter(other_items)
                        .for_each_concurrent(15, |item| {
                            let action = item["action"].as_str().unwrap_or_default().to_string();
                            let id = item["id"].as_str().unwrap_or_default().to_string();
                            let h_task = h_in.clone();
                            let c_task = c_in.clone();
                            let b_task = b_in.clone();
                            let token_task = token.clone();
                            let data_type_task = data_type_base.clone();
                            let wq_task = wq_in.clone();
                            let pending_task = pending.clone();
                            let total_tasks_task = total_tasks_in.clone();
                            let tx_internal_task = tx_internal_in.clone();
                            let manifest_received_task = manifest_received_in.clone();
                            let manifest_expected_task = manifest_expected_in.clone();
                            let manifest_phase_task = manifest_phase_in.clone();

                            async move {
                                let mut should_decrement = true;
                                if action == "PULL" {
                                    if data_type_task == SyncDataType::Avatar {
                                        let parts: Vec<&str> = id.split(':').collect();
                                        if parts.len() == 2 {
                                            let _ = PullExecutor::pull_avatar(
                                                &h_task,
                                                &c_task,
                                                &b_task,
                                                &token_task,
                                                parts[0],
                                                parts[1],
                                                &wq_task,
                                            )
                                            .await;
                                        }
                                    } else if data_type_task == SyncDataType::Agent {
                                        let _ = PullExecutor::pull_agent(
                                            &h_task, &c_task, &b_task, &token_task, &id, &wq_task,
                                        )
                                        .await;
                                    } else if data_type_task == SyncDataType::Group {
                                        let _ = PullExecutor::pull_group(
                                            &h_task, &c_task, &b_task, &token_task, &id, &wq_task,
                                        )
                                        .await;
                                    } else {
                                        should_decrement = false;
                                    }
                                } else if action == "PUSH" {
                                    if data_type_task == SyncDataType::Agent {
                                        let _ = PushExecutor::push_agent(
                                            &h_task, &c_task, &b_task, &token_task, &id,
                                        )
                                        .await;
                                    } else if data_type_task == SyncDataType::Group {
                                        let _ = PushExecutor::push_group(
                                            &h_task, &c_task, &b_task, &token_task, &id,
                                        )
                                        .await;
                                    } else if data_type_task == SyncDataType::Avatar {
                                        let parts: Vec<&str> = id.split(':').collect();
                                        if parts.len() == 2 {
                                            let _ = PushExecutor::push_avatar(
                                                &h_task, &c_task, &b_task, &token_task, parts[0],
                                                parts[1],
                                            )
                                            .await;
                                        }
                                    } else {
                                        should_decrement = false;
                                    }
                                } else if action == "DELETE" || action == "PUSH_DELETE" {
                                    use crate::vcp_modules::sync_executor::delete_executor::DeleteExecutor;
                                    match data_type_task {
                                        SyncDataType::Agent => {
                                            let _ =
                                                DeleteExecutor::soft_delete_agent(&h_task, &id)
                                                    .await;
                                        }
                                        SyncDataType::Group => {
                                            let _ =
                                                DeleteExecutor::soft_delete_group(&h_task, &id)
                                                    .await;
                                        }
                                        SyncDataType::Avatar => {
                                            let parts: Vec<&str> = id.split(':').collect();
                                            if parts.len() == 2 {
                                                let _ = DeleteExecutor::soft_delete_avatar(
                                                    &h_task, parts[0], parts[1],
                                                )
                                                .await;
                                            }
                                        }
                                        SyncDataType::Topic => {
                                            let _ =
                                                DeleteExecutor::soft_delete_topic(&h_task, &id)
                                                    .await;
                                        }
                                        _ => {}
                                    }
                                    if action == "PUSH_DELETE" {
                                        let _ = tx_internal_task.send(SyncCommand::NotifyDelete {
                                            data_type: data_type_task,
                                            id: id.clone(),
                                        });
                                    }
                                } else {
                                    should_decrement = false;
                                }

                                if should_decrement {
                                    pending_task.fetch_sub(1, Ordering::SeqCst);
                                    let current_pending = pending_task.load(Ordering::SeqCst);
                                    let total = total_tasks_task.load(Ordering::SeqCst);
                                    let done = total.saturating_sub(current_pending);
                                    let _ = h_task.emit(
                                        "vcp-sync-progress",
                                        json!({
                                            "phase": if manifest_phase_task.load(Ordering::SeqCst) == 1 {
                                                "owner_metadata"
                                            } else {
                                                "topic_metadata"
                                            },
                                            "total": total,
                                            "completed": done,
                                            "message": format!("Syncing: {}/{}", done, total)
                                        }),
                                    );
                                    if current_pending == 0
                                        && manifest_received_task.load(Ordering::SeqCst)
                                            == manifest_expected_task.load(Ordering::SeqCst)
                                    {
                                        let phase = manifest_phase_task.load(Ordering::SeqCst);
                                        if phase == 1 {
                                            let _ = tx_internal_task
                                                .send(SyncCommand::StartTopicMetadata);
                                        } else if phase == 2 {
                                            let _ = tx_internal_task
                                                .send(SyncCommand::StartTopicValidation);
                                        }
                                    }
                                }
                            }
                        })
                        .await;
                });
            }
        }
        Ok(())
    }
}
