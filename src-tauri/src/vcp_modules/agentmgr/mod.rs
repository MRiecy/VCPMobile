//! Agent 管理（AgentAssistant agents 配置 CRUD）：远程管理 VCPToolBox 的 Agent 定义。
//!
//! 上游契约：`/admin_api/agent-assistant/config`（顶层浅合并）与 `/admin_api/ai/models`。
//! 详见 `plan/vcpmobile-more-tools-research/08-Agent管理-上游契约与移植方案.md`。

mod agent_service;

pub use agent_service::{agentmgr_get_config, agentmgr_list_models, agentmgr_save_config};
