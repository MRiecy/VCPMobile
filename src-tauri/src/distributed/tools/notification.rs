// distributed/tools/notification.rs
// [OneShot] MobileNotification — send a local notification on the device.
// Mirrors VCPChat's VCPAlarm plugin.

use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::distributed::tool_registry::OneShotTool;
use crate::distributed::types::ToolManifest;

use crate::distributed::types::InvocationCommand;

pub struct NotificationTool;

#[async_trait]
impl OneShotTool for NotificationTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileNotification".to_string(),
            description: "代理可在移动端系统的状态栏发布进度通知或重要弹窗提醒，用于及时通知用户重要信息。".to_string(),
            display_name: "通知中心".to_string(),
            placeholder: None,
            invocation_commands: vec![
                InvocationCommand {
                    command_identifier: "MobileNotification".to_string(),
                    description: "向移动设备系统通知栏发送一条通知弹窗，用于提醒用户查看重要信息。\n\
参数:\n\
- title (字符串, 必需): 通知标题\n\
- body (字符串, 必需): 通知正文内容\n\
调用格式:\n\
<<<[TOOL_REQUEST]>>>\n\
tool_name:「始」MobileNotification「末」\n\
title:「始」通知标题「末」\n\
body:「始」通知内容「末」\n\
<<<[END_TOOL_REQUEST]>>>".to_string(),
                    example: "<<<[TOOL_REQUEST]>>>\ntool_name:「始」MobileNotification「末」\ntitle:「始」任务完成「末」\nbody:「始」您请求的文件已处理完毕，请查收。「末」\n<<<[END_TOOL_REQUEST]>>>".to_string(),
                },
            ],
            web_socket_push: None,
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

        let notification_result = tauri_plugin_vcp_mobile::system::show_system_notification(
            app.clone(),
            title.clone(),
            body.clone(),
        );
        let notification_delivered = notification_result.is_ok();
        let notification_error = notification_result.err();
        if let Some(error) = &notification_error {
            log::warn!("[MobileNotification] Android notification push failed: {error}");
        }

        // Keep the frontend event for in-app UI/diagnostics. Android delivery is
        // handled here so it does not depend on WebView Notification support.
        app.emit(
            "distributed-notification",
            json!({
                "title": title,
                "body": body,
                "androidNotification": {
                    "attempted": true,
                    "delivered": notification_delivered,
                    "error": notification_error,
                },
            }),
        )
        .map_err(|e| format!("Failed to emit notification event: {}", e))?;

        let message = if notification_delivered {
            format!("Notification sent: {}", title)
        } else {
            format!(
                "Notification requested but Android delivery failed: {}",
                title
            )
        };

        Ok(json!({
            "status": "success",
            "message": message,
            "androidNotification": {
                "attempted": true,
                "delivered": notification_delivered
            }
        }))
    }
}
