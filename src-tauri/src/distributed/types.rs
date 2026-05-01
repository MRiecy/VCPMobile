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

/// Tool manifest matching VCPToolBox's plugin manifest format.
/// VCPChat ref: Plugin.js getAllPluginManifests()
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolManifest {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub parameters: Value,
    /// "service" | "hybridservice" | "static" | "mobile" — VCPMobile tools use "mobile"
    #[serde(rename = "type", default = "default_tool_type")]
    pub tool_type: String,
}

fn default_tool_type() -> String {
    "mobile".to_string()
}

// ============================================================
// Connection / status types
// ============================================================

/// Connection status emitted to the Vue frontend via Tauri events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributedStatus {
    pub connected: bool,
    pub server_id: Option<String>,
    pub client_id: Option<String>,
    pub registered_tools: usize,
    pub last_error: Option<String>,
}

impl Default for DistributedStatus {
    fn default() -> Self {
        Self {
            connected: false,
            server_id: None,
            client_id: None,
            registered_tools: 0,
            last_error: None,
        }
    }
}
