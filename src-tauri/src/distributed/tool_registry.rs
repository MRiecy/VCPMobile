// distributed/tool_registry.rs
// Three-mode tool trait system + registry.
// Mirrors VCPChat/VCPDistributedServer/Plugin.js (class PluginManager)
// Self-contained — does NOT import anything from vcp_modules/.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Write};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::Value;
use tauri::{AppHandle, Manager};

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
    fn read_current(&self, app: &AppHandle) -> Result<String, String>;
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
    disabled_names: RwLock<HashSet<String>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            disabled_names: RwLock::new(HashSet::new()),
        }
    }

    /// Sync disabled tools list from frontend. Returns true if the set changed.
    pub fn update_disabled(&self, names: Vec<String>) -> bool {
        if let Ok(mut guard) = self.disabled_names.write() {
            let new_set: HashSet<String> = names.into_iter().collect();
            if *guard != new_set {
                *guard = new_set;
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    /// Check if a tool is enabled.
    pub fn is_enabled(&self, name: &str) -> bool {
        if let Ok(guard) = self.disabled_names.read() {
            !guard.contains(name)
        } else {
            true
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
    /// 上报全部已注册工具（OneShot/Interactive/Streaming），
    /// 服务端通过 pluginType 字段区分可执行与静态占位符类型。
    pub fn get_all_manifests(&self) -> Vec<ToolManifest> {
        self.tools
            .iter()
            .filter(|(name, _)| self.is_enabled(name))
            .map(|(_, e)| e.manifest())
            .collect()
    }

    /// Get all tool metadata with categories and placeholders for the frontend config.
    pub fn get_tools_metadata(&self) -> Vec<serde_json::Value> {
        self.tools
            .iter()
            .map(|(name, entry)| {
                let manifest = entry.manifest();
                let mut val = serde_json::to_value(&manifest).unwrap_or(serde_json::Value::Null);
                if val.is_object() {
                    let category = match entry {
                        ToolEntry::OneShot(_) => "oneshot",
                        ToolEntry::Interactive(_) => "interactive",
                        ToolEntry::Streaming(_) => "streaming",
                    };
                    val["category"] = serde_json::json!(category);
                    val["enabled"] = serde_json::json!(self.is_enabled(name));
                }
                val
            })
            .collect()
    }

    /// Get all streaming placeholder values for update_static_placeholders.
    /// Mirrors Plugin.js getAllPlaceholderValues()
    pub fn get_all_placeholder_values(&self, app: &AppHandle) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for (name, entry) in self.tools.iter() {
            if self.is_enabled(name) {
                if let ToolEntry::Streaming(tool) = entry {
                    if let Ok(value) = tool.read_current(app) {
                        map.insert(tool.placeholder_key().to_string(), value);
                    }
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
        if !self.is_enabled(tool_name) {
            return Err(format!(
                "Tool '{}' is currently disabled on this mobile node.",
                tool_name
            ));
        }

        let entry = self
            .tools
            .get(tool_name)
            .ok_or_else(|| format!("Tool '{}' not found in registry.", tool_name))?;

        match entry {
            ToolEntry::OneShot(tool) => tool.execute(args, app).await,
            ToolEntry::Interactive(tool) => tool.execute(args, app).await,
            ToolEntry::Streaming(tool) => {
                // For streaming tools, execute_tool returns a current snapshot.
                tool.read_current(app).map(serde_json::Value::String)
            }
        }
    }

    /// Number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// Load disabled tools list from local JSON config file and populate memory state.
    pub fn load_disabled_config(&self, app: &AppHandle) {
        if let Ok(config_dir) = app.path().app_config_dir() {
            let config_path = config_dir.join("distributed_tools.json");
            if config_path.exists() {
                if let Ok(mut file) = File::open(&config_path) {
                    let mut content = String::new();
                    if file.read_to_string(&mut content).is_ok() {
                        if let Ok(names) = serde_json::from_str::<Vec<String>>(&content) {
                            if let Ok(mut guard) = self.disabled_names.write() {
                                *guard = names.into_iter().collect();
                                log::info!(
                                    "[Distributed] Loaded disabled tools config: {:?}",
                                    guard
                                );
                            }
                        }
                    }
                }
            } else {
                // 如果配置文件不存在（通常是首次运行），默认将所有已注册的工具标记为禁用（关闭），符合默认禁用插件的要求
                if let Ok(mut guard) = self.disabled_names.write() {
                    *guard = self.tools.keys().cloned().collect();
                    log::info!("[Distributed] Config file not found, defaulting all tools to disabled: {:?}", guard);
                }
                let _ = self.save_disabled_config(app);
            }
        }
    }

    /// Save current disabled tools list to local JSON config file.
    pub fn save_disabled_config(&self, app: &AppHandle) -> Result<(), String> {
        if let Ok(config_dir) = app.path().app_config_dir() {
            // Ensure directory exists
            let _ = std::fs::create_dir_all(&config_dir);
            let config_path = config_dir.join("distributed_tools.json");
            let names: Vec<String> = if let Ok(guard) = self.disabled_names.read() {
                guard.iter().cloned().collect()
            } else {
                Vec::new()
            };
            let content = serde_json::to_string_pretty(&names).map_err(|e| e.to_string())?;
            let mut file = File::create(&config_path).map_err(|e| e.to_string())?;
            file.write_all(content.as_bytes())
                .map_err(|e| e.to_string())?;
            log::info!("[Distributed] Saved disabled tools config: {:?}", names);
        }
        Ok(())
    }
}
