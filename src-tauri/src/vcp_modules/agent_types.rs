use crate::vcp_modules::topic_types::Topic;
use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

/// 智能体(Agent)的完整配置结构
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    /// 智能体 ID
    #[serde(default)]
    pub id: String,
    /// 智能体名称
    #[serde(default = "default_agent_name")]
    pub name: String,
    /// 系统提示词 (System Prompt)
    #[serde(default)]
    pub system_prompt: String,
    /// 使用的模型 (如: "gemini-2.0-flash")
    #[serde(default = "default_model")]
    pub model: String,
    /// 模型采样温度 (0.0-2.0)
    #[serde(default = "default_temperature")]
    pub temperature: f64,
    /// 上下文 Token 限制
    #[serde(default = "default_context_limit")]
    pub context_token_limit: i32,
    /// 单次输出最大 Token 数
    #[serde(default = "default_max_output")]
    pub max_output_tokens: i32,

    #[serde(default = "default_true")]
    pub stream_output: bool,

    // avatars 表派生字段
    #[serde(default)]
    pub avatar_calculated_color: Option<String>,

    /// 话题列表
    #[serde(default)]
    pub topics: Vec<Topic>,

    /// 当前活跃话题 ID
    #[serde(default)]
    pub current_topic_id: Option<String>,
}

fn default_agent_name() -> String {
    "Unnamed Agent".to_string()
}
fn default_model() -> String {
    "gemini-2.5-flash".to_string()
}
fn default_temperature() -> f64 {
    1.0
}
fn default_context_limit() -> i32 {
    1000000
}
fn default_max_output() -> i32 {
    64000
}
