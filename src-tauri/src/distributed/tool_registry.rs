// distributed/tool_registry.rs
// Three-mode tool trait system + registry.
// Mirrors VCPChat/VCPDistributedServer/Plugin.js (class PluginManager)
// Self-contained — does NOT import anything from vcp_modules/.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tauri::AppHandle;

use super::types::ToolManifest;

// ============================================================
// Tool traits — three execution modes
// ============================================================

/// OneShot: call and return immediately, no frontend UI interaction needed.
/// Mirrors VCPChat's stdio plugins (child_process.spawn → stdout → result).
#[async_trait]
pub trait OneShotTool: Send + Sync {
    fn manifest(&self) -> ToolManifest;
    async fn execute(&self, args: Value, app: &AppHandle) -> Result<Value, String>;
}

/// Interactive: requires frontend UI participation (camera, biometric, etc.).
/// Mirrors VCPChat's handler-injection pattern (handleMusicControl, handleDesktopRemoteControl).
/// Execution triggers a Tauri event → Vue shows UI → user completes action → result returns.
#[allow(dead_code)]
#[async_trait]
pub trait InteractiveTool: Send + Sync {
    fn manifest(&self) -> ToolManifest;
    /// Execute with frontend round-trip. Implementors use app.emit() + oneshot channel.
    async fn execute(&self, args: Value, app: &AppHandle) -> Result<Value, String>;
    /// Android/iOS permissions required by this tool.
    fn required_permissions(&self) -> Vec<&'static str>;
}

/// Streaming: continuously produces data, pushed via update_static_placeholders.
/// Mirrors VCPChat's static plugins + 30s cron push.
pub trait StreamingTool: Send + Sync {
    fn manifest(&self) -> ToolManifest;
    /// Placeholder key, e.g. "{{MobileSensorGyro}}"
    fn placeholder_key(&self) -> &str;
    /// Polling interval in seconds (metadata — not yet used by client.rs push loop, see C2)
    #[allow(dead_code)]
    fn poll_interval_secs(&self) -> u64;
    /// Read current snapshot value (must be fast/non-blocking)
    fn read_current(&self) -> Result<String, String>;
}

// ============================================================
// Unified tool wrapper — so the registry can store all types
// ============================================================

#[allow(dead_code)]
pub enum ToolEntry {
    OneShot(Arc<dyn OneShotTool>),
    Interactive(Arc<dyn InteractiveTool>),
    Streaming(Arc<dyn StreamingTool>),
}

impl ToolEntry {
    pub fn manifest(&self) -> ToolManifest {
        match self {
            ToolEntry::OneShot(t) => t.manifest(),
            ToolEntry::Interactive(t) => t.manifest(),
            ToolEntry::Streaming(t) => t.manifest(),
        }
    }
}

// ============================================================
// ToolRegistry — the central tool manager
// Mirrors Plugin.js: loadPlugins(), getAllPluginManifests(), processToolCall()
// ============================================================

pub struct ToolRegistry {
    tools: HashMap<String, ToolEntry>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a OneShot tool.
    pub fn register_oneshot<T: OneShotTool + 'static>(&mut self, tool: T) {
        let name = tool.manifest().name.clone();
        self.tools.insert(name, ToolEntry::OneShot(Arc::new(tool)));
    }

    /// Register an Interactive tool.
    #[allow(dead_code)]
    pub fn register_interactive<T: InteractiveTool + 'static>(&mut self, tool: T) {
        let name = tool.manifest().name.clone();
        self.tools
            .insert(name, ToolEntry::Interactive(Arc::new(tool)));
    }

    /// Register a Streaming tool.
    pub fn register_streaming<T: StreamingTool + 'static>(&mut self, tool: T) {
        let name = tool.manifest().name.clone();
        self.tools
            .insert(name, ToolEntry::Streaming(Arc::new(tool)));
    }

    /// Get all tool manifests for register_tools message.
    /// Mirrors Plugin.js getAllPluginManifests()
    pub fn get_all_manifests(&self) -> Vec<ToolManifest> {
        self.tools.values().map(|e| e.manifest()).collect()
    }

    /// Get all streaming placeholder values for update_static_placeholders.
    /// Mirrors Plugin.js getAllPlaceholderValues()
    pub fn get_all_placeholder_values(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for entry in self.tools.values() {
            if let ToolEntry::Streaming(tool) = entry {
                if let Ok(value) = tool.read_current() {
                    map.insert(tool.placeholder_key().to_string(), value);
                }
            }
        }
        map
    }

    /// Execute a tool by name. Routes to the correct handler.
    /// Mirrors Plugin.js processToolCall()
    pub async fn execute(
        &self,
        tool_name: &str,
        args: Value,
        app: &AppHandle,
    ) -> Result<Value, String> {
        let entry = self
            .tools
            .get(tool_name)
            .ok_or_else(|| format!("Tool '{}' not found in registry.", tool_name))?;

        match entry {
            ToolEntry::OneShot(tool) => tool.execute(args, app).await,
            ToolEntry::Interactive(tool) => tool.execute(args, app).await,
            ToolEntry::Streaming(tool) => {
                // For streaming tools, execute_tool returns a current snapshot.
                tool.read_current().map(serde_json::Value::String)
            }
        }
    }

    /// Number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }
}
