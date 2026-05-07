use crate::vcp_modules::agent_types::AgentConfig;
use crate::vcp_modules::chat_manager::ChatMessage;
use serde::{Deserialize, Serialize};

/// 预计算的消息外壳属性
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageShell {
    pub avatar_color: String,
    pub bubble_border_color: String,
    pub bubble_box_shadow: String,
    pub display_name: String,
    pub avatar_fallback_text: String,
    pub avatar_fallback_color: String,
    pub is_user: bool,
}

/// 根据消息元数据和 agents 配置预计算外壳
pub fn precompute_shell(
    message: &ChatMessage,
    agents: &[AgentConfig],
    user_name: &str,
) -> MessageShell {
    let is_user = message.role == "user";

    if is_user {
        return MessageShell {
            avatar_color: "rgb(226,54,56)".to_string(),
            bubble_border_color: "transparent".to_string(),
            bubble_box_shadow: "none".to_string(),
            display_name: user_name.to_string(),
            avatar_fallback_text: user_name.chars().next().unwrap_or('U').to_string(),
            avatar_fallback_color: "rgb(226,54,56)".to_string(),
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
        .unwrap_or_else(|| "#374151".to_string());

    let display_name = message
        .name
        .clone()
        .or_else(|| agent.map(|a| a.name.clone()))
        .unwrap_or_else(|| "AI".to_string());

    MessageShell {
        avatar_color: color.clone(),
        bubble_border_color: format!("{}30", color), // 18% opacity approx in hex is 2E or 30
        bubble_box_shadow: format!("0 4px 12px {}15", color), // 8% opacity approx in hex is 14 or 15
        display_name: display_name.clone(),
        avatar_fallback_text: display_name.chars().next().unwrap_or('A').to_string(),
        avatar_fallback_color: color,
        is_user: false,
    }
}
