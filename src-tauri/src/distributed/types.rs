// distributed/types.rs
// Protocol message types for VCP Distributed Node
// Mirrors the JSON protocol in VCPChat/VCPDistributedServer/VCPDistributedServer.js

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

// ============================================================
// Outgoing messages (VCPMobile → VCPToolBox main server)
// ============================================================

/// Top-level envelope for all outgoing messages.
/// Matches VCPChat's `sendMessage({ type: '...', data: { ... } })` pattern.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum OutgoingMessage {
    /// Register available tools with the main server.
    /// VCPChat ref: DistributedServer.registerTools() line 271-308
    #[serde(rename = "register_tools")]
    RegisterTools {
        #[serde(rename = "serverName")]
        server_name: String,
        tools: Vec<ToolManifest>,
    },

    /// Report this node's IP addresses.
    /// VCPChat ref: DistributedServer.reportIPAddress() line 310-347
    #[serde(rename = "report_ip")]
    ReportIp {
        #[serde(rename = "serverName")]
        server_name: String,
        #[serde(rename = "localIPs")]
        local_ips: Vec<String>,
        #[serde(rename = "publicIP")]
        public_ip: Option<String>,
    },

    /// Return the result of a tool execution.
    /// VCPChat ref: handleToolExecutionRequest() line 628-645
    #[serde(rename = "tool_result")]
    ToolResult {
        #[serde(rename = "requestId")]
        request_id: String,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Push static placeholder values (streaming tool data).
    /// VCPChat ref: pushStaticPlaceholderValues() line 374-398
    #[serde(rename = "update_static_placeholders")]
    UpdateStaticPlaceholders {
        #[serde(rename = "serverName")]
        server_name: String,
        placeholders: HashMap<String, String>,
    },

    /// Forward a plugin callback payload so VCPToolBox can reuse existing
    /// webSocketPush routing for distributed tools.
    #[serde(rename = "plugin_callback_forward")]
    PluginCallbackForward {
        #[serde(rename = "callbackData")]
        callback_data: Value,
    },
}

// ============================================================
// Incoming messages (VCPToolBox main server → VCPMobile)
// ============================================================

/// Top-level envelope for all incoming messages.
#[derive(Debug, Clone, Deserialize)]
pub struct IncomingEnvelope {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(default)]
    pub data: Option<Value>,
    /// connection_ack has `message` at top level
    #[serde(default)]
    #[allow(dead_code)]
    pub message: Option<String>,
}

/// Parsed incoming message variants.
#[derive(Debug, Clone)]
pub enum IncomingMessage {
    /// Connection acknowledged by the main server.
    /// Server ref: WebSocketServer.js line 166-172
    /// `{ type: "connection_ack", message: "...", data: { serverId, clientId } }`
    ConnectionAck {
        server_id: String,
        client_id: String,
    },

    /// Main server requests execution of a tool on this node.
    /// `{ type: "execute_tool", data: { requestId, toolName, toolArgs } }`
    ExecuteTool {
        request_id: String,
        tool_name: String,
        tool_args: Value,
    },

    /// Unknown message type (forward-compatible)
    Unknown(String),
}

impl IncomingEnvelope {
    /// Parse the raw envelope into a typed message.
    pub fn parse(self) -> IncomingMessage {
        match self.msg_type.as_str() {
            "connection_ack" => {
                let data = self.data.unwrap_or_default();
                let server_id = data
                    .get("serverId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let client_id = data
                    .get("clientId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                IncomingMessage::ConnectionAck {
                    server_id,
                    client_id,
                }
            }
            "execute_tool" => {
                let data = self.data.unwrap_or_default();
                let request_id = data
                    .get("requestId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let tool_name = data
                    .get("toolName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let tool_args = data
                    .get("toolArgs")
                    .cloned()
                    .unwrap_or(Value::Object(Default::default()));
                IncomingMessage::ExecuteTool {
                    request_id,
                    tool_name,
                    tool_args,
                }
            }
            other => IncomingMessage::Unknown(other.to_string()),
        }
    }
}

// ============================================================
// Tool manifest (registered with main server)
// ============================================================

/// 单条可调用命令的完整描述，严格对应 VCPChat Plugin.js 标准 manifest 格式。
/// capabilities.invocationCommands[] 的每一个元素。
#[derive(Debug, Clone, Deserialize)]
pub struct InvocationCommand {
    /// 命令唯一标识符（通常与工具名相同）
    pub command_identifier: String,
    /// 完整的调用说明，包含参数列表和 VCP 语法示例
    pub description: String,
    /// 完整的 <<<[TOOL_REQUEST]>>> 示例块
    pub example: String,
}

/// Optional VCPToolBox websocket push config from plugin-manifest.json.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WebSocketPushConfig {
    pub enabled: bool,
    #[serde(rename = "usePluginResultAsMessage", default)]
    pub use_plugin_result_as_message: bool,
    #[serde(
        rename = "targetClientType",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub target_client_type: Option<String>,
    #[serde(
        rename = "messageType",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub message_type: Option<String>,
}

/// Tool manifest matching VCPToolBox's plugin manifest format.
/// VCPChat ref: Plugin.js getAllPluginManifests()
#[derive(Debug, Clone, Deserialize)]
pub struct ToolManifest {
    pub name: String,
    pub description: String,

    // UI Metadata（仅前端消费，Serialize 实现中不发往服务端的字段在此注明）
    #[serde(default)]
    pub display_name: String,
    /// 有 placeholder 的工具走静态占位符管道（Streaming），否则走可执行管道（OneShot）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,

    /// OneShot 工具的完整调用命令描述；Streaming 工具留空 Vec
    #[serde(default)]
    pub invocation_commands: Vec<InvocationCommand>,

    /// 兼容旧 VCPToolBox 插件 manifest 的 WebSocket 推送配置。
    #[serde(
        default,
        rename = "webSocketPush",
        skip_serializing_if = "Option::is_none"
    )]
    pub web_socket_push: Option<WebSocketPushConfig>,
}

impl Serialize for ToolManifest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        // 动态分流：有 placeholder 的是静态占位符工具（传感器类），
        // 无 placeholder 的是可执行工具（OneShot/Interactive）
        let is_static = self.placeholder.is_some();
        let plugin_type = if is_static { "static" } else { "synchronous" };

        let mut map = serializer.serialize_map(None)?;

        // ── 顶层标准字段（服务端强校验） ──────────────────────────────────────
        map.serialize_entry("manifestVersion", "1.0.0")?;
        map.serialize_entry("name", &self.name)?;
        map.serialize_entry("version", "1.0.0")?;
        map.serialize_entry("displayName", &self.display_name)?;
        map.serialize_entry("description", &self.description)?;
        map.serialize_entry("author", "VCPMobile")?;
        map.serialize_entry("pluginType", plugin_type)?;
        map.serialize_entry(
            "entryPoint",
            &serde_json::json!({
                "type": "mobile",
                "command": "native"
            }),
        )?;
        map.serialize_entry(
            "communication",
            &serde_json::json!({
                "protocol": "mobile",
                "timeout": 10000
            }),
        )?;
        if let Some(web_socket_push) = &self.web_socket_push {
            map.serialize_entry("webSocketPush", web_socket_push)?;
        }

        // ── capabilities 双轨分流 ─────────────────────────────────────────────
        if is_static {
            // 静态工具：走 systemPromptPlaceholders 管道
            let placeholder_key = self.placeholder.as_deref().unwrap_or_default();
            map.serialize_entry(
                "capabilities",
                &serde_json::json!({
                    "systemPromptPlaceholders": [{
                        "placeholder": placeholder_key,
                        "description": self.description
                    }],
                    "invocationCommands": []
                }),
            )?;
        } else {
            // 可执行工具：走 invocationCommands 管道，使用完整标准格式
            let commands: Vec<serde_json::Value> = self
                .invocation_commands
                .iter()
                .map(|cmd| {
                    serde_json::json!({
                        "commandIdentifier": cmd.command_identifier,
                        "description": cmd.description,
                        "example": cmd.example
                    })
                })
                .collect();
            map.serialize_entry(
                "capabilities",
                &serde_json::json!({
                    "systemPromptPlaceholders": [],
                    "invocationCommands": commands
                }),
            )?;
        }

        map.end()
    }
}

// ============================================================
// Connection / status types
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
}

/// Connection status emitted to the Vue frontend via Tauri events.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DistributedStatus {
    pub state: ConnectionState,
    pub connected: bool,
    pub server_id: Option<String>,
    pub client_id: Option<String>,
    pub registered_tools: usize,
    pub last_error: Option<String>,
}
