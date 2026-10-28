use crate::vcp_modules::chat::voice_service::VoiceReplyPolicy;
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::topic_types::TopicKey;
use serde_json::{json, Value};

// =================================================================
// vcp_modules/chat/context_assembler.rs - 上下文级联装配中枢
// =================================================================
// 本模块承载了整个 VCP 大模型会话上下文注入的核心生命周期：
// 1. 【微观编织阶段】(assemble_history_for_vcp)：逐条迭代 SQLite 强类型 ChatMessage，
//    将分钟级时间戳 (带 \n 物理 Token 防火墙) 以及发言人前缀消歧编织进每条消息正文。
// 2. 【宏观拦截阶段】(apply_tarven_pipeline)：针对已序列化好的 messages 列表，
//    进行 System Metadata 环境真理注入、System/User Tavern 规则终极前后拼接拼接与虚拟节点插入。
// 3. 【统一装配外观】(orchestrate_chat_context)：向单聊与群聊业务模块提供极度纯净的 Facade 入口。

/// 统一上下文级联装配外观入口 (Facade Orchestrator)
pub(crate) async fn orchestrate_chat_context(
    db: &DbState,
    history: &[ChatMessage],
    topic: &TopicKey,
    agent_name: &str,
    scope: &str, // "agent" | "group"
    base_system_prompt: String,
    invite_prompt: Option<String>,
    reply_policy: Option<&VoiceReplyPolicy>,
) -> Result<Vec<Value>, String> {
    // One normalized snapshot drives both time anchoring and rule injection.
    let rules = crate::vcp_modules::chat::context_injection::fetch_active_rules(db, scope).await?;
    let enable_time_anchoring = rules.iter().any(|rule| rule.id == "time_anchoring_v2");

    // 2. 第一阶段：微观编织。进行强类型的发言人前缀及物理 Token 换行符时间隔离注入
    let is_group = scope == "group";
    let mut messages = assemble_history_for_vcp(history, is_group, enable_time_anchoring);

    // 3. 如果是群聊且存在主动邀请词 (Invite Prompt)，将其作为最新一轮用户消息拼装，以接受后续 Tavern 规则注入
    if let Some(invite) = invite_prompt {
        if !invite.is_empty() {
            messages.push(json!({
                "role": "user",
                "content": invite
            }));
        }
    }

    // 4. 将基础的 System Prompt 注入 Payload 首部
    if !base_system_prompt.is_empty() {
        messages.insert(
            0,
            json!({
                "role": "system",
                "content": base_system_prompt
            }),
        );
    }

    // 5. 第二阶段：宏观拦截。调用 Tavern 拦截器流水线进行环境真理及 System/User 规则的终极拼装
    crate::vcp_modules::chat::context_injection::apply_tarven_pipeline(
        &db.pool,
        topic,
        agent_name,
        &rules,
        &mut messages,
    )
    .await?;

    // A call owns this request's immutable policy. Append after every Tavern
    // rule. The user's rules are the entire final, non-persisted node, even blank.
    if let Some(policy) = reply_policy {
        messages.push(json!({ "role": "user", "content": policy.guidance_rules }));
    }

    Ok(messages)
}

fn bounded_attachment_name(value: &str) -> String {
    let basename = value.rsplit(['/', '\\']).next().unwrap_or_default();
    let filtered = basename
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    let trimmed = filtered.trim();
    if trimmed.is_empty() {
        return "附件".to_string();
    }
    let mut end = trimmed.len().min(256);
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    trimmed[..end].to_string()
}

/// =================================================================
/// 🌌 微观历史记录编织器 (assemble_history_for_vcp)
/// =================================================================
/// 该函数负责把扁平的、面向 SQLite 的强类型 ChatMessage 关系数据结构，
/// 降维并映射为符合大模型 (LLM) Chat Completion API 规范的多模态 JSON Payload。
///
/// 🛡️ 双重换行物理防火墙 (BPE Token Barrier) 设计：
/// -------------------------------------------------------------
/// 为了防范 LLM 的 BPE 分词器 (Tokenizer) 将 "元数据前缀" 与 "消息正文" 的首个单词
/// 强行融合成单个不可预知的 Token，从而导致指示词语义降级甚至产生幻觉，
/// 我们在 "发言人消歧元数据" 与 "消息内容正文" 之间，
/// 强行硬编码级联插入了物理换行符 `\n`。这在字节层面上彻底切断了前缀与正文的融合通道。
///
/// 格式示意：
/// [Sender的发言]:\n                <--- 物理换行：阻断发言人与正文特征融合
/// 这是正文内容...
///
/// 📂 附件/多模态与内联物理隔离逻辑：
/// -------------------------------------------------------------
/// 1. 【文档类提取】：若附件（如 PDF、DOCX、TXT 等）已被 Rust 底层流水线提取为文本 `extracted_text`，
///    将以极其工整的形式通过内联闭环标签嵌入到文本尾部：
///    `\n\n[附加文件] (文件名: {name})\n{text}\n[/附加文件结束: {name}]`
/// 2. 【多模态富资产】：图片、音频或视频只编译为带 CAS hash/MIME 的 `local_file` 描述；
///    host path 不进入模型消息。底层 VCP Client 按数据库 CAS 事实解析，River full 则生成 attempt copy。
pub fn assemble_history_for_vcp(
    history: &[ChatMessage],
    is_group: bool,
    enable_time_anchoring: bool,
) -> Vec<Value> {
    let mut result = Vec::new();

    for msg in history
        .iter()
        .filter(|msg| !msg.is_thinking.unwrap_or(false))
    {
        use chrono::TimeZone;
        let formatted_time = if let Some(dt) = chrono::Local
            .timestamp_millis_opt(msg.timestamp as i64)
            .single()
        {
            dt.format("%Y-%m-%d %H:%M").to_string()
        } else {
            chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
        };

        let mut combined_text = String::new();

        // 2. 发言人消歧前缀 (元数据 B) + 物理换行 2
        if is_group {
            let speaker_name = msg
                .name
                .as_ref()
                .filter(|name| !name.is_empty())
                .cloned()
                .unwrap_or_else(|| {
                    if msg.role == "user" {
                        "User".to_string()
                    } else {
                        "AI".to_string()
                    }
                });
            combined_text.push_str(&format!("[{}的发言]:\n", speaker_name));
        }

        // 3. 核心消息正文
        combined_text.push_str(&msg.content);

        let mut content_parts = Vec::new();

        if let Some(attachments) = &msg.attachments {
            for att in attachments {
                let attachment_name = bounded_attachment_name(&att.name);
                let is_desktop_only = att.status.as_deref() == Some("desktop_only");
                let valid_hash = att
                    .hash
                    .as_deref()
                    .filter(|hash| crate::vcp_modules::infra::utils::is_valid_cas_hash(hash));
                let is_local_ready =
                    matches!(att.status.as_deref(), Some("ready" | "done")) && valid_hash.is_some();
                // 1. 处理提取的文本内容 (文档类)
                if let Some(text) = &att.extracted_text {
                    if !text.is_empty() {
                        if is_desktop_only {
                            combined_text.push_str(&format!(
                                "\n\n[桌面附件文本: {}]\n{}\n[/桌面附件文本结束: {}]",
                                attachment_name, text, attachment_name
                            ));
                        } else {
                            combined_text.push_str(&format!(
                                "\n\n[附加文件] (文件名: {})\n{}\n[/附加文件结束: {}]",
                                attachment_name, text, attachment_name
                            ));
                        }
                    }
                }

                if is_desktop_only {
                    if att.extracted_text.as_deref().unwrap_or_default().is_empty() {
                        combined_text.push_str(&format!(
                            "\n\n[桌面专用附件: {}，内容未同步至移动端]",
                            attachment_name
                        ));
                    }
                    continue;
                }

                // 2. 处理多模态文件 (图片/音频/视频)
                let mime = &att.r#type;
                let is_image = mime.starts_with("image/");
                let is_audio = mime.starts_with("audio/");
                let is_video = mime.starts_with("video/");

                if (is_image || is_audio || is_video) && is_local_ready {
                    if is_image {
                        combined_text
                            .push_str(&format!("\n\n[附加图片] (文件名: {})", attachment_name));
                    } else {
                        combined_text
                            .push_str(&format!("\n\n[附加文件] (文件名: {})", attachment_name));
                    }

                    content_parts.push(json!({
                        "type": "local_file",
                        "hash": valid_hash,
                        "name": attachment_name,
                        "mime": mime,
                        "size_bytes": att.size
                    }));
                } else if att.extracted_text.is_none()
                    && matches!(att.status.as_deref(), Some("ready" | "done"))
                {
                    combined_text.push_str(&format!(
                        "\n\n[附加文件] (文件名: {}；本机 CAS 引用不可用)",
                        attachment_name
                    ));
                }
            }
        }

        // 4. 新版时间锚定机制 (元数据 A - 伪系统/user内联块格式)
        if enable_time_anchoring && msg.role == "user" {
            // 对于 user 消息块，直接在内部注入
            let username = msg
                .name
                .as_deref()
                .filter(|n| !n.is_empty())
                .unwrap_or("User");
            combined_text.push_str(&format!(
                "\n<system_meta>[系统提示]：{}发送于{}.</system_meta>",
                username, formatted_time
            ));
        }

        if !combined_text.trim().is_empty() {
            content_parts.insert(
                0,
                json!({
                    "type": "text",
                    "text": combined_text
                }),
            );
        }

        let final_content = if content_parts.is_empty() {
            Value::String(String::new())
        } else if content_parts.len() == 1 && content_parts[0]["type"] == "text" {
            content_parts[0]["text"].clone()
        } else {
            json!(content_parts)
        };

        let mut val = json!({
            "role": msg.role,
            "name": msg.name,
            "content": final_content
        });
        if !msg.id.is_empty() {
            val["__vcpchatTimestampMeta"] = json!({
                "messageId": msg.id,
                "role": msg.role,
                "timestamp": msg.timestamp,
                "contentHash": msg.content_hash
            });
        }
        // 将当前消息推入结果列表
        result.push(val);

        // 如果是 非user 消息且启用了时间锚定，在后面追加一个伪系统 user 块
        if enable_time_anchoring && msg.role != "user" {
            let agent_name = msg
                .name
                .as_deref()
                .filter(|n| !n.is_empty())
                .unwrap_or("AI");
            let pseudo_user_msg = json!({
                "role": "user",
                "content": format!(
                    "<system_meta>[系统提示]：上条消息由{}发送于{}.</system_meta>",
                    agent_name, formatted_time
                )
            });
            result.push(pseudo_user_msg);
        }
    }

    result
}

#[cfg(test)]
mod voice_policy_tests {
    use super::*;
    use crate::vcp_modules::settings_manager::{
        Settings, VoiceConfig, VoiceGuidanceConfig, VoiceTtsConfig, VoiceTtsMode,
    };

    async fn pool_with_tail_rule() -> DbState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../../../migrations/0100_baseline_v2.sql"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO tarven_rules
             (id, name, rule_type, is_enabled, content, scope, wrap, role, depth,
              sort_order, created_at, updated_at)
             VALUES ('tail', 'tail', 'context_inject', 1, 'Tavern tail', 'global',
                     0, 'user', 0, 1, 1, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        DbState::new(pool, std::path::PathBuf::new())
    }

    fn policy(mode: VoiceTtsMode, rules: &str) -> VoiceReplyPolicy {
        VoiceReplyPolicy::from_settings(&Settings {
            voice: VoiceConfig {
                tts: VoiceTtsConfig {
                    mode,
                    ..Default::default()
                },
                guidance: VoiceGuidanceConfig {
                    rules: rules.into(),
                },
                ..Default::default()
            },
            ..Settings::default()
        })
    }

    #[tokio::test]
    async fn voice_rules_are_the_exact_request_only_node_after_tavern_and_group_invite() {
        let pool = pool_with_tail_rule().await;
        let history = vec![ChatMessage {
            id: "origin".into(),
            role: "user".into(),
            content: "用户原话".into(),
            ..Default::default()
        }];
        for (mode, scope, invite) in [
            (VoiceTtsMode::Builtin, "agent", None),
            (VoiceTtsMode::Builtin, "group", Some("请成员发言".into())),
            (VoiceTtsMode::Clone, "agent", None),
            (VoiceTtsMode::Clone, "group", Some("请成员发言".into())),
        ] {
            let rules = "  用户自定义风格规则\n只回答正文，不生成标签。\n";
            let reply_policy = policy(mode, rules);
            let topic = TopicKey::new(scope, "owner", "topic");
            let messages = orchestrate_chat_context(
                &pool,
                &history,
                &topic,
                "成员",
                scope,
                "角色规则".into(),
                invite.clone(),
                Some(&reply_policy),
            )
            .await
            .unwrap();
            let last = messages.last().unwrap();
            assert_eq!(last["role"], "user");
            let content = last["content"].as_str().unwrap();
            assert_eq!(content, rules);
            assert_eq!(messages[messages.len() - 2]["content"], "Tavern tail");
            assert!(last.get("__vcpchatTimestampMeta").is_none());

            let ordinary = orchestrate_chat_context(
                &pool,
                &history,
                &topic,
                "成员",
                scope,
                "角色规则".into(),
                invite,
                None,
            )
            .await
            .unwrap();
            assert_eq!(&messages[..messages.len() - 1], ordinary.as_slice());
        }
    }

    #[tokio::test]
    async fn blank_voice_rules_are_injected_without_defaults_or_protocol_instructions() {
        let pool = pool_with_tail_rule().await;
        let topic = TopicKey::new("agent", "owner", "topic");
        let ordinary = orchestrate_chat_context(
            &pool,
            &[],
            &topic,
            "成员",
            "agent",
            String::new(),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(ordinary.last().unwrap()["content"], "Tavern tail");
        for rules in ["", " \n\t", "占位中"] {
            let reply_policy = policy(VoiceTtsMode::Builtin, rules);
            let messages = orchestrate_chat_context(
                &pool,
                &[],
                &topic,
                "成员",
                "agent",
                String::new(),
                None,
                Some(&reply_policy),
            )
            .await
            .unwrap();
            assert_eq!(&messages[..messages.len() - 1], ordinary.as_slice());
            assert_eq!(messages.last().unwrap(), &json!({"role":"user", "content":rules}));
        }
    }

    #[test]
    fn history_preserves_guidance_and_body_verbatim_for_both_roles() {
        let block = "<voice_guidance>\n低声轻柔\n</voice_guidance>\n";
        let source = format!("{block}[轻笑]保留正文与[链接](https://example.invalid)。");
        let mut assistant = ChatMessage {
            role: "assistant".into(),
            content: source.clone(),
            ..Default::default()
        };
        let user = ChatMessage {
            role: "user".into(),
            content: source.clone(),
            ..Default::default()
        };
        let projected = assemble_history_for_vcp(&[assistant.clone(), user], false, false);
        assert_eq!(projected[0]["content"], source);
        assert_eq!(projected[1]["content"], source);

        assistant.content = format!("```text\n{block}```\n> 引用中的普通标签 [叹气]");
        let example = assemble_history_for_vcp(&[assistant.clone()], false, false);
        assert_eq!(example[0]["content"], assistant.content);
    }
}

#[cfg(test)]
mod desktop_only_tests {
    use super::assemble_history_for_vcp;
    use crate::vcp_modules::chat_manager::{Attachment, ChatMessage};

    fn message_with_attachment(attachment: Attachment) -> ChatMessage {
        ChatMessage {
            id: "message".into(),
            role: "user".into(),
            content: "hello".into(),
            timestamp: 1,
            attachments: Some(vec![attachment]),
            ..Default::default()
        }
    }

    #[test]
    fn desktop_only_attachment_keeps_text_without_creating_local_file() {
        let message = message_with_attachment(Attachment {
            r#type: "image/png".into(),
            name: "desktop.png".into(),
            hash: Some("a".repeat(64)),
            status: Some("desktop_only".into()),
            extracted_text: Some("可信提取文本".into()),
            ..Default::default()
        });

        let assembled = assemble_history_for_vcp(&[message], false, false);
        let serialized = serde_json::to_string(&assembled).expect("serialize context");
        assert!(serialized.contains("桌面附件文本"));
        assert!(serialized.contains("可信提取文本"));
        assert!(!serialized.contains("local_file"));
        assert!(!serialized.contains("file://"));
    }

    #[test]
    fn ready_media_uses_hash_metadata_without_exposing_a_host_path() {
        let empty = message_with_attachment(Attachment {
            r#type: "image/png".into(),
            name: "missing.png".into(),
            status: Some("ready".into()),
            ..Default::default()
        });
        let ready = message_with_attachment(Attachment {
            r#type: "image/png".into(),
            name: "/private/attachments/ready.png".into(),
            status: Some("ready".into()),
            hash: Some("b".repeat(64)),
            internal_path: "/private/attachments/secret.png".into(),
            src: "file:///private/attachments/secret.png".into(),
            ..Default::default()
        });

        let empty_json = serde_json::to_string(&assemble_history_for_vcp(&[empty], false, false))
            .expect("serialize empty context");
        let ready_json = serde_json::to_string(&assemble_history_for_vcp(&[ready], false, false))
            .expect("serialize ready context");
        assert!(!empty_json.contains("local_file"));
        assert!(ready_json.contains("local_file"));
        assert!(ready_json.contains(&"b".repeat(64)));
        assert!(!ready_json.contains("/private/attachments"));
        assert!(!ready_json.contains("file://"));
    }
}
