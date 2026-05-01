// distributed/tools/notification.rs
// [OneShot] MobileNotification — send a local notification on the device.
// Mirrors VCPChat's VCPAlarm plugin.

use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::distributed::tool_registry::OneShotTool;
use crate::distributed::types::ToolManifest;

pub struct NotificationTool;

#[async_trait]
impl OneShotTool for NotificationTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileNotification".to_string(),
            description: "在移动设备上发送本地通知。Send a local notification on the mobile device.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "通知标题 / Notification title"
                    },
                    "body": {
                        "type": "string",
                        "description": "通知内容 / Notification body"
                    }
                },
                "required": ["title", "body"]
            }),
            tool_type: "mobile".to_string(),
        }
    }

    async fn execute(&self, args: Value, app: &AppHandle) -> Result<Value, String> {
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("VCP Notification")
            .to_string();
        let body = args
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Emit to Vue frontend to show the notification via system API.
        // The frontend listens for "distributed-notification" and calls the native notification API.
        app.emit(
            "distributed-notification",
            json!({ "title": title, "body": body }),
        )
        .map_err(|e| format!("Failed to emit notification event: {}", e))?;

        Ok(json!({
            "status": "success",
            "message": format!("Notification sent: {}", title)
        }))
    }
}
