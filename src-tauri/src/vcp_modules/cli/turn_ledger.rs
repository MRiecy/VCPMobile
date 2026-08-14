//! SQLite owner ledger for one bounded local CLI turn.

use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{Pool, Row, Sqlite};

use super::result::VcpCliResultEnvelope;
use super::turn_types::{
    DurableRiverProjection, DurableVrefProjection, FrozenModelRequest, LocalCliStepRecord,
    LocalCliTurnRecord, LocalCliTurnRoute, LocalCliTurnStart, LocalCliTurnState,
    MarkedHistoryEntry, MAX_ASSISTANT_STEP_BYTES, MAX_CONTINUATION_MESSAGES_BYTES,
    MAX_LOCAL_CLI_TOOL_STEPS, MAX_LOCAL_CLI_TURN_WALL_MS, MAX_MARKED_HISTORY_BYTES,
    MAX_RIVER_ARTIFACTS, MAX_RIVER_ARTIFACT_BYTES, MAX_RIVER_ARTIFACT_TOTAL_BYTES,
    MAX_RIVER_PROJECTION_BYTES, MAX_TOOL_PAYLOAD_BYTES,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolClaim {
    Claimed {
        operation_id: String,
    },
    Replay {
        operation_id: String,
        result: Box<VcpCliResultEnvelope>,
        local_payload: String,
        should_continue: bool,
    },
    InFlight {
        operation_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinalizerClaim {
    Claimed,
    ReplayTerminal {
        content: String,
        finish_reason: String,
    },
    InFlight,
}

pub async fn create_turn(
    pool: &Pool<Sqlite>,
    start: &LocalCliTurnStart,
    route: LocalCliTurnRoute,
    now_ms: u64,
) -> Result<LocalCliTurnRecord, String> {
    validate_owner(start)?;
    let turn_attempt = format!("turn-{}", uuid::Uuid::new_v4());
    let frozen_request = FrozenModelRequest {
        messages: start.messages.clone(),
        model_config: start.model_config.clone(),
        context: start.context.clone(),
    };
    let frozen_request_json = bounded_json(
        &frozen_request,
        MAX_CONTINUATION_MESSAGES_BYTES,
        "frozen model request",
    )?;
    let inserted = sqlx::query(
        "INSERT INTO local_cli_turn_ledger (
            turn_attempt, outer_message_id, topic_id, owner_id, owner_type,
            speaker_agent_id, route, state, step_index, tool_steps,
            started_at_ms, deadline_at_ms, updated_at_ms, version,
            frozen_request_json, continuation_messages_json, expected_calls, step_records_json,
            marked_history_json, final_content, terminal_reason
         )
         SELECT ?, ?, ?, ?, ?, ?, ?, 'claimed', 0, 0, ?, ?, ?, 0, ?, NULL, 0, '[]', '[]', NULL, NULL
         WHERE EXISTS (
            SELECT 1 FROM messages m
            JOIN topics t ON t.topic_id = m.topic_id
            WHERE m.topic_id = ? AND m.msg_id = ?
              AND m.deleted_at IS NULL AND t.deleted_at IS NULL
              AND t.owner_id = ? AND t.owner_type = ?
         )
         ON CONFLICT(topic_id, outer_message_id) DO NOTHING",
    )
    .bind(&turn_attempt)
    .bind(&start.outer_message_id)
    .bind(&start.topic_id)
    .bind(&start.owner_id)
    .bind(&start.owner_type)
    .bind(&start.speaker_agent_id)
    .bind(route.as_db_value())
    .bind(i64_from_u64(now_ms, "started_at_ms")?)
    .bind(i64_from_u64(
        now_ms.saturating_add(MAX_LOCAL_CLI_TURN_WALL_MS),
        "deadline_at_ms",
    )?)
    .bind(i64_from_u64(now_ms, "updated_at_ms")?)
    .bind(frozen_request_json)
    .bind(&start.topic_id)
    .bind(&start.outer_message_id)
    .bind(&start.owner_id)
    .bind(&start.owner_type)
    .execute(pool)
    .await
    .map_err(|error| format!("cannot create local CLI turn: {error}"))?;

    let record = load_live_turn(pool, &start.outer_message_id).await?;
    match (inserted.rows_affected(), record) {
        (1, Some(record)) => Ok(record),
        (0, Some(record)) if record.route == route => Ok(record),
        (0, Some(_)) => Err("outer message is already bound to a different CLI route".to_string()),
        _ => Err("cannot create local CLI turn for a deleted or foreign message".to_string()),
    }
}

pub async fn load_live_turn(
    pool: &Pool<Sqlite>,
    outer_message_id: &str,
) -> Result<Option<LocalCliTurnRecord>, String> {
    purge_tombstoned_turns(pool).await?;
    let row = sqlx::query(
        "SELECT l.* FROM local_cli_turn_ledger l
         JOIN messages m ON m.topic_id = l.topic_id AND m.msg_id = l.outer_message_id
         JOIN topics t ON t.topic_id = l.topic_id
         WHERE l.outer_message_id = ? AND m.deleted_at IS NULL AND t.deleted_at IS NULL",
    )
    .bind(outer_message_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| format!("cannot load local CLI turn: {error}"))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let record = decode_turn(row)?;
    reconcile_finalizing_record(pool, record).await.map(Some)
}

pub async fn purge_tombstoned_turns(pool: &Pool<Sqlite>) -> Result<u64, String> {
    sqlx::query(
        "DELETE FROM local_cli_turn_ledger
         WHERE NOT EXISTS (
            SELECT 1 FROM messages m
            JOIN topics t ON t.topic_id = m.topic_id
            WHERE m.topic_id = local_cli_turn_ledger.topic_id
              AND m.msg_id = local_cli_turn_ledger.outer_message_id
              AND m.deleted_at IS NULL AND t.deleted_at IS NULL
         )",
    )
    .execute(pool)
    .await
    .map(|result| result.rows_affected())
    .map_err(|error| format!("cannot purge tombstoned local CLI turns: {error}"))
}

pub async fn mark_model_running(
    pool: &Pool<Sqlite>,
    turn_attempt: &str,
    expected_step: u32,
    now_ms: u64,
) -> Result<LocalCliTurnRecord, String> {
    transition_step_state(
        pool,
        turn_attempt,
        expected_step,
        &[
            LocalCliTurnState::Claimed,
            LocalCliTurnState::ContinuationPending,
            LocalCliTurnState::Continued,
            LocalCliTurnState::Running,
        ],
        LocalCliTurnState::Running,
        now_ms,
    )
    .await
}

pub async fn mark_model_continued(
    pool: &Pool<Sqlite>,
    turn_attempt: &str,
    expected_step: u32,
    now_ms: u64,
) -> Result<LocalCliTurnRecord, String> {
    transition_step_state(
        pool,
        turn_attempt,
        expected_step,
        &[LocalCliTurnState::Running],
        LocalCliTurnState::Continued,
        now_ms,
    )
    .await
}

/// Atomically binds every call in one assistant response before any command may execute.
pub async fn claim_tool_batch(
    pool: &Pool<Sqlite>,
    turn_attempt: &str,
    model_step_index: u32,
    tool_digests: &[String],
    assistant_content: &str,
    now_ms: u64,
) -> Result<Vec<ToolClaim>, String> {
    if tool_digests.is_empty() {
        return Err("tool batch cannot be empty".to_string());
    }
    if assistant_content.len() > MAX_ASSISTANT_STEP_BYTES {
        return Err("assistant tool step exceeds its hard byte limit".to_string());
    }
    let expected_calls = u32::try_from(tool_digests.len())
        .map_err(|_| "tool batch exceeds u32 identity space".to_string())?;
    for digest in tool_digests {
        validate_digest(digest)?;
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|error| format!("cannot begin local CLI tool batch claim: {error}"))?;
    let row = sqlx::query("SELECT * FROM local_cli_turn_ledger WHERE turn_attempt = ?")
        .bind(turn_attempt)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| format!("cannot read local CLI tool batch owner: {error}"))?
        .ok_or_else(|| "local CLI turn is missing".to_string())?;
    let mut record = decode_turn(row)?;
    require_live_step(&record, model_step_index, now_ms)?;
    if record.expected_calls != 0 && record.expected_calls != expected_calls {
        return Err("assistant tool batch size changed after durable claim".to_string());
    }

    let mut claims = Vec::with_capacity(tool_digests.len());
    let mut new_claims = 0_u32;
    for (index, digest) in tool_digests.iter().enumerate() {
        let call_index =
            u32::try_from(index).map_err(|_| "tool batch call index exceeds u32".to_string())?;
        if let Some(existing) = record
            .step_records
            .iter()
            .find(|step| step.model_step_index == model_step_index && step.call_index == call_index)
        {
            if existing.tool_digest != *digest || existing.assistant_content != assistant_content {
                return Err("durable assistant tool batch conflicts with replay".to_string());
            }
            claims.push(match (&existing.result, &existing.local_payload) {
                (Some(result), Some(local_payload)) => ToolClaim::Replay {
                    operation_id: existing.operation_id.clone(),
                    result: Box::new(result.clone()),
                    local_payload: local_payload.clone(),
                    should_continue: existing.should_continue,
                },
                _ => ToolClaim::InFlight {
                    operation_id: existing.operation_id.clone(),
                },
            });
            continue;
        }
        new_claims = new_claims.saturating_add(1);
        let operation_id = operation_id(turn_attempt, model_step_index, call_index, digest);
        record.step_records.push(LocalCliStepRecord {
            model_step_index,
            call_index,
            tool_digest: digest.clone(),
            operation_id: operation_id.clone(),
            assistant_content: assistant_content.to_string(),
            river_projection: None,
            vref_projection: None,
            local_payload: None,
            result: None,
            mark_history: false,
            should_continue: true,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        });
        claims.push(ToolClaim::Claimed { operation_id });
    }
    if record.tool_steps.saturating_add(new_claims) > MAX_LOCAL_CLI_TOOL_STEPS {
        return Err("local CLI turn reached its tool step limit".to_string());
    }
    record.expected_calls = expected_calls;
    record.tool_steps = record.tool_steps.saturating_add(new_claims);
    let completed = record
        .step_records
        .iter()
        .filter(|step| step.model_step_index == model_step_index && step.result.is_some())
        .count();
    record.state = if completed == expected_calls as usize {
        LocalCliTurnState::ResultReady
    } else {
        LocalCliTurnState::Claimed
    };
    update_record_in_transaction(&mut tx, &record, now_ms).await?;
    tx.commit()
        .await
        .map_err(|error| format!("cannot commit local CLI tool batch claim: {error}"))?;
    Ok(claims)
}

/// Freezes the exact River bytes before Runtime may observe this operation. A retry must reuse
/// these bytes even when semantic availability or attachment availability has changed.
pub async fn bind_tool_projection(
    pool: &Pool<Sqlite>,
    turn_attempt: &str,
    operation_id: &str,
    projection: &DurableRiverProjection,
    now_ms: u64,
) -> Result<DurableRiverProjection, String> {
    validate_durable_projection(projection)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|error| format!("cannot begin local CLI projection bind: {error}"))?;
    let row = sqlx::query("SELECT * FROM local_cli_turn_ledger WHERE turn_attempt = ?")
        .bind(turn_attempt)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| format!("cannot read local CLI projection owner: {error}"))?
        .ok_or_else(|| "local CLI turn is missing".to_string())?;
    let mut record = decode_turn(row)?;
    let step = record
        .step_records
        .iter_mut()
        .find(|step| step.operation_id == operation_id)
        .ok_or_else(|| "local CLI projection has no durable tool claim".to_string())?;
    let frozen = match &step.river_projection {
        Some(existing) if existing == projection => existing.clone(),
        Some(_) => return Err("local CLI projection conflicts with durable replay".to_string()),
        None if step.result.is_none() => {
            step.river_projection = Some(projection.clone());
            step.updated_at_ms = now_ms;
            projection.clone()
        }
        None => return Err("completed local CLI operation has no durable projection".to_string()),
    };
    update_record_in_transaction(&mut tx, &record, now_ms).await?;
    tx.commit()
        .await
        .map_err(|error| format!("cannot commit local CLI projection bind: {error}"))?;
    Ok(frozen)
}

/// Freezes the vref selection and its source holds in the same SQLite transaction. Recovery must
/// reuse these bytes and operation identity rather than performing another semantic search.
pub async fn bind_tool_vref_projection(
    pool: &Pool<Sqlite>,
    turn_attempt: &str,
    operation_id: &str,
    projection: &DurableVrefProjection,
    now_ms: u64,
) -> Result<DurableVrefProjection, String> {
    super::knowledge_projection::validate_durable_vref_projection(projection)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|error| format!("cannot begin local CLI vref bind: {error}"))?;
    let row = sqlx::query("SELECT * FROM local_cli_turn_ledger WHERE turn_attempt = ?")
        .bind(turn_attempt)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| format!("cannot read local CLI vref owner: {error}"))?
        .ok_or_else(|| "local CLI turn is missing".to_string())?;
    let mut record = decode_turn(row)?;
    let step = record
        .step_records
        .iter_mut()
        .find(|step| step.operation_id == operation_id)
        .ok_or_else(|| "local CLI vref has no durable tool claim".to_string())?;
    let frozen = match &step.vref_projection {
        Some(existing) if existing == projection => existing.clone(),
        Some(_) => return Err("local CLI vref conflicts with durable replay".to_string()),
        None if step.result.is_none() => {
            for source in &projection.sources {
                let inserted = sqlx::query(
                    "INSERT INTO local_knowledge_attempt_holds(
                       turn_attempt, operation_id, source_id, source_sha256, created_at_ms
                     )
                     SELECT ?, ?, source_id, source_sha256, ?
                     FROM local_knowledge_sources
                     WHERE source_id = ? AND source_sha256 = ? AND revoked_at_ms IS NULL
                     ON CONFLICT(turn_attempt, operation_id, source_id) DO NOTHING",
                )
                .bind(turn_attempt)
                .bind(operation_id)
                .bind(i64_from_u64(now_ms, "vref_hold_created_at")?)
                .bind(&source.source_id)
                .bind(&source.source_sha256)
                .execute(&mut *tx)
                .await
                .map_err(|error| format!("cannot persist vref source hold: {error}"))?;
                if inserted.rows_affected() == 0 {
                    let exists: i64 = sqlx::query_scalar(
                        "SELECT EXISTS(
                           SELECT 1 FROM local_knowledge_attempt_holds
                           WHERE turn_attempt = ? AND operation_id = ? AND source_id = ?
                             AND source_sha256 = ?
                         )",
                    )
                    .bind(turn_attempt)
                    .bind(operation_id)
                    .bind(&source.source_id)
                    .bind(&source.source_sha256)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|error| format!("cannot verify durable vref hold: {error}"))?;
                    if exists == 0 {
                        return Err("vref grant changed before durable bind".to_string());
                    }
                }
            }
            step.vref_projection = Some(projection.clone());
            step.updated_at_ms = now_ms;
            projection.clone()
        }
        None => return Err("completed local CLI operation has no durable vref".to_string()),
    };
    update_record_in_transaction(&mut tx, &record, now_ms).await?;
    tx.commit()
        .await
        .map_err(|error| format!("cannot commit local CLI vref bind: {error}"))?;
    Ok(frozen)
}

pub async fn release_tool_vref_holds(
    pool: &Pool<Sqlite>,
    turn_attempt: &str,
    operation_id: &str,
) -> Result<(), String> {
    sqlx::query(
        "DELETE FROM local_knowledge_attempt_holds
         WHERE turn_attempt = ? AND operation_id = ?",
    )
    .bind(turn_attempt)
    .bind(operation_id)
    .execute(pool)
    .await
    .map(|_| ())
    .map_err(|error| format!("cannot release durable vref holds: {error}"))
}

#[allow(clippy::too_many_arguments)]
pub async fn store_tool_result(
    pool: &Pool<Sqlite>,
    turn_attempt: &str,
    operation_id: &str,
    result: &VcpCliResultEnvelope,
    local_payload: &str,
    mark_history: bool,
    should_continue: bool,
    marked_block: Option<&str>,
    now_ms: u64,
) -> Result<LocalCliTurnRecord, String> {
    if local_payload.len() > MAX_TOOL_PAYLOAD_BYTES {
        return Err("local CLI tool payload exceeds its hard byte limit".to_string());
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|error| format!("cannot begin local CLI result write: {error}"))?;
    let row = sqlx::query("SELECT * FROM local_cli_turn_ledger WHERE turn_attempt = ?")
        .bind(turn_attempt)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| format!("cannot read local CLI result owner: {error}"))?
        .ok_or_else(|| "local CLI turn is missing".to_string())?;
    let mut record = decode_turn(row)?;
    let step = record
        .step_records
        .iter_mut()
        .find(|step| step.operation_id == operation_id)
        .ok_or_else(|| "local CLI result has no durable claim".to_string())?;
    if let Some(existing) = &step.result {
        if existing != result || step.local_payload.as_deref() != Some(local_payload) {
            return Err("local CLI result replay conflicts with durable result".to_string());
        }
    } else {
        step.result = Some(result.clone());
        step.local_payload = Some(local_payload.to_string());
        step.mark_history = mark_history;
        step.should_continue = should_continue;
        step.updated_at_ms = now_ms;
    }
    if let Some(block) = marked_block {
        if block.len() > MAX_MARKED_HISTORY_BYTES {
            return Err("marked CLI history block exceeds its hard byte limit".to_string());
        }
        if !record
            .marked_history
            .iter()
            .any(|item| item.operation_id == step.operation_id)
        {
            let candidate = MarkedHistoryEntry {
                model_step_index: step.model_step_index,
                operation_id: step.operation_id.clone(),
                block: block.to_string(),
            };
            let mut projected = record.marked_history.clone();
            projected.push(candidate.clone());
            bounded_json(&projected, MAX_MARKED_HISTORY_BYTES, "marked CLI history")?;
            record.marked_history.push(candidate);
        }
    }
    let completed_for_step = record
        .step_records
        .iter()
        .filter(|step| step.model_step_index == record.step_index && step.result.is_some())
        .count();
    record.state =
        if record.expected_calls > 0 && completed_for_step == record.expected_calls as usize {
            LocalCliTurnState::ResultReady
        } else {
            LocalCliTurnState::Claimed
        };
    update_record_in_transaction(&mut tx, &record, now_ms).await?;
    tx.commit()
        .await
        .map_err(|error| format!("cannot commit local CLI result: {error}"))?;
    load_turn_by_attempt(pool, turn_attempt)
        .await?
        .ok_or_else(|| "local CLI turn disappeared after result commit".to_string())
}

pub async fn store_pending_continuation(
    pool: &Pool<Sqlite>,
    turn_attempt: &str,
    current_step: u32,
    continuation_messages: &[Value],
    now_ms: u64,
) -> Result<LocalCliTurnRecord, String> {
    let messages_json = bounded_json(
        continuation_messages,
        MAX_CONTINUATION_MESSAGES_BYTES,
        "continuation messages",
    )?;
    let next_step = current_step
        .checked_add(1)
        .ok_or_else(|| "local CLI model step overflow".to_string())?;
    let result = sqlx::query(
        "UPDATE local_cli_turn_ledger
         SET state = 'continuation_pending', step_index = ?, continuation_messages_json = ?,
             expected_calls = 0,
             updated_at_ms = ?, version = version + 1
         WHERE turn_attempt = ? AND step_index = ?
           AND state IN ('result_ready', 'continuation_pending')",
    )
    .bind(i64::from(next_step))
    .bind(messages_json)
    .bind(i64_from_u64(now_ms, "updated_at_ms")?)
    .bind(turn_attempt)
    .bind(i64::from(current_step))
    .execute(pool)
    .await
    .map_err(|error| format!("cannot persist local CLI continuation: {error}"))?;
    if result.rows_affected() != 1 {
        let current = load_turn_by_attempt(pool, turn_attempt)
            .await?
            .ok_or_else(|| "local CLI turn is missing".to_string())?;
        if current.state == LocalCliTurnState::ContinuationPending
            && current.step_index == next_step
            && current.continuation_messages.as_deref() == Some(continuation_messages)
        {
            return Ok(current);
        }
        return Err("local CLI continuation owner/state changed".to_string());
    }
    load_turn_by_attempt(pool, turn_attempt)
        .await?
        .ok_or_else(|| "local CLI turn disappeared after continuation commit".to_string())
}

pub async fn store_model_retry_pending(
    pool: &Pool<Sqlite>,
    turn_attempt: &str,
    current_step: u32,
    request_messages: &[Value],
    reason: &str,
    now_ms: u64,
) -> Result<LocalCliTurnRecord, String> {
    let messages_json = bounded_json(
        request_messages,
        MAX_CONTINUATION_MESSAGES_BYTES,
        "model retry messages",
    )?;
    let result = sqlx::query(
        "UPDATE local_cli_turn_ledger
         SET state='continuation_pending', continuation_messages_json=?, terminal_reason=?,
             updated_at_ms=?, version=version+1
         WHERE turn_attempt=? AND step_index=?
           AND state IN ('claimed','running','continued','continuation_pending')",
    )
    .bind(messages_json)
    .bind(reason)
    .bind(i64_from_u64(now_ms, "updated_at_ms")?)
    .bind(turn_attempt)
    .bind(i64::from(current_step))
    .execute(pool)
    .await
    .map_err(|error| format!("cannot persist model retry continuation: {error}"))?;
    if result.rows_affected() != 1 {
        return Err("local CLI model retry owner/state changed".to_string());
    }
    load_turn_by_attempt(pool, turn_attempt)
        .await?
        .ok_or_else(|| "local CLI turn disappeared after retry persistence".to_string())
}

pub async fn claim_finalizer(
    pool: &Pool<Sqlite>,
    turn_attempt: &str,
    content: &str,
    finish_reason: &str,
    now_ms: u64,
) -> Result<FinalizerClaim, String> {
    if content.len() > MAX_ASSISTANT_STEP_BYTES + MAX_MARKED_HISTORY_BYTES {
        return Err("final local CLI message exceeds its hard byte limit".to_string());
    }
    let record = load_turn_by_attempt(pool, turn_attempt)
        .await?
        .ok_or_else(|| "local CLI turn is missing".to_string())?;
    let record = reconcile_finalizing_record(pool, record).await?;
    if record.state == LocalCliTurnState::Terminal {
        return Ok(FinalizerClaim::ReplayTerminal {
            content: record.final_content.unwrap_or_default(),
            finish_reason: record
                .terminal_reason
                .unwrap_or_else(|| "completed".to_string()),
        });
    }
    if record.state == LocalCliTurnState::Finalizing {
        if record.final_content.as_deref() == Some(content)
            && record.terminal_reason.as_deref() == Some(finish_reason)
        {
            // The outer ActiveRequestLease serializes the current owner. Returning Claimed here
            // lets recovery finish a crash between the ledger claim and message commit; the
            // message finalizer itself is CAS-protected by active_generations.
            return Ok(FinalizerClaim::Claimed);
        }
        return Err("local CLI finalizer conflicts with the durable claim".to_string());
    }
    let result = sqlx::query(
        "UPDATE local_cli_turn_ledger
         SET state = 'finalizing', final_content = ?, terminal_reason = ?,
             updated_at_ms = ?, version = version + 1
         WHERE turn_attempt = ? AND version = ? AND state != 'terminal'",
    )
    .bind(content)
    .bind(finish_reason)
    .bind(i64_from_u64(now_ms, "updated_at_ms")?)
    .bind(turn_attempt)
    .bind(i64_from_u64(record.version, "version")?)
    .execute(pool)
    .await
    .map_err(|error| format!("cannot claim local CLI finalizer: {error}"))?;
    if result.rows_affected() == 1 {
        Ok(FinalizerClaim::Claimed)
    } else {
        Ok(FinalizerClaim::InFlight)
    }
}

pub async fn mark_terminal(
    pool: &Pool<Sqlite>,
    turn_attempt: &str,
    now_ms: u64,
) -> Result<(), String> {
    let result = sqlx::query(
        "UPDATE local_cli_turn_ledger
         SET state = 'terminal', continuation_messages_json = NULL,
             updated_at_ms = ?, version = version + 1
         WHERE turn_attempt = ? AND state = 'finalizing'",
    )
    .bind(i64_from_u64(now_ms, "updated_at_ms")?)
    .bind(turn_attempt)
    .execute(pool)
    .await
    .map_err(|error| format!("cannot commit local CLI terminal state: {error}"))?;
    if result.rows_affected() == 1 {
        return Ok(());
    }
    let current = load_turn_by_attempt(pool, turn_attempt).await?;
    if current.is_some_and(|record| record.state == LocalCliTurnState::Terminal) {
        Ok(())
    } else {
        Err("local CLI finalizer claim is not active".to_string())
    }
}

pub async fn mark_interrupted(
    pool: &Pool<Sqlite>,
    turn_attempt: &str,
    reason: &str,
    now_ms: u64,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE local_cli_turn_ledger
         SET state = 'interrupted', terminal_reason = ?, updated_at_ms = ?, version = version + 1
         WHERE turn_attempt = ? AND state NOT IN ('terminal', 'finalizing')",
    )
    .bind(reason)
    .bind(i64_from_u64(now_ms, "updated_at_ms")?)
    .bind(turn_attempt)
    .execute(pool)
    .await
    .map(|_| ())
    .map_err(|error| format!("cannot interrupt local CLI turn: {error}"))
}

async fn load_turn_by_attempt(
    pool: &Pool<Sqlite>,
    turn_attempt: &str,
) -> Result<Option<LocalCliTurnRecord>, String> {
    sqlx::query("SELECT * FROM local_cli_turn_ledger WHERE turn_attempt = ?")
        .bind(turn_attempt)
        .fetch_optional(pool)
        .await
        .map_err(|error| format!("cannot load local CLI turn attempt: {error}"))?
        .map(decode_turn)
        .transpose()
}

async fn reconcile_finalizing_record(
    pool: &Pool<Sqlite>,
    mut record: LocalCliTurnRecord,
) -> Result<LocalCliTurnRecord, String> {
    if record.state != LocalCliTurnState::Finalizing {
        return Ok(record);
    }
    let facts: Option<(String, Option<String>, i64)> = sqlx::query_as(
        "SELECT m.content, m.finish_reason,
                EXISTS(SELECT 1 FROM active_generations ag
                       WHERE ag.msg_id = m.msg_id AND ag.topic_id = m.topic_id)
         FROM messages m WHERE m.topic_id = ? AND m.msg_id = ? AND m.deleted_at IS NULL",
    )
    .bind(&record.topic_id)
    .bind(&record.outer_message_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| format!("cannot reconcile local CLI finalizer: {error}"))?;
    let Some((content, Some(finish_reason), has_active_generation)) = facts else {
        return Ok(record);
    };
    if has_active_generation != 0 {
        return Ok(record);
    }
    sqlx::query(
        "UPDATE local_cli_turn_ledger
         SET state='terminal', final_content=?, terminal_reason=?,
             continuation_messages_json=NULL, updated_at_ms=?, version=version+1
         WHERE turn_attempt=? AND state='finalizing'",
    )
    .bind(&content)
    .bind(&finish_reason)
    .bind(i64_from_u64(
        record.updated_at_ms.max(record.started_at_ms),
        "updated_at_ms",
    )?)
    .bind(&record.turn_attempt)
    .execute(pool)
    .await
    .map_err(|error| format!("cannot reconcile committed local CLI finalizer: {error}"))?;
    record.state = LocalCliTurnState::Terminal;
    record.final_content = Some(content);
    record.terminal_reason = Some(finish_reason);
    record.continuation_messages = None;
    record.version = record.version.saturating_add(1);
    Ok(record)
}

async fn transition_step_state(
    pool: &Pool<Sqlite>,
    turn_attempt: &str,
    expected_step: u32,
    allowed: &[LocalCliTurnState],
    target: LocalCliTurnState,
    now_ms: u64,
) -> Result<LocalCliTurnRecord, String> {
    let current = load_turn_by_attempt(pool, turn_attempt)
        .await?
        .ok_or_else(|| "local CLI turn is missing".to_string())?;
    require_live_step(&current, expected_step, now_ms)?;
    if !allowed.contains(&current.state) {
        return Err(format!(
            "local CLI turn cannot transition from {} to {}",
            current.state.as_db_value(),
            target.as_db_value()
        ));
    }
    let result = sqlx::query(
        "UPDATE local_cli_turn_ledger
         SET state = ?, updated_at_ms = ?, version = version + 1
         WHERE turn_attempt = ? AND version = ? AND step_index = ?",
    )
    .bind(target.as_db_value())
    .bind(i64_from_u64(now_ms, "updated_at_ms")?)
    .bind(turn_attempt)
    .bind(i64_from_u64(current.version, "version")?)
    .bind(i64::from(expected_step))
    .execute(pool)
    .await
    .map_err(|error| format!("cannot transition local CLI turn: {error}"))?;
    if result.rows_affected() != 1 {
        return Err("local CLI turn owner/version changed".to_string());
    }
    load_turn_by_attempt(pool, turn_attempt)
        .await?
        .ok_or_else(|| "local CLI turn disappeared after transition".to_string())
}

async fn update_record_in_transaction(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    record: &LocalCliTurnRecord,
    now_ms: u64,
) -> Result<(), String> {
    let step_records_json = bounded_json(
        &record.step_records,
        MAX_CONTINUATION_MESSAGES_BYTES,
        "local CLI step records",
    )?;
    let marked_history_json = bounded_json(
        &record.marked_history,
        MAX_MARKED_HISTORY_BYTES,
        "marked CLI history",
    )?;
    let result = sqlx::query(
        "UPDATE local_cli_turn_ledger
         SET state = ?, tool_steps = ?, expected_calls = ?, step_records_json = ?, marked_history_json = ?,
             updated_at_ms = ?, version = version + 1
         WHERE turn_attempt = ? AND version = ?",
    )
    .bind(record.state.as_db_value())
    .bind(i64::from(record.tool_steps))
    .bind(i64::from(record.expected_calls))
    .bind(step_records_json)
    .bind(marked_history_json)
    .bind(i64_from_u64(now_ms, "updated_at_ms")?)
    .bind(&record.turn_attempt)
    .bind(i64_from_u64(record.version, "version")?)
    .execute(&mut **tx)
    .await
    .map_err(|error| format!("cannot update local CLI turn record: {error}"))?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err("local CLI turn owner/version changed".to_string())
    }
}

fn decode_turn(row: sqlx::sqlite::SqliteRow) -> Result<LocalCliTurnRecord, String> {
    let route = match row
        .try_get::<String, _>("route")
        .map_err(|error| error.to_string())?
        .as_str()
    {
        "local_loopback" => LocalCliTurnRoute::LocalLoopback,
        "vcp_plugin" => LocalCliTurnRoute::VcpPlugin,
        value => return Err(format!("unknown local CLI route: {value}")),
    };
    let state = LocalCliTurnState::from_db_value(
        &row.try_get::<String, _>("state")
            .map_err(|error| error.to_string())?,
    )?;
    let frozen_request_json: String = row
        .try_get("frozen_request_json")
        .map_err(|error| error.to_string())?;
    let continuation_messages_json: Option<String> = row
        .try_get("continuation_messages_json")
        .map_err(|error| error.to_string())?;
    let step_records_json: String = row
        .try_get("step_records_json")
        .map_err(|error| error.to_string())?;
    let marked_history_json: String = row
        .try_get("marked_history_json")
        .map_err(|error| error.to_string())?;
    Ok(LocalCliTurnRecord {
        turn_attempt: row
            .try_get("turn_attempt")
            .map_err(|error| error.to_string())?,
        outer_message_id: row
            .try_get("outer_message_id")
            .map_err(|error| error.to_string())?,
        topic_id: row.try_get("topic_id").map_err(|error| error.to_string())?,
        owner_id: row.try_get("owner_id").map_err(|error| error.to_string())?,
        owner_type: row
            .try_get("owner_type")
            .map_err(|error| error.to_string())?,
        speaker_agent_id: row
            .try_get("speaker_agent_id")
            .map_err(|error| error.to_string())?,
        route,
        state,
        step_index: u32_from_i64(
            row.try_get("step_index")
                .map_err(|error| error.to_string())?,
            "step_index",
        )?,
        tool_steps: u32_from_i64(
            row.try_get("tool_steps")
                .map_err(|error| error.to_string())?,
            "tool_steps",
        )?,
        started_at_ms: u64_from_i64(
            row.try_get("started_at_ms")
                .map_err(|error| error.to_string())?,
            "started_at_ms",
        )?,
        deadline_at_ms: u64_from_i64(
            row.try_get("deadline_at_ms")
                .map_err(|error| error.to_string())?,
            "deadline_at_ms",
        )?,
        updated_at_ms: u64_from_i64(
            row.try_get("updated_at_ms")
                .map_err(|error| error.to_string())?,
            "updated_at_ms",
        )?,
        version: u64_from_i64(
            row.try_get("version").map_err(|error| error.to_string())?,
            "version",
        )?,
        frozen_request: serde_json::from_str(&frozen_request_json)
            .map_err(|error| format!("invalid frozen local CLI request: {error}"))?,
        continuation_messages: continuation_messages_json
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(|error| format!("invalid local CLI continuation: {error}"))?,
        expected_calls: u32_from_i64(
            row.try_get("expected_calls")
                .map_err(|error| error.to_string())?,
            "expected_calls",
        )?,
        step_records: serde_json::from_str(&step_records_json)
            .map_err(|error| format!("invalid local CLI step records: {error}"))?,
        marked_history: serde_json::from_str(&marked_history_json)
            .map_err(|error| format!("invalid marked CLI history: {error}"))?,
        final_content: row
            .try_get("final_content")
            .map_err(|error| error.to_string())?,
        terminal_reason: row
            .try_get("terminal_reason")
            .map_err(|error| error.to_string())?,
    })
}

fn validate_owner(start: &LocalCliTurnStart) -> Result<(), String> {
    if start.outer_message_id.is_empty()
        || start.topic_id.is_empty()
        || start.owner_id.is_empty()
        || !matches!(start.owner_type.as_str(), "agent" | "group")
    {
        return Err("local CLI turn requires a valid live message owner".to_string());
    }
    Ok(())
}

fn require_live_step(
    record: &LocalCliTurnRecord,
    expected_step: u32,
    now_ms: u64,
) -> Result<(), String> {
    if record.step_index != expected_step {
        return Err("local CLI model step owner changed".to_string());
    }
    if matches!(
        record.state,
        LocalCliTurnState::Terminal
            | LocalCliTurnState::Interrupted
            | LocalCliTurnState::Finalizing
    ) {
        return Err("local CLI turn is terminal".to_string());
    }
    if now_ms > record.deadline_at_ms {
        return Err("local CLI turn exceeded its wall-clock budget".to_string());
    }
    Ok(())
}

fn validate_digest(value: &str) -> Result<(), String> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("tool digest must be a SHA-256 hex string".to_string())
    }
}

fn validate_durable_projection(projection: &DurableRiverProjection) -> Result<(), String> {
    if projection.canonical_json.len() > MAX_RIVER_PROJECTION_BYTES
        || projection.size_bytes != projection.canonical_json.len() as u64
    {
        return Err(
            "durable River projection exceeds or disagrees with its byte fence".to_string(),
        );
    }
    let actual = format!("{:x}", Sha256::digest(projection.canonical_json.as_bytes()));
    if projection.sha256 != actual {
        return Err("durable River projection SHA-256 mismatch".to_string());
    }
    if projection.artifacts.len() > MAX_RIVER_ARTIFACTS {
        return Err("durable River projection has too many artifacts".to_string());
    }
    let document: Value = serde_json::from_str(&projection.canonical_json)
        .map_err(|error| format!("durable River projection is invalid JSON: {error}"))?;
    if document.get("schema").and_then(Value::as_str) != Some("vcp.mobile.attempt-projection.v1") {
        return Err("durable River projection schema is unsupported".to_string());
    }
    let descriptors = document
        .get("artifacts")
        .and_then(Value::as_array)
        .ok_or_else(|| "durable River projection has no artifact array".to_string())?;
    if descriptors.len() != projection.artifacts.len() {
        return Err("durable River artifact identities disagree with canonical JSON".to_string());
    }
    let mut total = 0_u64;
    for (descriptor, artifact) in descriptors.iter().zip(&projection.artifacts) {
        if artifact.file_name.is_empty()
            || artifact.file_name.len() > 160
            || artifact.file_name.contains(['/', '\\'])
            || matches!(artifact.file_name.as_str(), "." | "..")
            || artifact.guest_path != format!("/run/{}", artifact.file_name)
            || artifact.size_bytes > MAX_RIVER_ARTIFACT_BYTES
            || artifact.sha256.len() != 64
            || !artifact
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
            || descriptor.get("guest_path").and_then(Value::as_str)
                != Some(artifact.guest_path.as_str())
            || descriptor.get("size_bytes").and_then(Value::as_u64) != Some(artifact.size_bytes)
            || descriptor.get("sha256").and_then(Value::as_str) != Some(artifact.sha256.as_str())
        {
            return Err("durable River artifact identity is invalid".to_string());
        }
        total = total
            .checked_add(artifact.size_bytes)
            .ok_or_else(|| "durable River artifact budget overflowed".to_string())?;
    }
    if total > MAX_RIVER_ARTIFACT_TOTAL_BYTES {
        return Err("durable River artifacts exceed the attempt byte budget".to_string());
    }
    Ok(())
}

fn operation_id(turn_attempt: &str, step: u32, call: u32, digest: &str) -> String {
    format!("local:{turn_attempt}:{step}:{call}:{}", &digest[..16])
}

fn bounded_json<T: serde::Serialize + ?Sized>(
    value: &T,
    max_bytes: usize,
    label: &str,
) -> Result<String, String> {
    let json = serde_json::to_string(value)
        .map_err(|error| format!("cannot serialize {label}: {error}"))?;
    if json.len() > max_bytes {
        return Err(format!("{label} exceeds its hard byte limit"));
    }
    Ok(json)
}

fn i64_from_u64(value: u64, field: &str) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| format!("{field} exceeds SQLite integer range"))
}

fn u64_from_i64(value: i64, field: &str) -> Result<u64, String> {
    u64::try_from(value).map_err(|_| format!("{field} is negative"))
}

fn u32_from_i64(value: i64, field: &str) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| format!("{field} is outside u32 range"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;
    use crate::vcp_modules::cli::result::{VcpCliResultBody, VcpCliRuntimeInfo};

    async fn test_pool() -> Pool<Sqlite> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open sqlite");
        sqlx::raw_sql(
            "CREATE TABLE topics (
                topic_id TEXT PRIMARY KEY, owner_type TEXT NOT NULL, owner_id TEXT NOT NULL,
                deleted_at INTEGER
             );
             CREATE TABLE messages (
                msg_id TEXT NOT NULL, topic_id TEXT NOT NULL, content TEXT NOT NULL DEFAULT '',
                finish_reason TEXT, deleted_at INTEGER,
                PRIMARY KEY(topic_id, msg_id)
             );
             CREATE TABLE active_generations (
                msg_id TEXT PRIMARY KEY, topic_id TEXT NOT NULL
             );",
        )
        .execute(&pool)
        .await
        .expect("create base tables");
        sqlx::raw_sql(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/migrations/0007_create_local_cli_turn_ledger.sql"
        )))
        .execute(&pool)
        .await
        .expect("apply local turn migration");
        sqlx::query("INSERT INTO topics VALUES ('topic', 'agent', 'agent', NULL)")
            .execute(&pool)
            .await
            .expect("insert topic");
        sqlx::query("INSERT INTO messages(msg_id, topic_id) VALUES ('message', 'topic')")
            .execute(&pool)
            .await
            .expect("insert message");
        sqlx::query("INSERT INTO active_generations VALUES ('message', 'topic')")
            .execute(&pool)
            .await
            .expect("insert active generation");
        pool
    }

    fn start() -> LocalCliTurnStart {
        LocalCliTurnStart {
            outer_message_id: "message".to_string(),
            topic_id: "topic".to_string(),
            owner_id: "agent".to_string(),
            owner_type: "agent".to_string(),
            speaker_agent_id: Some("agent".to_string()),
            messages: vec![json!({"role":"user","content":"hello"})],
            model_config: json!({"model":"test"}),
            context: Some(json!({"topicId":"topic"})),
            vcp_url: "https://not-persisted.invalid".to_string(),
            vcp_api_key: "secret-not-persisted".to_string(),
        }
    }

    #[tokio::test]
    async fn closed_batch_claims_all_calls_before_any_result_and_replays_policy() {
        let pool = test_pool().await;
        let turn = create_turn(&pool, &start(), LocalCliTurnRoute::LocalLoopback, 100)
            .await
            .expect("create turn");
        mark_model_running(&pool, &turn.turn_attempt, 0, 101)
            .await
            .expect("mark running");
        let digests = [b"first".as_slice(), b"second".as_slice()]
            .into_iter()
            .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
            .collect::<Vec<_>>();
        let claims = claim_tool_batch(
            &pool,
            &turn.turn_attempt,
            0,
            &digests,
            "assistant closed batch",
            102,
        )
        .await
        .expect("claim complete batch");
        assert_eq!(claims.len(), 2);
        let durable = load_live_turn(&pool, "message")
            .await
            .expect("load batch")
            .expect("turn");
        assert_eq!(durable.expected_calls, 2);
        assert_eq!(durable.step_records.len(), 2);
        assert!(durable
            .step_records
            .iter()
            .all(|step| step.result.is_none()));

        let operation_ids = claims
            .into_iter()
            .map(|claim| match claim {
                ToolClaim::Claimed { operation_id } => operation_id,
                _ => panic!("new closed batch must claim every call"),
            })
            .collect::<Vec<_>>();
        let result = VcpCliResultEnvelope::success(VcpCliResultBody {
            content: vec![],
            job: None,
            jobs: None,
            skill: None,
            skills: None,
            runtime: Some(VcpCliRuntimeInfo::local_loopback()),
        });
        let after_first = store_tool_result(
            &pool,
            &turn.turn_attempt,
            &operation_ids[0],
            &result,
            "<!-- VCP_TOOL_PAYLOAD -->\n[]",
            false,
            false,
            None,
            103,
        )
        .await
        .expect("store first result");
        assert_eq!(after_first.state, LocalCliTurnState::Claimed);
        let after_second = store_tool_result(
            &pool,
            &turn.turn_attempt,
            &operation_ids[1],
            &result,
            "<!-- VCP_TOOL_PAYLOAD -->\n[]",
            false,
            true,
            None,
            104,
        )
        .await
        .expect("store second result");
        assert_eq!(after_second.state, LocalCliTurnState::ResultReady);

        let replay = claim_tool_batch(
            &pool,
            &turn.turn_attempt,
            0,
            &digests,
            "assistant closed batch",
            105,
        )
        .await
        .expect("replay closed batch");
        assert!(matches!(
            &replay[0],
            ToolClaim::Replay {
                should_continue: false,
                operation_id,
                ..
            } if operation_id == &operation_ids[0]
        ));
        assert!(matches!(
            &replay[1],
            ToolClaim::Replay {
                should_continue: true,
                operation_id,
                ..
            } if operation_id == &operation_ids[1]
        ));
    }

    #[tokio::test]
    async fn river_projection_is_frozen_before_runtime_and_cannot_drift_on_retry() {
        let pool = test_pool().await;
        let turn = create_turn(&pool, &start(), LocalCliTurnRoute::LocalLoopback, 100)
            .await
            .expect("create turn");
        mark_model_running(&pool, &turn.turn_attempt, 0, 101)
            .await
            .expect("mark running");
        let digest = format!("{:x}", Sha256::digest(b"semantic call"));
        let operation_id =
            match claim_tool_batch(&pool, &turn.turn_attempt, 0, &[digest], "assistant", 102)
                .await
                .expect("claim")
                .remove(0)
            {
                ToolClaim::Claimed { operation_id } => operation_id,
                _ => panic!("new tool must be claimed"),
            };
        let json = r#"{"schema":"vcp.mobile.attempt-projection.v1","river":{"mode":"semantic:2","resolved_mode":"fallback_last","messages":[]},"artifacts":[],"omissions":[]}"#;
        let projection = DurableRiverProjection {
            canonical_json: json.to_string(),
            sha256: format!("{:x}", Sha256::digest(json.as_bytes())),
            size_bytes: json.len() as u64,
            artifacts: Vec::new(),
        };
        assert_eq!(
            bind_tool_projection(&pool, &turn.turn_attempt, &operation_id, &projection, 103)
                .await
                .expect("first bind"),
            projection
        );
        assert_eq!(
            bind_tool_projection(&pool, &turn.turn_attempt, &operation_id, &projection, 104)
                .await
                .expect("idempotent bind"),
            projection
        );
        let mut drifted = projection.clone();
        drifted.canonical_json = drifted.canonical_json.replace("fallback_last", "semantic");
        drifted.size_bytes = drifted.canonical_json.len() as u64;
        drifted.sha256 = format!("{:x}", Sha256::digest(drifted.canonical_json.as_bytes()));
        assert!(
            bind_tool_projection(&pool, &turn.turn_attempt, &operation_id, &drifted, 105,)
                .await
                .expect_err("availability drift must conflict")
                .contains("conflicts")
        );
        let loaded = load_live_turn(&pool, "message")
            .await
            .expect("load")
            .expect("turn");
        assert_eq!(loaded.step_records[0].river_projection, Some(projection));
    }

    #[tokio::test]
    async fn exact_once_claim_replays_after_many_state_reads() {
        let pool = test_pool().await;
        let turn = create_turn(&pool, &start(), LocalCliTurnRoute::LocalLoopback, 100)
            .await
            .expect("create turn");
        mark_model_running(&pool, &turn.turn_attempt, 0, 101)
            .await
            .expect("mark running");
        let digest = format!("{:x}", Sha256::digest(b"tool"));
        let first = claim_tool_batch(
            &pool,
            &turn.turn_attempt,
            0,
            std::slice::from_ref(&digest),
            "assistant",
            102,
        )
        .await
        .expect("claim tool")
        .into_iter()
        .next()
        .expect("single claim");
        let operation_id = match first {
            ToolClaim::Claimed { operation_id } => operation_id,
            _ => panic!("expected first claim"),
        };
        let result = VcpCliResultEnvelope::success(VcpCliResultBody {
            content: vec![],
            job: None,
            jobs: None,
            skill: None,
            skills: None,
            runtime: Some(VcpCliRuntimeInfo::local_loopback()),
        });
        store_tool_result(
            &pool,
            &turn.turn_attempt,
            &operation_id,
            &result,
            "<!-- VCP_TOOL_PAYLOAD -->\n[]",
            false,
            true,
            None,
            103,
        )
        .await
        .expect("store result");
        for _ in 0..300 {
            let loaded = load_live_turn(&pool, "message")
                .await
                .expect("load")
                .expect("live turn");
            assert_eq!(loaded.tool_steps, 1);
        }
        let replay = claim_tool_batch(
            &pool,
            &turn.turn_attempt,
            0,
            std::slice::from_ref(&digest),
            "assistant",
            104,
        )
        .await
        .expect("replay claim")
        .into_iter()
        .next()
        .expect("single replay");
        assert!(matches!(
            replay,
            ToolClaim::Replay { operation_id: ref replay_id, .. } if replay_id == &operation_id
        ));

        store_pending_continuation(
            &pool,
            &turn.turn_attempt,
            0,
            &[json!({"role":"user","content":"continue"})],
            105,
        )
        .await
        .expect("advance model step");
        mark_model_running(&pool, &turn.turn_attempt, 1, 106)
            .await
            .expect("run next step");
        let repeated = claim_tool_batch(
            &pool,
            &turn.turn_attempt,
            1,
            std::slice::from_ref(&digest),
            "same canonical call in a later step",
            107,
        )
        .await
        .expect("claim repeated later call")
        .into_iter()
        .next()
        .expect("single later claim");
        assert!(matches!(
            repeated,
            ToolClaim::Claimed { operation_id: ref next_id } if next_id != &operation_id
        ));
    }

    #[tokio::test]
    async fn continuation_is_durable_before_model_retry_and_secrets_are_absent() {
        let pool = test_pool().await;
        let turn = create_turn(&pool, &start(), LocalCliTurnRoute::LocalLoopback, 100)
            .await
            .expect("create turn");
        let continuation = vec![json!({"role":"user","content":"payload"})];
        sqlx::query("UPDATE local_cli_turn_ledger SET state='result_ready' WHERE turn_attempt=?")
            .bind(&turn.turn_attempt)
            .execute(&pool)
            .await
            .expect("seed result ready");
        let pending = store_pending_continuation(&pool, &turn.turn_attempt, 0, &continuation, 110)
            .await
            .expect("persist continuation");
        assert_eq!(pending.state, LocalCliTurnState::ContinuationPending);
        assert_eq!(pending.step_index, 1);
        assert_eq!(pending.continuation_messages, Some(continuation));
        let raw: String = sqlx::query_scalar(
            "SELECT frozen_request_json || COALESCE(continuation_messages_json,'') FROM local_cli_turn_ledger",
        )
        .fetch_one(&pool)
        .await
        .expect("read raw ledger");
        assert!(!raw.contains("secret-not-persisted"));
        assert!(!raw.contains("not-persisted.invalid"));
    }

    #[tokio::test]
    async fn message_and_topic_tombstones_purge_pending_turns() {
        let pool = test_pool().await;
        create_turn(&pool, &start(), LocalCliTurnRoute::LocalLoopback, 100)
            .await
            .expect("create turn");
        sqlx::query(
            "UPDATE messages SET deleted_at=200 WHERE topic_id='topic' AND msg_id='message'",
        )
        .execute(&pool)
        .await
        .expect("soft delete message");
        assert!(load_live_turn(&pool, "message")
            .await
            .expect("load")
            .is_none());

        sqlx::query(
            "UPDATE messages SET deleted_at=NULL WHERE topic_id='topic' AND msg_id='message'",
        )
        .execute(&pool)
        .await
        .expect("restore fixture");
        create_turn(&pool, &start(), LocalCliTurnRoute::LocalLoopback, 300)
            .await
            .expect("recreate turn");
        sqlx::query("UPDATE topics SET deleted_at=400 WHERE topic_id='topic'")
            .execute(&pool)
            .await
            .expect("soft delete topic");
        assert!(load_live_turn(&pool, "message")
            .await
            .expect("load")
            .is_none());
    }

    #[tokio::test]
    async fn finalizing_crash_reconciles_from_terminal_message_fact() {
        let pool = test_pool().await;
        let turn = create_turn(&pool, &start(), LocalCliTurnRoute::LocalLoopback, 100)
            .await
            .expect("create turn");
        assert_eq!(
            claim_finalizer(&pool, &turn.turn_attempt, "done", "completed", 110)
                .await
                .expect("claim finalizer"),
            FinalizerClaim::Claimed
        );
        sqlx::query(
            "UPDATE messages SET content='done', finish_reason='completed'
             WHERE topic_id='topic' AND msg_id='message'",
        )
        .execute(&pool)
        .await
        .expect("commit message fact");
        sqlx::query("DELETE FROM active_generations WHERE msg_id='message'")
            .execute(&pool)
            .await
            .expect("release active generation");
        let reconciled = load_live_turn(&pool, "message")
            .await
            .expect("load reconciled")
            .expect("turn remains for audit");
        assert_eq!(reconciled.state, LocalCliTurnState::Terminal);
        assert_eq!(reconciled.final_content.as_deref(), Some("done"));
    }
}
