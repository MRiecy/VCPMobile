use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_dto::{AgentTopicSyncDTO, GroupTopicSyncDTO};
use crate::vcp_modules::sync_executor::{PullExecutor, PushExecutor};
use crate::vcp_modules::sync_service::{
    emit_sync_log, ManifestResponseTracker, SyncCommand, SyncTaskTracker,
};
use crate::vcp_modules::sync_types::{
    is_valid_avatar_owner, AvatarManifestDecision, DeleteTarget, EntityPushData, EntityPushItem,
    EntitySelector, ManifestAction, ManifestResultFrame, ManifestType, OwnerManifestDecision,
    OwnerType, TopicManifestDecision,
};
use crate::vcp_modules::topic_types::{OwnerKey, TopicKey};
use futures_util::StreamExt;
use serde_json::json;
use sqlx::Row;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

pub struct DiffHandler;

// Entity operations can include a 20 MiB avatar, so keep only a small waiting concurrency.
const ENTITY_OPERATION_CONCURRENCY: usize = 3;
const MAX_SAFE_JSON_INTEGER: i64 = (1_i64 << 53) - 1;

fn next_manifest_command(current_phase: u8, attempt_id: u64) -> Option<SyncCommand> {
    match current_phase {
        1 => Some(SyncCommand::StartAvatarMetadata { attempt_id }),
        2 => Some(SyncCommand::StartTopicMetadata { attempt_id }),
        3 => Some(SyncCommand::StartTopicValidation { attempt_id }),
        _ => None,
    }
}

#[derive(Debug, Clone)]
enum ManifestDecision {
    Owner(OwnerManifestDecision),
    Topic(TopicManifestDecision),
    Avatar(AvatarManifestDecision),
}

impl ManifestDecision {
    fn action(&self) -> ManifestAction {
        match self {
            ManifestDecision::Owner(item) => item.action,
            ManifestDecision::Topic(item) => item.action,
            ManifestDecision::Avatar(item) => item.action,
        }
    }

    fn id(&self) -> &str {
        match self {
            ManifestDecision::Owner(item) => &item.owner_id,
            ManifestDecision::Topic(item) => &item.topic_id,
            ManifestDecision::Avatar(item) => &item.owner_id,
        }
    }

    fn deleted_at(&self) -> Option<i64> {
        match self {
            ManifestDecision::Owner(item) => item.deleted_at,
            ManifestDecision::Topic(item) => item.deleted_at,
            ManifestDecision::Avatar(item) => item.deleted_at,
        }
    }

    fn delete_target(&self) -> DeleteTarget {
        match self {
            ManifestDecision::Owner(item) => DeleteTarget::Owner {
                owner_type: item.owner_type,
                owner_id: item.owner_id.clone(),
            },
            ManifestDecision::Topic(item) => DeleteTarget::Topic(TopicKey::new(
                item.owner_type.as_str(),
                &item.owner_id,
                &item.topic_id,
            )),
            ManifestDecision::Avatar(item) => DeleteTarget::Avatar {
                owner_type: item.owner_type,
                owner_id: item.owner_id.clone(),
            },
        }
    }
}

fn validate_decision_timestamp(item: &ManifestDecision) -> Result<(), String> {
    let action = item.action();
    let deleted_at = item.deleted_at();
    if action.is_delete() {
        if deleted_at.is_none_or(|value| !(0..=MAX_SAFE_JSON_INTEGER).contains(&value)) {
            return Err(format!(
                "SYNC_MANIFEST_RESULT {} {} requires a non-negative safe-integer deletedAt",
                action,
                item.id()
            ));
        }
    } else if deleted_at.is_some() {
        return Err(format!(
            "SYNC_MANIFEST_RESULT {} {} must not carry deletedAt",
            action,
            item.id()
        ));
    }
    Ok(())
}

fn validate_manifest_result(
    result: ManifestResultFrame,
) -> Result<(ManifestType, Vec<ManifestDecision>), String> {
    let manifest_type = result.manifest_type();
    let decisions = match result {
        ManifestResultFrame::Owner { results, .. } => results
            .into_iter()
            .map(ManifestDecision::Owner)
            .collect::<Vec<_>>(),
        ManifestResultFrame::Topic { results, .. } => results
            .into_iter()
            .map(ManifestDecision::Topic)
            .collect::<Vec<_>>(),
        ManifestResultFrame::Avatar { results, .. } => results
            .into_iter()
            .map(ManifestDecision::Avatar)
            .collect::<Vec<_>>(),
    };

    let mut seen = HashSet::new();
    for decision in &decisions {
        if decision.id().is_empty() {
            return Err("SYNC_MANIFEST_RESULT contains an empty identity".to_string());
        }
        validate_decision_timestamp(decision)?;
        let identity = match decision {
            ManifestDecision::Owner(item) => {
                if item.content_hash_mismatch && item.action.is_delete() {
                    return Err(format!(
                        "Owner {} delete decision must not report contentHashMismatch",
                        item.owner_id
                    ));
                }
                if item.action == ManifestAction::Skip && !item.content_hash_mismatch {
                    return Err(format!(
                        "Owner {} SKIP decision requires contentHashMismatch",
                        item.owner_id
                    ));
                }
                format!("{}\0{}", item.owner_type, item.owner_id)
            }
            ManifestDecision::Topic(item) => {
                if item.owner_id.is_empty() || item.topic_id.is_empty() {
                    return Err("SYNC_MANIFEST_RESULT contains an invalid topic identity".into());
                }
                if item.action == ManifestAction::Skip {
                    return Err("Topic manifest must not contain SKIP decisions".into());
                }
                format!("{}\0{}\0{}", item.owner_type, item.owner_id, item.topic_id)
            }
            ManifestDecision::Avatar(item) => {
                if !is_valid_avatar_owner(item.owner_type.as_str(), &item.owner_id) {
                    return Err("SYNC_MANIFEST_RESULT contains an invalid avatar identity".into());
                }
                if item.action == ManifestAction::Skip {
                    return Err("Avatar manifest must not contain SKIP decisions".into());
                }
                format!("{}\0{}", item.owner_type, item.owner_id)
            }
        };
        if !seen.insert(identity) {
            return Err(format!(
                "SYNC_MANIFEST_RESULT contains a duplicate {manifest_type} identity"
            ));
        }
    }
    Ok((manifest_type, decisions))
}

fn validate_topic_owner_scope(
    decisions: &[ManifestDecision],
    targeted_owners: &HashSet<OwnerKey>,
) -> Result<(), String> {
    for decision in decisions {
        let ManifestDecision::Topic(topic) = decision else {
            continue;
        };
        let owner = OwnerKey::new(topic.owner_type.as_str(), &topic.owner_id);
        if !targeted_owners.contains(&owner) {
            return Err(format!(
                "SYNC_MANIFEST_RESULT topic {}/{}/{} is outside targeted owner scope",
                topic.owner_type, topic.owner_id, topic.topic_id
            ));
        }
    }
    Ok(())
}

impl DiffHandler {
    #[allow(clippy::too_many_arguments)]
    pub async fn handle_diff(
        app_handle: &AppHandle,
        result: ManifestResultFrame,
        http_client: &reqwest::Client,
        base_url: &str,
        token: &str,
        write_queue: &Arc<DbWriteQueue>,
        pending_tasks: &Arc<AtomicU32>,
        total_tasks: &Arc<AtomicU32>,
        manifest_responses: &Arc<ManifestResponseTracker>,
        manifest_phase: &Arc<AtomicU8>,
        tx_internal: &mpsc::UnboundedSender<SyncCommand>,
        changed_owners: &Arc<tokio::sync::Mutex<HashSet<OwnerKey>>>,
        task_tracker: &Arc<SyncTaskTracker>,
        session_id: u64,
        attempt_id: u64,
    ) -> Result<(), String> {
        let (manifest_type, decisions) = validate_manifest_result(result)?;

        if manifest_type == ManifestType::Topic {
            let targeted_owners = changed_owners.lock().await;
            validate_topic_owner_scope(&decisions, &targeted_owners)?;
        }

        let current_phase = manifest_phase.load(Ordering::SeqCst);
        manifest_responses.consume(current_phase, manifest_type)?;

        {
            // 统计有效操作数（排除 SKIP）
            let count_action = |expected| {
                decisions
                    .iter()
                    .filter(|item| item.action() == expected)
                    .count() as u32
            };
            let pull_count = count_action(ManifestAction::Pull);
            let push_count = count_action(ManifestAction::Push);
            let pull_delete_count = count_action(ManifestAction::PullDelete);
            let push_delete_count = count_action(ManifestAction::PushDelete);
            let total_ops = pull_count + push_count + pull_delete_count + push_delete_count;

            if total_ops > 0 {
                let msg = format!(
                    "[{}] Diff: pull={} push={} pull_delete={} push_delete={}",
                    manifest_type, pull_count, push_count, pull_delete_count, push_delete_count
                );
                emit_sync_log(app_handle, "info", &msg);
            }
            pending_tasks.fetch_add(total_ops, Ordering::SeqCst);
            total_tasks.fetch_add(total_ops, Ordering::SeqCst);

            let current_pending = pending_tasks.load(Ordering::SeqCst);
            log::info!(
                "[SyncService] Manifest received for wave {}: manifestType={}, pending={}",
                current_phase,
                manifest_type,
                current_pending
            );

            if current_pending == 0 {
                if let Some(command) = next_manifest_command(current_phase, attempt_id) {
                    let _ = tx_internal.send(command);
                }
            }

            // 归类任务
            let mut batch_pull_requests = Vec::new();
            let mut push_topics_to_fetch = Vec::new();
            let mut other_items = Vec::new();

            for item in decisions {
                let action = item.action();

                if let ManifestDecision::Owner(owner) = &item {
                    let is_mismatched = owner.content_hash_mismatch;
                    if matches!(action, ManifestAction::Push | ManifestAction::Pull)
                        || is_mismatched
                    {
                        let mut owners = changed_owners.lock().await;
                        owners.insert(OwnerKey::new(owner.owner_type.as_str(), &owner.owner_id));
                    }
                }

                if action == ManifestAction::Skip {
                    continue;
                }

                if action == ManifestAction::Pull {
                    match &item {
                        ManifestDecision::Topic(topic) => {
                            let key = TopicKey::new(
                                topic.owner_type.as_str(),
                                &topic.owner_id,
                                &topic.topic_id,
                            );
                            batch_pull_requests.push(EntitySelector::topic(&key)?);
                            continue;
                        }
                        ManifestDecision::Owner(owner) => {
                            batch_pull_requests
                                .push(EntitySelector::owner(owner.owner_type, &owner.owner_id));
                            continue;
                        }
                        ManifestDecision::Avatar(_) => {}
                    }
                }
                if action == ManifestAction::Push {
                    if let ManifestDecision::Topic(topic) = &item {
                        push_topics_to_fetch.push(TopicKey::new(
                            topic.owner_type.as_str(),
                            &topic.owner_id,
                            &topic.topic_id,
                        ));
                        continue;
                    }
                }
                other_items.push(item);
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
                let manifest_phase_in = manifest_phase.clone();
                let manifest_type_inner = manifest_type;
                let attempt_id_inner = attempt_id;

                task_tracker
                    .spawn(async move {
                        let chunk_size = match manifest_type_inner {
                            ManifestType::Owner => 50,
                            ManifestType::Topic => 1000,
                            _ => 100,
                        };
                        for chunk in batch_pull_requests.chunks(chunk_size) {
                            let sub_batch = chunk.to_vec();
                            let sub_count = sub_batch.len() as u32;
                            let failed_topic_ids = if manifest_type_inner == ManifestType::Topic {
                                sub_batch
                                    .iter()
                                    .filter_map(EntitySelector::topic_id)
                                    .map(str::to_owned)
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
                            if current_pending == 0 {
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
                let manifest_phase_in = manifest_phase.clone();
                let http_url = base_url.to_string();
                let attempt_id_inner = attempt_id;

                task_tracker.spawn(async move {
                    let db = h_in.state::<DbState>();
                    const TOPIC_QUERY_CHUNK: usize = 300;
                    let requested_topics = push_topics_to_fetch;
                    let mut items_by_topic = HashMap::with_capacity(requested_topics.len());

                    for chunk in requested_topics.chunks(TOPIC_QUERY_CHUNK) {
                        let placeholders = vec!["(?, ?, ?)"; chunk.len()].join(", ");
                        let query_sql = format!(
                            "SELECT topic_id, title, created_at, locked, unread, owner_id, owner_type,
                                    config_hash, updated_at
                             FROM topics
                             WHERE (owner_type, owner_id, topic_id) IN ({placeholders})
                               AND deleted_at IS NULL"
                        );
                        let mut query = sqlx::query(sqlx::AssertSqlSafe(query_sql));
                        for key in chunk {
                            query = query
                                .bind(&key.owner_type)
                                .bind(&key.owner_id)
                                .bind(&key.topic_id);
                        }
                        let rows = match query.fetch_all(&db.pool).await {
                            Ok(rows) => rows,
                            Err(error) => {
                                let _ = tx_internal_in.send(SyncCommand::FailAttemptDetailed {
                                    attempt_id: attempt_id_inner,
                                    code: "TOPIC_PUSH_DB_FAILED".to_string(),
                                    message: format!("Failed to load topics for push: {error}"),
                                    failed_topic_ids: chunk
                                        .iter()
                                        .map(|key| key.topic_id.clone())
                                        .collect(),
                                });
                                return;
                            }
                        };

                        for row in rows {
                            let decoded = (|| -> Result<_, String> {
                                let config_hash = row
                                    .try_get::<String, _>("config_hash")
                                    .map_err(|error| format!("config_hash: {error}"))?;
                                if !crate::vcp_modules::infra::utils::is_valid_cas_hash(
                                    &config_hash,
                                ) || config_hash.to_ascii_lowercase() != config_hash
                                {
                                    return Err("config_hash must be lowercase SHA-256".to_string());
                                }
                                let updated_at = row
                                    .try_get::<i64, _>("updated_at")
                                    .map_err(|error| format!("updated_at: {error}"))?;
                                if !(0..=(1_i64 << 53) - 1).contains(&updated_at) {
                                    return Err(
                                        "updated_at must be a non-negative safe integer".to_string(),
                                    );
                                }
                                Ok((
                                    row.try_get::<String, _>("topic_id")
                                        .map_err(|error| format!("topic id: {error}"))?,
                                    row.try_get::<String, _>("title")
                                        .map_err(|error| format!("title: {error}"))?,
                                    row.try_get::<i64, _>("created_at")
                                        .map_err(|error| format!("created_at: {error}"))?,
                                    row.try_get::<i64, _>("locked")
                                        .map_err(|error| format!("locked: {error}"))?,
                                    row.try_get::<i64, _>("unread")
                                        .map_err(|error| format!("unread: {error}"))?,
                                    row.try_get::<String, _>("owner_id")
                                        .map_err(|error| format!("owner_id: {error}"))?,
                                    row.try_get::<String, _>("owner_type")
                                        .map_err(|error| format!("owner_type: {error}"))?,
                                    config_hash,
                                    updated_at,
                                ))
                            })();
                            let (
                                topic_id,
                                title,
                                created_at,
                                locked,
                                unread,
                                owner_id,
                                owner_type,
                                config_hash,
                                updated_at,
                            ) = match decoded {
                                Ok(decoded) => decoded,
                                Err(error) => {
                                    let _ = tx_internal_in.send(
                                        SyncCommand::FailAttemptDetailed {
                                            attempt_id: attempt_id_inner,
                                            code: "TOPIC_PUSH_DB_DECODE_FAILED".to_string(),
                                            message: format!(
                                                "Failed to decode a topic for push: {error}"
                                            ),
                                            failed_topic_ids: Vec::new(),
                                        },
                                    );
                                    return;
                                }
                            };
                            let key = TopicKey::new(&owner_type, &owner_id, &topic_id);
                            let item = match owner_type.as_str() {
                                "agent" => EntityPushItem::Topic {
                                    owner_type: OwnerType::Agent,
                                    owner_id: owner_id.clone(),
                                    topic_id: topic_id.clone(),
                                    data: EntityPushData::AgentTopic(AgentTopicSyncDTO {
                                        id: topic_id,
                                        name: title,
                                        created_at,
                                        locked: locked != 0,
                                        unread: unread != 0,
                                        owner_id,
                                        config_hash,
                                        updated_at,
                                    }),
                                },
                                "group" => EntityPushItem::Topic {
                                    owner_type: OwnerType::Group,
                                    owner_id: owner_id.clone(),
                                    topic_id: topic_id.clone(),
                                    data: EntityPushData::GroupTopic(GroupTopicSyncDTO {
                                        id: topic_id,
                                        name: title,
                                        created_at,
                                        owner_id,
                                        config_hash,
                                        updated_at,
                                    }),
                                },
                                _ => {
                                    let _ = tx_internal_in.send(
                                        SyncCommand::FailAttemptDetailed {
                                            attempt_id: attempt_id_inner,
                                            code: "TOPIC_PUSH_OWNER_CONFLICT".to_string(),
                                            message: "Topic selected for push has an invalid owner type"
                                                .to_string(),
                                            failed_topic_ids: vec![key.topic_id],
                                        },
                                    );
                                    return;
                                }
                            };
                            if items_by_topic.insert(key.clone(), item).is_some() {
                                let _ = tx_internal_in.send(SyncCommand::FailAttemptDetailed {
                                    attempt_id: attempt_id_inner,
                                    code: "TOPIC_PUSH_DB_FAILED".to_string(),
                                    message: format!(
                                        "Topic query returned duplicate identity {}",
                                        key.topic_id
                                    ),
                                    failed_topic_ids: vec![key.topic_id],
                                });
                                return;
                            }
                        }
                    }

                    let mut batch_push_requests = Vec::with_capacity(requested_topics.len());
                    for key in requested_topics {
                        let Some(item) = items_by_topic.remove(&key) else {
                            let _ = tx_internal_in.send(SyncCommand::FailAttemptDetailed {
                                attempt_id: attempt_id_inner,
                                code: "TOPIC_PUSH_SOURCE_MISSING".to_string(),
                                message: format!(
                                    "Topic selected for push is missing: {}",
                                    key.topic_id
                                ),
                                failed_topic_ids: vec![key.topic_id],
                            });
                            return;
                        };
                        batch_push_requests.push(item);
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
                            .filter_map(|item| item.selector().topic_id().map(str::to_owned))
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

                    // 单响应已经在任务派发前被精确消费；这里只需等待所有操作归零。
                    let current_pending = pending.load(Ordering::SeqCst);
                    if current_pending == 0 {
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
                let manifest_phase_in = manifest_phase.clone();
                let manifest_type_base = manifest_type;
                let attempt_id_inner = attempt_id;

                task_tracker.spawn(async move {
                    futures_util::stream::iter(other_items)
                        .for_each_concurrent(ENTITY_OPERATION_CONCURRENCY, |item| {
                            let id = item.id().to_string();
                            let action = item.action();
                            let deleted_at = item.deleted_at();
                            let h_task = h_in.clone();
                            let c_task = c_in.clone();
                            let b_task = b_in.clone();
                            let token_task = token.clone();
                            let manifest_type_task = manifest_type_base;
                            let wq_task = wq_in.clone();
                            let pending_task = pending.clone();
                            let total_tasks_task = total_tasks_in.clone();
                            let tx_internal_task = tx_internal_in.clone();
                            let manifest_phase_task = manifest_phase_in.clone();
                            let attempt_id_task = attempt_id_inner;

                            async move {
                                let operation_result: Result<(), String> = async {
                                    match (action, &item) {
                                        (
                                            ManifestAction::Pull,
                                            ManifestDecision::Avatar(avatar),
                                        ) => {
                                            PullExecutor::pull_avatar(
                                                &h_task,
                                                &c_task,
                                                &b_task,
                                                &token_task,
                                                avatar.owner_type.as_str(),
                                                &avatar.owner_id,
                                                &wq_task,
                                            )
                                            .await
                                        }
                                        (
                                            ManifestAction::Push,
                                            ManifestDecision::Owner(owner),
                                        ) => {
                                            match owner.owner_type {
                                                OwnerType::Agent => {
                                                    PushExecutor::push_agent(
                                                        &h_task,
                                                        &c_task,
                                                        &b_task,
                                                        &token_task,
                                                        &owner.owner_id,
                                                    )
                                                    .await
                                                }
                                                OwnerType::Group => {
                                                    PushExecutor::push_group(
                                                        &h_task,
                                                        &c_task,
                                                        &b_task,
                                                        &token_task,
                                                        &owner.owner_id,
                                                    )
                                                    .await
                                                }
                                            }
                                        }
                                        (
                                            ManifestAction::Push,
                                            ManifestDecision::Avatar(avatar),
                                        ) => {
                                            PushExecutor::push_avatar(
                                                &h_task,
                                                &c_task,
                                                &b_task,
                                                &token_task,
                                                avatar.owner_type.as_str(),
                                                &avatar.owner_id,
                                            )
                                            .await
                                        }
                                        (
                                            ManifestAction::PullDelete
                                            | ManifestAction::PushDelete,
                                            _,
                                        ) => {
                                            use crate::vcp_modules::sync_executor::delete_executor::DeleteExecutor;

                                            let deleted_at = deleted_at.ok_or_else(|| {
                                                format!(
                                                    "{action} action for {id} is missing a valid deletedAt"
                                                )
                                            })?;
                                            let target = item.delete_target();
                                            match &target {
                                                DeleteTarget::Owner {
                                                    owner_type: OwnerType::Agent,
                                                    owner_id,
                                                } => {
                                                    DeleteExecutor::soft_delete_agent(
                                                        &h_task,
                                                        owner_id,
                                                        deleted_at,
                                                    )
                                                    .await?;
                                                }
                                                DeleteTarget::Owner {
                                                    owner_type: OwnerType::Group,
                                                    owner_id,
                                                } => {
                                                    DeleteExecutor::soft_delete_group(
                                                        &h_task,
                                                        owner_id,
                                                        deleted_at,
                                                    )
                                                    .await?;
                                                }
                                                DeleteTarget::Topic(key) => {
                                                    DeleteExecutor::soft_delete_topic(
                                                        &h_task,
                                                        key,
                                                        deleted_at,
                                                    )
                                                    .await?;
                                                }
                                                DeleteTarget::Avatar {
                                                    owner_type,
                                                    owner_id,
                                                } => {
                                                    DeleteExecutor::soft_delete_avatar(
                                                        &h_task,
                                                        owner_type.as_str(),
                                                        owner_id,
                                                        deleted_at,
                                                    )
                                                    .await?;
                                                }
                                                DeleteTarget::Message(_) => {
                                                    return Err(
                                                        "Manifest cannot contain message deletion"
                                                            .to_string(),
                                                    );
                                                }
                                            }
                                            if action == ManifestAction::PushDelete {
                                                tx_internal_task
                                                    .send(SyncCommand::NotifyDelete {
                                                        target,
                                                        deleted_at,
                                                    })
                                                    .map_err(|error| error.to_string())?;
                                            }
                                            Ok(())
                                        }
                                        _ => Err(format!(
                                            "unsupported {action} action for {manifest_type_task}"
                                        )),
                                    }
                                }
                                .await;

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
                                        if current_pending == 0 {
                                            let phase = manifest_phase_task.load(Ordering::SeqCst);
                                            if let Some(command) =
                                                next_manifest_command(phase, attempt_id_task)
                                            {
                                                let _ = tx_internal_task.send(command);
                                            }
                                        }
                                    }
                                    Err(error) => {
                                        let failed_topic_ids =
                                            if manifest_type_task == ManifestType::Topic {
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
        next_manifest_command, validate_manifest_result, validate_topic_owner_scope,
        MAX_SAFE_JSON_INTEGER,
    };
    use crate::vcp_modules::chat::topic_types::OwnerKey;
    use crate::vcp_modules::sync_service::{ManifestResponseTracker, SyncCommand};
    use crate::vcp_modules::sync_types::{ManifestResultFrame, ManifestType};
    use serde_json::json;
    use std::collections::HashSet;

    #[test]
    fn typed_topic_results_use_the_full_identity_and_reject_duplicates() {
        let result = |owners: &[&str]| {
            serde_json::from_value::<ManifestResultFrame>(json!({
                "type": "SYNC_MANIFEST_RESULT",
                "manifestType": "topic",
                "results": owners.iter().map(|owner_id| json!({
                    "ownerType": "agent",
                    "ownerId": owner_id,
                    "topicId": "default",
                    "action": "PULL"
                })).collect::<Vec<_>>()
            }))
            .expect("valid topic result frame")
        };
        let (_, items) = validate_manifest_result(result(&["agent-a", "agent-b"]))
            .expect("same topic id under different owners is valid");
        assert_eq!(items.len(), 2);

        assert!(validate_manifest_result(result(&["agent-a", "agent-a"])).is_err());
    }

    #[test]
    fn topic_results_must_stay_within_the_targeted_owner_scope() {
        let result = |owner_type: &str, owner_id: &str, topic_id: &str| {
            serde_json::from_value::<ManifestResultFrame>(json!({
                "type": "SYNC_MANIFEST_RESULT",
                "manifestType": "topic",
                "results": [{
                    "ownerType": owner_type,
                    "ownerId": owner_id,
                    "topicId": topic_id,
                    "action": "PULL"
                }]
            }))
            .expect("valid topic result frame")
        };
        let targeted_owners = HashSet::from([OwnerKey::new("agent", "shared-id")]);

        let (_, desktop_only_topic) =
            validate_manifest_result(result("agent", "shared-id", "desktop-only"))
                .expect("valid in-scope topic result");
        validate_topic_owner_scope(&desktop_only_topic, &targeted_owners)
            .expect("desktop-only topics under a targeted owner are valid");

        let (_, wrong_owner_type) =
            validate_manifest_result(result("group", "shared-id", "group-topic"))
                .expect("structurally valid topic result");
        assert!(validate_topic_owner_scope(&wrong_owner_type, &targeted_owners).is_err());

        let (_, wrong_owner_id) =
            validate_manifest_result(result("agent", "other-id", "other-topic"))
                .expect("structurally valid topic result");
        assert!(validate_topic_owner_scope(&wrong_owner_id, &targeted_owners).is_err());
    }

    #[test]
    fn manifest_responses_are_exactly_once_and_waves_keep_their_order() {
        let tracker = ManifestResponseTracker::default();
        tracker.arm(1, ManifestType::Owner).expect("arm owner wave");
        assert!(tracker.arm(2, ManifestType::Avatar).is_err());
        assert!(tracker.consume(1, ManifestType::Topic).is_err());
        assert!(tracker.consume(2, ManifestType::Owner).is_err());
        tracker
            .consume(1, ManifestType::Owner)
            .expect("consume owner response");
        assert!(tracker.consume(1, ManifestType::Owner).is_err());
        tracker
            .arm(2, ManifestType::Avatar)
            .expect("arm avatar wave after owner response");
        tracker
            .consume(2, ManifestType::Avatar)
            .expect("consume avatar response");
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
    fn delete_decisions_require_a_safe_integer_deleted_at() {
        let result = |deleted_at: Option<i64>| {
            serde_json::from_value::<ManifestResultFrame>(json!({
                "type": "SYNC_MANIFEST_RESULT",
                "manifestType": "topic",
                "results": [{
                    "ownerType": "agent",
                    "ownerId": "agent-a",
                    "topicId": "topic-a",
                    "action": "PULL_DELETE",
                    "deletedAt": deleted_at
                }]
            }))
            .expect("valid topic result frame")
        };
        assert!(validate_manifest_result(result(Some(42))).is_ok());
        assert!(validate_manifest_result(result(None)).is_err());
        assert!(validate_manifest_result(result(Some(-1))).is_err());
        assert!(validate_manifest_result(result(Some(MAX_SAFE_JSON_INTEGER + 1))).is_err());
    }
}
