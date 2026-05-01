// distributed/tools/clipboard.rs
// [OneShot] MobileClipboard — read/write the system clipboard.
// No direct VCPChat equivalent; mobile-specific tool.

use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::distributed::tool_registry::OneShotTool;
use crate::distributed::types::ToolManifest;

pub struct ClipboardTool;

#[async_trait]
impl OneShotTool for ClipboardTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileClipboard".to_string(),
            description: "读取或写入移动设备的系统剪贴板。Read or write the mobile device system clipboard.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["read", "write"],
                        "description": "操作类型：read 读取剪贴板，write 写入剪贴板 / Action: read or write clipboard"
                    },
                    "content": {
                        "type": "string",
                        "description": "写入剪贴板的内容（action=write 时必填） / Content to write (required when action=write)"
                    }
                },
                "required": ["action"]
            }),
            tool_type: "mobile".to_string(),
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
