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
                    let file_data = att.file_manager_data.as_ref();
                    
                    // 1. 处理提取的文本内容 (文档类)
                    if let Some(data) = file_data {
                        if let Some(text) = &data.extracted_text {
                            if !text.is_empty() {
                                combined_text.push_str(&format!(
                                    "\n\n[附加文件: {}]\n{}\n[/附加文件结束: {}]",
                                    att.name, text, att.name
                                ));
                            }
                        }
                    }

                    // 2. 处理多模态文件 (图片/音频/视频)
                    let mime = &att.r#type;
                    let is_multimodal = mime.starts_with("image/") 
                                     || mime.starts_with("audio/") 
                                     || mime.starts_with("video/");

                    if is_multimodal {
                        // 优先使用物理路径 internal_path
                        let path = file_data.map(|d| d.internal_path.clone())
                                           .unwrap_or_else(|| att.src.clone());
                        
                        content_parts.push(json!({
                            "type": "local_file",
                            "path": path,
                            "mime": mime
                        }));
                    } else if file_data.and_then(|d| d.extracted_text.as_ref()).is_none() {
                        // 既没有提取文本也不是多模态，仅做标记
                        combined_text.push_str(&format!(
                            "\n\n[附加文件: {}] (不支持直接读取内容)",
                            att.name
                        ));
                    }
                }
            }

            // 如果有文本内容，将其作为第一个 part 插入
            if !combined_text.trim().is_empty() {
                content_parts.insert(0, json!({
                    "type": "text",
                    "text": combined_text
                }));
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
