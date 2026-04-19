use crate::vcp_modules::content_parser::{ensure_html_fenced, parse_content, ContentBlock};
use crate::vcp_modules::emoticon_manager::{internal_fix_url, EmoticonItem, EmoticonManagerState};
use percent_encoding::percent_decode_str;
use regex::{Captures, Regex};
use tauri::{AppHandle, State};

pub struct MessageRenderCompiler;

impl MessageRenderCompiler {
    /// Compiles raw message content into AST blocks (the "astbin" format base)
    pub fn compile(content: &str, emoticon_library: &[EmoticonItem]) -> Vec<ContentBlock> {
        // 1. Pre-process (ported from message_processor.rs)
        let fixed_content = if emoticon_library.is_empty() {
            content.to_string()
        } else {
            // A. Handle HTML tags: <img src="...">
            let html_re =
                Regex::new(r#"(?i)<img\b([^>]*?\bsrc\s*=\s*["'])([^"']+)(["'])([^>]*?)>"#).unwrap();
            let step1 = html_re
                .replace_all(content, |caps: &Captures| {
                    let src = &caps[2];
                    let fixed_src = internal_fix_url(src, emoticon_library);

                    let is_emoticon = fixed_src.contains("images/")
                        && (fixed_src.contains("表情包")
                            || percent_decode_str(&fixed_src)
                                .decode_utf8_lossy()
                                .contains("表情包"));

                    if is_emoticon {
                        format!(
                            "<img src=\"{}\" class=\"vcp-emoticon\" {}>",
                            fixed_src, &caps[4]
                        )
                    } else {
                        format!("<img src=\"{}\" {}>", fixed_src, &caps[4])
                    }
                })
                .to_string();

            // B. Handle Markdown syntax: ![alt](url)
            let md_re = Regex::new(r#"(?i)!\[(.*?)\]\(([^)]+)\)"#).unwrap();
            md_re
                .replace_all(&step1, |caps: &Captures| {
                    let alt = &caps[1];
                    let src = &caps[2];
                    let fixed_src = internal_fix_url(src, emoticon_library);

                    let is_emoticon = fixed_src.contains("images/")
                        && (fixed_src.contains("表情包")
                            || percent_decode_str(&fixed_src)
                                .decode_utf8_lossy()
                                .contains("表情包"));

                    if is_emoticon {
                        format!(
                            "<img src=\"{}\" alt=\"{}\" class=\"vcp-emoticon\">",
                            fixed_src, alt
                        )
                    } else {
                        format!("![{}]({})", alt, fixed_src)
                    }
                })
                .to_string()
        };

        // 2. Pre-process HTML fencing (Ported from content_parser robustly)
        let fenced_content = ensure_html_fenced(&fixed_content);

        // 3. Core parse
        parse_content(&fenced_content)
    }

    /// Serializes AST blocks to binary (currently just JSON for simplicity, but abstracted)
    pub fn serialize(blocks: &[ContentBlock]) -> Result<Vec<u8>, String> {
        serde_json::to_vec(blocks).map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub async fn process_message_content(
    _app_handle: AppHandle,
    content: String,
    emoticon_state: State<'_, EmoticonManagerState>,
) -> Result<Vec<ContentBlock>, String> {
    // 1. 全量预解析 (调用统一的渲染编译器)
    let library = emoticon_state.library.lock().await;
    let blocks = MessageRenderCompiler::compile(&content, &library);

    Ok(blocks)
}
