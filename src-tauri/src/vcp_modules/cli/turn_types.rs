//! Frozen limits and durable DTOs for one local Agent/Group CLI turn.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::result::VcpCliResultEnvelope;

pub const MAX_LOCAL_CLI_TOOL_STEPS: u32 = 8;
pub const MAX_LOCAL_CLI_TURN_WALL_MS: u64 = 30 * 60 * 1_000;
pub const MAX_ASSISTANT_STEP_BYTES: usize = 512 * 1024;
pub const MAX_TOOL_PAYLOAD_BYTES: usize = 256 * 1024;
pub const MAX_RIVER_PROJECTION_BYTES: usize = 128 * 1024;
pub const MAX_RIVER_MESSAGES: usize = 50;
pub const MAX_RIVER_ATTACHMENT_DESCRIPTORS: usize = 64;
pub const MAX_RIVER_ARTIFACTS: usize = 16;
pub const MAX_RIVER_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_RIVER_ARTIFACT_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_CONTINUATION_MESSAGES_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_MARKED_HISTORY_BYTES: usize = 32 * 1024;

pub const ERROR_TURN_STEP_LIMIT: &str = "local_cli_step_limit";
pub const ERROR_TURN_WALL_LIMIT: &str = "local_cli_turn_timeout";
pub const ERROR_ASSISTANT_TOO_LARGE: &str = "local_cli_assistant_too_large";
pub const ERROR_TOOL_PAYLOAD_TOO_LARGE: &str = "local_cli_tool_payload_too_large";
pub const ERROR_CONTINUATION_TOO_LARGE: &str = "local_cli_continuation_too_large";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalCliTurnRoute {
    LocalLoopback,
    VcpPlugin,
}

impl LocalCliTurnRoute {
    pub const fn as_db_value(self) -> &'static str {
        match self {
            Self::LocalLoopback => "local_loopback",
            Self::VcpPlugin => "vcp_plugin",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalCliTurnState {
    Claimed,
    Running,
    ResultReady,
    ContinuationPending,
    Continued,
    Finalizing,
    Terminal,
    Interrupted,
}

impl LocalCliTurnState {
    pub const fn as_db_value(self) -> &'static str {
        match self {
            Self::Claimed => "claimed",
            Self::Running => "running",
            Self::ResultReady => "result_ready",
            Self::ContinuationPending => "continuation_pending",
            Self::Continued => "continued",
            Self::Finalizing => "finalizing",
            Self::Terminal => "terminal",
            Self::Interrupted => "interrupted",
        }
    }

    pub fn from_db_value(value: &str) -> Result<Self, String> {
        match value {
            "claimed" => Ok(Self::Claimed),
            "running" => Ok(Self::Running),
            "result_ready" => Ok(Self::ResultReady),
            "continuation_pending" => Ok(Self::ContinuationPending),
            "continued" => Ok(Self::Continued),
            "finalizing" => Ok(Self::Finalizing),
            "terminal" => Ok(Self::Terminal),
            "interrupted" => Ok(Self::Interrupted),
            _ => Err(format!("unknown local CLI turn state: {value}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct FrozenModelRequest {
    pub messages: Vec<Value>,
    pub model_config: Value,
    pub context: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct LocalCliStepRecord {
    pub model_step_index: u32,
    pub call_index: u32,
    pub tool_digest: String,
    pub operation_id: String,
    pub assistant_content: String,
    pub local_payload: Option<String>,
    pub result: Option<VcpCliResultEnvelope>,
    pub mark_history: bool,
    pub should_continue: bool,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct MarkedHistoryEntry {
    pub model_step_index: u32,
    pub operation_id: String,
    pub block: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct LocalCliTurnRecord {
    pub turn_attempt: String,
    pub outer_message_id: String,
    pub topic_id: String,
    pub owner_id: String,
    pub owner_type: String,
    pub speaker_agent_id: Option<String>,
    pub route: LocalCliTurnRoute,
    pub state: LocalCliTurnState,
    pub step_index: u32,
    pub tool_steps: u32,
    pub started_at_ms: u64,
    pub deadline_at_ms: u64,
    pub updated_at_ms: u64,
    pub version: u64,
    pub frozen_request: FrozenModelRequest,
    pub continuation_messages: Option<Vec<Value>>,
    pub expected_calls: u32,
    pub step_records: Vec<LocalCliStepRecord>,
    pub marked_history: Vec<MarkedHistoryEntry>,
    pub final_content: Option<String>,
    pub terminal_reason: Option<String>,
}

impl LocalCliTurnRecord {
    pub fn transport_request_id(&self) -> String {
        format!(
            "{}:step:{}:{}",
            self.outer_message_id, self.step_index, self.turn_attempt
        )
    }
}

#[derive(Debug, Clone)]
pub struct LocalCliTurnStart {
    pub outer_message_id: String,
    pub topic_id: String,
    pub owner_id: String,
    pub owner_type: String,
    pub speaker_agent_id: Option<String>,
    pub messages: Vec<Value>,
    pub model_config: Value,
    pub context: Option<Value>,
    pub vcp_url: String,
    pub vcp_api_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalCliTurnOutcome {
    Finalized {
        turn_attempt: String,
        highest_step_index: u32,
        content: String,
        finish_reason: String,
        is_aborted: bool,
    },
    ContinuationPending {
        turn_attempt: String,
        step_index: u32,
        reason: String,
    },
    AlreadyTerminal {
        content: String,
        finish_reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_p2_budgets_are_explicit_and_bounded() {
        assert_eq!(MAX_LOCAL_CLI_TOOL_STEPS, 8);
        assert_eq!(MAX_LOCAL_CLI_TURN_WALL_MS, 1_800_000);
        assert_eq!(MAX_ASSISTANT_STEP_BYTES, 524_288);
        assert_eq!(MAX_TOOL_PAYLOAD_BYTES, 262_144);
        assert_eq!(MAX_RIVER_PROJECTION_BYTES, 131_072);
        assert_eq!(MAX_RIVER_MESSAGES, 50);
        assert_eq!(MAX_RIVER_ATTACHMENT_DESCRIPTORS, 64);
        assert_eq!(MAX_RIVER_ARTIFACTS, 16);
        assert_eq!(MAX_RIVER_ARTIFACT_BYTES, 67_108_864);
        assert_eq!(MAX_RIVER_ARTIFACT_TOTAL_BYTES, 268_435_456);
    }

    #[test]
    fn transport_identity_keeps_visible_message_stable_and_step_unique() {
        let record = LocalCliTurnRecord {
            turn_attempt: "attempt-a".to_string(),
            outer_message_id: "msg-visible".to_string(),
            topic_id: "topic".to_string(),
            owner_id: "owner".to_string(),
            owner_type: "agent".to_string(),
            speaker_agent_id: None,
            route: LocalCliTurnRoute::LocalLoopback,
            state: LocalCliTurnState::Running,
            step_index: 3,
            tool_steps: 1,
            started_at_ms: 1,
            deadline_at_ms: 2,
            updated_at_ms: 1,
            version: 0,
            frozen_request: FrozenModelRequest {
                messages: Vec::new(),
                model_config: Value::Null,
                context: None,
            },
            continuation_messages: None,
            expected_calls: 0,
            step_records: Vec::new(),
            marked_history: Vec::new(),
            final_content: None,
            terminal_reason: None,
        };
        assert_eq!(
            record.transport_request_id(),
            "msg-visible:step:3:attempt-a"
        );
    }
}
