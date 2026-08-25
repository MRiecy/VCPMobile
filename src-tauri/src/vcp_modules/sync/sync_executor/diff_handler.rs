use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_executor::{PullExecutor, PushExecutor};
use crate::vcp_modules::sync_service::{emit_sync_log, SyncCommand, SyncTaskTracker};
use crate::vcp_modules::sync_types::SyncDataType;
use crate::vcp_modules::topic_types::{OwnerKey, TopicKey};
use futures_util::StreamExt;
use serde_json::{json, Value};
use sqlx::Row;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

pub struct DiffHandler;

fn consume_manifest_response_type(
    payload: &Value,
    data_type: &SyncDataType,
    current_phase: u8,
    expected_manifest_types: &Mutex<HashSet<String>>,
) -> Result<bool, String> {
    let msg_phase = payload
        .get("phase")
        .and_then(Value::as_u64)
        .and_then(|phase| u8::try_from(phase).ok())
        .ok_or_else(|| "SYNC_DIFF_RESULTS.phase must be an integer".to_string())?;
    let expected_wire_phase = match current_phase {
        1 | 2 => 1,
        3 => 2,
        _ => current_phase,
    };
    if msg_phase != expected_wire_phase {
        return Err(format!(
            "SYNC_DIFF_RESULTS phase mismatch: expected {expected_wire_phase}, got {msg_phase}"
        ));
    }
    let data_type_name = data_type.to_string();
    let mut remaining = expected_manifest_types
        .lock()
        .map_err(|_| "Expected manifest type set is poisoned".to_string())?;
    if !remaining.remove(&data_type_name) {
        return Err(format!(
            "SYNC_DIFF_RESULTS contains duplicate or unexpected dataType {data_type_name} for phase {current_phase}"
        ));
    }
    Ok(remaining.is_empty())
}

fn next_manifest_command(current_phase: u8, attempt_id: u64) -> Option<SyncCommand> {
    match current_phase {
        1 => Some(SyncCommand::StartAvatarMetadata { attempt_id }),
        2 => Some(SyncCommand::StartTopicMetadata { attempt_id }),
        3 => Some(SyncCommand::StartTopicValidation { attempt_id }),
        _ => None,
    }
}

fn parse_delete_timestamp(item: &Value, id: &str, action: &str) -> Result<Option<i64>, String> {
    if !matches!(action, "DELETE" | "PUSH_DELETE") {
        return Ok(None);
    }
    item.get("deletedAt")
        .and_then(Value::as_i64)
        .filter(|deleted_at| *deleted_at >= 0)
        .map(Some)
        .ok_or_else(|| {
            format!(
                "SYNC_DIFF_RESULTS item {id} delete action requires a non-negative integer deletedAt"
            )
        })
}

/// 校验 SYNC_DIFF_RESULTS 条目。
fn validate_diff_items(items: &[Value], data_type: &SyncDataType) -> Result<Vec<Value>, String> {
    let mut seen_ids = HashSet::new();
    let mut seen_topics = HashSet::new();
    let mut validated = Vec::with_capacity(items.len());
    for item in items {
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| "SYNC_DIFF_RESULTS item requires a non-empty id".to_string())?;
        let action = item
            .get("action")
            .and_then(Value::as_str)
            .filter(|action| matches!(*action, "PULL" | "PUSH" | "DELETE" | "PUSH_DELETE" | "SKIP"))
            .ok_or_else(|| format!("SYNC_DIFF_RESULTS item {id} has an invalid action"))?;
        parse_delete_timestamp(item, id, action)?;
        if item.get("mismatchedContent").is_some()
            && item
                .get("mismatchedContent")
                .and_then(Value::as_bool)
                .is_none()
        {
            return Err(format!(
                "SYNC_DIFF_RESULTS item {id} mismatchedContent must be boolean"
            ));
        }
        if *data_type == SyncDataType::Topic {
            let owner_type = item
                .get("ownerType")
                .and_then(Value::as_str)
                .filter(|owner_type| matches!(*owner_type, "agent" | "group"))
                .ok_or_else(|| {
                    format!("SYNC_DIFF_RESULTS topic {id} requires agent/group ownerType")
                })?;
            let owner_id = item
                .get("ownerId")
                .and_then(Value::as_str)
                .filter(|owner_id| !owner_id.is_empty())
                .ok_or_else(|| format!("SYNC_DIFF_RESULTS topic {id} {action} requires ownerId"))?;
            if !seen_topics.insert(TopicKey::new(owner_type, owner_id, id)) {
                return Err(format!(
                    "SYNC_DIFF_RESULTS contains duplicate topic identity {owner_type}/{owner_id}/{id}"
                ));
            }
        } else if !seen_ids.insert(id) {
            return Err(format!("SYNC_DIFF_RESULTS contains duplicate id {id}"));
        }
        validated.push(item.clone());
    }
    Ok(validated)
}

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
        expected_manifest_types: &Arc<Mutex<HashSet<String>>>,
        manifest_phase: &Arc<AtomicU8>,
        tx_internal: &mpsc::UnboundedSender<SyncCommand>,
        changed_owners: &Arc<tokio::sync::Mutex<HashSet<OwnerKey>>>,
        task_tracker: &Arc<SyncTaskTracker>,
        session_id: u64,
        attempt_id: u64,
    ) -> Result<(), String> {
        let items = payload
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| "SYNC_DIFF_RESULTS.data must be an array".to_string())?;
        let items_clone = validate_diff_items(items, &data_type)?;

        let current_phase = manifest_phase.load(Ordering::SeqCst);
        let all_manifest_types_received = consume_manifest_response_type(
            payload,
            &data_type,
            current_phase,
            expected_manifest_types,
        )?;

        {
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
                let msg = format!(
                    "[{}] Diff: pull={} push={} delete={} push_delete={}",
                    data_type, pull_count, push_count, delete_count, push_delete_count
                );
                emit_sync_log(app_handle, "info", &msg);
            }
            pending_tasks.fetch_add(total_ops, Ordering::SeqCst);
            total_tasks.fetch_add(total_ops, Ordering::SeqCst);

            let received = manifest_responses_received.fetch_add(1, Ordering::SeqCst) + 1;
            let expected = expected_manifest_count.load(Ordering::SeqCst);
            if received > expected {
                return Err(format!(
                    "SYNC_DIFF_RESULTS response count exceeds phase {current_phase} expectation"
                ));
            }

            if all_manifest_types_received && received == expected {
                let current_pending = pending_tasks.load(Ordering::SeqCst);
                log::info!(
                    "[SyncService] All manifests received for Phase {}: dataType={}, pending={}",
                    current_phase,
                    data_type,
                    current_pending
                );

                if current_pending == 0 {
                    if let Some(command) = next_manifest_command(current_phase, attempt_id) {
                        let _ = tx_internal.send(command);
                    }
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
                        let owner_type = if data_type == SyncDataType::Group {
                            "group"
                        } else {
                            "agent"
                        };
                        owners.insert(OwnerKey::new(owner_type, &id));
                    }
                }

                if action == "SKIP" {
                    continue;
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
                    if data_type == SyncDataType::Topic {
                        batch_pull_requests.push(json!({
                            "id": id,
                            "type": type_str,
                            "ownerType": item["ownerType"],
                            "ownerId": item["ownerId"],
                        }));
                    } else {
                        batch_pull_requests.push(json!({ "id": id, "type": type_str }));
                    }
                } else if action == "PUSH" && data_type == SyncDataType::Topic {
                    let owner_id = item["ownerId"].as_str().unwrap_or_default().to_string();
                    let owner_type = item["ownerType"].as_str().unwrap_or_default().to_string();
                    push_topics_to_fetch.push(TopicKey::new(owner_type, owner_id, id));
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
                let attempt_id_inner = attempt_id;

                task_tracker
                    .spawn(async move {
                        let chunk_size = match data_type_inner {
                            SyncDataType::Agent | SyncDataType::Group => 50,
                            SyncDataType::Topic => 1000,
                            _ => 100,
                        };
                        for chunk in batch_pull_requests.chunks(chunk_size) {
                            let sub_batch = chunk.to_vec();
                            let sub_count = sub_batch.len() as u32;
                            let failed_topic_ids = if data_type_inner == SyncDataType::Topic {
                                sub_batch
                                    .iter()
                                    .filter_map(|item| item.get("id").and_then(Value::as_str))
                                    .map(str::to_string)
                                    .take(8)
                                    .collect()
                            } else {
                                Vec::new()
                            };
                            if let Err(error) = PullExecutor::pull_entities_batch(
                                &h_in, &c_in, &b_in, &token, sub_batch, &wq_in,
                            )
                            .await
                            {
                                let _ = tx_internal_in.send(SyncCommand::FailAttemptDetailed {
                                    attempt_id: attempt_id_inner,
                                    code: "ENTITY_PULL_FAILED".to_string(),
                                    message: format!("Batch pull failed: {error}"),
                                    failed_topic_ids,
                                });
                                return;
                            }
                            pending.fetch_sub(sub_count, Ordering::SeqCst);
                            let current_pending = pending.load(Ordering::SeqCst);
                            let total = total_tasks_in.load(Ordering::SeqCst);
                            let done = total.saturating_sub(current_pending);
                            let _ = h_in.emit(
                                "vcp-sync-progress",
                                json!({
                                    "sessionId": session_id,
                                    "phase": if manifest_phase_in.load(Ordering::SeqCst) <= 2 {
                                        "owner_metadata"
                                    } else {
                                        "topic_metadata"
                                    },
                                    "total": total,
                                    "completed": done,
                                }),
                            );
                            if current_pending == 0
                                && manifest_received_in.load(Ordering::SeqCst)
                                    == manifest_expected_in.load(Ordering::SeqCst)
                            {
                                let phase = manifest_phase_in.load(Ordering::SeqCst);
                                if let Some(command) =
                                    next_manifest_command(phase, attempt_id_inner)
                                {
                                    let _ = tx_internal_in.send(command);
                                }
                            }
                        }
                    })
                    .await;
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
                let attempt_id_inner = attempt_id;

                task_tracker.spawn(async move {
                    let db = h_in.state::<DbState>();
                    let mut batch_push_requests = Vec::new();

                    // 异步批量查询 Topic 元数据
                    for key in push_topics_to_fetch {
                        let id = key.topic_id.clone();
                        log::debug!("[SyncDebug] Fetching metadata for topic: {}", id);
                        let row_res = sqlx::query(
                            "SELECT topic_id, title, created_at, locked, unread, owner_id, owner_type
                             FROM topics
                             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
                               AND deleted_at IS NULL",
                        )
                            .bind(&key.owner_type)
                            .bind(&key.owner_id)
                            .bind(&id)
                            .fetch_optional(&db.pool)
                            .await;

                        match row_res {
                            Ok(Some(r)) => {
                                let decoded = (|| -> Result<_, String> {
                                    Ok((
                                        r.try_get::<String, _>("topic_id")
                                            .map_err(|error| format!("topic id: {error}"))?,
                                        r.try_get::<String, _>("title")
                                            .map_err(|error| format!("title: {error}"))?,
                                        r.try_get::<i64, _>("created_at")
                                            .map_err(|error| format!("created_at: {error}"))?,
                                        r.try_get::<i64, _>("locked")
                                            .map_err(|error| format!("locked: {error}"))?,
                                        r.try_get::<i64, _>("unread")
                                            .map_err(|error| format!("unread: {error}"))?,
                                        r.try_get::<String, _>("owner_id")
                                            .map_err(|error| format!("owner_id: {error}"))?,
                                        r.try_get::<String, _>("owner_type")
                                            .map_err(|error| format!("owner_type: {error}"))?,
                                    ))
                                })();
                                let (
                                    tid,
                                    title,
                                    created_at,
                                    locked,
                                    unread,
                                    db_owner_id,
                                    db_owner_type,
                                ) = match decoded {
                                    Ok(decoded) => decoded,
                                    Err(error) => {
                                        let _ = tx_internal_in.send(
                                            SyncCommand::FailAttemptDetailed {
                                                attempt_id: attempt_id_inner,
                                                code: "TOPIC_PUSH_DB_DECODE_FAILED".to_string(),
                                                message: format!(
                                                    "Failed to decode topic {id} for push: {error}"
                                                ),
                                                failed_topic_ids: vec![id],
                                            },
                                        );
                                        return;
                                    }
                                };
                                if db_owner_id != key.owner_id || db_owner_type != key.owner_type {
                                    let _ = tx_internal_in.send(
                                        SyncCommand::FailAttemptDetailed {
                                            attempt_id: attempt_id_inner,
                                            code: "TOPIC_PUSH_OWNER_CONFLICT".to_string(),
                                            message: format!(
                                                "Topic {id} owner does not match the Phase 1 decision"
                                            ),
                                            failed_topic_ids: vec![id],
                                        },
                                    );
                                    return;
                                }
                                log::debug!(
                                    "[SyncDebug] Found topic {} (owner: {})",
                                    tid,
                                    db_owner_id
                                );

                                let type_str = if key.owner_type == "group" {
                                    "group_topic"
                                } else {
                                    "agent_topic"
                                };
                                let dto = if key.owner_type == "group" {
                                    json!({ "id": tid, "name": title, "createdAt": created_at, "ownerId": db_owner_id })
                                } else {
                                    json!({ "id": tid, "name": title, "createdAt": created_at, "locked": locked != 0, "unread": unread != 0, "ownerId": db_owner_id })
                                };
                                batch_push_requests
                                    .push(json!({
                                        "id": id,
                                        "type": type_str,
                                        "ownerType": key.owner_type,
                                        "ownerId": key.owner_id,
                                        "data": dto,
                                    }));
                            }
                            Ok(None) => {
                                log::warn!("[SyncDebug] Topic NOT FOUND in database: {}", id);
                                let _ = tx_internal_in.send(SyncCommand::FailAttemptDetailed {
                                    attempt_id: attempt_id_inner,
                                    code: "TOPIC_PUSH_SOURCE_MISSING".to_string(),
                                    message: format!("Topic selected for push is missing: {id}"),
                                    failed_topic_ids: vec![id],
                                });
                                return;
                            }
                            Err(e) => {
                                log::error!("[SyncDebug] SQL ERROR fetching topic {}: {}", id, e);
                                let _ = tx_internal_in.send(SyncCommand::FailAttemptDetailed {
                                    attempt_id: attempt_id_inner,
                                    code: "TOPIC_PUSH_DB_FAILED".to_string(),
                                    message: format!("Failed to load topic {id} for push: {e}"),
                                    failed_topic_ids: vec![id],
                                });
                                return;
                            }
                        }
                    }

                    log::debug!(
                        "[SyncDebug] Prepared {} metadata push requests",
                        batch_push_requests.len()
                    );

                    // 分块发送
                    for chunk in batch_push_requests.chunks(1000) {
                        let sub_batch = chunk.to_vec();
                        let sub_count = sub_batch.len() as u32;
                        let failed_topic_ids = sub_batch
                            .iter()
                            .filter_map(|item| item.get("id").and_then(Value::as_str))
                            .map(str::to_string)
                            .take(8)
                            .collect::<Vec<_>>();
                        log::debug!(
                            "[SyncDebug] Sending batch of {} topics to desktop",
                            sub_count
                        );

                        let push_res = PushExecutor::push_entities_batch(
                            &h_in, &c_in, &http_url, &token, sub_batch,
                        )
                        .await;
                        match push_res {
                            Ok(_) => log::debug!(
                                "[SyncDebug] Successfully pushed metadata batch to desktop"
                            ),
                            Err(e) => {
                                log::error!("[SyncDebug] FAILED to push metadata batch: {}", e);
                                let _ = tx_internal_in.send(SyncCommand::FailAttemptDetailed {
                                    attempt_id: attempt_id_inner,
                                    code: "TOPIC_PUSH_FAILED".to_string(),
                                    message: format!("Batch topic push failed: {e}"),
                                    failed_topic_ids,
                                });
                                return;
                            }
                        }

                        pending.fetch_sub(sub_count, Ordering::SeqCst);

                        let current_pending = pending.load(Ordering::SeqCst);
                        let total = total_tasks_in.load(Ordering::SeqCst);
                        let done = total.saturating_sub(current_pending);
                        let _ = h_in.emit(
                            "vcp-sync-progress",
                            json!({ "sessionId": session_id, "phase": "topic_metadata", "total": total, "completed": done }),
                        );
                    }

                    // 信号外移：确保只要 pending 归零且 manifest 已收齐，就触发下一阶段
                    let current_pending = pending.load(Ordering::SeqCst);
                    if current_pending == 0
                        && manifest_received_in.load(Ordering::SeqCst)
                            == manifest_expected_in.load(Ordering::SeqCst)
                    {
                        let phase = manifest_phase_in.load(Ordering::SeqCst);
                        if let Some(command) = next_manifest_command(phase, attempt_id_inner) {
                            let _ = tx_internal_in.send(command);
                        }
                    }
                }).await;
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
                let attempt_id_inner = attempt_id;

                task_tracker.spawn(async move {
                    futures_util::stream::iter(other_items)
                        .for_each_concurrent(15, |item| {
                            let action = item["action"].as_str().unwrap_or_default().to_string();
                            let id = item["id"].as_str().unwrap_or_default().to_string();
                            let deleted_at = item.get("deletedAt").and_then(Value::as_i64);
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
                            let attempt_id_task = attempt_id_inner;

                            async move {
                                let operation_result: Result<(), String> = if action == "PULL" {
                                    match &data_type_task {
                                        SyncDataType::Avatar => {
                                            let parts: Vec<&str> = id.split(':').collect();
                                            if parts.len() != 2 {
                                                Err(format!("invalid avatar id: {id}"))
                                            } else {
                                                PullExecutor::pull_avatar(
                                                    &h_task,
                                                    &c_task,
                                                    &b_task,
                                                    &token_task,
                                                    parts[0],
                                                    parts[1],
                                                    &wq_task,
                                                )
                                                .await
                                            }
                                        }
                                        _ => Err(format!(
                                            "unsupported PULL data type: {:?}",
                                            data_type_task
                                        )),
                                    }
                                } else if action == "PUSH" {
                                    match &data_type_task {
                                        SyncDataType::Agent => {
                                            PushExecutor::push_agent(
                                                &h_task, &c_task, &b_task, &token_task, &id,
                                            )
                                            .await
                                        }
                                        SyncDataType::Group => {
                                            PushExecutor::push_group(
                                                &h_task, &c_task, &b_task, &token_task, &id,
                                            )
                                            .await
                                        }
                                        SyncDataType::Avatar => {
                                            let parts: Vec<&str> = id.split(':').collect();
                                            if parts.len() != 2 {
                                                Err(format!("invalid avatar id: {id}"))
                                            } else {
                                                PushExecutor::push_avatar(
                                                    &h_task,
                                                    &c_task,
                                                    &b_task,
                                                    &token_task,
                                                    parts[0],
                                                    parts[1],
                                                )
                                                .await
                                            }
                                        }
                                        _ => Err(format!(
                                            "unsupported PUSH data type: {:?}",
                                            data_type_task
                                        )),
                                    }
                                } else if action == "DELETE" || action == "PUSH_DELETE" {
                                    use crate::vcp_modules::sync_executor::delete_executor::DeleteExecutor;
                                    let delete_result = match deleted_at {
                                        Some(deleted_at) if deleted_at >= 0 => match &data_type_task {
                                            SyncDataType::Agent => {
                                                DeleteExecutor::soft_delete_agent(
                                                    &h_task,
                                                    &id,
                                                    deleted_at,
                                                )
                                                .await
                                            }
                                            SyncDataType::Group => {
                                                DeleteExecutor::soft_delete_group(
                                                    &h_task,
                                                    &id,
                                                    deleted_at,
                                                )
                                                .await
                                            }
                                            SyncDataType::Avatar => {
                                                let parts: Vec<&str> = id.split(':').collect();
                                                if parts.len() != 2 {
                                                    Err(format!("invalid avatar id: {id}"))
                                                } else {
                                                    DeleteExecutor::soft_delete_avatar(
                                                        &h_task,
                                                        parts[0],
                                                        parts[1],
                                                        deleted_at,
                                                    )
                                                    .await
                                                }
                                            }
                                            SyncDataType::Topic => {
                                                match (
                                                    item.get("ownerType").and_then(Value::as_str),
                                                    item.get("ownerId").and_then(Value::as_str),
                                                ) {
                                                    (Some(owner_type), Some(owner_id)) => {
                                                        let topic_key = TopicKey::new(owner_type, owner_id, &id);
                                                        DeleteExecutor::soft_delete_topic(
                                                            &h_task,
                                                            &topic_key,
                                                            deleted_at,
                                                        ).await
                                                    }
                                                    _ => Err(format!(
                                                        "Topic delete {id} is missing owner identity"
                                                    )),
                                                }
                                            }
                                            _ => Err(format!(
                                                "unsupported DELETE data type: {:?}",
                                                data_type_task
                                            )),
                                        },
                                        _ => Err(format!(
                                            "DELETE action for {id} is missing a valid deletedAt"
                                        )),
                                    };
                                    if delete_result.is_ok() && action == "PUSH_DELETE" {
                                        match deleted_at {
                                            Some(deleted_at) => tx_internal_task
                                                .send(SyncCommand::NotifyDelete {
                                                    data_type: data_type_task.clone(),
                                                    id: id.clone(),
                                                    deleted_at,
                                                    owner_type: if data_type_task == SyncDataType::Topic {
                                                        item.get("ownerType")
                                                            .and_then(Value::as_str)
                                                            .map(str::to_string)
                                                    } else {
                                                        None
                                                    },
                                                    owner_id: if data_type_task == SyncDataType::Topic {
                                                        item.get("ownerId")
                                                            .and_then(Value::as_str)
                                                            .map(str::to_string)
                                                    } else {
                                                        None
                                                    },
                                                })
                                                .map_err(|error| error.to_string()),
                                            None => Err(format!(
                                                "PUSH_DELETE action for {id} is missing deletedAt"
                                            )),
                                        }
                                    } else {
                                        delete_result
                                    }
                                } else {
                                    Err(format!("unsupported sync action: {action}"))
                                };

                                match operation_result {
                                    Ok(()) => {
                                    pending_task.fetch_sub(1, Ordering::SeqCst);
                                    let current_pending = pending_task.load(Ordering::SeqCst);
                                    let total = total_tasks_task.load(Ordering::SeqCst);
                                    let done = total.saturating_sub(current_pending);
                                    let _ = h_task.emit(
                                        "vcp-sync-progress",
                                        json!({
                                            "sessionId": session_id,
                                            "phase": if manifest_phase_task.load(Ordering::SeqCst) <= 2 {
                                                "owner_metadata"
                                            } else {
                                                "topic_metadata"
                                            },
                                            "total": total,
                                            "completed": done,
                                        }),
                                    );
                                    if current_pending == 0
                                        && manifest_received_task.load(Ordering::SeqCst)
                                            == manifest_expected_task.load(Ordering::SeqCst)
                                    {
                                        let phase = manifest_phase_task.load(Ordering::SeqCst);
                                        if let Some(command) =
                                            next_manifest_command(phase, attempt_id_task)
                                        {
                                            let _ = tx_internal_task.send(command);
                                        }
                                    }
                                    }
                                    Err(error) => {
                                        let failed_topic_ids = if data_type_task == SyncDataType::Topic {
                                            vec![id.clone()]
                                        } else {
                                            Vec::new()
                                        };
                                        let _ = tx_internal_task.send(SyncCommand::FailAttemptDetailed {
                                            attempt_id: attempt_id_task,
                                            code: "ENTITY_OPERATION_FAILED".to_string(),
                                            message: format!(
                                                "Sync {action} failed for {id}: {error}"
                                            ),
                                            failed_topic_ids,
                                        });
                                    }
                                }
                            }
                        })
                        .await;
                }).await;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        consume_manifest_response_type, next_manifest_command, parse_delete_timestamp,
        validate_diff_items,
    };
    use crate::vcp_modules::sync_service::SyncCommand;
    use crate::vcp_modules::sync_types::SyncDataType;
    use serde_json::json;
    use std::collections::HashSet;
    use std::sync::Mutex;

    #[test]
    fn default_topics_use_the_full_owner_identity() {
        let items = json!([
            {"id": "default", "action": "PULL", "ownerType": "agent", "ownerId": "agent-a"},
            {"id": "default", "action": "PULL", "ownerType": "agent", "ownerId": "agent-b"},
            {"id": "topic-1", "action": "PULL", "ownerType": "agent", "ownerId": "agent-a"},
        ]);
        let validated = validate_diff_items(items.as_array().unwrap(), &SyncDataType::Topic)
            .expect("cross-owner default topics must be dispatched independently");
        assert_eq!(validated.len(), 3);
    }

    #[test]
    fn topic_diff_uses_the_full_owner_identity_for_duplicates() {
        let items = json!([
            {"id": "topic-1", "action": "PULL", "ownerType": "agent", "ownerId": "agent-a"},
            {"id": "topic-1", "action": "PULL", "ownerType": "agent", "ownerId": "agent-b"},
        ]);
        assert_eq!(
            validate_diff_items(items.as_array().unwrap(), &SyncDataType::Topic)
                .expect("same topic id under different owners is valid")
                .len(),
            2
        );

        let duplicate = json!([
            {"id": "topic-1", "action": "PULL", "ownerType": "agent", "ownerId": "agent-a"},
            {"id": "topic-1", "action": "PULL", "ownerType": "agent", "ownerId": "agent-a"},
        ]);
        let error = validate_diff_items(duplicate.as_array().unwrap(), &SyncDataType::Topic)
            .expect_err("duplicate compound identity must fail");
        assert!(error.contains("duplicate topic identity"));
    }

    #[test]
    fn topic_pull_requires_exact_owner_identity() {
        let items = json!([
            {"id": "topic-1", "action": "PULL", "ownerType": "agent"},
        ]);
        let error = validate_diff_items(items.as_array().unwrap(), &SyncDataType::Topic)
            .expect_err("topic pull without ownerId must fail");
        assert!(error.contains("requires ownerId"));
    }

    #[test]
    fn manifest_responses_consume_exact_type_once_for_the_current_phase() {
        let expected = Mutex::new(HashSet::from(["agent".to_string(), "group".to_string()]));
        assert!(!consume_manifest_response_type(
            &json!({"phase": 1}),
            &SyncDataType::Agent,
            1,
            &expected,
        )
        .expect("first expected type"));
        assert!(consume_manifest_response_type(
            &json!({"phase": 1}),
            &SyncDataType::Agent,
            1,
            &expected,
        )
        .is_err());
        assert!(
            consume_manifest_response_type(&json!({}), &SyncDataType::Group, 1, &expected,)
                .is_err()
        );
        assert!(consume_manifest_response_type(
            &json!({"phase": 2}),
            &SyncDataType::Group,
            1,
            &expected,
        )
        .is_err());
        assert!(consume_manifest_response_type(
            &json!({"phase": 1}),
            &SyncDataType::Group,
            1,
            &expected,
        )
        .expect("last expected type"));
    }

    #[test]
    fn avatar_wave_uses_owner_wire_phase_and_precedes_topics() {
        let expected = Mutex::new(HashSet::from(["avatar".to_string()]));
        assert!(consume_manifest_response_type(
            &json!({"phase": 1}),
            &SyncDataType::Avatar,
            2,
            &expected,
        )
        .expect("avatar response"));
        assert!(matches!(
            next_manifest_command(1, 7),
            Some(SyncCommand::StartAvatarMetadata { attempt_id: 7 })
        ));
        assert!(matches!(
            next_manifest_command(2, 7),
            Some(SyncCommand::StartTopicMetadata { attempt_id: 7 })
        ));
        assert!(matches!(
            next_manifest_command(3, 7),
            Some(SyncCommand::StartTopicValidation { attempt_id: 7 })
        ));
    }

    #[test]
    fn delete_actions_require_a_stable_non_negative_timestamp() {
        assert_eq!(
            parse_delete_timestamp(
                &json!({"action": "DELETE", "deletedAt": 42}),
                "entity",
                "DELETE",
            )
            .expect("valid timestamp"),
            Some(42)
        );
        for value in [
            json!({"action": "DELETE"}),
            json!({"action": "DELETE", "deletedAt": null}),
            json!({"action": "DELETE", "deletedAt": "42"}),
            json!({"action": "DELETE", "deletedAt": -1}),
        ] {
            assert!(parse_delete_timestamp(&value, "entity", "DELETE").is_err());
        }
        assert_eq!(
            parse_delete_timestamp(&json!({"action": "SKIP"}), "entity", "SKIP")
                .expect("non-delete action"),
            None
        );
    }
}
