use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_error::{
    encode_wire_sync_error, parse_wire_sync_error, WireSyncError,
};
use crate::vcp_modules::sync_executor::{
    BatchPullResult, DeleteExecutor, PullExecutor, PullProgressContext, PushExecutor,
};
use crate::vcp_modules::sync_logger::SyncLogger;
use crate::vcp_modules::sync_service::{Phase3DiffBatch, Phase3Tracker, SyncCommand};
use crate::vcp_modules::sync_types::parse_topic_key;
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::AppHandle;
use tokio::sync::mpsc;

pub struct BatchDiffHandler;

const MAX_PHASE3_TOPICS: usize = 10_000;
const MAX_PHASE3_MESSAGES_PER_TOPIC: usize = 10_000;
const MAX_PHASE3_MESSAGES: usize = 100_000;
const MAX_SAFE_JSON_INTEGER: i64 = (1_i64 << 53) - 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase3ProtocolError {
    pub code: String,
    pub message: String,
    pub failed_topic_ids: Vec<String>,
}

impl Phase3ProtocolError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            failed_topic_ids: Vec::new(),
        }
    }

    fn for_topic(code: impl Into<String>, message: impl Into<String>, topic_id: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            failed_topic_ids: vec![topic_id.to_string()],
        }
    }

    fn from_wire(wire: WireSyncError, topic_id: &str) -> Result<Self, Self> {
        let code = wire.code.clone();
        let mut failed_topic_ids = wire.failed_topic_ids.clone();
        if !failed_topic_ids.iter().any(|id| id == topic_id) {
            failed_topic_ids.push(topic_id.to_string());
        }
        failed_topic_ids.truncate(8);
        let message = encode_wire_sync_error(&wire).map_err(|error| {
            Self::for_topic(
                "PHASE3_DECISION_INVALID",
                format!("Phase 3 rejection for {topic_id} has invalid error: {error}"),
                topic_id,
            )
        })?;
        Ok(Self {
            code,
            message,
            failed_topic_ids,
        })
    }
}

impl fmt::Display for Phase3ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Phase3ProtocolError {}

#[derive(Deserialize)]
struct Phase3BatchWire {
    #[serde(rename = "type")]
    message_type: String,
    results: Vec<Value>,
}

pub fn parse_phase3_batch_frame(text: &str) -> Result<Value, Phase3ProtocolError> {
    let wire: Phase3BatchWire = serde_json::from_str(text).map_err(|error| {
        Phase3ProtocolError::new(
            "PHASE3_FRAME_INVALID",
            format!("Invalid Phase 3 batch frame: {error}"),
        )
    })?;
    if wire.message_type != "SYNC_DIFF_RESULTS_BATCH" {
        return Err(Phase3ProtocolError::new(
            "PHASE3_FRAME_INVALID",
            "Phase 3 batch frame has an unexpected type",
        ));
    }
    Ok(json!({
        "type": wire.message_type,
        "results": wire.results,
    }))
}

#[derive(Debug)]
struct TopicBatchOutcome {
    topic: TopicKey,
    success: bool,
    error: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct TopicDecision {
    to_pull: Vec<String>,
    to_push: bool,
    to_delete: Vec<MessageDeleteDecision>,
}

#[derive(Debug, PartialEq, Eq)]
struct MessageDeleteDecision {
    message_id: String,
    deleted_at: i64,
}

fn parse_topic_decision(
    topic: &TopicKey,
    value: &Value,
) -> Result<TopicDecision, Phase3ProtocolError> {
    let topic_id = &topic.topic_id;
    let object = value.as_object().ok_or_else(|| {
        Phase3ProtocolError::for_topic(
            "PHASE3_DECISION_INVALID",
            format!("Phase 3 decision for {topic_id} must be an object"),
            topic_id,
        )
    })?;
    let ok = object.get("ok").and_then(Value::as_bool).ok_or_else(|| {
        Phase3ProtocolError::for_topic(
            "PHASE3_DECISION_INVALID",
            format!("Phase 3 decision for {topic_id} requires boolean ok"),
            topic_id,
        )
    })?;
    if !ok {
        if object.contains_key("toPull")
            || object.contains_key("toPush")
            || object.contains_key("toDelete")
        {
            return Err(Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!("Phase 3 rejection for {topic_id} must not contain toPull/toPush/toDelete"),
                topic_id,
            ));
        }
        let error = object.get("error").ok_or_else(|| {
            Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!("Phase 3 rejection for {topic_id} requires error object"),
                topic_id,
            )
        })?;
        let wire = parse_wire_sync_error(error).map_err(|parse_error| {
            Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!("Phase 3 rejection for {topic_id} has invalid error: {parse_error}"),
                topic_id,
            )
        })?;
        return Err(Phase3ProtocolError::from_wire(wire, topic_id)?);
    }

    if object.contains_key("error") {
        return Err(Phase3ProtocolError::for_topic(
            "PHASE3_DECISION_INVALID",
            format!("Successful Phase 3 decision for {topic_id} must not contain error"),
            topic_id,
        ));
    }

    let to_pull_values = object
        .get("toPull")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!("Phase 3 decision for {topic_id} requires string[] toPull"),
                topic_id,
            )
        })?;
    if to_pull_values.len() > MAX_PHASE3_MESSAGES_PER_TOPIC {
        return Err(Phase3ProtocolError::for_topic(
            "PHASE3_DECISION_BUDGET_EXCEEDED",
            format!(
                "Phase 3 toPull for {topic_id} exceeds {MAX_PHASE3_MESSAGES_PER_TOPIC} message budget"
            ),
            topic_id,
        ));
    }
    let mut seen = HashSet::new();
    let mut to_pull = Vec::with_capacity(to_pull_values.len());
    for value in to_pull_values {
        let message_id = value.as_str().filter(|id| !id.is_empty()).ok_or_else(|| {
            Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!("Phase 3 toPull for {topic_id} contains a non-string or empty id"),
                topic_id,
            )
        })?;
        if !seen.insert(message_id) {
            return Err(Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!("Phase 3 toPull for {topic_id} contains duplicate id {message_id}"),
                topic_id,
            ));
        }
        to_pull.push(message_id.to_string());
    }
    let to_push = object
        .get("toPush")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!("Phase 3 decision for {topic_id} requires boolean toPush"),
                topic_id,
            )
        })?;
    let to_delete_values = object
        .get("toDelete")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!("Phase 3 decision for {topic_id} requires array toDelete"),
                topic_id,
            )
        })
        .map(Vec::as_slice)?;
    if to_delete_values.len() > MAX_PHASE3_MESSAGES_PER_TOPIC {
        return Err(Phase3ProtocolError::for_topic(
            "PHASE3_DECISION_BUDGET_EXCEEDED",
            format!(
                "Phase 3 toDelete for {topic_id} exceeds {MAX_PHASE3_MESSAGES_PER_TOPIC} message budget"
            ),
            topic_id,
        ));
    }
    let mut seen_deleted = HashSet::new();
    let mut to_delete = Vec::with_capacity(to_delete_values.len());
    for value in to_delete_values {
        let item = value.as_object().ok_or_else(|| {
            Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!("Phase 3 toDelete for {topic_id} contains a non-object item"),
                topic_id,
            )
        })?;
        let message_id = item
            .get("msgId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                Phase3ProtocolError::for_topic(
                    "PHASE3_DECISION_INVALID",
                    format!("Phase 3 toDelete for {topic_id} requires non-empty msgId"),
                    topic_id,
                )
            })?;
        let deleted_at = item
            .get("deletedAt")
            .and_then(Value::as_i64)
            .filter(|timestamp| (0..=MAX_SAFE_JSON_INTEGER).contains(timestamp))
            .ok_or_else(|| {
                Phase3ProtocolError::for_topic(
                    "PHASE3_DECISION_INVALID",
                    format!(
                        "Phase 3 toDelete for {topic_id} requires a non-negative safe-integer deletedAt"
                    ),
                    topic_id,
                )
            })?;
        if seen.contains(message_id) {
            return Err(Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!(
                    "Phase 3 decision for {topic_id} contains {message_id} in both toPull and toDelete"
                ),
                topic_id,
            ));
        }
        if !seen_deleted.insert(message_id) {
            return Err(Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!("Phase 3 toDelete for {topic_id} contains duplicate id {message_id}"),
                topic_id,
            ));
        }
        to_delete.push(MessageDeleteDecision {
            message_id: message_id.to_string(),
            deleted_at,
        });
    }
    Ok(TopicDecision {
        to_pull,
        to_push,
        to_delete,
    })
}

fn validate_topic_batch_outcomes(
    operation: &str,
    expected: &[TopicKey],
    batch_result: Result<Vec<TopicBatchOutcome>, String>,
) -> Result<Vec<TopicKey>, String> {
    let outcomes = batch_result.map_err(|error| {
        let mut topics = expected.to_vec();
        topics.sort();
        format!(
            "Phase 3 {} batch failed for topics {:?}: {}",
            operation, topics, error
        )
    })?;
    let expected_set = expected.iter().cloned().collect::<HashSet<_>>();
    let mut outcomes_by_topic = HashMap::new();
    for outcome in outcomes {
        if !expected_set.contains(&outcome.topic) {
            return Err(format!(
                "Phase 3 {operation} response contains unexpected topic {}",
                outcome.topic.topic_id
            ));
        }
        if outcomes_by_topic
            .insert(outcome.topic.clone(), outcome)
            .is_some()
        {
            return Err(format!(
                "Phase 3 {operation} response contains duplicate topic"
            ));
        }
    }

    let mut successful = Vec::new();
    let mut failed = Vec::new();
    for topic in expected {
        match outcomes_by_topic.get(topic) {
            Some(outcome) if outcome.success => successful.push(topic.clone()),
            Some(outcome) => failed.push(format!(
                "{}: {}",
                topic.topic_id,
                outcome.error.as_deref().unwrap_or("unknown error")
            )),
            None => failed.push(format!("{}: missing from batch response", topic.topic_id)),
        }
    }

    if failed.is_empty() {
        Ok(successful)
    } else {
        failed.sort();
        Err(format!(
            "Phase 3 {} failed topics: {}",
            operation,
            failed.join(", ")
        ))
    }
}

fn validate_phase3_result_topics<'a>(
    expected: &HashSet<TopicKey>,
    results: &'a [Value],
) -> Result<Vec<(TopicKey, &'a Value)>, String> {
    let mut keyed_results = Vec::with_capacity(results.len());
    let mut actual = HashSet::new();
    for (index, result) in results.iter().enumerate() {
        let key = parse_topic_key(result, &format!("SYNC_DIFF_RESULTS_BATCH.results[{index}]"))?;
        if !actual.insert(key.clone()) {
            return Err("Phase 3 response contains a duplicate topic identity".to_string());
        }
        keyed_results.push((key, result));
    }
    if actual == *expected {
        return Ok(keyed_results);
    }

    let mut missing = expected.difference(&actual).cloned().collect::<Vec<_>>();
    let mut unexpected = actual.difference(expected).cloned().collect::<Vec<_>>();
    missing.sort();
    unexpected.sort();
    Err(format!(
        "Phase 3 response topic mismatch: missing={:?}, unexpected={:?}",
        missing, unexpected
    ))
}

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
        pending_diff_batches: &Arc<tokio::sync::Mutex<std::collections::VecDeque<Phase3DiffBatch>>>,
        prerender_enabled: bool,
        uploaded_hashes: &Arc<tokio::sync::RwLock<HashSet<String>>>,
        expected_batch_topics: &Arc<tokio::sync::Mutex<HashSet<TopicKey>>>,
        attempt_id: u64,
    ) -> Result<(), Phase3ProtocolError> {
        let results = payload["results"].as_array().ok_or_else(|| {
            Phase3ProtocolError::new(
                "PHASE3_FRAME_INVALID",
                "Phase 3 response is missing results",
            )
        })?;
        if results.len() > MAX_PHASE3_TOPICS {
            return Err(Phase3ProtocolError::new(
                "PHASE3_DECISION_BUDGET_EXCEEDED",
                format!("Phase 3 response exceeds {MAX_PHASE3_TOPICS} topic budget"),
            ));
        }
        let keyed_results = {
            let expected = expected_batch_topics.lock().await;
            validate_phase3_result_topics(&expected, results).map_err(|message| {
                let mut failed_topic_ids = expected
                    .iter()
                    .map(|topic| topic.topic_id.clone())
                    .collect::<Vec<_>>();
                failed_topic_ids.sort();
                failed_topic_ids.truncate(8);
                Phase3ProtocolError {
                    code: "PHASE3_TOPIC_MISMATCH".to_string(),
                    message,
                    failed_topic_ids,
                }
            })?
        };

        {
            // 分类 topics: push、pull、delete 可以在同一个 topic 上组合出现。
            let mut push_topics: Vec<TopicKey> = Vec::new();
            let mut pull_batch: Vec<(TopicKey, Vec<String>)> = Vec::new();
            let mut delete_batch: Vec<(TopicKey, Vec<MessageDeleteDecision>)> = Vec::new();
            let mut total_message_operations = 0usize;

            for (topic, result) in keyed_results {
                let topic_id = &topic.topic_id;
                let decision = parse_topic_decision(&topic, result)?;
                let to_pull_ids = decision.to_pull;
                let to_push = decision.to_push;
                let to_delete = decision.to_delete;
                total_message_operations = total_message_operations
                    .checked_add(to_pull_ids.len())
                    .and_then(|total| total.checked_add(to_delete.len()))
                    .ok_or_else(|| {
                        Phase3ProtocolError::for_topic(
                            "PHASE3_DECISION_BUDGET_EXCEEDED",
                            "Phase 3 message operation count overflow",
                            topic_id,
                        )
                    })?;
                if total_message_operations > MAX_PHASE3_MESSAGES {
                    return Err(Phase3ProtocolError::for_topic(
                        "PHASE3_DECISION_BUDGET_EXCEEDED",
                        format!(
                            "Phase 3 response exceeds {MAX_PHASE3_MESSAGES} message operation budget"
                        ),
                        topic_id,
                    ));
                }

                // Phase 2.5 已判定该 topic 聚合哈希有变化；即使消息 diff 是合法
                // no-op，也必须进入 finalizer 的 hash-repair 集。
                tracker.mark_modified(&topic).await;

                if !to_push && to_pull_ids.is_empty() && to_delete.is_empty() {
                    // 无需操作，直接标记完成
                    tracker
                        .mark_completed(&topic, logger, tx_internal, app_handle, true)
                        .await;
                    continue;
                }

                if to_push {
                    push_topics.push(topic.clone());
                }
                if !to_pull_ids.is_empty() {
                    pull_batch.push((topic.clone(), to_pull_ids));
                }
                if !to_delete.is_empty() {
                    delete_batch.push((topic, to_delete));
                }
            }

            let has_push = !push_topics.is_empty();
            let has_pull = !pull_batch.is_empty();
            let has_delete = !delete_batch.is_empty();

            if has_push || has_pull || has_delete {
                // 收集所有涉及的 topic ID（去重）
                let mut all_topics: HashSet<TopicKey> = HashSet::new();
                for topic in &push_topics {
                    all_topics.insert(topic.clone());
                }
                for (topic, _) in &pull_batch {
                    all_topics.insert(topic.clone());
                }
                for (topic, _) in &delete_batch {
                    all_topics.insert(topic.clone());
                }

                // 桌面墓碑先落到本地，避免同批次 push 把已经删除的 live 消息复活。
                for (topic, tombstones) in &delete_batch {
                    for tombstone in tombstones {
                        let message_key = MessageKey::new(topic.clone(), &tombstone.message_id);
                        if let Err(error) = DeleteExecutor::soft_delete_message(
                            app_handle,
                            &message_key,
                            tombstone.deleted_at,
                        )
                        .await
                        {
                            tracker.mark_failed(topic).await;
                            return Err(Phase3ProtocolError::for_topic(
                                "SYNC_DELETE_FAILED",
                                format!(
                                    "Failed to apply desktop message tombstone {} for {}: {error}",
                                    tombstone.message_id, topic.topic_id,
                                ),
                                &topic.topic_id,
                            ));
                        }
                    }
                    tracker.mark_modified(topic).await;
                }

                if has_pull {
                    let pull_topics = pull_batch
                        .iter()
                        .map(|(topic, _)| topic.clone())
                        .collect::<Vec<_>>();
                    // 展示型进度基数：批次开始前快照 tracker，使 NDJSON 流内
                    // 逐 topic 上报的 completed = 基数 + 本批次已成功数
                    let pull_progress = PullProgressContext {
                        session_id: tracker.session_id,
                        base_completed: tracker.completed.lock().await.len(),
                        total: tracker.total.load(std::sync::atomic::Ordering::SeqCst),
                        failed: tracker.failed.lock().await.len(),
                        legacy_attachment_warnings: tracker
                            .legacy_attachment_warnings
                            .load(std::sync::atomic::Ordering::SeqCst),
                    };
                    let pull_result = PullExecutor::pull_messages_batch(
                        app_handle,
                        http_client,
                        base_url,
                        token,
                        &pull_batch,
                        write_queue,
                        prerender_enabled,
                        Some(pull_progress),
                    )
                    .await
                    .map(|results| {
                        let warning_count = results
                            .iter()
                            .map(|result| result.legacy_attachment_warnings)
                            .sum();
                        tracker.add_legacy_attachment_warnings(warning_count);
                        results
                            .into_iter()
                            .map(|result: BatchPullResult| TopicBatchOutcome {
                                topic: result.topic,
                                success: result.success,
                                error: result.error,
                            })
                            .collect()
                    });
                    match validate_topic_batch_outcomes("pull", &pull_topics, pull_result) {
                        Ok(successful) => {
                            for topic in successful {
                                tracker.mark_modified(&topic).await;
                            }
                        }
                        Err(message) => {
                            for topic in &pull_topics {
                                tracker.mark_failed(topic).await;
                            }
                            let mut failed_topic_ids = pull_topics
                                .iter()
                                .map(|topic| topic.topic_id.clone())
                                .collect::<Vec<_>>();
                            failed_topic_ids.sort();
                            failed_topic_ids.truncate(8);
                            return Err(Phase3ProtocolError {
                                code: "PHASE3_PULL_FAILED".to_string(),
                                message,
                                failed_topic_ids,
                            });
                        }
                    }
                    if let Err(message) = write_queue.flush().await {
                        for topic in &pull_topics {
                            tracker.mark_failed(topic).await;
                        }
                        let mut failed_topic_ids = pull_topics
                            .iter()
                            .map(|topic| topic.topic_id.clone())
                            .collect::<Vec<_>>();
                        failed_topic_ids.sort();
                        failed_topic_ids.truncate(8);
                        return Err(Phase3ProtocolError {
                            code: "PHASE3_PULL_FAILED".to_string(),
                            message: format!(
                                "Phase 3 pull write drain failed before merged push: {message}"
                            ),
                            failed_topic_ids,
                        });
                    }
                }

                // At most one Phase 3 HTTP batch may be in flight. Pull winners are durable
                // before this whole-topic push reads the merged Mobile view.
                if has_push {
                    let push_result = PushExecutor::push_messages_batch(
                        app_handle,
                        http_client,
                        base_url,
                        token,
                        &push_topics,
                        uploaded_hashes.clone(),
                    )
                    .await
                    .map(|results| {
                        results
                            .into_iter()
                            .map(|result| TopicBatchOutcome {
                                topic: result.topic,
                                success: result.success,
                                error: result.error,
                            })
                            .collect()
                    });
                    match validate_topic_batch_outcomes("push", &push_topics, push_result) {
                        Ok(successful) => {
                            for topic in successful {
                                tracker.mark_modified(&topic).await;
                            }
                        }
                        Err(message) => {
                            for topic in &push_topics {
                                tracker.mark_failed(topic).await;
                            }
                            let mut failed_topic_ids = push_topics
                                .iter()
                                .map(|topic| topic.topic_id.clone())
                                .collect::<Vec<_>>();
                            failed_topic_ids.sort();
                            failed_topic_ids.truncate(8);
                            return Err(Phase3ProtocolError {
                                code: "PHASE3_PUSH_FAILED".to_string(),
                                message,
                                failed_topic_ids,
                            });
                        }
                    }
                }

                for topic in &all_topics {
                    tracker
                        .mark_completed(topic, logger, tx_internal, app_handle, false)
                        .await;
                }
                log::info!(
                    "[SyncService] Phase 3 batch done: push={} pull={} delete={}",
                    push_topics.len(),
                    pull_batch.len(),
                    delete_batch
                        .iter()
                        .map(|(_, tombstones)| tombstones.len())
                        .sum::<usize>()
                );
            }

            // 当前批次处理完毕，发送下一批（如果还有）
            let mut pending = pending_diff_batches.lock().await;
            if let Some(next_batch) = pending.pop_front() {
                log::debug!(
                    "[SyncService] Sending next diff batch, {} remaining",
                    pending.len()
                );
                let msg = json!({
                    "type": "SYNC_MESSAGE_DIFF_BATCH",
                    "topics": next_batch.topics,
                });
                let mut expected = expected_batch_topics.lock().await;
                *expected = next_batch.keys;
                drop(expected);
                let _ = tx_internal.send(SyncCommand::SendWsMessage {
                    attempt_id,
                    value: msg,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topic(topic_id: &str) -> TopicKey {
        TopicKey::new("agent", "agent-a", topic_id)
    }

    #[test]
    fn phase3_results_must_cover_the_exact_requested_topic_set() {
        let expected = HashSet::from([topic("topic-a"), topic("topic-b")]);
        let complete = vec![
            json!({"topicId":"topic-a","ownerType":"agent","ownerId":"agent-a"}),
            json!({"topicId":"topic-b","ownerType":"agent","ownerId":"agent-a"}),
        ];
        assert!(validate_phase3_result_topics(&expected, &complete).is_ok());

        let incomplete = vec![json!({"topicId":"topic-a","ownerType":"agent","ownerId":"agent-a"})];
        let error = validate_phase3_result_topics(&expected, &incomplete)
            .expect_err("missing topic must fail phase 3");
        assert!(error.contains("topic-b"));

        let duplicate = vec![complete[0].clone(), complete[0].clone()];
        assert!(validate_phase3_result_topics(&expected, &duplicate)
            .expect_err("duplicate compound identity must fail phase 3")
            .contains("duplicate topic identity"));
    }

    #[test]
    fn phase3_decision_is_a_strict_discriminated_union() {
        assert_eq!(
            parse_topic_decision(
                &topic("topic-a"),
                &json!({
                    "ok": true,
                    "toPull": ["message-a"],
                    "toPush": false,
                    "toDelete": []
                })
            )
            .expect("valid decision"),
            TopicDecision {
                to_pull: vec!["message-a".to_string()],
                to_push: false,
                to_delete: Vec::new(),
            }
        );
        assert_eq!(
            parse_topic_decision(
                &topic("topic-a"),
                &json!({
                    "ok": true,
                    "toPull": [],
                    "toPush": false,
                    "toDelete": [{ "msgId": "message-b", "deletedAt": 1234 }]
                })
            )
            .expect("valid delete decision")
            .to_delete,
            vec![MessageDeleteDecision {
                message_id: "message-b".to_string(),
                deleted_at: 1234,
            }]
        );

        for invalid in [
            json!({ "toPull": [], "toPush": false }),
            json!({ "ok": true, "toPull": "message-a", "toPush": false }),
            json!({ "ok": true, "toPull": [], "toPush": false }),
            json!({ "ok": true, "toPull": [], "toPush": false, "toDelete": [{ "msgId": "message-a", "deletedAt": -1 }] }),
            json!({ "ok": true, "toPull": [], "toPush": false, "toDelete": [{ "msgId": "message-a", "deletedAt": 1 }, { "msgId": "message-a", "deletedAt": 2 }] }),
            json!({ "ok": true, "toPull": ["message-a"], "toPush": false, "toDelete": [{ "msgId": "message-a", "deletedAt": 1 }] }),
        ] {
            assert!(parse_topic_decision(&topic("topic-a"), &invalid).is_err());
        }

        let rejection = parse_topic_decision(
            &topic("topic-a"),
            &json!({
                "ok": false,
                "error": {
                    "code": "SYNC_OWNER_CONFLICT",
                    "origin": "desktop_cds",
                    "stage": "messages",
                    "kind": "data",
                    "retry": "manual",
                    "message": "failed",
                    "failedTopicIds": ["topic-a"]
                }
            }),
        )
        .expect_err("desktop rejection must terminate phase 3");
        assert_eq!(rejection.code, "SYNC_OWNER_CONFLICT");
        assert_eq!(rejection.failed_topic_ids, vec!["topic-a"]);
        assert_eq!(
            crate::vcp_modules::sync_error::decode_wire_sync_error(&rejection.message)
                .expect("encoded root error")
                .origin,
            crate::vcp_modules::sync_error::SyncErrorOrigin::DesktopCds
        );
    }

    #[test]
    fn raw_phase3_parser_rejects_legacy_result_maps() {
        let legacy = r#"{
            "type":"SYNC_DIFF_RESULTS_BATCH",
            "results":{
                "topic-a":{"ok":true,"toPull":[],"toPush":false}
            }
        }"#;
        let error =
            parse_phase3_batch_frame(legacy).expect_err("legacy result maps must not be accepted");
        assert_eq!(error.code, "PHASE3_FRAME_INVALID");
    }

    #[test]
    fn push_topic_failure_rejects_the_batch() {
        let expected = vec![topic("topic-a")];
        let error = validate_topic_batch_outcomes(
            "push",
            &expected,
            Ok(vec![TopicBatchOutcome {
                topic: topic("topic-a"),
                success: false,
                error: Some("desktop rejected upload".to_string()),
            }]),
        )
        .expect_err("a false push result must fail phase 3");

        assert!(error.contains("topic-a"));
        assert!(error.contains("desktop rejected upload"));
    }

    #[test]
    fn missing_pull_topic_rejects_the_batch() {
        let expected = vec![topic("topic-a"), topic("topic-b")];
        let error = validate_topic_batch_outcomes(
            "pull",
            &expected,
            Ok(vec![TopicBatchOutcome {
                topic: topic("topic-a"),
                success: true,
                error: None,
            }]),
        )
        .expect_err("a missing pull response must fail phase 3");

        assert!(error.contains("topic-b"));
        assert!(error.contains("missing from batch response"));
    }

    #[test]
    fn duplicate_or_unexpected_batch_outcomes_are_rejected() {
        let expected = vec![topic("topic-a")];
        let duplicate = validate_topic_batch_outcomes(
            "push",
            &expected,
            Ok(vec![
                TopicBatchOutcome {
                    topic: topic("topic-a"),
                    success: true,
                    error: None,
                },
                TopicBatchOutcome {
                    topic: topic("topic-a"),
                    success: true,
                    error: None,
                },
            ]),
        )
        .expect_err("duplicate response topic must fail");
        assert!(duplicate.contains("duplicate topic"));

        let unexpected = validate_topic_batch_outcomes(
            "push",
            &expected,
            Ok(vec![TopicBatchOutcome {
                topic: topic("topic-b"),
                success: true,
                error: None,
            }]),
        )
        .expect_err("unexpected response topic must fail");
        assert!(unexpected.contains("unexpected topic topic-b"));
    }

    #[test]
    fn batch_error_rejects_all_expected_topics() {
        let expected = vec![topic("topic-b"), topic("topic-a")];
        let error =
            validate_topic_batch_outcomes("push", &expected, Err("transport closed".to_string()))
                .expect_err("a batch-level error must fail phase 3");

        assert!(error.contains("topic-a"));
        assert!(error.contains("topic-b"));
        assert!(error.contains("transport closed"));
    }
}
