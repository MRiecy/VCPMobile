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

#[derive(Debug, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "lowercase")]
enum ClipboardArgs {
    Read,
    Write { content: String },
}

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
        let args: ClipboardArgs = serde_json::from_value(args)
            .map_err(|error| format!("Invalid clipboard arguments: {error}"))?;
        match args {
            ClipboardArgs::Write { content } => {
                let native_app = app.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    tauri_plugin_vcp_mobile::system::write_clipboard_native(native_app, content)
                })
                .await
                .map_err(|e| format!("Native clipboard task failed: {e}"))?
                .map_err(|e| format!("Native clipboard write failed: {e}"))?;

                Ok(json!({
                    "status": "success",
                    "message": "Content written to clipboard."
                }))
            }
            ClipboardArgs::Read => {
                let native_app = app.clone();
                let content = tauri::async_runtime::spawn_blocking(move || {
                    tauri_plugin_vcp_mobile::system::read_clipboard_native(native_app)
                })
                .await
                .map_err(|e| format!("Native clipboard task failed: {e}"))?
                .map_err(|e| format!("Native clipboard read failed: {e}"))?;

                Ok(json!({
                    "status": "success",
                    "content": content
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_clipboard_intent_is_not_replaced_by_read_or_empty_write() {
        for args in [
            json!({}), json!({"action":null}), json!({"action":false}),
            json!({"action":"unknown"}), json!({"action":"write"}),
            json!({"action":"write","content":null}), json!({"action":"write","content":123}),
        ] {
            assert!(serde_json::from_value::<ClipboardArgs>(args).is_err());
        }
        assert_eq!(serde_json::from_value::<ClipboardArgs>(json!({"action":"read"})).unwrap(), ClipboardArgs::Read);
        assert_eq!(serde_json::from_value::<ClipboardArgs>(json!({"action":"write","content":""})).unwrap(), ClipboardArgs::Write { content: String::new() });
    }
}
