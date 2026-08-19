use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_error::{
    encode_wire_sync_error, parse_wire_sync_error, WireSyncError,
};
use crate::vcp_modules::sync_executor::{
    BatchPullResult, PullExecutor, PullProgressContext, PushExecutor,
};
use crate::vcp_modules::sync_logger::SyncLogger;
use crate::vcp_modules::sync_service::{Phase3Tracker, SyncCommand};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::AppHandle;
use tokio::sync::mpsc;

pub struct BatchDiffHandler;

const MAX_PHASE3_TOPICS: usize = 10_000;
const MAX_PHASE3_MESSAGES: usize = 100_000;

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

struct UniqueResults(Map<String, Value>);

impl<'de> Deserialize<'de> for UniqueResults {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct UniqueResultsVisitor;

        impl<'de> Visitor<'de> for UniqueResultsVisitor {
            type Value = UniqueResults;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object with unique topic ids")
            }

            fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut results = Map::new();
                while let Some((topic_id, value)) = access.next_entry::<String, Value>()? {
                    if results.insert(topic_id.clone(), value).is_some() {
                        return Err(de::Error::custom(format!(
                            "duplicate Phase 3 topic id {topic_id}"
                        )));
                    }
                }
                Ok(UniqueResults(results))
            }
        }

        deserializer.deserialize_map(UniqueResultsVisitor)
    }
}

#[derive(Deserialize)]
struct Phase3BatchWire {
    #[serde(rename = "type")]
    message_type: String,
    results: UniqueResults,
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
        "results": wire.results.0,
    }))
}

#[derive(Debug)]
struct TopicBatchOutcome {
    topic_id: String,
    success: bool,
    error: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct TopicDecision {
    to_pull: Vec<String>,
    to_push: bool,
}

fn parse_topic_decision(
    topic_id: &str,
    value: &Value,
) -> Result<TopicDecision, Phase3ProtocolError> {
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
        if object.contains_key("toPull") || object.contains_key("toPush") {
            return Err(Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!("Phase 3 rejection for {topic_id} must not contain toPull/toPush"),
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
    if to_pull_values.len() > MAX_PHASE3_TOPICS {
        return Err(Phase3ProtocolError::for_topic(
            "PHASE3_DECISION_BUDGET_EXCEEDED",
            format!("Phase 3 toPull for {topic_id} exceeds {MAX_PHASE3_TOPICS} message budget"),
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
    Ok(TopicDecision { to_pull, to_push })
}

fn validate_topic_batch_outcomes(
    operation: &str,
    expected: &[String],
    batch_result: Result<Vec<TopicBatchOutcome>, String>,
) -> Result<Vec<String>, String> {
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
        if !expected_set.contains(&outcome.topic_id) {
            return Err(format!(
                "Phase 3 {operation} response contains unexpected topic {}",
                outcome.topic_id
            ));
        }
        if outcomes_by_topic
            .insert(outcome.topic_id.clone(), outcome)
            .is_some()
        {
            return Err(format!(
                "Phase 3 {operation} response contains duplicate topic"
            ));
        }
    }

    let mut successful = Vec::new();
    let mut failed = Vec::new();
    for topic_id in expected {
        match outcomes_by_topic.get(topic_id) {
            Some(outcome) if outcome.success => successful.push(topic_id.clone()),
            Some(outcome) => failed.push(format!(
                "{}: {}",
                topic_id,
                outcome.error.as_deref().unwrap_or("unknown error")
            )),
            None => failed.push(format!("{}: missing from batch response", topic_id)),
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

fn validate_phase3_result_topics(
    expected: &HashSet<String>,
    results: &serde_json::Map<String, Value>,
) -> Result<(), String> {
    let actual: HashSet<String> = results.keys().cloned().collect();
    if actual == *expected {
        return Ok(());
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
        pending_diff_batches: &Arc<
            tokio::sync::Mutex<
                std::collections::VecDeque<serde_json::Map<String, serde_json::Value>>,
            >,
        >,
        prerender_enabled: bool,
        uploaded_hashes: &Arc<tokio::sync::RwLock<HashSet<String>>>,
        expected_batch_topics: &Arc<tokio::sync::Mutex<HashSet<String>>>,
        attempt_id: u64,
    ) -> Result<(), Phase3ProtocolError> {
        let results = payload["results"].as_object().ok_or_else(|| {
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
        {
            let expected = expected_batch_topics.lock().await;
            validate_phase3_result_topics(&expected, results).map_err(|message| {
                let mut failed_topic_ids = expected.iter().cloned().collect::<Vec<_>>();
                failed_topic_ids.sort();
                failed_topic_ids.truncate(8);
                Phase3ProtocolError {
                    code: "PHASE3_TOPIC_MISMATCH".to_string(),
                    message,
                    failed_topic_ids,
                }
            })?;
        }

        {
            // 分类 topics: push_only, push_pull, pull_only
            let mut push_topic_ids: Vec<String> = Vec::new();
            let mut pull_batch: Vec<(String, Vec<String>)> = Vec::new();
            let mut total_pull_messages = 0usize;

            for (topic_id, result) in results {
                let decision = parse_topic_decision(topic_id, result)?;
                let to_pull_ids = decision.to_pull;
                let to_push = decision.to_push;
                total_pull_messages = total_pull_messages
                    .checked_add(to_pull_ids.len())
                    .ok_or_else(|| {
                        Phase3ProtocolError::for_topic(
                            "PHASE3_DECISION_BUDGET_EXCEEDED",
                            "Phase 3 toPull message count overflow",
                            topic_id,
                        )
                    })?;
                if total_pull_messages > MAX_PHASE3_MESSAGES {
                    return Err(Phase3ProtocolError::for_topic(
                        "PHASE3_DECISION_BUDGET_EXCEEDED",
                        format!(
                            "Phase 3 response exceeds {MAX_PHASE3_MESSAGES} toPull message budget"
                        ),
                        topic_id,
                    ));
                }

                // Phase 2.5 已判定该 topic 聚合哈希有变化；即使消息 diff 是合法
                // no-op，也必须进入 finalizer 的 hash-repair 集。
                tracker.mark_modified(topic_id).await;

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
                // 收集所有涉及的 topic ID（去重）
                let mut all_topic_ids: HashSet<String> = HashSet::new();
                for tid in &push_topic_ids {
                    all_topic_ids.insert(tid.clone());
                }
                for (tid, _) in &pull_batch {
                    all_topic_ids.insert(tid.clone());
                }

                // At most one Phase 3 HTTP batch may be in flight. The WebSocket owner awaits
                // this work before requesting the next diff batch, so final ACK cannot overtake
                // DB/HTTP failures and multiple 256 MiB responses cannot stack in memory.
                if has_push {
                    let push_result = PushExecutor::push_messages_batch(
                        app_handle,
                        http_client,
                        base_url,
                        token,
                        &push_topic_ids,
                        uploaded_hashes.clone(),
                    )
                    .await
                    .map(|results| {
                        results
                            .into_iter()
                            .map(|result| TopicBatchOutcome {
                                topic_id: result.topic_id,
                                success: result.success,
                                error: result.error,
                            })
                            .collect()
                    });
                    match validate_topic_batch_outcomes("push", &push_topic_ids, push_result) {
                        Ok(successful) => {
                            for topic_id in successful {
                                tracker.mark_modified(&topic_id).await;
                            }
                        }
                        Err(message) => {
                            for topic_id in &push_topic_ids {
                                tracker.mark_failed(topic_id).await;
                            }
                            let mut failed_topic_ids = push_topic_ids.clone();
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

                if has_pull {
                    let pull_topic_ids = pull_batch
                        .iter()
                        .map(|(topic_id, _)| topic_id.clone())
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
                                topic_id: result.topic_id,
                                success: result.success,
                                error: result.error,
                            })
                            .collect()
                    });
                    match validate_topic_batch_outcomes("pull", &pull_topic_ids, pull_result) {
                        Ok(successful) => {
                            for topic_id in successful {
                                tracker.mark_modified(&topic_id).await;
                            }
                        }
                        Err(message) => {
                            for topic_id in &pull_topic_ids {
                                tracker.mark_failed(topic_id).await;
                            }
                            let mut failed_topic_ids = pull_topic_ids;
                            failed_topic_ids.sort();
                            failed_topic_ids.truncate(8);
                            return Err(Phase3ProtocolError {
                                code: "PHASE3_PULL_FAILED".to_string(),
                                message,
                                failed_topic_ids,
                            });
                        }
                    }
                }

                for topic_id in &all_topic_ids {
                    tracker
                        .mark_completed(topic_id, logger, tx_internal, app_handle, false)
                        .await;
                }
                log::info!(
                    "[SyncService] Phase 3 batch done: push={} pull={}",
                    push_topic_ids.len(),
                    pull_batch.len()
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
                    "topics": next_batch,
                });
                let mut expected = expected_batch_topics.lock().await;
                *expected = msg["topics"]
                    .as_object()
                    .map(|topics| topics.keys().cloned().collect())
                    .unwrap_or_default();
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

    #[test]
    fn phase3_results_must_cover_the_exact_requested_topic_set() {
        let expected = HashSet::from(["topic-a".to_string(), "topic-b".to_string()]);
        let complete = serde_json::json!({
            "topic-a": { "ok": true, "toPush": false, "toPull": [] },
            "topic-b": { "ok": true, "toPush": false, "toPull": [] }
        });
        assert!(validate_phase3_result_topics(
            &expected,
            complete.as_object().expect("complete result map")
        )
        .is_ok());

        let incomplete = serde_json::json!({
            "topic-a": { "ok": true, "toPush": false, "toPull": [] }
        });
        let error = validate_phase3_result_topics(
            &expected,
            incomplete.as_object().expect("incomplete result map"),
        )
        .expect_err("missing topic must fail phase 3");
        assert!(error.contains("topic-b"));
    }

    #[test]
    fn phase3_decision_is_a_strict_discriminated_union() {
        assert_eq!(
            parse_topic_decision(
                "topic-a",
                &json!({ "ok": true, "toPull": ["message-a"], "toPush": false })
            )
            .expect("valid decision"),
            TopicDecision {
                to_pull: vec!["message-a".to_string()],
                to_push: false,
            }
        );

        for invalid in [
            json!({ "toPull": [], "toPush": false }),
            json!({ "ok": true, "toPull": "message-a", "toPush": false }),
            json!({ "ok": true, "toPull": [], "toPush": "false" }),
            json!({ "ok": true, "toPull": ["message-a", "message-a"], "toPush": false }),
            json!({ "ok": true, "toPull": [], "toPush": false, "error": { "code": "X", "message": "bad" } }),
            json!({ "ok": false, "error": { "code": "DESKTOP_DB", "message": "failed" } }),
            json!({ "ok": false, "toPull": [], "toPush": false, "error": { "code": "DESKTOP_DB", "message": "failed" } }),
        ] {
            assert!(parse_topic_decision("topic-a", &invalid).is_err());
        }

        let rejection = parse_topic_decision(
            "topic-a",
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
    fn raw_phase3_parser_rejects_duplicate_topic_keys() {
        let duplicate = r#"{
            "type":"SYNC_DIFF_RESULTS_BATCH",
            "results":{
                "topic-a":{"ok":true,"toPull":[],"toPush":false},
                "topic-a":{"ok":true,"toPull":[],"toPush":false}
            }
        }"#;
        let error = parse_phase3_batch_frame(duplicate)
            .expect_err("duplicate raw JSON topic keys must not be overwritten");
        assert_eq!(error.code, "PHASE3_FRAME_INVALID");
        assert!(error.message.contains("duplicate Phase 3 topic id topic-a"));
    }

    #[test]
    fn push_topic_failure_rejects_the_batch() {
        let expected = vec!["topic-a".to_string()];
        let error = validate_topic_batch_outcomes(
            "push",
            &expected,
            Ok(vec![TopicBatchOutcome {
                topic_id: "topic-a".to_string(),
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
        let expected = vec!["topic-a".to_string(), "topic-b".to_string()];
        let error = validate_topic_batch_outcomes(
            "pull",
            &expected,
            Ok(vec![TopicBatchOutcome {
                topic_id: "topic-a".to_string(),
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
        let expected = vec!["topic-a".to_string()];
        let duplicate = validate_topic_batch_outcomes(
            "push",
            &expected,
            Ok(vec![
                TopicBatchOutcome {
                    topic_id: "topic-a".to_string(),
                    success: true,
                    error: None,
                },
                TopicBatchOutcome {
                    topic_id: "topic-a".to_string(),
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
                topic_id: "topic-b".to_string(),
                success: true,
                error: None,
            }]),
        )
        .expect_err("unexpected response topic must fail");
        assert!(unexpected.contains("unexpected topic topic-b"));
    }

    #[test]
    fn batch_error_rejects_all_expected_topics() {
        let expected = vec!["topic-b".to_string(), "topic-a".to_string()];
        let error =
            validate_topic_batch_outcomes("push", &expected, Err("transport closed".to_string()))
                .expect_err("a batch-level error must fail phase 3");

        assert!(error.contains("topic-a"));
        assert!(error.contains("topic-b"));
        assert!(error.contains("transport closed"));
    }
}
