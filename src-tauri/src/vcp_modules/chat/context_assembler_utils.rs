use crate::vcp_modules::chat_manager::ChatMessage;
use serde_json::{json, Value};

/// 将数据库中的 ChatMessage 历史记录转换为发送给 VCP 的 JSON 格式
/// 处理逻辑：
/// 1. 过滤掉正在思考中的消息 (is_thinking == true)
/// 2. 提取附件中的文本内容 (extracted_text) 并拼接到文本中
/// 3. 将多模态附件 (图片/音频/视频) 转换为 {"type": "local_file", "path": "..."} 结构
pub fn assemble_history_for_vcp(history: &[ChatMessage]) -> Vec<Value> {
    history
        .iter()
        .filter(|msg| !msg.is_thinking.unwrap_or(false))
        .map(|msg| {
            let mut combined_text = msg.content.clone();
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
                        // 优先使用物理路径 internal_path
                        let path = if !att.internal_path.is_empty() {
                            att.internal_path.clone()
                        } else {
                            att.src.clone()
                        };

                        // 注入路径标记，对齐桌面端逻辑
                        if is_image {
                            combined_text.push_str(&format!("\n\n[附加图片: {}]", path));
                        } else {
                            combined_text.push_str(&format!("\n\n[附加文件: {}]", path));
                        }

                        content_parts.push(json!({
                            "type": "local_file",
                            "path": path,
                            "mime": mime
                        }));
                    } else if att.extracted_text.is_none() {
                        // 既没有提取文本也不是多模态，仅做标记 (对齐桌面端 fallback)
                        let path = if !att.internal_path.is_empty() {
                            att.internal_path.clone()
                        } else {
                            att.src.clone()
                        };
                        combined_text.push_str(&format!("\n\n[附加文件: {}]", path));
                    }
                }
            }

            // 如果有文本内容，将其作为第一个 part 插入
            if !combined_text.trim().is_empty() {
                content_parts.insert(
                    0,
                    json!({
                        "type": "text",
                        "text": combined_text
                    }),
                );
            }

            // 构造最终的消息对象
            // 如果只有文本且没有多模态 Part，则简化 content 为字符串
            let final_content = if content_parts.len() == 1 && content_parts[0]["type"] == "text" {
                content_parts[0]["text"].clone()
            } else {
                json!(content_parts)
            };

            json!({
                "role": msg.role,
                "name": msg.name,
                "content": final_content
            })
        })
        .collect()
}
