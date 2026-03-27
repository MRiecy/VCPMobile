use tauri::{AppHandle, State};
use regex::{Regex, Captures};
use percent_encoding::percent_decode_str;
use crate::vcp_modules::content_parser::{ensure_html_fenced, parse_content, ContentBlock};
use crate::vcp_modules::emoticon_manager::{EmoticonManagerState, internal_fix_url};

#[tauri::command]
pub async fn process_message_content(
    _app_handle: AppHandle,
    content: String,
    emoticon_state: State<'_, EmoticonManagerState>,
) -> Result<Vec<ContentBlock>, String> {
    // 1. 全量预修复 & 样式注入 (下沉核心层)
    let fixed_content = {
        let library = emoticon_state.library.lock().await;
        if library.is_empty() {
            content
        } else {
            // A. 处理 HTML 标签: <img src="...">
            let html_re = Regex::new(r#"(?i)<img\b([^>]*?\bsrc\s*=\s*["'])([^"']+)(["'])([^>]*?)>"#).unwrap();
            let step1 = html_re.replace_all(&content, |caps: &Captures| {
                let src = &caps[2];
                let fixed_src = internal_fix_url(src, &library);
                
                // 附加优化：如果是 file:// 且文件不存在，尝试路径重定心 (针对流式传输中可能带入的同步路径)
                if fixed_src.starts_with("file://") {
                    let raw_path = fixed_src.trim_start_matches("file://");
                    if !std::path::Path::new(raw_path).exists() {
                        // 尝试从文件名推断 hash (这里只是概率性修复，真正严谨的在附件元数据里)
                        // 但对于流式 HTML 中的图片，我们尽量保持原样或基于表情包库修复
                    }
                }

                let is_emoticon = fixed_src.contains("images/") && 
                                 (fixed_src.contains("表情包") || 
                                  percent_decode_str(&fixed_src).decode_utf8_lossy().contains("表情包"));

                if is_emoticon {
                    format!("<img src=\"{}\" class=\"vcp-emoticon\" {}>", fixed_src, &caps[4])
                } else {
                    format!("<img src=\"{}\" {}>", fixed_src, &caps[4])
                }
            }).to_string();

            // B. 处理 Markdown 语法: ![alt](url)
            let md_re = Regex::new(r#"(?i)!\[(.*?)\]\(([^)]+)\)"#).unwrap();
            md_re.replace_all(&step1, |caps: &Captures| {
                let alt = &caps[1];
                let src = &caps[2];
                let fixed_src = internal_fix_url(src, &library);
                
                let is_emoticon = fixed_src.contains("images/") && 
                                 (fixed_src.contains("表情包") || 
                                  percent_decode_str(&fixed_src).decode_utf8_lossy().contains("表情包"));

                if is_emoticon {
                    format!("<img src=\"{}\" alt=\"{}\" class=\"vcp-emoticon\">", fixed_src, alt)
                } else {
                    format!("![{}]({})", alt, fixed_src)
                }
            }).to_string()
        }
    };

    // 2. 预处理：确保裸露的 HTML 被 Markdown 代码块包裹
    let fenced_content = ensure_html_fenced(&fixed_content);

    // 3. 核心解析：将文本切割为 AST 块数组
    let blocks = parse_content(&fenced_content);

    Ok(blocks)
}
