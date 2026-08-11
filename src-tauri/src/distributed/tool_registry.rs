// distributed/tool_registry.rs
// Three-mode tool trait system + registry.
// Mirrors VCPChat/VCPDistributedServer/Plugin.js (class PluginManager)
// Self-contained — does NOT import anything from vcp_modules/.

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::Value;
use tauri::{AppHandle, Manager};

use super::types::ToolManifest;

const TOOL_CONFIG_FILE: &str = "distributed_tools.json";
const MAX_TOOL_CONFIG_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolConfigStatus {
    pub state: ToolConfigState,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolConfigState {
    Uninitialized,
    Ready,
    RecoveredDisabled,
    PersistenceError,
}

impl ToolConfigStatus {
    fn ready() -> Self {
        Self {
            state: ToolConfigState::Ready,
            message: None,
        }
    }

    fn uninitialized() -> Self {
        Self {
            state: ToolConfigState::Uninitialized,
            message: Some("工具配置尚未加载，全部工具保持禁用".to_string()),
        }
    }

    fn recovered(message: impl Into<String>) -> Self {
        Self {
            state: ToolConfigState::RecoveredDisabled,
            message: Some(message.into()),
        }
    }

    fn persistence_error(message: impl Into<String>) -> Self {
        Self {
            state: ToolConfigState::PersistenceError,
            message: Some(message.into()),
        }
    }
}

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
    config_status: RwLock<ToolConfigStatus>,
    config_update_lock: tokio::sync::Mutex<()>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            disabled_names: RwLock::new(HashSet::new()),
            config_status: RwLock::new(ToolConfigStatus::uninitialized()),
            config_update_lock: tokio::sync::Mutex::new(()),
        }
    }

    fn all_tool_names(&self) -> HashSet<String> {
        self.tools.keys().cloned().collect()
    }

    fn validate_disabled_names(&self, names: Vec<String>) -> Result<HashSet<String>, String> {
        let known = self.all_tool_names();
        let mut disabled = HashSet::with_capacity(names.len());
        for name in names {
            if !known.contains(&name) {
                return Err(format!("未知的分布式工具名称: {name}"));
            }
            if !disabled.insert(name.clone()) {
                return Err(format!("分布式工具配置包含重复名称: {name}"));
            }
        }
        Ok(disabled)
    }

    fn replace_disabled(&self, disabled: HashSet<String>) -> Result<bool, String> {
        let mut guard = self
            .disabled_names
            .write()
            .map_err(|_| "分布式工具状态锁已损坏，全部工具保持禁用".to_string())?;
        let changed = *guard != disabled;
        *guard = disabled;
        Ok(changed)
    }

    fn set_config_status(&self, status: ToolConfigStatus) {
        match self.config_status.write() {
            Ok(mut guard) => *guard = status,
            Err(poisoned) => *poisoned.into_inner() = status,
        }
    }

    fn fail_closed(&self, message: impl Into<String>) -> ToolConfigStatus {
        let message = message.into();
        match self.disabled_names.write() {
            Ok(mut guard) => *guard = self.all_tool_names(),
            Err(poisoned) => *poisoned.into_inner() = self.all_tool_names(),
        }
        let status = ToolConfigStatus::recovered(message);
        self.set_config_status(status.clone());
        status
    }

    pub fn config_status(&self) -> ToolConfigStatus {
        self.config_status
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| {
                ToolConfigStatus::persistence_error("分布式工具配置状态锁已损坏，全部工具保持禁用")
            })
    }

    /// Persist the complete disabled set before exposing it to the running node.
    /// A failed write leaves the in-memory policy unchanged.
    pub async fn persist_and_update_disabled(
        &self,
        app: &AppHandle,
        names: Vec<String>,
    ) -> Result<bool, String> {
        let disabled = self.validate_disabled_names(names)?;
        let _update_guard = self.config_update_lock.lock().await;
        let config_dir = app
            .path()
            .app_config_dir()
            .map_err(|error| format!("获取应用配置目录失败: {error}"))?;
        let config_path = config_dir.join(TOOL_CONFIG_FILE);
        let persisted = disabled.clone();
        tauri::async_runtime::spawn_blocking(move || {
            persist_disabled_config(&config_path, &persisted)
        })
        .await
        .map_err(|error| format!("分布式工具配置写任务失败: {error}"))??;
        let changed = self.replace_disabled(disabled)?;
        self.set_config_status(ToolConfigStatus::ready());
        Ok(changed)
    }

    pub async fn reset_all_disabled(&self, app: &AppHandle) -> Result<(), String> {
        let all_disabled = self.all_tool_names();
        let _update_guard = self.config_update_lock.lock().await;
        let config_dir = app
            .path()
            .app_config_dir()
            .map_err(|error| format!("获取应用配置目录失败: {error}"))?;
        let config_path = config_dir.join(TOOL_CONFIG_FILE);
        let persisted = all_disabled.clone();
        tauri::async_runtime::spawn_blocking(move || {
            persist_disabled_config(&config_path, &persisted)
        })
        .await
        .map_err(|error| format!("分布式工具配置重置任务失败: {error}"))??;
        self.replace_disabled(all_disabled)?;
        self.set_config_status(ToolConfigStatus::ready());
        Ok(())
    }

    /// Check if a tool is enabled.
    pub fn is_enabled(&self, name: &str) -> bool {
        if let Ok(guard) = self.disabled_names.read() {
            !guard.contains(name)
        } else {
            false
        }
    }

    /// Register a OneShot tool.
    pub fn register_oneshot<T: OneShotTool + 'static>(&mut self, tool: T) {
        let name = tool.manifest().name.clone();
        self.disabled_names
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(name.clone());
        self.tools.insert(name, ToolEntry::OneShot(Arc::new(tool)));
    }

    /// Register an Interactive tool.
    #[allow(dead_code)]
    pub fn register_interactive<T: InteractiveTool + 'static>(&mut self, tool: T) {
        let name = tool.manifest().name.clone();
        self.disabled_names
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(name.clone());
        self.tools
            .insert(name, ToolEntry::Interactive(Arc::new(tool)));
    }

    /// Register a Streaming tool.
    pub fn register_streaming<T: StreamingTool + 'static>(&mut self, tool: T) {
        let name = tool.manifest().name.clone();
        self.disabled_names
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(name.clone());
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
                if let Some(obj) = val.as_object_mut() {
                    let category = match entry {
                        ToolEntry::OneShot(_) => "oneshot",
                        ToolEntry::Interactive(_) => "interactive",
                        ToolEntry::Streaming(_) => "streaming",
                    };
                    obj.insert("category".to_string(), serde_json::json!(category));
                    obj.insert(
                        "enabled".to_string(),
                        serde_json::json!(self.is_enabled(name)),
                    );
                    if let Some(ref p) = manifest.placeholder {
                        obj.insert("placeholder".to_string(), serde_json::json!(p));
                    }
                    obj.insert(
                        "display_name".to_string(),
                        serde_json::json!(manifest.display_name),
                    );
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

    /// Load the policy. Missing, unreadable, malformed, oversized or unknown-name
    /// configurations fail closed to all-disabled and expose a typed recovery status.
    pub async fn load_disabled_config(&self, app: &AppHandle) -> ToolConfigStatus {
        let _update_guard = self.config_update_lock.lock().await;
        let config_dir = match app.path().app_config_dir() {
            Ok(path) => path,
            Err(error) => {
                return self.fail_closed(format!("获取应用配置目录失败: {error}"));
            }
        };
        let config_path = config_dir.join(TOOL_CONFIG_FILE);
        let load_path = config_path.clone();
        let load_known = self.all_tool_names();
        let loaded = tauri::async_runtime::spawn_blocking(move || {
            load_disabled_config_file(&load_path, &load_known)
        })
        .await
        .map_err(|error| format!("分布式工具配置读取任务失败: {error}"))
        .and_then(|result| result);

        match loaded {
            Ok(disabled) => match self.replace_disabled(disabled) {
                Ok(_) => {
                    let status = ToolConfigStatus::ready();
                    self.set_config_status(status.clone());
                    status
                }
                Err(error) => self.fail_closed(error),
            },
            Err(load_error) => {
                let all_disabled = self.all_tool_names();
                let status = self.fail_closed(load_error);
                let recovery_path = config_path.clone();
                let recovery_disabled = all_disabled.clone();
                let persisted = tauri::async_runtime::spawn_blocking(move || {
                    persist_disabled_config(&recovery_path, &recovery_disabled)
                })
                .await
                .map_err(|error| format!("全禁用恢复配置写任务失败: {error}"))
                .and_then(|result| result);
                match persisted {
                    Ok(()) => status,
                    Err(persist_error) => {
                        let status = ToolConfigStatus::persistence_error(format!(
                            "{}；全禁用恢复配置写入失败: {}",
                            status.message.unwrap_or_default(),
                            persist_error
                        ));
                        self.set_config_status(status.clone());
                        status
                    }
                }
            }
        }
    }
}

fn load_disabled_config_file(
    config_path: &Path,
    known_names: &HashSet<String>,
) -> Result<HashSet<String>, String> {
    let metadata = std::fs::metadata(config_path)
        .map_err(|error| format!("读取分布式工具配置元数据失败: {error}"))?;
    if !metadata.is_file() {
        return Err("分布式工具配置不是普通文件".to_string());
    }
    if metadata.len() > MAX_TOOL_CONFIG_BYTES {
        return Err(format!(
            "分布式工具配置超过 {} 字节上限",
            MAX_TOOL_CONFIG_BYTES
        ));
    }

    let mut file =
        File::open(config_path).map_err(|error| format!("打开分布式工具配置失败: {error}"))?;
    let mut content = String::with_capacity(metadata.len() as usize);
    file.read_to_string(&mut content)
        .map_err(|error| format!("读取分布式工具配置失败: {error}"))?;
    let names: Vec<String> = serde_json::from_str(&content)
        .map_err(|error| format!("解析分布式工具配置失败: {error}"))?;

    let mut disabled = HashSet::with_capacity(names.len());
    for name in names {
        if !known_names.contains(&name) {
            return Err(format!("配置包含未知的分布式工具名称: {name}"));
        }
        if !disabled.insert(name.clone()) {
            return Err(format!("配置包含重复的分布式工具名称: {name}"));
        }
    }
    Ok(disabled)
}

fn persist_disabled_config(config_path: &Path, disabled: &HashSet<String>) -> Result<(), String> {
    let parent = config_path
        .parent()
        .ok_or_else(|| "分布式工具配置缺少父目录".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("创建分布式工具配置目录失败: {error}"))?;

    let mut names: Vec<&String> = disabled.iter().collect();
    names.sort_unstable();
    let content = serde_json::to_vec_pretty(&names)
        .map_err(|error| format!("序列化分布式工具配置失败: {error}"))?;
    let temp_path = parent.join(format!(".{TOOL_CONFIG_FILE}.{}.tmp", uuid::Uuid::new_v4()));

    let write_result = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
            .map_err(|error| format!("创建分布式工具临时配置失败: {error}"))?;
        file.write_all(&content)
            .map_err(|error| format!("写入分布式工具临时配置失败: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("同步分布式工具临时配置失败: {error}"))?;
        std::fs::rename(&temp_path, config_path)
            .map_err(|error| format!("原子替换分布式工具配置失败: {error}"))?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("同步分布式工具配置目录失败: {error}"))?;
        Ok(())
    })();

    if write_result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    write_result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known_names() -> HashSet<String> {
        ["Clipboard", "DeviceInfo"]
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn malformed_or_unknown_config_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(TOOL_CONFIG_FILE);
        std::fs::write(&path, b"not-json").unwrap();
        assert!(load_disabled_config_file(&path, &known_names()).is_err());

        std::fs::write(&path, br#"["Clipboard","UnknownTool"]"#).unwrap();
        assert!(load_disabled_config_file(&path, &known_names()).is_err());
    }

    #[test]
    fn atomic_persist_round_trips_complete_policy() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(TOOL_CONFIG_FILE);
        let disabled = known_names();
        persist_disabled_config(&path, &disabled).unwrap();
        assert_eq!(
            load_disabled_config_file(&path, &known_names()).unwrap(),
            disabled
        );
        assert!(std::fs::read_dir(temp.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
    }

    #[test]
    fn poisoned_policy_lock_fails_closed() {
        let registry = ToolRegistry::new();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = registry.disabled_names.write().unwrap();
            panic!("poison policy lock");
        }));
        assert!(!registry.is_enabled("Clipboard"));
    }
}
