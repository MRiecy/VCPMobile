use crate::vcp_modules::agent_types::AgentConfig;
use crate::vcp_modules::chat_manager::ChatMessage;
use serde::{Deserialize, Serialize};

/// 预计算的消息外壳属性
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageShell {
    pub avatar_color: String,
    pub display_name: String,
    pub is_user: bool,
}

/// 根据消息元数据和 agents 配置预计算外壳
pub fn precompute_shell(
    message: &ChatMessage,
    agents: &[AgentConfig],
    user_name: &str,
    user_avatar_color: Option<&str>,
) -> MessageShell {
    let is_user = message.role == "user";

    if is_user {
        let user_color = user_avatar_color.unwrap_or("rgb(226,54,56)");
        return MessageShell {
            avatar_color: user_color.to_string(),
            display_name: user_name.to_string(),
            is_user: true,
        };
    }

    // 查找 agent 配置
    let agent = message
        .agent_id
        .as_ref()
        .and_then(|id| agents.iter().find(|a| a.id == *id))
        .or_else(|| {
            message
                .name
                .as_ref()
                .and_then(|name| agents.iter().find(|a| a.name == *name))
        });

    let color = agent
        .and_then(|a| a.avatar_calculated_color.clone())
        .unwrap_or_default();

    let display_name = message
        .name
        .clone()
        .or_else(|| agent.map(|a| a.name.clone()))
        .unwrap_or_else(|| "AI".to_string());

    MessageShell {
        avatar_color: color,
        display_name,
        is_user: false,
    }
}
