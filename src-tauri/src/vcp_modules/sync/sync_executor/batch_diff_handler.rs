use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_error::{encode_wire_sync_error, WireSyncError};
use crate::vcp_modules::sync_executor::{
    BatchPullResult, DeleteExecutor, PullExecutor, PushExecutor,
};
use crate::vcp_modules::sync_logger::SyncLogger;
use crate::vcp_modules::sync_service::{Phase3DiffBatch, Phase3Tracker, SyncCommand};
use crate::vcp_modules::sync_types::{
    MessageDeleteDecision, MessageDiffDecision, MessageDiffResultFrame,
};
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
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

pub fn parse_message_diff_result_frame(
    text: &str,
) -> Result<MessageDiffResultFrame, Phase3ProtocolError> {
    serde_json::from_str(text).map_err(|error| {
        Phase3ProtocolError::new(
            "PHASE3_FRAME_INVALID",
            format!("Invalid SYNC_MESSAGE_DIFF_RESULT frame: {error}"),
        )
    })
}

#[derive(Debug)]
struct TopicBatchOutcome {
    topic: TopicKey,
    success: bool,
    error: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct TopicDecision {
    pull_message_ids: Vec<String>,
    push_topic: bool,
    delete_messages: Vec<MessageDeleteDecision>,
}

fn parse_topic_decision(
    topic: &TopicKey,
    decision: &MessageDiffDecision,
) -> Result<TopicDecision, Phase3ProtocolError> {
    let topic_id = &topic.topic_id;
    if !decision.ok {
        if decision.pull_message_ids.is_some()
            || decision.push_topic.is_some()
            || decision.delete_messages.is_some()
        {
            return Err(Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!("Phase 3 rejection for {topic_id} must not contain decision fields"),
                topic_id,
            ));
        }
        let wire = decision.error.clone().ok_or_else(|| {
            Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!("Phase 3 rejection for {topic_id} requires error object"),
                topic_id,
            )
        })?;
        return Err(Phase3ProtocolError::from_wire(wire, topic_id)?);
    }

    if decision.error.is_some() {
        return Err(Phase3ProtocolError::for_topic(
            "PHASE3_DECISION_INVALID",
            format!("Successful Phase 3 decision for {topic_id} must not contain error"),
            topic_id,
        ));
    }

    let pull_message_ids = decision.pull_message_ids.as_ref().ok_or_else(|| {
        Phase3ProtocolError::for_topic(
            "PHASE3_DECISION_INVALID",
            format!("Phase 3 decision for {topic_id} requires pullMessageIds"),
            topic_id,
        )
    })?;
    if pull_message_ids.len() > MAX_PHASE3_MESSAGES_PER_TOPIC {
        return Err(Phase3ProtocolError::for_topic(
            "PHASE3_DECISION_BUDGET_EXCEEDED",
            format!(
                "Phase 3 pullMessageIds for {topic_id} exceeds {MAX_PHASE3_MESSAGES_PER_TOPIC} message budget"
            ),
            topic_id,
        ));
    }
    let mut seen = HashSet::new();
    for message_id in pull_message_ids {
        if message_id.is_empty() {
            return Err(Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!("Phase 3 pullMessageIds for {topic_id} contains an empty id"),
                topic_id,
            ));
        }
        if !seen.insert(message_id) {
            return Err(Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!("Phase 3 pullMessageIds for {topic_id} contains duplicate id {message_id}"),
                topic_id,
            ));
        }
    }
    let push_topic = decision.push_topic.ok_or_else(|| {
        Phase3ProtocolError::for_topic(
            "PHASE3_DECISION_INVALID",
            format!("Phase 3 decision for {topic_id} requires boolean pushTopic"),
            topic_id,
        )
    })?;
    let delete_messages = decision.delete_messages.as_ref().ok_or_else(|| {
        Phase3ProtocolError::for_topic(
            "PHASE3_DECISION_INVALID",
            format!("Phase 3 decision for {topic_id} requires deleteMessages"),
            topic_id,
        )
    })?;
    if delete_messages.len() > MAX_PHASE3_MESSAGES_PER_TOPIC {
        return Err(Phase3ProtocolError::for_topic(
            "PHASE3_DECISION_BUDGET_EXCEEDED",
            format!(
                "Phase 3 deleteMessages for {topic_id} exceeds {MAX_PHASE3_MESSAGES_PER_TOPIC} message budget"
            ),
            topic_id,
        ));
    }
    let mut seen_deleted = HashSet::new();
    for item in delete_messages {
        if item.msg_id.is_empty() {
            return Err(Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!("Phase 3 deleteMessages for {topic_id} requires non-empty msgId"),
                topic_id,
            ));
        }
        if !(0..=MAX_SAFE_JSON_INTEGER).contains(&item.deleted_at) {
            return Err(Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!(
                    "Phase 3 deleteMessages for {topic_id} requires a non-negative safe-integer deletedAt"
                ),
                topic_id,
            ));
        }
        if seen.contains(&item.msg_id) {
            return Err(Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!(
                    "Phase 3 decision for {topic_id} contains {} in both pullMessageIds and deleteMessages",
                    item.msg_id
                ),
                topic_id,
            ));
        }
        if !seen_deleted.insert(&item.msg_id) {
            return Err(Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!(
                    "Phase 3 deleteMessages for {topic_id} contains duplicate id {}",
                    item.msg_id
                ),
                topic_id,
            ));
        }
    }
    Ok(TopicDecision {
        pull_message_ids: pull_message_ids.clone(),
        push_topic,
        delete_messages: delete_messages.clone(),
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
    results: &'a [MessageDiffDecision],
) -> Result<Vec<(TopicKey, &'a MessageDiffDecision)>, String> {
    let mut keyed_results = Vec::with_capacity(results.len());
    let mut actual = HashSet::new();
    for result in results {
        let key = TopicKey::new(
            result.owner_type.as_str(),
            &result.owner_id,
            &result.topic_id,
        );
        if !key.is_valid() {
            return Err("SYNC_MESSAGE_DIFF_RESULT contains an invalid topic identity".into());
        }
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
        frame: MessageDiffResultFrame,
        http_client: &reqwest::Client,
        base_url: &str,
        token: &str,
        tracker: &Arc<Phase3Tracker>,
        tx_internal: &mpsc::UnboundedSender<SyncCommand>,
        logger: &Arc<Mutex<SyncLogger>>,
        write_queue: &Arc<DbWriteQueue>,
        pending_diff_batches: &Arc<tokio::sync::Mutex<std::collections::VecDeque<Phase3DiffBatch>>>,
        prerender_enabled: bool,
        expected_batch_topics: &Arc<tokio::sync::Mutex<HashSet<TopicKey>>>,
        attempt_id: u64,
    ) -> Result<(), Phase3ProtocolError> {
        let results = frame.results;
        if results.len() > MAX_PHASE3_TOPICS {
            return Err(Phase3ProtocolError::new(
                "PHASE3_DECISION_BUDGET_EXCEEDED",
                format!("Phase 3 response exceeds {MAX_PHASE3_TOPICS} topic budget"),
            ));
        }
        let keyed_results = {
            let expected = expected_batch_topics.lock().await;
            validate_phase3_result_topics(&expected, &results).map_err(|message| {
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
                let to_pull_ids = decision.pull_message_ids;
                let to_push = decision.push_topic;
                let to_delete = decision.delete_messages;
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
                        let message_key = MessageKey::new(topic.clone(), &tombstone.msg_id);
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
                                    tombstone.msg_id, topic.topic_id,
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
                    let pull_result = PullExecutor::pull_messages_batch(
                        app_handle,
                        http_client,
                        base_url,
                        token,
                        &pull_batch,
                        write_queue,
                        prerender_enabled,
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
                let mut expected = expected_batch_topics.lock().await;
                *expected = next_batch.keys;
                drop(expected);
                let _ = tx_internal.send(SyncCommand::SendMessageDiff {
                    attempt_id,
                    topics: next_batch.topics,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcp_modules::sync_types::OwnerType;

    fn topic(topic_id: &str) -> TopicKey {
        TopicKey::new("agent", "agent-a", topic_id)
    }

    fn decision(topic_id: &str) -> MessageDiffDecision {
        MessageDiffDecision {
            owner_type: OwnerType::Agent,
            owner_id: "agent-a".to_string(),
            topic_id: topic_id.to_string(),
            ok: true,
            pull_message_ids: Some(Vec::new()),
            push_topic: Some(false),
            delete_messages: Some(Vec::new()),
            error: None,
        }
    }

    #[test]
    fn phase3_results_must_cover_the_exact_requested_topic_set() {
        let expected = HashSet::from([topic("topic-a"), topic("topic-b")]);
        let complete = vec![decision("topic-a"), decision("topic-b")];
        assert!(validate_phase3_result_topics(&expected, &complete).is_ok());

        let incomplete = vec![decision("topic-a")];
        let error = validate_phase3_result_topics(&expected, &incomplete)
            .expect_err("missing topic must fail phase 3");
        assert!(error.contains("topic-b"));

        let duplicate = vec![decision("topic-a"), decision("topic-a")];
        assert!(validate_phase3_result_topics(&expected, &duplicate)
            .expect_err("duplicate compound identity must fail phase 3")
            .contains("duplicate topic identity"));
    }

    #[test]
    fn phase3_decision_is_a_strict_discriminated_union() {
        let mut valid = decision("topic-a");
        valid.pull_message_ids = Some(vec!["message-a".to_string()]);
        assert_eq!(
            parse_topic_decision(&topic("topic-a"), &valid).expect("valid decision"),
            TopicDecision {
                pull_message_ids: vec!["message-a".to_string()],
                push_topic: false,
                delete_messages: Vec::new(),
            }
        );

        let mut deleted = decision("topic-a");
        deleted.delete_messages = Some(vec![MessageDeleteDecision {
            msg_id: "message-b".to_string(),
            deleted_at: 1234,
        }]);
        assert_eq!(
            parse_topic_decision(&topic("topic-a"), &deleted)
                .expect("valid delete decision")
                .delete_messages,
            vec![MessageDeleteDecision {
                msg_id: "message-b".to_string(),
                deleted_at: 1234,
            }]
        );

        let mut invalid = decision("topic-a");
        invalid.pull_message_ids = None;
        assert!(parse_topic_decision(&topic("topic-a"), &invalid).is_err());
    }

    #[test]
    fn raw_phase3_parser_rejects_legacy_result_maps() {
        let legacy = r#"{
            "type":"SYNC_DIFF_RESULTS_BATCH",
            "results":{
                "topic-a":{"ok":true,"toPull":[],"toPush":false}
            }
        }"#;
        let error = parse_message_diff_result_frame(legacy)
            .expect_err("legacy result maps must not be accepted");
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
