use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_executor::{BatchPullResult, PullExecutor, PushExecutor};
use crate::vcp_modules::sync_logger::{LogLevel, SyncLogger};
use crate::vcp_modules::sync_service::{
    emit_sync_log, Phase3Tracker, SyncCommand, SyncTaskTracker,
};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::Mutex;
use tauri::AppHandle;
use tokio::sync::mpsc;

pub struct BatchDiffHandler;

#[derive(Debug)]
struct TopicBatchOutcome {
    topic_id: String,
    success: bool,
    error: Option<String>,
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
    let outcomes_by_topic: HashMap<String, TopicBatchOutcome> = outcomes
        .into_iter()
        .map(|outcome| (outcome.topic_id.clone(), outcome))
        .collect();

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

fn fail_phase3_attempt(
    app_handle: &AppHandle,
    logger: &Arc<Mutex<SyncLogger>>,
    tx_internal: &mpsc::UnboundedSender<SyncCommand>,
    attempt_id: u64,
    message: String,
) {
    if let Ok(mut logger) = logger.lock() {
        logger.log(LogLevel::Error, "messages", &message);
    }
    emit_sync_log(app_handle, "error", &message);
    let _ = tx_internal.send(SyncCommand::FailAttempt {
        attempt_id,
        message,
    });
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
        task_tracker: &Arc<SyncTaskTracker>,
        expected_batch_topics: &Arc<tokio::sync::Mutex<HashSet<String>>>,
        attempt_id: u64,
    ) -> Result<(), String> {
        let results = payload["results"]
            .as_object()
            .ok_or_else(|| "Phase 3 response is missing results".to_string())?;
        {
            let expected = expected_batch_topics.lock().await;
            validate_phase3_result_topics(&expected, results)?;
        }

        {
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

                let uploaded_hashes = uploaded_hashes.clone();

                // 收集所有涉及的 topic ID（去重）
                let mut all_topic_ids: HashSet<String> = HashSet::new();
                for tid in &push_topic_ids {
                    all_topic_ids.insert(tid.clone());
                }
                for (tid, _) in &pull_batch {
                    all_topic_ids.insert(tid.clone());
                }

                task_tracker
                    .spawn(async move {
                        // 1. Push 批量（先执行，确保 push_pull 的 topic 推送完再拉取）
                        if has_push {
                            let push_result = PushExecutor::push_messages_batch(
                                &h_in,
                                &c_in,
                                &b_in,
                                &token,
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
                            let successful_pushes = match validate_topic_batch_outcomes(
                                "push",
                                &push_topic_ids,
                                push_result,
                            ) {
                                Ok(successful) => successful,
                                Err(message) => {
                                    fail_phase3_attempt(
                                        &h_in,
                                        &sync_logger_msg,
                                        &tx_internal_msg,
                                        attempt_id,
                                        message,
                                    );
                                    return;
                                }
                            };
                            for topic_id in successful_pushes {
                                tracker_clone.mark_modified(&topic_id).await;
                            }
                        }

                        // 2. Pull 批量（push 完成后再 pull，确保 push_pull 的 topic 数据已合并）
                        if has_pull {
                            let pull_topic_ids: Vec<String> = pull_batch
                                .iter()
                                .map(|(topic_id, _)| topic_id.clone())
                                .collect();
                            let pull_result = PullExecutor::pull_messages_batch(
                                &h_in,
                                &c_in,
                                &b_in,
                                &token,
                                &pull_batch,
                                &wq_in,
                                prerender_enabled,
                            )
                            .await
                            .map(|results| {
                                results
                                    .into_iter()
                                    .map(|result: BatchPullResult| TopicBatchOutcome {
                                        topic_id: result.topic_id,
                                        success: result.success,
                                        error: result.error,
                                    })
                                    .collect()
                            });
                            let successful_pulls = match validate_topic_batch_outcomes(
                                "pull",
                                &pull_topic_ids,
                                pull_result,
                            ) {
                                Ok(successful) => successful,
                                Err(message) => {
                                    fail_phase3_attempt(
                                        &h_in,
                                        &sync_logger_msg,
                                        &tx_internal_msg,
                                        attempt_id,
                                        message,
                                    );
                                    return;
                                }
                            };
                            for topic_id in successful_pulls {
                                tracker_clone.mark_modified(&topic_id).await;
                            }
                        }

                        // 3. 所有 topic 标记完成
                        for tid in &all_topic_ids {
                            tracker_clone
                                .mark_completed(
                                    tid,
                                    &sync_logger_msg,
                                    &tx_internal_msg,
                                    &h_in,
                                    false,
                                )
                                .await;
                        }

                        log::info!(
                            "[SyncService] Phase 3 batch done: push={} pull={}",
                            push_topic_ids.len(),
                            pull_batch.len()
                        );
                    })
                    .await;
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
            "topic-a": { "toPush": false, "toPull": [] },
            "topic-b": { "toPush": false, "toPull": [] }
        });
        assert!(validate_phase3_result_topics(
            &expected,
            complete.as_object().expect("complete result map")
        )
        .is_ok());

        let incomplete = serde_json::json!({
            "topic-a": { "toPush": false, "toPull": [] }
        });
        let error = validate_phase3_result_topics(
            &expected,
            incomplete.as_object().expect("incomplete result map"),
        )
        .expect_err("missing topic must fail phase 3");
        assert!(error.contains("topic-b"));
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
