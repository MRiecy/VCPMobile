//! 任务调度中心（VCP Task Assistant）：远程调度 VCPToolBox 的 Agent 自动任务。
//!
//! 上游契约：`/admin_api/task-assistant/*` 与 `/admin_api/agent-assistant/delegations/*`。
//! 详见 `plan/vcpmobile-more-tools-research/02-任务调度中心-上游契约与移植方案.md`。

mod task_service;

pub use task_service::{
    delegation_cancel, delegation_list, task_agent_list, task_create, task_delete, task_get_config,
    task_get_status, task_set_enabled, task_set_global_enabled, task_trigger, task_update,
};
