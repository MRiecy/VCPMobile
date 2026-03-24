// GroupOrchestrator: 群组决策引擎与上下文组装器
// 职责: 1. 运行 NatureRandom 算法 2. 注入群组全局设定 3. 处理 SessionWatcher 动态感知

use crate::vcp_modules::agent_config_manager::AgentConfig;
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::group_manager::GroupConfig;
use rand::Rng;
use regex::Regex;
use serde_json::json;

/// NatureRandom 决策引擎
pub fn determine_naturerandom_speakers(
    active_members: &[AgentConfig],
    history: &[ChatMessage],
    group_config: &GroupConfig,
    user_message: &ChatMessage,
) -> Vec<AgentConfig> {
    let mut speakers = Vec::new();
    let mut spoken_this_turn = std::collections::HashSet::new();

    let user_text = user_message.content.to_lowercase();

    // 基础上下文窗口 (对齐桌面端 CONTEXT_WINDOW = 8)
    const CONTEXT_WINDOW: usize = 8;
    let history_len = history.len();
    let start_idx = history_len.saturating_sub(CONTEXT_WINDOW + 1);
    // 排除最后一条 (当前用户消息)
    let end_idx = history_len.saturating_sub(1);

    let recent_history = if start_idx < end_idx {
        &history[start_idx..end_idx]
    } else {
        &[]
    };

    let context_text = recent_history
        .iter()
        .map(|m| {
            // 简单去除发言头 [Nova的发言]:
            let re = Regex::new(r"^\[.*?的发言\]:\s*").unwrap();
            re.replace(&m.content, "").to_lowercase()
        })
        .collect::<Vec<_>>()
        .join(" \n ");

    let tag_match_mode = group_config.tag_match_mode.as_deref().unwrap_or("strict");

    // 1. 优先级: @角色名
    for member in active_members {
        if user_text.contains(&format!("@{}", member.name.to_lowercase()))
            && spoken_this_turn.insert(member.id.clone())
        {
            speakers.push(member.clone());
            println!(
                "[GroupOrchestrator] @{} triggered by direct mention.",
                member.name
            );
        }
    }

    // 2. 优先级: Tag 匹配
    let tag_re = Regex::new(r"[,，]").unwrap();
    let name_prefix_re = Regex::new(r"^\[.*?的发言\]:\s*").unwrap();
    for member in active_members {
        if spoken_this_turn.contains(&member.id) {
            continue;
        }

        let tags_val = group_config
            .member_tags
            .as_ref()
            .and_then(|t| t.get(&member.id))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if tags_val.is_empty() {
            continue;
        }
        let tags: Vec<&str> = tag_re
            .split(tags_val)
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
            .collect();

        if tag_match_mode == "natural" {
            // Natural 模式: 分档逻辑
            let tag_in_others = recent_history
                .iter()
                .filter(|m| m.extra.get("agentId").and_then(|id| id.as_str()) != Some(&member.id))
                .any(|m| {
                    let clean_content = name_prefix_re.replace(&m.content, "").to_lowercase();
                    tags.iter()
                        .any(|t| clean_content.contains(&t.to_lowercase()))
                })
                || tags.iter().any(|t| user_text.contains(&t.to_lowercase()));

            if tags
                .iter()
                .any(|t| user_text.contains(&format!("@{}", t.to_lowercase())))
                || tag_in_others
            {
                speakers.push(member.clone());
                spoken_this_turn.insert(member.id.clone());
            } else {
                // 仅出现在自身历史消息 -> 动态概率
                let tag_in_own = recent_history
                    .iter()
                    .filter(|m| {
                        m.extra.get("agentId").and_then(|id| id.as_str()) == Some(&member.id)
                    })
                    .any(|m| {
                        tags.iter()
                            .any(|t| m.content.to_lowercase().contains(&t.to_lowercase()))
                    });

                if tag_in_own {
                    let last_ai_msg = history
                        .iter()
                        .rev()
                        .skip(1) // 跳过当前用户消息
                        .find(|m| m.role == "assistant");

                    let is_last_speaker = last_ai_msg
                        .and_then(|m| m.extra.get("agentId"))
                        .and_then(|id| id.as_str())
                        == Some(&member.id);
                    let own_msg_count = recent_history
                        .iter()
                        .filter(|m| {
                            m.extra.get("agentId").and_then(|id| id.as_str()) == Some(&member.id)
                        })
                        .count();

                    let speak_chance = if is_last_speaker {
                        (0.5 + (own_msg_count as f32 * 0.1)).min(0.75)
                    } else {
                        0.2
                    };

                    if rand::thread_rng().gen::<f32>() < speak_chance {
                        speakers.push(member.clone());
                        spoken_this_turn.insert(member.id.clone());
                    }
                }
            }
        } else {
            // Strict 模式: 包含即触发
            if tags.iter().any(|t| {
                context_text.contains(&t.to_lowercase()) || user_text.contains(&t.to_lowercase())
            }) {
                speakers.push(member.clone());
                spoken_this_turn.insert(member.id.clone());
            }
        }
    }

    // 3. 优先级: @所有人
    if user_text.contains("@所有人") {
        for member in active_members {
            if spoken_this_turn.insert(member.id.clone()) {
                speakers.push(member.clone());
            }
        }
    }

    // 4. 优先级: 概率发言 (15%)
    let mut rng = rand::thread_rng();
    for member in active_members {
        if spoken_this_turn.contains(&member.id) {
            continue;
        }

        let mut speak_chance = 0.15;
        if tag_match_mode == "strict" {
            let tags_val = group_config
                .member_tags
                .as_ref()
                .and_then(|t| t.get(&member.id))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !tags_val.is_empty() {
                let tags: Vec<&str> = tag_re.split(tags_val).map(|t| t.trim()).collect();
                if tags
                    .iter()
                    .any(|t| context_text.contains(&t.to_lowercase()))
                {
                    speak_chance = 0.85;
                }
            }
        }

        if rng.gen::<f32>() < speak_chance {
            speakers.push(member.clone());
            spoken_this_turn.insert(member.id.clone());
        }
    }

    // 5. 优先级: 保底发言
    if speakers.is_empty() && !active_members.is_empty() {
        let relevant_members: Vec<_> = active_members
            .iter()
            .filter(|m| {
                let tags_val = group_config
                    .member_tags
                    .as_ref()
                    .and_then(|t| t.get(&m.id))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if tags_val.is_empty() {
                    return false;
                }
                let tags: Vec<&str> = tag_re.split(tags_val).map(|t| t.trim()).collect();
                tags.iter()
                    .any(|t| context_text.contains(&t.to_lowercase()))
            })
            .collect();

        let fallback = if !relevant_members.is_empty() {
            relevant_members[rng.gen_range(0..relevant_members.len())].clone()
        } else {
            active_members[rng.gen_range(0..active_members.len())].clone()
        };
        speakers.push(fallback);
    }

    // 排序优化: 用户最新发言命中的排在最前
    speakers.sort_by_cached_key(|member| {
        let tags_val = group_config
            .member_tags
            .as_ref()
            .and_then(|t| t.get(&member.id))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let tags: Vec<&str> = tag_re.split(tags_val).map(|t| t.trim()).collect();

        if tags.iter().any(|t| user_text.contains(&t.to_lowercase())) {
            0 // Rank 0 (highest)
        } else if tags
            .iter()
            .any(|t| context_text.contains(&t.to_lowercase()))
        {
            1 // Rank 1
        } else {
            2 // Rank 2
        }
    });

    speakers
}

/// 上下文组装器: 合并 Agent 设定与群组设定
pub async fn assemble_context(
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
