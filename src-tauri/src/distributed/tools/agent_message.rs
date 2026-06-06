// distributed/tools/agent_message.rs
// [OneShot] AgentMessage - push formatted agent messages through VCPToolBox.

use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::distributed::tool_registry::OneShotTool;
use crate::distributed::types::{InvocationCommand, ToolManifest, WebSocketPushConfig};

pub struct AgentMessageTool;
pub struct MobileAgentMessageTool;

#[async_trait]
impl OneShotTool for AgentMessageTool {
    fn manifest(&self) -> ToolManifest {
        agent_message_manifest(
            "AgentMessage",
            "代理消息推送插件",
            "允许 AI 通过分布式工具向用户前端发送格式化消息。",
            "调用此工具向用户的前端发送一条消息。",
        )
    }

    async fn execute(&self, args: Value, app: &AppHandle) -> Result<Value, String> {
        execute_agent_message(args, app, "AgentMessage").await
    }
}

#[async_trait]
impl OneShotTool for MobileAgentMessageTool {
    fn manifest(&self) -> ToolManifest {
        agent_message_manifest(
            "MobileAgentMessage",
            "移动端 Agent 消息推送",
            "允许 AI 明确向移动端应用内通知中心和 Android 系统通知栏发送格式化消息。",
            "调用此工具向移动设备系统通知栏发送一条 Agent 消息。若桌面端已存在本地 AgentMessage 插件，应优先使用此移动端专用工具。",
        )
    }

    async fn execute(&self, args: Value, app: &AppHandle) -> Result<Value, String> {
        execute_agent_message(args, app, "MobileAgentMessage").await
    }
}

fn agent_message_manifest(
    name: &str,
    display_name: &str,
    description: &str,
    command_description: &str,
) -> ToolManifest {
    ToolManifest {
        name: name.to_string(),
        display_name: display_name.to_string(),
        description: description.to_string(),
        placeholder: None,
        invocation_commands: vec![InvocationCommand {
            command_identifier: name.to_string(),
            description: format!(
                "{command_description} AI 需要提供消息内容，并可选择提供发送者（Maid）的名称。\n\
参数:\n\
- Maid (字符串, 可选): 消息发送者名称；省略时为匿名消息\n\
- message (字符串, 必需): 要发送的消息内容\n\
调用格式:\n\
<<<[TOOL_REQUEST]>>>\n\
tool_name:「始」{name}「末」\n\
Maid:「始」小克「末」\n\
message:「始」主人，您的咖啡已经准备好了。「末」\n\
<<<[END_TOOL_REQUEST]>>>"
            ),
            example: format!(
                "<<<[TOOL_REQUEST]>>>\ntool_name:「始」{name}「末」\nMaid:「始」小克「末」\nmessage:「始」主人，您的咖啡已经准备好了，请到餐厅享用。「末」\n<<<[END_TOOL_REQUEST]>>>"
            ),
        }],
        web_socket_push: Some(WebSocketPushConfig {
            enabled: true,
            use_plugin_result_as_message: true,
            target_client_type: Some("VCPLog".to_string()),
            message_type: Some("vcp_log".to_string()),
        }),
    }
}

async fn execute_agent_message(
    args: Value,
    app: &AppHandle,
    tool_name: &str,
) -> Result<Value, String> {
    let message = get_string_arg(&args, "message")
        .ok_or_else(|| "Missing required argument: message (消息内容)".to_string())?;
    let maid_name = get_string_arg(&args, "Maid")
        .or_else(|| get_string_arg(&args, "maid"))
        .or_else(|| get_string_arg(&args, "sender_name"));

    let display_timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let iso_timestamp = chrono::Utc::now().to_rfc3339();
    let formatted_message = match maid_name.as_deref() {
        Some(name) => format!("{} - {}\n{}", display_timestamp, name, message),
        None => format!("{}\n{}", display_timestamp, message),
    };

    let notification_title = agent_notification_title(maid_name.as_deref());
    let notification_result = tauri_plugin_vcp_mobile::system::show_system_notification(
        app.clone(),
        notification_title,
        message.clone(),
    );
    if let Err(error) = &notification_result {
        log::warn!(
            "[{}] Android notification push failed: {}",
            tool_name,
            error
        );
    }

    let result = json!({
        "type": "agent_message",
        "message": formatted_message,
        "recipient": maid_name,
        "originalContent": message,
        "timestamp": iso_timestamp,
        "mobileToolName": tool_name,
        "androidNotification": {
            "attempted": true,
            "delivered": notification_result.is_ok(),
            "error": notification_result.err(),
        },
    });

    app.emit("vcp-system-event", result.clone())
        .map_err(|e| format!("Failed to emit agent message event: {}", e))?;

    Ok(result)
}

fn get_string_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn agent_notification_title(maid_name: Option<&str>) -> String {
    maid_name
        .map(|name| format!("{} 的消息", name))
        .unwrap_or_else(|| "Agent 消息".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_declares_websocket_push_for_vcp_log() {
        let manifest = AgentMessageTool.manifest();
        let push = manifest.web_socket_push.expect("webSocketPush missing");

        assert_eq!(manifest.name, "AgentMessage");
        assert!(push.enabled);
        assert!(push.use_plugin_result_as_message);
        assert_eq!(push.target_client_type.as_deref(), Some("VCPLog"));
        assert_eq!(push.message_type.as_deref(), Some("vcp_log"));
    }

    #[test]
    fn mobile_alias_uses_distinct_tool_name() {
        let manifest = MobileAgentMessageTool.manifest();
        let push = manifest.web_socket_push.expect("webSocketPush missing");

        assert_eq!(manifest.name, "MobileAgentMessage");
        assert_eq!(
            manifest.invocation_commands[0].command_identifier,
            "MobileAgentMessage"
        );
        assert_eq!(push.target_client_type.as_deref(), Some("VCPLog"));
        assert_eq!(push.message_type.as_deref(), Some("vcp_log"));
    }

    #[test]
    fn string_args_are_trimmed() {
        let args = json!({ "Maid": "  小克  ", "message": "  你好  " });

        assert_eq!(get_string_arg(&args, "Maid").as_deref(), Some("小克"));
        assert_eq!(get_string_arg(&args, "message").as_deref(), Some("你好"));
    }

    #[test]
    fn notification_title_uses_sender_when_available() {
        assert_eq!(agent_notification_title(Some("小克")), "小克 的消息");
        assert_eq!(agent_notification_title(None), "Agent 消息");
    }
}
