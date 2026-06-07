use crate::vcp_modules::chat_manager::ChatMessage;
use serde_json::{json, Value};
use sqlx::{Pool, Sqlite};

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
pub async fn orchestrate_chat_context(
    pool: &Pool<Sqlite>,
    history: &[ChatMessage],
    topic_id: &str,
    agent_name: &str,
    scope: &str, // "agent" | "group"
    base_system_prompt: String,
    invite_prompt: Option<String>,
) -> Result<Vec<Value>, String> {
    // 1. 快速查询会话内时间锚定机制 V2 的启用状态
    let enable_time_anchoring = match sqlx::query_scalar::<_, i32>(
        "SELECT is_enabled FROM tarven_rules WHERE id = 'time_anchoring_v2'",
    )
    .fetch_optional(pool)
    .await
    {
        Ok(Some(val)) => val != 0,
        _ => false,
    };

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
        pool,
        topic_id,
        agent_name,
        scope,
        &mut messages,
    )
    .await?;

    Ok(messages)
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
/// 我们在 "时间元数据"、"发言人消歧元数据" 与 "消息内容正文" 之间，
/// 强行硬编码级联插入了物理换行符 `\n`。这在字节层面上彻底切断了前缀与正文的融合通道。
///
/// 格式示意：
/// [Time: 2026-05-30 11:30]\n       <--- 物理换行 1：阻断时间与发言人特征融合
/// [Sender的发言]:\n                <--- 物理换行 2：阻断发言人与正文特征融合
/// 这是正文内容...
///
/// 📂 附件/多模态与内联物理隔离逻辑：
/// -------------------------------------------------------------
/// 1. 【文档类提取】：若附件（如 PDF、DOCX、TXT 等）已被 Rust 底层流水线提取为文本 `extracted_text`，
///    将以极其工整的形式通过内联闭环标签嵌入到文本尾部：
///    `\n\n[附加文件: {path}]\n{text}\n[/附加文件结束: {name}]`
/// 2. 【多模态富资产】：如果是图片、音频或视频资产，自动将其编译为带 MIME 与本地安全路径的 `local_file`
///    标准 JSON 对象（供底层 VCP Client 执行多模态 Payload 投递），并辅以内联标记供纯文本后备降级渲染。
pub fn assemble_history_for_vcp(
    history: &[ChatMessage],
    is_group: bool,
    enable_time_anchoring: bool,
) -> Vec<Value> {
    history
        .iter()
        .filter(|msg| !msg.is_thinking.unwrap_or(false))
        .map(|msg| {
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
                    // 1. 处理提取的文本内容 (文档类)
                    if let Some(text) = &att.extracted_text {
                        if !text.is_empty() {
                            combined_text.push_str(&format!(
                                "\n\n[附加文件: {}]\n{}\n[/附加文件结束: {}]",
                                att.internal_path, text, att.name
                            ));
                        }
                    }

                    // 2. 处理多模态文件 (图片/音频/视频)
                    let mime = &att.r#type;
                    let is_image = mime.starts_with("image/");
                    let is_audio = mime.starts_with("audio/");
                    let is_video = mime.starts_with("video/");

                    if is_image || is_audio || is_video {
                        let path = if !att.internal_path.is_empty() {
                            att.internal_path.clone()
                        } else {
                            att.src.clone()
                        };

                        if is_image {
                            combined_text.push_str(&format!(
                                "\n\n[附加图片: {}] (文件名: {})",
                                path, att.name
                            ));
                        } else {
                            combined_text.push_str(&format!(
                                "\n\n[附加文件: {}] (文件名: {})",
                                path, att.name
                            ));
                        }

                        content_parts.push(json!({
                            "type": "local_file",
                            "path": path,
                            "mime": mime
                        }));
                    } else if att.extracted_text.is_none() {
                        let path = if !att.internal_path.is_empty() {
                            att.internal_path.clone()
                        } else {
                            att.src.clone()
                        };
                        combined_text
                            .push_str(&format!("\n\n[附加文件: {}] (文件名: {})", path, att.name));
                    }
                }
            }

            // 4. 追加末尾时间锚定 (元数据 A - XML 标签格式)
            if enable_time_anchoring {
                combined_text.push_str(&format!(
                    "\n<message_time>{}</message_time>",
                    formatted_time
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

            let final_content = if content_parts.len() == 1 && content_parts[0]["type"] == "text" {
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
            val
        })
        .collect()
}
