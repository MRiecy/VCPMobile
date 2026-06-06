// distributed/tools/clipboard.rs
// [OneShot] MobileClipboard — read/write the system clipboard.
// No direct VCPChat equivalent; mobile-specific tool.

use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

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
            web_socket_push: None,
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

                // Emit to frontend — Vue will call navigator.clipboard.writeText()
                app.emit("distributed-clipboard-write", json!({ "content": content }))
                    .map_err(|e| format!("Failed to emit clipboard write: {}", e))?;

                Ok(json!({
                    "status": "success",
                    "message": "Content written to clipboard."
                }))
            }
            "read" => {
                // Reading clipboard requires frontend round-trip (navigator.clipboard.readText()).
                // For Phase 2, we emit a request and return a placeholder.
                // Full Interactive round-trip will be refined in Phase 3.
                app.emit("distributed-clipboard-read", json!({}))
                    .map_err(|e| format!("Failed to emit clipboard read: {}", e))?;

                Ok(json!({
                    "status": "success",
                    "message": "Clipboard read request sent to device. Content will be available in the next interaction."
                }))
            }
            _ => Err(format!("Unknown clipboard action: '{}'", action)),
        }
    }
}
