//! Single outer owner for the local VCPMobileCLI model/tool continuation loop.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{Pool, Sqlite};
use tauri::{ipc::Channel, AppHandle, Manager, Runtime};
use tokio_util::sync::CancellationToken;

use crate::vcp_modules::aurora_pipeline::AuroraUpdate;
use crate::vcp_modules::message_service;
use crate::vcp_modules::settings_manager::{read_settings, MobileCliAgentRoute};
use crate::vcp_modules::stream_block_parser::StreamBlockParser;
use crate::vcp_modules::vcp_client::{
    is_typed_assistant_budget_error, perform_vcp_request_registered, StreamEvent, VcpRequestPayload,
};

use super::protocol::{parse_vcp_tool_requests, validate_vcp_mobile_cli_request, VcpCliAction};
use super::result::{serialize_local_model_payload, VcpCliErrorCode, VcpCliResultEnvelope};
use super::runtime::{ExecuteVcpMobileCliRequest, MobileCliAdmissionError, MobileCliRuntimeState};
use super::turn_ledger::{
    claim_finalizer, claim_tool_batch, create_turn, load_live_turn, mark_interrupted,
    mark_model_continued, mark_model_running, mark_terminal, store_model_retry_pending,
    store_pending_continuation, store_tool_result, FinalizerClaim, ToolClaim,
};
use super::turn_meta::{
    append_marked_history, local_optional_context_notices, marked_history_block_with_projection,
    plan_local_policy, LocalContinuationPolicy,
};
use super::turn_types::{
    LocalCliTurnOutcome, LocalCliTurnRecord, LocalCliTurnRoute, LocalCliTurnStart,
    LocalCliTurnState, MAX_ASSISTANT_STEP_BYTES, MAX_LOCAL_CLI_TOOL_STEPS, MAX_TOOL_PAYLOAD_BYTES,
};

pub(crate) async fn run_local_cli_turn<R: Runtime>(
    app: &AppHandle<R>,
    pool: &Pool<Sqlite>,
    start: LocalCliTurnStart,
    frozen_route: MobileCliAgentRoute,
    stream_channel: Option<Channel<StreamEvent>>,
    cancellation_token: CancellationToken,
) -> Result<LocalCliTurnOutcome, String> {
    if frozen_route != MobileCliAgentRoute::LocalLoopback {
        return Err("local CLI coordinator cannot own the vcpPlugin route".to_string());
    }
    let record = create_turn(pool, &start, LocalCliTurnRoute::LocalLoopback, now_ms()?).await?;
    run_record(
        app,
        pool,
        record,
        start.vcp_url,
        start.vcp_api_key,
        stream_channel,
        cancellation_token,
    )
    .await
}

/// Recovery hook used before the legacy SSE disk/helper branches.
/// `None` means no live local-loopback ledger row and the caller must continue its old path.
/// `Some` means this coordinator fully owns the recovery result and the caller must return it.
pub(crate) async fn recover_local_cli_turn<R: Runtime>(
    app: &AppHandle<R>,
    pool: &Pool<Sqlite>,
    outer_message_id: &str,
    stream_channel: Channel<StreamEvent>,
    cancellation_token: CancellationToken,
) -> Result<Option<Value>, String> {
    let Some(mut record) = load_live_turn(pool, outer_message_id).await? else {
        return Ok(None);
    };
    if record.route != LocalCliTurnRoute::LocalLoopback {
        return Ok(None);
    }
    if record.state == LocalCliTurnState::Terminal {
        return Ok(Some(json!({
            "status": "completed",
            "content": record.final_content.unwrap_or_default(),
            "finishReason": record.terminal_reason.unwrap_or_else(|| "completed".to_string()),
        })));
    }
    if record.state == LocalCliTurnState::Finalizing {
        // `load_live_turn` already reconciled a committed message. Remaining Finalizing means the
        // message CAS is still pending, so run_record may safely retry that exact finalizer claim.
    } else if record.state == LocalCliTurnState::ResultReady {
        match consume_result_ready(pool, record).await? {
            ResultReadyAction::Continue(next) => record = next,
            ResultReadyAction::Finalize { record, content } => {
                let outcome = finalize_turn(
                    app,
                    pool,
                    &record,
                    content,
                    "completed".to_string(),
                    false,
                    Some(stream_channel),
                )
                .await?;
                return Ok(Some(outcome_json(outcome)));
            }
        }
    } else if record.state == LocalCliTurnState::Claimed
        && record.step_records.iter().any(|step| step.result.is_none())
    {
        match recover_claimed_batch(app, pool, &record, &cancellation_token).await {
            Ok(recovered) => {
                record = recovered;
                if record.state == LocalCliTurnState::ResultReady {
                    match consume_result_ready(pool, record).await? {
                        ResultReadyAction::Continue(next) => record = next,
                        ResultReadyAction::Finalize { record, content } => {
                            let outcome = finalize_turn(
                                app,
                                pool,
                                &record,
                                content,
                                "completed".to_string(),
                                false,
                                Some(stream_channel),
                            )
                            .await?;
                            return Ok(Some(outcome_json(outcome)));
                        }
                    }
                }
            }
            Err(ClaimedRecoveryError::RetryPending(error)) => {
                return Ok(Some(outcome_json(
                    LocalCliTurnOutcome::ContinuationPending {
                        turn_attempt: record.turn_attempt,
                        step_index: record.step_index,
                        reason: error,
                    },
                )));
            }
            Err(ClaimedRecoveryError::Cancelled) => {
                let outcome = finalize_turn(
                    app,
                    pool,
                    &record,
                    "本地 CLI 工具尚未启动；本轮已取消。".to_string(),
                    "cancelled_by_user".to_string(),
                    true,
                    Some(stream_channel),
                )
                .await?;
                return Ok(Some(outcome_json(outcome)));
            }
            Err(ClaimedRecoveryError::Deadline) => {
                let outcome = finalize_turn(
                    app,
                    pool,
                    &record,
                    "本地 CLI 工具尚未启动；本轮已达到 30 分钟总时限。".to_string(),
                    "local_cli_turn_timeout".to_string(),
                    false,
                    Some(stream_channel),
                )
                .await?;
                return Ok(Some(outcome_json(outcome)));
            }
            Err(ClaimedRecoveryError::Integrity(error)) => {
                mark_interrupted(pool, &record.turn_attempt, &error, now_ms()?).await?;
                record = load_live_turn(pool, outer_message_id)
                    .await?
                    .ok_or_else(|| "interrupted local CLI turn disappeared".to_string())?;
                let outcome = finalize_turn(
                    app,
                    pool,
                    &record,
                    "本地 CLI 闭合批次快照不一致；为避免错误执行，恢复已终止。".to_string(),
                    "interrupted".to_string(),
                    false,
                    Some(stream_channel),
                )
                .await?;
                return Ok(Some(outcome_json(outcome)));
            }
        }
    }

    let settings = read_settings(app.clone(), app.state()).await?;
    let outcome = run_record(
        app,
        pool,
        record,
        settings.vcp_server_url,
        settings.vcp_api_key,
        Some(stream_channel),
        cancellation_token,
    )
    .await?;
    Ok(Some(outcome_json(outcome)))
}

async fn run_record<R: Runtime>(
    app: &AppHandle<R>,
    pool: &Pool<Sqlite>,
    mut record: LocalCliTurnRecord,
    vcp_url: String,
    vcp_api_key: String,
    stream_channel: Option<Channel<StreamEvent>>,
    cancellation_token: CancellationToken,
) -> Result<LocalCliTurnOutcome, String> {
    if record.state == LocalCliTurnState::Terminal {
        return Ok(LocalCliTurnOutcome::AlreadyTerminal {
            content: record.final_content.unwrap_or_default(),
            finish_reason: record
                .terminal_reason
                .unwrap_or_else(|| "completed".to_string()),
        });
    }

    let mut messages = record
        .continuation_messages
        .clone()
        .unwrap_or_else(|| record.frozen_request.messages.clone());
    loop {
        let now = now_ms()?;
        if cancellation_token.is_cancelled() {
            return finalize_turn(
                app,
                pool,
                &record,
                "本地 CLI 工具尚未启动；本轮已取消。".to_string(),
                "cancelled_by_user".to_string(),
                true,
                stream_channel.clone(),
            )
            .await;
        }
        if now >= record.deadline_at_ms {
            return finalize_turn(
                app,
                pool,
                &record,
                "本地 CLI 工具循环已达到 30 分钟总时限。".to_string(),
                "local_cli_turn_timeout".to_string(),
                false,
                stream_channel.clone(),
            )
            .await;
        }
        if record.tool_steps >= MAX_LOCAL_CLI_TOOL_STEPS
            && record.state != LocalCliTurnState::ContinuationPending
        {
            return finalize_turn(
                app,
                pool,
                &record,
                "本地 CLI 工具循环已达到 8 次工具调用上限。".to_string(),
                "local_cli_step_limit".to_string(),
                false,
                stream_channel.clone(),
            )
            .await;
        }
        if record.state == LocalCliTurnState::Finalizing {
            return finalize_turn(
                app,
                pool,
                &record,
                record.final_content.clone().unwrap_or_default(),
                record
                    .terminal_reason
                    .clone()
                    .unwrap_or_else(|| "completed".to_string()),
                false,
                stream_channel.clone(),
            )
            .await;
        }

        project_durable_tool_results(stream_channel.as_ref(), &record, record.step_index > 0);

        record = mark_model_running(pool, &record.turn_attempt, record.step_index, now).await?;
        let payload = VcpRequestPayload {
            vcp_url: vcp_url.clone(),
            vcp_api_key: vcp_api_key.clone(),
            messages: messages.clone(),
            model_config: record.frozen_request.model_config.clone(),
            message_id: record.outer_message_id.clone(),
            context: record.frozen_request.context.clone(),
            transport_request_id: Some(record.transport_request_id()),
            turn_attempt: Some(record.turn_attempt.clone()),
            step_index: Some(record.step_index),
            projection_reset: Some(record.step_index > 0),
            mobile_cli_agent_route: Some(MobileCliAgentRoute::LocalLoopback),
            local_cli_projection_prefix: durable_tool_result_prefix(&record),
        };
        let model_result = perform_vcp_request_registered(
            app,
            payload,
            stream_channel.clone(),
            cancellation_token.clone(),
        )
        .await;
        let (response, is_aborted) = match model_result {
            Ok(result) => result,
            Err(error) => {
                if let Some((content, finish_reason)) = terminal_model_error(&error) {
                    return finalize_turn(
                        app,
                        pool,
                        &record,
                        content.to_string(),
                        finish_reason.to_string(),
                        false,
                        stream_channel.clone(),
                    )
                    .await;
                }
                record = store_model_retry_pending(
                    pool,
                    &record.turn_attempt,
                    record.step_index,
                    &messages,
                    &format!("model continuation pending: {error}"),
                    now_ms()?,
                )
                .await?;
                return Ok(LocalCliTurnOutcome::ContinuationPending {
                    turn_attempt: record.turn_attempt,
                    step_index: record.step_index,
                    reason: error,
                });
            }
        };
        let assistant_content = response
            .get("fullContent")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if assistant_content.len() > MAX_ASSISTANT_STEP_BYTES {
            return finalize_turn(
                app,
                pool,
                &record,
                "模型单步输出超过 512 KiB 安全上限，工具调用未执行。".to_string(),
                "local_cli_assistant_too_large".to_string(),
                false,
                stream_channel.clone(),
            )
            .await;
        }
        if is_aborted || cancellation_token.is_cancelled() {
            return finalize_turn(
                app,
                pool,
                &record,
                assistant_content,
                "cancelled_by_user".to_string(),
                true,
                stream_channel.clone(),
            )
            .await;
        }
        record =
            mark_model_continued(pool, &record.turn_attempt, record.step_index, now_ms()?).await?;
        let raw_requests = parse_vcp_tool_requests(&assistant_content);
        if raw_requests.is_empty() {
            let finish_reason = response
                .get("finishReason")
                .and_then(Value::as_str)
                .unwrap_or("completed")
                .to_string();
            return finalize_turn(
                app,
                pool,
                &record,
                assistant_content,
                finish_reason,
                false,
                stream_channel.clone(),
            )
            .await;
        }

        let request_count = match u32::try_from(raw_requests.len()) {
            Ok(count) => count,
            Err(_) => {
                return finalize_turn(
                    app,
                    pool,
                    &record,
                    "模型单步包含过多工具调用，未执行。".to_string(),
                    "local_cli_step_limit".to_string(),
                    false,
                    stream_channel.clone(),
                )
                .await;
            }
        };
        if batch_exceeds_tool_budget(record.tool_steps, request_count) {
            return finalize_turn(
                app,
                pool,
                &record,
                "本地 CLI 工具循环已达到 8 次工具调用上限；本轮超额调用未执行。".to_string(),
                "local_cli_step_limit".to_string(),
                false,
                stream_channel.clone(),
            )
            .await;
        }
        let digests = raw_requests
            .iter()
            .map(tool_digest)
            .collect::<Result<Vec<_>, _>>()?;
        let claims = match claim_tool_batch(
            pool,
            &record.turn_attempt,
            record.step_index,
            &digests,
            &assistant_content,
            now_ms()?,
        )
        .await
        {
            Ok(claims) => claims,
            Err(error) => {
                return finalize_turn(
                    app,
                    pool,
                    &record,
                    format!("本地 CLI 工具批次未执行：{error}"),
                    "local_cli_claim_failed".to_string(),
                    false,
                    stream_channel.clone(),
                )
                .await;
            }
        };

        let mut continuation_payloads = Vec::new();
        let mut must_continue = false;
        for ((raw_request, _digest), claim) in raw_requests.iter().zip(&digests).zip(claims) {
            let (operation_id, result, local_payload, continuation_policy) = match claim {
                ToolClaim::Replay {
                    operation_id,
                    result,
                    local_payload,
                    should_continue,
                } => (
                    operation_id,
                    *result,
                    local_payload,
                    if should_continue {
                        LocalContinuationPolicy::Continue
                    } else {
                        LocalContinuationPolicy::NoReply
                    },
                ),
                ToolClaim::InFlight { .. } => {
                    return Ok(LocalCliTurnOutcome::ContinuationPending {
                        turn_attempt: record.turn_attempt.clone(),
                        step_index: record.step_index,
                        reason: "durable CLI operation is in flight; recovery will replay the same operation_id"
                            .to_string(),
                    });
                }
                ToolClaim::Claimed { operation_id } => {
                    match execute_claimed_tool(
                        app,
                        pool,
                        &record,
                        raw_request,
                        &operation_id,
                        &messages,
                        &cancellation_token,
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(ClaimedToolError::Cancelled) => {
                            return finalize_turn(
                                app,
                                pool,
                                &record,
                                "本地 CLI 工具尚未启动；本轮已取消。".to_string(),
                                "cancelled_by_user".to_string(),
                                true,
                                stream_channel.clone(),
                            )
                            .await;
                        }
                        Err(ClaimedToolError::Deadline) => {
                            return finalize_turn(
                                app,
                                pool,
                                &record,
                                "本地 CLI 工具尚未启动；本轮已达到 30 分钟总时限。".to_string(),
                                "local_cli_turn_timeout".to_string(),
                                false,
                                stream_channel.clone(),
                            )
                            .await;
                        }
                        Err(ClaimedToolError::RetryPending(error)) => {
                            return Ok(LocalCliTurnOutcome::ContinuationPending {
                                turn_attempt: record.turn_attempt.clone(),
                                step_index: record.step_index,
                                reason: format!(
                                    "CLI operation is durably claimed; exact replay is pending: {error}"
                                ),
                            });
                        }
                    }
                }
            };
            let successful = matches!(result, VcpCliResultEnvelope::Success { .. });
            must_continue |=
                continuation_policy == LocalContinuationPolicy::Continue || !successful;
            if continuation_policy == LocalContinuationPolicy::Continue || !successful {
                continuation_payloads.push(local_payload);
            }
            record = load_live_turn(pool, &record.outer_message_id)
                .await?
                .ok_or_else(|| "local CLI turn was deleted during execution".to_string())?;
            project_durable_tool_results(stream_channel.as_ref(), &record, false);
            let _ = operation_id;
        }

        if !must_continue {
            return finalize_turn(
                app,
                pool,
                &record,
                assistant_content,
                "completed".to_string(),
                false,
                stream_channel.clone(),
            )
            .await;
        }
        let combined_payload = continuation_payloads.join("\n");
        if combined_payload.len() > MAX_TOOL_PAYLOAD_BYTES {
            return finalize_turn(
                app,
                pool,
                &record,
                "本地 CLI 工具结果超过 256 KiB 续轮上限；完整输出仍可通过 Job cursor 读取。"
                    .to_string(),
                "local_cli_tool_payload_too_large".to_string(),
                false,
                stream_channel.clone(),
            )
            .await;
        }
        messages.push(json!({"role":"assistant", "content":assistant_content}));
        messages.push(json!({"role":"user", "content":combined_payload}));
        record = store_pending_continuation(
            pool,
            &record.turn_attempt,
            record.step_index,
            &messages,
            now_ms()?,
        )
        .await?;
    }
}

fn terminal_model_error(error: &str) -> Option<(&'static str, &'static str)> {
    is_typed_assistant_budget_error(error).then_some((
        "模型单步输出超过 512 KiB 安全上限，工具调用未执行。",
        "local_cli_assistant_too_large",
    ))
}

enum ClaimedToolError {
    RetryPending(String),
    Cancelled,
    Deadline,
}

impl From<String> for ClaimedToolError {
    fn from(error: String) -> Self {
        Self::RetryPending(error)
    }
}

impl From<MobileCliAdmissionError> for ClaimedToolError {
    fn from(error: MobileCliAdmissionError) -> Self {
        match error {
            MobileCliAdmissionError::Cancelled => Self::Cancelled,
            MobileCliAdmissionError::Deadline => Self::Deadline,
            MobileCliAdmissionError::Runtime(error) => Self::RetryPending(error),
        }
    }
}

fn ensure_claimed_work_allowed(
    record: &LocalCliTurnRecord,
    cancellation_token: &CancellationToken,
) -> Result<(), ClaimedToolError> {
    if cancellation_token.is_cancelled() {
        return Err(ClaimedToolError::Cancelled);
    }
    if now_ms()? >= record.deadline_at_ms {
        return Err(ClaimedToolError::Deadline);
    }
    Ok(())
}

async fn execute_claimed_tool<R: Runtime>(
    app: &AppHandle<R>,
    pool: &Pool<Sqlite>,
    record: &LocalCliTurnRecord,
    raw_request: &super::protocol::RawVcpToolRequest,
    operation_id: &str,
    _messages: &[Value],
    cancellation_token: &CancellationToken,
) -> Result<
    (
        String,
        VcpCliResultEnvelope,
        String,
        LocalContinuationPolicy,
    ),
    ClaimedToolError,
> {
    ensure_claimed_work_allowed(record, cancellation_token)?;
    let (result, policy, mark_history) = match validate_vcp_mobile_cli_request(raw_request) {
        Err(error) => (
            VcpCliResultEnvelope::error(
                error.code,
                error.to_string(),
                "Correct the VCPMobileCLI request fields and retry.",
            ),
            LocalContinuationPolicy::Continue,
            false,
        ),
        Ok(mut validated) => match plan_local_policy(&validated) {
            Err(error) => (
                VcpCliResultEnvelope::error(
                    VcpCliErrorCode::UnsupportedMode,
                    error,
                    "Remove the unsupported meta field and retry.",
                ),
                LocalContinuationPolicy::Continue,
                validated.meta.ink.is_some(),
            ),
            Ok((mark_history, continuation)) => {
                let notices = local_optional_context_notices(&validated);
                if matches!(
                    continuation,
                    LocalContinuationPolicy::Parallel | LocalContinuationPolicy::NoReply
                ) {
                    force_background(&mut validated.action);
                }
                ensure_claimed_work_allowed(record, cancellation_token)?;
                let runtime = app.state::<MobileCliRuntimeState>();
                let mut response = runtime
                    .execute_with_turn_admission(
                        app,
                        ExecuteVcpMobileCliRequest {
                            operation_id: operation_id.to_string(),
                            action: validated.action,
                            session_id: Some(local_cli_session_id(record)),
                        },
                        cancellation_token.clone(),
                        record.deadline_at_ms,
                    )
                    .await?;
                response.envelope.prepend_optional_context_notices(&notices);
                (response.envelope, continuation, mark_history)
            }
        },
    };
    let local_payload = serialize_local_model_payload(&result)
        .map_err(|error| format!("cannot serialize local CLI result payload: {error}"))?;
    if local_payload.len() > MAX_TOOL_PAYLOAD_BYTES {
        return Err(ClaimedToolError::RetryPending(
            "local CLI result payload exceeds its hard byte limit".to_string(),
        ));
    }
    // Mobile currently has no ShowVCP toggle, so every local call receives the bounded existing
    // ToolResult projection in final history. `mark_history` remains durable force-persist metadata
    // for a future explicit hide policy.
    let marked = Some(marked_history_block_with_projection(
        operation_id,
        &result,
        None,
    ));
    store_tool_result(
        pool,
        &record.turn_attempt,
        operation_id,
        &result,
        &local_payload,
        mark_history,
        policy == LocalContinuationPolicy::Continue
            || matches!(result, VcpCliResultEnvelope::Error { .. }),
        marked.as_deref(),
        now_ms()?,
    )
    .await?;
    Ok((operation_id.to_string(), result, local_payload, policy))
}

enum ClaimedRecoveryError {
    RetryPending(String),
    Integrity(String),
    Cancelled,
    Deadline,
}

async fn recover_claimed_batch<R: Runtime>(
    app: &AppHandle<R>,
    pool: &Pool<Sqlite>,
    record: &LocalCliTurnRecord,
    cancellation_token: &CancellationToken,
) -> Result<LocalCliTurnRecord, ClaimedRecoveryError> {
    let (step_records, raw_requests) =
        validate_closed_batch(record).map_err(ClaimedRecoveryError::Integrity)?;
    let messages = record
        .continuation_messages
        .as_deref()
        .unwrap_or(&record.frozen_request.messages);
    for (index, (step, raw_request)) in step_records.iter().zip(&raw_requests).enumerate() {
        if step.call_index != index as u32
            || tool_digest(raw_request).map_err(ClaimedRecoveryError::Integrity)?
                != step.tool_digest
        {
            return Err(ClaimedRecoveryError::Integrity(
                "durable CLI batch digest/index mismatch".to_string(),
            ));
        }
        require_recoverable_step(step).map_err(ClaimedRecoveryError::Integrity)?;
        if step.result.is_some() {
            continue;
        }
        execute_claimed_tool(
            app,
            pool,
            record,
            raw_request,
            &step.operation_id,
            messages,
            cancellation_token,
        )
        .await
        .map_err(|error| match error {
            ClaimedToolError::RetryPending(error) => ClaimedRecoveryError::RetryPending(error),
            ClaimedToolError::Cancelled => ClaimedRecoveryError::Cancelled,
            ClaimedToolError::Deadline => ClaimedRecoveryError::Deadline,
        })?;
    }
    load_live_turn(pool, &record.outer_message_id)
        .await
        .map_err(ClaimedRecoveryError::RetryPending)?
        .ok_or_else(|| {
            ClaimedRecoveryError::Integrity(
                "local CLI turn disappeared during recovery".to_string(),
            )
        })
}

fn require_recoverable_step(step: &super::turn_types::LocalCliStepRecord) -> Result<(), String> {
    if step.result.is_none() && step.river_projection.is_some() {
        return Err("旧版 River 上下文任务无法在当前本地回环中安全恢复；请重试本轮。".to_string());
    }
    Ok(())
}

fn validate_closed_batch(
    record: &LocalCliTurnRecord,
) -> Result<
    (
        Vec<&super::turn_types::LocalCliStepRecord>,
        Vec<super::protocol::RawVcpToolRequest>,
    ),
    String,
> {
    let step_records = record
        .step_records
        .iter()
        .filter(|step| step.model_step_index == record.step_index)
        .collect::<Vec<_>>();
    if step_records.len() != record.expected_calls as usize || step_records.is_empty() {
        return Err("durable CLI batch call count is inconsistent".to_string());
    }
    let assistant_content = &step_records[0].assistant_content;
    if step_records
        .iter()
        .any(|step| step.assistant_content != *assistant_content)
    {
        return Err("durable CLI batch has conflicting assistant snapshots".to_string());
    }
    let raw_requests = parse_vcp_tool_requests(assistant_content);
    if raw_requests.len() != step_records.len() {
        return Err(
            "durable assistant snapshot no longer matches its closed CLI batch".to_string(),
        );
    }
    for (index, (step, raw_request)) in step_records.iter().zip(&raw_requests).enumerate() {
        if step.call_index != index as u32 || tool_digest(raw_request)? != step.tool_digest {
            return Err("durable CLI batch digest/index mismatch".to_string());
        }
    }
    Ok((step_records, raw_requests))
}

enum ResultReadyAction {
    Continue(LocalCliTurnRecord),
    Finalize {
        record: LocalCliTurnRecord,
        content: String,
    },
}

async fn consume_result_ready(
    pool: &Pool<Sqlite>,
    record: LocalCliTurnRecord,
) -> Result<ResultReadyAction, String> {
    let mut messages = record
        .continuation_messages
        .clone()
        .unwrap_or_else(|| record.frozen_request.messages.clone());
    let step_records = record
        .step_records
        .iter()
        .filter(|step| step.model_step_index == record.step_index)
        .collect::<Vec<_>>();
    if step_records.is_empty() || step_records.iter().any(|step| step.local_payload.is_none()) {
        return Err("result_ready turn has no complete durable tool result".to_string());
    }
    let assistant_content = step_records[0].assistant_content.clone();
    if step_records
        .iter()
        .any(|step| step.assistant_content != assistant_content)
    {
        return Err("result_ready turn has conflicting assistant snapshots".to_string());
    }
    if !result_ready_should_continue(&step_records) {
        return Ok(ResultReadyAction::Finalize {
            record,
            content: assistant_content,
        });
    }
    let payload = step_records
        .iter()
        .filter(|step| step.should_continue)
        .filter_map(|step| step.local_payload.clone())
        .collect::<Vec<_>>()
        .join("\n");
    messages.push(json!({"role":"assistant", "content":assistant_content}));
    messages.push(json!({"role":"user", "content":payload}));
    store_pending_continuation(
        pool,
        &record.turn_attempt,
        record.step_index,
        &messages,
        now_ms()?,
    )
    .await
    .map(ResultReadyAction::Continue)
}

fn result_ready_should_continue(step_records: &[&super::turn_types::LocalCliStepRecord]) -> bool {
    step_records.iter().any(|step| step.should_continue)
}

async fn finalize_turn<R: Runtime>(
    app: &AppHandle<R>,
    pool: &Pool<Sqlite>,
    record: &LocalCliTurnRecord,
    content: String,
    finish_reason: String,
    is_aborted: bool,
    stream_channel: Option<Channel<StreamEvent>>,
) -> Result<LocalCliTurnOutcome, String> {
    let final_content = final_content_for_claim(record, content, is_aborted);
    match claim_finalizer(
        pool,
        &record.turn_attempt,
        &final_content,
        &finish_reason,
        now_ms()?,
    )
    .await?
    {
        FinalizerClaim::ReplayTerminal {
            content,
            finish_reason,
        } => Ok(LocalCliTurnOutcome::AlreadyTerminal {
            content,
            finish_reason,
        }),
        FinalizerClaim::InFlight => Ok(LocalCliTurnOutcome::ContinuationPending {
            turn_attempt: record.turn_attempt.clone(),
            step_index: record.step_index,
            reason: "local CLI finalizer is already in flight".to_string(),
        }),
        FinalizerClaim::Claimed => {
            if let Err(error) = message_service::finalize_stream_message_with_turn_projection(
                app.clone(),
                pool,
                &record.owner_id,
                &record.owner_type,
                record.topic_id.clone(),
                record.outer_message_id.clone(),
                final_content.clone(),
                false,
                Some(finish_reason.clone()),
                stream_channel,
                record.speaker_agent_id.clone(),
                record.turn_attempt.clone(),
                record.step_index,
            )
            .await
            {
                return Ok(LocalCliTurnOutcome::ContinuationPending {
                    turn_attempt: record.turn_attempt.clone(),
                    step_index: record.step_index,
                    reason: format!("final message commit pending recovery: {error}"),
                });
            }
            if let Err(error) = mark_terminal(pool, &record.turn_attempt, now_ms()?).await {
                return Ok(LocalCliTurnOutcome::ContinuationPending {
                    turn_attempt: record.turn_attempt.clone(),
                    step_index: record.step_index,
                    reason: format!("terminal ledger reconciliation pending: {error}"),
                });
            }
            Ok(LocalCliTurnOutcome::Finalized {
                turn_attempt: record.turn_attempt.clone(),
                highest_step_index: record.step_index,
                content: final_content,
                finish_reason,
                is_aborted,
            })
        }
    }
}

fn final_content_for_claim(
    record: &LocalCliTurnRecord,
    content: String,
    is_aborted: bool,
) -> String {
    if record.state == LocalCliTurnState::Finalizing {
        return content;
    }
    let marked = record
        .marked_history
        .iter()
        .map(|entry| entry.block.clone())
        .collect::<Vec<_>>();
    let mut final_content = append_marked_history(&content, &marked);
    if is_aborted {
        final_content.push_str("\n\n> VCP流式错误: 请求已中止");
    }
    final_content
}

fn project_durable_tool_results(
    stream_channel: Option<&Channel<StreamEvent>>,
    record: &LocalCliTurnRecord,
    reset: bool,
) {
    let Some(channel) = stream_channel else {
        return;
    };
    let Some(content) = durable_tool_result_prefix(record) else {
        return;
    };
    let stable_blocks = StreamBlockParser::new().finalize(&content);
    let update = AuroraUpdate {
        stable_blocks: Some(stable_blocks),
        stable_changed: true,
        tail_block: None,
        tail: None,
        tail_changed: false,
        tail_frame: None,
        tail_snapshot: None,
        content: Some(content),
        chunk: None,
    };
    let event = StreamEvent::aurora(
        record.outer_message_id.clone(),
        update,
        record.frozen_request.context.clone(),
    )
    .with_turn_projection(record.turn_attempt.clone(), record.step_index, reset);
    let _ = channel.send(event);
}

fn durable_tool_result_prefix(record: &LocalCliTurnRecord) -> Option<String> {
    let summaries = record
        .step_records
        .iter()
        .filter_map(|step| {
            step.result.as_ref().map(|result| {
                marked_history_block_with_projection(
                    &step.operation_id,
                    result,
                    step.river_projection.as_ref(),
                )
            })
        })
        .collect::<Vec<_>>();
    (!summaries.is_empty()).then(|| format!("{}\n\n", summaries.join("\n\n")))
}

fn force_background(action: &mut VcpCliAction) {
    if let VcpCliAction::Run {
        run_in_background, ..
    } = action
    {
        *run_in_background = Some(true);
    }
}

fn batch_exceeds_tool_budget(current: u32, incoming: u32) -> bool {
    current.saturating_add(incoming) > MAX_LOCAL_CLI_TOOL_STEPS
}

fn tool_digest(request: &super::protocol::RawVcpToolRequest) -> Result<String, String> {
    let bytes = serde_json::to_vec(request)
        .map_err(|error| format!("cannot serialize canonical tool request: {error}"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn local_cli_session_id(record: &LocalCliTurnRecord) -> String {
    let identity = format!(
        "{}\0{}\0{}",
        record.owner_type, record.owner_id, record.topic_id
    );
    format!("chat:{:x}", Sha256::digest(identity.as_bytes()))
}

fn now_ms() -> Result<u64, String> {
    u64::try_from(crate::vcp_modules::infra::utils::now_millis())
        .map_err(|_| "system clock is before Unix epoch".to_string())
}

fn outcome_json(outcome: LocalCliTurnOutcome) -> Value {
    match outcome {
        LocalCliTurnOutcome::Finalized {
            content,
            finish_reason,
            ..
        }
        | LocalCliTurnOutcome::AlreadyTerminal {
            content,
            finish_reason,
        } => json!({
            "status":"completed",
            "content":content,
            "finishReason":finish_reason,
        }),
        LocalCliTurnOutcome::ContinuationPending {
            turn_attempt,
            step_index,
            reason,
        } => json!({
            "status":"continuation_pending",
            "turnAttempt":turn_attempt,
            "stepIndex":step_index,
            "reason":reason,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcp_modules::cli::protocol::{VcpArcheryMode, VcpCliAction};
    use crate::vcp_modules::cli::result::{VcpCliContentPart, VcpCliResultBody, VcpCliRuntimeInfo};
    use crate::vcp_modules::cli::turn_types::{
        DurableRiverProjection, FrozenModelRequest, LocalCliStepRecord, MarkedHistoryEntry,
    };
    use crate::vcp_modules::stream_block_parser::StreamBlock;

    fn record(state: LocalCliTurnState, assistant_content: &str) -> LocalCliTurnRecord {
        let requests = parse_vcp_tool_requests(assistant_content);
        let step_records = requests
            .iter()
            .enumerate()
            .map(|(index, request)| LocalCliStepRecord {
                model_step_index: 0,
                call_index: index as u32,
                tool_digest: tool_digest(request).expect("digest"),
                operation_id: format!("operation-{index}"),
                assistant_content: assistant_content.to_string(),
                river_projection: None,
                local_payload: None,
                result: None,
                mark_history: false,
                should_continue: true,
                created_at_ms: 1,
                updated_at_ms: 1,
            })
            .collect::<Vec<_>>();
        LocalCliTurnRecord {
            turn_attempt: "attempt".to_string(),
            outer_message_id: "message".to_string(),
            topic_id: "topic".to_string(),
            owner_id: "agent".to_string(),
            owner_type: "agent".to_string(),
            speaker_agent_id: Some("agent".to_string()),
            route: LocalCliTurnRoute::LocalLoopback,
            state,
            step_index: 0,
            tool_steps: step_records.len() as u32,
            started_at_ms: 1,
            deadline_at_ms: 2,
            updated_at_ms: 1,
            version: 0,
            frozen_request: FrozenModelRequest {
                messages: vec![json!({"role":"user","content":"test"})],
                model_config: json!({}),
                context: None,
            },
            continuation_messages: None,
            expected_calls: step_records.len() as u32,
            step_records,
            marked_history: Vec::new(),
            final_content: None,
            terminal_reason: None,
        }
    }

    #[test]
    fn archery_background_mapping_never_changes_command_text() {
        let command = "printf '%s' '$HOME'".to_string();
        let mut action = VcpCliAction::Run {
            command: command.clone(),
            description: None,
            cwd: Some("/workspace".to_string()),
            timeout_ms: Some(1_000),
            run_in_background: Some(false),
        };
        force_background(&mut action);
        assert!(matches!(
            action,
            VcpCliAction::Run {
                command: ref actual,
                run_in_background: Some(true),
                ..
            } if actual == &command
        ));
    }

    #[test]
    fn frozen_route_has_no_disabled_or_fallback_branch() {
        assert_eq!(
            LocalCliTurnRoute::LocalLoopback.as_db_value(),
            "local_loopback"
        );
        assert_eq!(LocalCliTurnRoute::VcpPlugin.as_db_value(), "vcp_plugin");
    }

    #[test]
    fn unfinished_legacy_river_step_is_not_reexecuted_without_its_projection() {
        let assistant = "<<<[TOOL_REQUEST]>>>\ntool_name:「始」VCPMobileCLI「末」,\naction:「始」run「末」,\ncommand:「始」pwd「末」\n<<<[END_TOOL_REQUEST]>>>";
        let mut claimed = record(LocalCliTurnState::Running, assistant);
        claimed.step_records[0].river_projection = Some(DurableRiverProjection {
            canonical_json: "{}".to_string(),
            sha256: "0".repeat(64),
            size_bytes: 2,
            artifacts: Vec::new(),
        });

        assert!(require_recoverable_step(&claimed.step_records[0])
            .expect_err("legacy unfinished projection must be interrupted")
            .contains("请重试本轮"));
        claimed.step_records[0].result = Some(VcpCliResultEnvelope::success(
            VcpCliResultBody::content_only(vec![VcpCliContentPart::text("completed legacy")]),
        ));
        assert!(require_recoverable_step(&claimed.step_records[0]).is_ok());
    }

    #[test]
    fn no_reply_and_parallel_are_scheduling_not_shell_fields() {
        for mode in [VcpArcheryMode::Parallel, VcpArcheryMode::NoReply] {
            let encoded = serde_json::to_string(&mode).expect("serialize archery");
            assert!(matches!(encoded.as_str(), "\"true\"" | "\"no_reply\""));
        }
    }

    #[test]
    fn closed_batch_snapshot_validates_before_zero_or_partial_result_recovery() {
        let assistant = "<<<[TOOL_REQUEST]>>>\ntool_name:「始」VCPMobileCLI「末」,\naction:「始」list「末」\n<<<[END_TOOL_REQUEST]>>>\n<<<[TOOL_REQUEST]>>>\ntool_name:「始」VCPMobileCLI「末」,\naction:「始」list_skills「末」\n<<<[END_TOOL_REQUEST]>>>";
        let mut claimed = record(LocalCliTurnState::Claimed, assistant);
        assert_eq!(
            validate_closed_batch(&claimed)
                .expect("zero-result batch")
                .0
                .len(),
            2
        );
        claimed.step_records[0].result = Some(VcpCliResultEnvelope::success(
            VcpCliResultBody::content_only(vec![VcpCliContentPart::text("first")]),
        ));
        claimed.step_records[0].local_payload = Some("payload".to_string());
        assert_eq!(
            validate_closed_batch(&claimed)
                .expect("partial-result batch")
                .0
                .len(),
            2
        );
        claimed.step_records[1].tool_digest = "0".repeat(64);
        assert!(validate_closed_batch(&claimed).is_err());
    }

    #[test]
    fn tool_budget_rejects_entire_ninth_call_before_claim() {
        assert!(!batch_exceeds_tool_budget(7, 1));
        assert!(batch_exceeds_tool_budget(8, 1));
        assert!(batch_exceeds_tool_budget(7, 2));
    }

    #[test]
    fn durable_tool_prefix_is_parser_visible_and_finalizing_is_byte_stable() {
        let assistant = "<<<[TOOL_REQUEST]>>>\ntool_name:「始」VCPMobileCLI「末」,\naction:「始」list「末」\n<<<[END_TOOL_REQUEST]>>>";
        let mut claimed = record(LocalCliTurnState::ResultReady, assistant);
        claimed.step_records[0].result = Some(VcpCliResultEnvelope::success(VcpCliResultBody {
            content: vec![VcpCliContentPart::text("bounded")],
            job: None,
            jobs: None,
            skill: None,
            skills: None,
            runtime: Some(VcpCliRuntimeInfo::local_loopback()),
        }));
        let prefix = durable_tool_result_prefix(&claimed).expect("tool prefix");
        assert!(StreamBlockParser::new()
            .finalize(&prefix)
            .iter()
            .any(|block| matches!(block, StreamBlock::ToolResult { .. })));

        claimed.marked_history.push(MarkedHistoryEntry {
            model_step_index: 0,
            operation_id: "operation-0".to_string(),
            block: prefix.trim().to_string(),
        });
        let first = final_content_for_claim(&claimed, "answer".to_string(), true);
        assert_eq!(first.matches("VCP调用结果信息汇总").count(), 1);
        assert_eq!(first.matches("请求已中止").count(), 1);
        claimed.state = LocalCliTurnState::Finalizing;
        claimed.final_content = Some(first.clone());
        let replay = final_content_for_claim(&claimed, first.clone(), true);
        assert_eq!(replay, first);
    }

    #[test]
    fn result_ready_no_reply_finalizes_without_model_continuation() {
        let assistant = "<<<[TOOL_REQUEST]>>>\ntool_name:「始」VCPMobileCLI「末」,\naction:「始」list「末」\n<<<[END_TOOL_REQUEST]>>>";
        let mut claimed = record(LocalCliTurnState::ResultReady, assistant);
        claimed.step_records[0].should_continue = false;
        let records = claimed.step_records.iter().collect::<Vec<_>>();
        assert!(!result_ready_should_continue(&records));
        claimed.step_records[0].should_continue = true;
        let records = claimed.step_records.iter().collect::<Vec<_>>();
        assert!(result_ready_should_continue(&records));
    }

    #[test]
    fn claimed_tool_guard_stops_before_runtime_on_cancel_or_deadline() {
        let mut claimed = record(LocalCliTurnState::Claimed, "tool request");
        claimed.deadline_at_ms = u64::MAX;
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(matches!(
            ensure_claimed_work_allowed(&claimed, &cancelled),
            Err(ClaimedToolError::Cancelled)
        ));

        claimed.deadline_at_ms = 0;
        assert!(matches!(
            ensure_claimed_work_allowed(&claimed, &CancellationToken::new()),
            Err(ClaimedToolError::Deadline)
        ));
    }

    #[test]
    fn typed_assistant_budget_error_is_terminal_not_retryable() {
        assert_eq!(
            terminal_model_error("模型单步输出超过 512 KiB 安全上限"),
            Some((
                "模型单步输出超过 512 KiB 安全上限，工具调用未执行。",
                "local_cli_assistant_too_large",
            ))
        );
        assert!(terminal_model_error(
            "模型单步输出超过 512 KiB 安全上限; helper cleanup failed: timeout"
        )
        .is_some());
        assert!(terminal_model_error("temporary network failure").is_none());
    }
}
