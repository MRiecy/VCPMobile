// distributed/tools/clipboard.rs
// [OneShot] MobileClipboard — read/write the system clipboard.
// No direct VCPChat equivalent; mobile-specific tool.

use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::distributed::tool_registry::OneShotTool;
use crate::distributed::types::ToolManifest;

use crate::distributed::types::InvocationCommand;

pub struct ClipboardTool;

#[async_trait]
impl OneShotTool for ClipboardTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileClipboard".to_string(),
            description: "允许 AI Agent 安全读取和向移动端系统剪贴板写入文本数据。".to_string(),
            display_name: "系统剪贴板".to_string(),
            placeholder: None,
            invocation_commands: vec![
                InvocationCommand {
                    command_identifier: "MobileClipboard".to_string(),
                    description: "操作移动端系统剪贴板，支持读取当前内容或写入新内容。\n\
参数:\n\
- action (字符串, 必需): 操作类型，可选值: \"read\"（读取剪贴板）| \"write\"（写入剪贴板）\n\
- content (字符串, write 时必需): 需要写入剪贴板的文本内容\n\
调用格式:\n\
<<<[TOOL_REQUEST]>>>\n\
tool_name:「始」MobileClipboard「末」\n\
action:「始」write「末」\n\
content:「始」需要写入的内容「末」\n\
<<<[END_TOOL_REQUEST]>>>".to_string(),
                    example: "<<<[TOOL_REQUEST]>>>\ntool_name:「始」MobileClipboard「末」\naction:「始」read「末」\n<<<[END_TOOL_REQUEST]>>>".to_string(),
                },
            ],
        }
    }

    async fn execute(&self, args: Value, app: &AppHandle) -> Result<Value, String> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("read");

        match action {
            "write" => {
                let content = args
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                tauri_plugin_vcp_mobile::system::write_clipboard_native(app.clone(), content)
                    .map_err(|e| format!("Native clipboard write failed: {}", e))?;

                Ok(json!({
                    "status": "success",
                    "message": "Content written to clipboard."
                }))
            }
            "read" => {
                let content = tauri_plugin_vcp_mobile::system::read_clipboard_native(app.clone())
                    .map_err(|e| format!("Native clipboard read failed: {}", e))?;

                Ok(json!({
                    "status": "success",
                    "content": content
                }))
            }
            _ => Err(format!("Unknown clipboard action: '{}'", action)),
        }
    }
}
