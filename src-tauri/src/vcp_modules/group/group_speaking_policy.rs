use crate::vcp_modules::agent_types::AgentConfig;
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::group_types::{GroupConfig, JevFallbackPolicy};
use rand::Rng;
use regex::Regex;
use crate::vcp_modules::group::{
    group_session::GroupDecisionContext,
    jev_protocol::{JevDecisionRequest, JevDecisionResponse, JevQuestion},
};
use std::collections::BTreeMap;

pub(crate) struct GroupMemberHistoryStats {
    pub speech_count: i64,
    pub last_speech_distance: i64,
}

pub(crate) fn determine_sequential_speakers(
    members: &[AgentConfig],
    group: &GroupConfig,
) -> Vec<AgentConfig> {
    let order = &group.local_settings.mode_settings.sequential.speaker_order;
    let mut speakers = members.to_vec();
    speakers.sort_by_key(|member| {
        order
            .iter()
            .position(|id| id == &member.id)
            .unwrap_or(usize::MAX)
    });
    speakers
}

pub(crate) fn build_jev_request(
    members: &[AgentConfig],
    history: &[ChatMessage],
    group: &GroupConfig,
    runtime: &GroupDecisionContext,
    history_stats: &BTreeMap<String, GroupMemberHistoryStats>,
) -> JevDecisionRequest {
    let settings = &group.local_settings.mode_settings.jev;
    let mut criteria = BTreeMap::new();
    for (index, member) in members.iter().enumerate() {
        let style = settings
            .member_styles
            .get(&member.id)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("未配置特殊触发风格；根据上下文相关性判断。");
        criteria.insert(
            format!("agent_{index}"),
            format!("{}：{style}", member.name),
        );
    }
    criteria.insert(
        "end_conversation".into(),
        "当前话题已经自然结束、问题已解决，或继续发言只会重复已有内容。".into(),
    );
    let mut questions = BTreeMap::new();
    questions.insert(
        "speaker_routing".into(),
        JevQuestion::Choice {
            instructions: settings.navigator_prompt.clone(),
            criteria,
        },
    );
    questions.insert(
        "continue_discussion".into(),
        JevQuestion::Noul {
            instructions:
                "基于当前对话，继续由群聊成员自主推进是否仍然自然、有价值且不会只是重复？".into(),
        },
    );
    JevDecisionRequest {
        state: serde_json::json!({
            "group": { "id": group.id, "name": group.name, "prompt": group.group_prompt.as_deref().unwrap_or("") },
            "recentMessages": history.iter().map(|message| serde_json::json!({
                "id": message.id, "role": message.role, "speakerName": message.name.as_deref().unwrap_or(if message.role == "user" { "用户" } else { "AI" }),
                "agentId": message.agent_id, "text": message.content, "timestamp": message.timestamp,
            })).collect::<Vec<_>>(),
            "members": members.iter().map(|member| serde_json::json!({
                "agentId": member.id, "name": member.name,
                "triggerStyle": settings.member_styles.get(&member.id).map(String::as_str).unwrap_or(""),
                "messagesSinceLastSpeech": history_stats.get(&member.id).map(|stats| stats.last_speech_distance),
                "totalSpeechCount": history_stats.get(&member.id).map(|stats| stats.speech_count).unwrap_or(0),
                "autonomousRunSpeechCount": runtime.counts.get(&member.id).copied().unwrap_or(0),
                "manuallyQueued": false,
            })).collect::<Vec<_>>(),
            "runtime": { "trigger": runtime.trigger, "autonomousRound": runtime.rounds + 1,
                "forceContinue": runtime.force_continue, "latestUserMessageId": runtime.scope.as_ref().map(|scope| &scope.origin) }
        }),
        questions,
    }
}

pub(crate) fn parse_jev_decision(
    response: &JevDecisionResponse,
    request: &JevDecisionRequest,
    members: &[AgentConfig],
    group: &GroupConfig,
    force_continue: bool,
) -> Result<(Vec<String>, Option<String>), String> {
    response.validate_for(request).map_err(str::to_string)?;
    let settings = &group.local_settings.mode_settings.jev;
    let routing = response
        .answers
        .get("speaker_routing")
        .and_then(|answer| answer.probabilities.as_ref())
        .ok_or("JEV 缺少发言概率")?;
    let continuation = response
        .answers
        .get("continue_discussion")
        .and_then(|answer| answer.noul)
        .ok_or("JEV 缺少继续概率")?;
    let mut ranked = members
        .iter()
        .enumerate()
        .map(|(index, member)| {
            routing
                .get(&format!("agent_{index}"))
                .copied()
                .map(|probability| (member.id.clone(), probability))
                .ok_or("JEV 缺少成员概率")
        })
        .collect::<Result<Vec<_>, _>>()?;
    ranked.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let end = routing
        .get("end_conversation")
        .copied()
        .ok_or("JEV 缺少结束概率")?;
    if !force_continue && continuation < settings.continue_threshold {
        return Ok((vec![], Some("continue_probability_below_threshold".into())));
    }
    if !force_continue
        && end >= settings.stop_threshold
        && end >= ranked.first().map(|(_, value)| *value).unwrap_or(0.0)
    {
        return Ok((vec![], Some("end_option_selected".into())));
    }
    let mut selected = ranked
        .iter()
        .filter(|(_, value)| *value >= settings.speaker_threshold)
        .take(settings.max_speakers_per_round as usize)
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    if selected.is_empty() && (force_continue || settings.fallback_policy == JevFallbackPolicy::TopSpeaker) {
        if let Some((id, _)) = ranked.first() {
            selected.push(id.clone());
        }
    }
    let reason = selected
        .is_empty()
        .then(|| "no_speaker_above_threshold".to_string());
    Ok((selected, reason))
}

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
            log::info!(
                "[GroupSpeakingPolicy] @{} triggered by direct mention.",
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
            .map(String::as_str)
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
                .filter(|m| m.agent_id.as_deref() != Some(&member.id))
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
                    .filter(|m| m.agent_id.as_deref() == Some(&member.id))
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

                    let is_last_speaker =
                        last_ai_msg.and_then(|m| m.agent_id.as_deref()) == Some(&member.id);
                    let own_msg_count = recent_history
                        .iter()
                        .filter(|m| m.agent_id.as_deref() == Some(&member.id))
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
                .map(String::as_str)
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
                    .map(String::as_str)
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
            .map(String::as_str)
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
