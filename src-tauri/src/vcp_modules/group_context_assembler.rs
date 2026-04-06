use crate::vcp_modules::agent_types::AgentConfig;
use crate::vcp_modules::group_types::GroupConfig;
use serde_json::json;

/// 上下文组装器: 合并 Agent 设定与群组设定
pub async fn assemble_group_context(
    agent_config: &AgentConfig,
    group_config: &GroupConfig,
    active_members: &[AgentConfig],
) -> String {
    let agent_name = if agent_config.name.is_empty() {
        &agent_config.id
    } else {
        &agent_config.name
    };
    let mut system_prompt = agent_config.system_prompt.clone();

    // 注入群组全局设定
    if let Some(group_prompt) = &group_config.group_prompt {
        let mut final_group_prompt = group_prompt.clone();

        // 处理 SessionWatcher 感知占位符
        if final_group_prompt.contains("{{VCPChatGroupSessionWatcher}}") {
            let session_info = json!({
                "groupId": group_config.id,
                "groupName": group_config.name,
                "activeMembers": active_members.iter().map(|m| {
                    json!({
                        "id": m.id,
                        "name": m.name,
                        "model": m.model
                    })
                }).collect::<Vec<_>>()
            });
            final_group_prompt = final_group_prompt
                .replace("{{VCPChatGroupSessionWatcher}}", &session_info.to_string());
        }

        system_prompt = format!("{}\n\n[群聊设定]:\n{}", system_prompt, final_group_prompt);
    }

    // 注入邀请发言逻辑
    if let Some(invite_prompt) = &group_config.invite_prompt {
        let processed_invite = invite_prompt.replace("{{VCPChatAgentName}}", agent_name);
        system_prompt = format!("{}\n\n[指令]:\n{}", system_prompt, processed_invite);
    }

    system_prompt
}
