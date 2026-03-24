use super::content_parser::{ensure_html_fenced, parse_content, ContentBlock};

#[tauri::command]
pub async fn process_message_content(content: String) -> Result<Vec<ContentBlock>, String> {
    // 1. 预处理：确保裸露的 HTML 被 Markdown 代码块包裹
    let fenced_content = ensure_html_fenced(&content);

    // 2. 核心解析：将文本切割为 AST 块数组
    let blocks = parse_content(&fenced_content);

    Ok(blocks)
}
