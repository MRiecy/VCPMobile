use crate::vcp_modules::content_parser::{ensure_html_fenced, parse_content, ContentBlock};
use tauri::AppHandle;

pub struct MessageRenderCompiler;

impl MessageRenderCompiler {
    /// Compiles raw message content into AST blocks (the "astbin" format base)
    pub fn compile(content: &str) -> Vec<ContentBlock> {
        // 1. Pre-process HTML fencing (Ported from content_parser robustly)
        let fenced_content = ensure_html_fenced(content);

        // 2. Core parse
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
) -> Result<Vec<ContentBlock>, String> {
    // 1. 全量预解析 (调用统一的渲染编译器)
    let blocks = MessageRenderCompiler::compile(&content);

    Ok(blocks)
}
