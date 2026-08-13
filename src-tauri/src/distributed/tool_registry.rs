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
const TOOL_CONFIG_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, PartialEq, Eq)]
enum LoadedToolConfig {
    Missing,
    LegacyDisabledNames,
    Current {
        enabled: HashSet<String>,
        orphaned: Vec<String>,
    },
}

struct ResolvedToolConfig {
    enabled: HashSet<String>,
    rewrite_empty: bool,
    status: ToolConfigStatus,
}

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

    fn ready_with_message(message: impl Into<String>) -> Self {
        Self {
            state: ToolConfigState::Ready,
            message: Some(message.into()),
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
    enabled_names: RwLock<HashSet<String>>,
    config_status: RwLock<ToolConfigStatus>,
    config_update_lock: tokio::sync::Mutex<()>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            enabled_names: RwLock::new(HashSet::new()),
            config_status: RwLock::new(ToolConfigStatus::uninitialized()),
            config_update_lock: tokio::sync::Mutex::new(()),
        }
    }

    fn all_tool_names(&self) -> HashSet<String> {
        self.tools.keys().cloned().collect()
    }

    fn validate_known_names(
        &self,
        names: Vec<String>,
        policy_label: &str,
    ) -> Result<HashSet<String>, String> {
        let known = self.all_tool_names();
        let mut validated = HashSet::with_capacity(names.len());
        for name in names {
            if !known.contains(&name) {
                return Err(format!("未知的分布式工具名称: {name}"));
            }
            if !validated.insert(name.clone()) {
                return Err(format!("分布式工具{policy_label}包含重复名称: {name}"));
            }
        }
        Ok(validated)
    }

    fn validate_enabled_names(&self, names: Vec<String>) -> Result<HashSet<String>, String> {
        self.validate_known_names(names, "授权配置")
    }

    fn replace_enabled(&self, enabled: HashSet<String>) -> Result<bool, String> {
        let mut guard = self
            .enabled_names
            .write()
            .map_err(|_| "分布式工具状态锁已损坏，全部工具保持禁用".to_string())?;
        let changed = *guard != enabled;
        *guard = enabled;
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
        match self.enabled_names.write() {
            Ok(mut guard) => guard.clear(),
            Err(poisoned) => poisoned.into_inner().clear(),
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

    /// Persist the complete enabled allowlist before exposing it to the running node.
    /// A failed write leaves the in-memory policy unchanged.
    pub async fn persist_and_update_enabled(
        &self,
        app: &AppHandle,
        names: Vec<String>,
    ) -> Result<bool, String> {
        let enabled = self.validate_enabled_names(names)?;
        let _update_guard = self.config_update_lock.lock().await;
        self.persist_and_replace_enabled_locked(app, enabled).await
    }

    async fn persist_and_replace_enabled_locked(
        &self,
        app: &AppHandle,
        enabled: HashSet<String>,
    ) -> Result<bool, String> {
        let config_dir = app
            .path()
            .app_config_dir()
            .map_err(|error| format!("获取应用配置目录失败: {error}"))?;
        let config_path = config_dir.join(TOOL_CONFIG_FILE);
        let persisted = enabled.clone();
        tauri::async_runtime::spawn_blocking(move || {
            persist_enabled_config(&config_path, &persisted)
        })
        .await
        .map_err(|error| format!("分布式工具配置写任务失败: {error}"))??;
        let changed = self.replace_enabled(enabled)?;
        self.set_config_status(ToolConfigStatus::ready());
        Ok(changed)
    }

    pub async fn reset_all_disabled(&self, app: &AppHandle) -> Result<(), String> {
        let _update_guard = self.config_update_lock.lock().await;
        self.persist_and_replace_enabled_locked(app, HashSet::new())
            .await?;
        Ok(())
    }

    /// Check if a tool is enabled.
    pub fn is_enabled(&self, name: &str) -> bool {
        if !self.tools.contains_key(name) {
            return false;
        }
        if let Ok(guard) = self.enabled_names.read() {
            guard.contains(name)
        } else {
            false
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

    /// Get enabled tool manifests for the register_tools message.
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

    /// Number of tools discovered in the local catalog.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// Load the enabled allowlist. A missing file creates an empty v2 policy.
    /// Legacy disabled-name files and malformed policies are explicitly invalidated
    /// and rewritten as an empty allowlist; no complement-based migration is allowed.
    pub async fn load_enabled_config(&self, app: &AppHandle) -> ToolConfigStatus {
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
            load_tool_config_file(&load_path, &load_known)
        })
        .await
        .map_err(|error| format!("分布式工具配置读取任务失败: {error}"))
        .and_then(|result| result);

        let resolved = resolve_tool_config(loaded);
        if let Err(error) = self.replace_enabled(resolved.enabled) {
            return self.fail_closed(error);
        }

        let status = resolved.status;
        self.set_config_status(status.clone());
        if !resolved.rewrite_empty {
            return status;
        }

        let recovery_path = config_path.clone();
        let persisted = tauri::async_runtime::spawn_blocking(move || {
            persist_enabled_config(&recovery_path, &HashSet::new())
        })
        .await
        .map_err(|error| format!("空授权恢复配置写任务失败: {error}"))
        .and_then(|result| result);
        match persisted {
            Ok(()) => status,
            Err(persist_error) => {
                let status = ToolConfigStatus::persistence_error(format!(
                    "{}；空授权恢复配置写入失败: {}",
                    status
                        .message
                        .unwrap_or_else(|| "全部工具保持禁用".to_string()),
                    persist_error
                ));
                self.set_config_status(status.clone());
                status
            }
        }
    }
}

fn resolve_tool_config(loaded: Result<LoadedToolConfig, String>) -> ResolvedToolConfig {
    match loaded {
        Ok(LoadedToolConfig::Missing) => ResolvedToolConfig {
            enabled: HashSet::new(),
            rewrite_empty: true,
            status: ToolConfigStatus::ready(),
        },
        Ok(LoadedToolConfig::LegacyDisabledNames) => ResolvedToolConfig {
            enabled: HashSet::new(),
            rewrite_empty: true,
            status: ToolConfigStatus::recovered(
                "检测到旧版 disabled-name 工具配置。旧配置已显式失效，未反向推导任何授权；请逐个重新授权需要的工具。",
            ),
        },
        Ok(LoadedToolConfig::Current { enabled, orphaned }) => {
            let status = if orphaned.is_empty() {
                ToolConfigStatus::ready()
            } else {
                ToolConfigStatus::ready_with_message(format!(
                    "授权配置包含已下架工具，已忽略: {}",
                    orphaned.join(", ")
                ))
            };
            ResolvedToolConfig {
                enabled,
                rewrite_empty: false,
                status,
            }
        }
        Err(error) => ResolvedToolConfig {
            enabled: HashSet::new(),
            rewrite_empty: true,
            status: ToolConfigStatus::recovered(format!(
                "{error}；配置已失效，未授权任何分布式工具。"
            )),
        },
    }
}

fn load_tool_config_file(
    config_path: &Path,
    known_names: &HashSet<String>,
) -> Result<LoadedToolConfig, String> {
    let metadata = match std::fs::metadata(config_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LoadedToolConfig::Missing);
        }
        Err(error) => {
            return Err(format!("读取分布式工具配置元数据失败: {error}"));
        }
    };
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
    let value: Value = serde_json::from_str(&content)
        .map_err(|error| format!("解析分布式工具配置失败: {error}"))?;

    if value.is_array() {
        serde_json::from_value::<Vec<String>>(value)
            .map_err(|error| format!("解析旧版 disabled-name 工具配置失败: {error}"))?;
        return Ok(LoadedToolConfig::LegacyDisabledNames);
    }

    let object = value
        .as_object()
        .ok_or_else(|| "分布式工具授权配置必须是 JSON 对象".to_string())?;
    let schema_version = object
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .ok_or_else(|| "分布式工具授权配置缺少有效 schemaVersion".to_string())?;
    if schema_version != u64::from(TOOL_CONFIG_SCHEMA_VERSION) {
        return Err(format!("不支持的分布式工具配置版本: {}", schema_version));
    }
    let names = object
        .get("enabledTools")
        .cloned()
        .ok_or_else(|| "分布式工具授权配置缺少 enabledTools".to_string())?;
    let names: Vec<String> = serde_json::from_value(names)
        .map_err(|error| format!("解析 enabledTools 失败: {error}"))?;

    let mut seen = HashSet::with_capacity(names.len());
    let mut enabled = HashSet::with_capacity(names.len());
    let mut orphaned = Vec::new();
    for name in names {
        if !seen.insert(name.clone()) {
            return Err(format!("授权配置包含重复的分布式工具名称: {name}"));
        }
        if known_names.contains(&name) {
            enabled.insert(name);
        } else {
            orphaned.push(name);
        }
    }
    orphaned.sort_unstable();
    Ok(LoadedToolConfig::Current { enabled, orphaned })
}

fn persist_enabled_config(config_path: &Path, enabled: &HashSet<String>) -> Result<(), String> {
    let parent = config_path
        .parent()
        .ok_or_else(|| "分布式工具配置缺少父目录".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("创建分布式工具配置目录失败: {error}"))?;

    let mut names: Vec<String> = enabled.iter().cloned().collect();
    names.sort_unstable();
    let document = serde_json::json!({
        "schemaVersion": TOOL_CONFIG_SCHEMA_VERSION,
        "enabledTools": names,
    });
    let content = serde_json::to_vec_pretty(&document)
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

    struct DummyTool {
        name: &'static str,
        description: &'static str,
    }

    #[async_trait]
    impl OneShotTool for DummyTool {
        fn manifest(&self) -> ToolManifest {
            ToolManifest {
                name: self.name.to_string(),
                description: self.description.to_string(),
                display_name: self.name.to_string(),
                placeholder: None,
                invocation_commands: vec![],
            }
        }

        async fn execute(&self, _args: Value, _app: &AppHandle) -> Result<Value, String> {
            Ok(Value::Null)
        }
    }

    fn known_names() -> HashSet<String> {
        ["Clipboard", "DeviceInfo"]
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    fn test_registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register_oneshot(DummyTool {
            name: "Clipboard",
            description: "clipboard",
        });
        registry.register_oneshot(DummyTool {
            name: "DeviceInfo",
            description: "device",
        });
        registry
    }

    fn enabled_metadata(registry: &ToolRegistry, name: &str) -> bool {
        registry
            .get_tools_metadata()
            .into_iter()
            .find(|item| item.get("name").and_then(Value::as_str) == Some(name))
            .and_then(|item| item.get("enabled").and_then(Value::as_bool))
            .unwrap_or(false)
    }

    #[test]
    fn clean_install_catalog_is_visible_but_allowlist_is_empty() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(TOOL_CONFIG_FILE);
        let loaded = load_tool_config_file(&path, &known_names()).unwrap();
        assert_eq!(loaded, LoadedToolConfig::Missing);

        let resolved = resolve_tool_config(Ok(loaded));
        assert!(resolved.enabled.is_empty());
        assert!(resolved.rewrite_empty);
        assert_eq!(resolved.status.state, ToolConfigState::Ready);

        let registry = test_registry();
        assert_eq!(registry.get_tools_metadata().len(), 2);
        assert!(!enabled_metadata(&registry, "Clipboard"));
        assert!(!enabled_metadata(&registry, "DeviceInfo"));
        assert!(registry.get_all_manifests().is_empty());
    }

    #[test]
    fn legacy_disabled_names_are_explicitly_invalidated_to_empty_allowlist() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(TOOL_CONFIG_FILE);
        std::fs::write(&path, br#"["Clipboard"]"#).unwrap();

        let loaded = load_tool_config_file(&path, &known_names()).unwrap();
        assert_eq!(loaded, LoadedToolConfig::LegacyDisabledNames);
        let resolved = resolve_tool_config(Ok(loaded));
        assert!(resolved.enabled.is_empty());
        assert!(resolved.rewrite_empty);
        assert_eq!(resolved.status.state, ToolConfigState::RecoveredDisabled);
        assert!(resolved
            .status
            .message
            .as_deref()
            .unwrap_or_default()
            .contains("重新授权"));

        persist_enabled_config(&path, &resolved.enabled).unwrap();
        assert_eq!(
            load_tool_config_file(&path, &known_names()).unwrap(),
            LoadedToolConfig::Current {
                enabled: HashSet::new(),
                orphaned: vec![],
            }
        );
    }

    #[test]
    fn malformed_or_duplicate_v2_config_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(TOOL_CONFIG_FILE);
        std::fs::write(&path, b"not-json").unwrap();
        let resolved = resolve_tool_config(load_tool_config_file(&path, &known_names()));
        assert!(resolved.enabled.is_empty());
        assert_eq!(resolved.status.state, ToolConfigState::RecoveredDisabled);

        std::fs::write(
            &path,
            br#"{"schemaVersion":2,"enabledTools":["Clipboard","Clipboard"]}"#,
        )
        .unwrap();
        let resolved = resolve_tool_config(load_tool_config_file(&path, &known_names()));
        assert!(resolved.enabled.is_empty());
        assert_eq!(resolved.status.state, ToolConfigState::RecoveredDisabled);
    }

    #[test]
    fn atomic_persist_round_trips_versioned_enabled_allowlist() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(TOOL_CONFIG_FILE);
        let enabled = known_names();
        persist_enabled_config(&path, &enabled).unwrap();
        assert_eq!(
            load_tool_config_file(&path, &known_names()).unwrap(),
            LoadedToolConfig::Current {
                enabled,
                orphaned: vec![],
            }
        );
        let persisted: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(persisted.get("schemaVersion"), Some(&Value::from(2)));
        assert!(persisted.get("enabledTools").is_some());
        assert!(persisted.get("disabledTools").is_none());
        assert!(std::fs::read_dir(temp.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
    }

    #[test]
    fn newly_scanned_tool_stays_off_while_existing_grant_survives() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(TOOL_CONFIG_FILE);
        persist_enabled_config(&path, &HashSet::from(["Clipboard".to_string()])).unwrap();

        let loaded = load_tool_config_file(&path, &known_names()).unwrap();
        let resolved = resolve_tool_config(Ok(loaded));
        assert_eq!(resolved.enabled, HashSet::from(["Clipboard".to_string()]));
        assert!(!resolved.rewrite_empty);

        let registry = test_registry();
        registry.replace_enabled(resolved.enabled).unwrap();
        assert!(enabled_metadata(&registry, "Clipboard"));
        assert!(!enabled_metadata(&registry, "DeviceInfo"));
        let manifests = registry.get_all_manifests();
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].name, "Clipboard");
    }

    #[test]
    fn per_tool_enable_disable_and_duplicate_registration_are_idempotent() {
        let mut registry = test_registry();
        registry.register_oneshot(DummyTool {
            name: "Clipboard",
            description: "replacement",
        });
        assert_eq!(registry.tool_count(), 2);
        assert!(registry.get_all_manifests().is_empty());

        registry
            .replace_enabled(HashSet::from(["Clipboard".to_string()]))
            .unwrap();
        let manifests = registry.get_all_manifests();
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].name, "Clipboard");
        assert_eq!(manifests[0].description, "replacement");

        assert!(registry
            .validate_enabled_names(vec!["Clipboard".to_string(), "Clipboard".to_string()])
            .is_err());
        registry.replace_enabled(HashSet::new()).unwrap();
        assert!(registry.get_all_manifests().is_empty());
        assert!(!registry.is_enabled("Clipboard"));
    }

    #[test]
    fn orphaned_v2_grants_are_ignored_without_enabling_new_catalog_entries() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(TOOL_CONFIG_FILE);
        let persisted = HashSet::from(["Clipboard".to_string(), "RemovedTool".to_string()]);
        persist_enabled_config(&path, &persisted).unwrap();

        let resolved = resolve_tool_config(load_tool_config_file(&path, &known_names()));
        assert_eq!(resolved.enabled, HashSet::from(["Clipboard".to_string()]));
        assert_eq!(resolved.status.state, ToolConfigState::Ready);
        assert!(resolved.status.message.is_some());
    }

    #[test]
    fn future_v2_metadata_does_not_invalidate_the_authorization_core() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(TOOL_CONFIG_FILE);
        std::fs::write(
            &path,
            br#"{"schemaVersion":2,"enabledTools":["Clipboard"],"migrationNoticeAcknowledged":true}"#,
        )
        .unwrap();

        assert_eq!(
            load_tool_config_file(&path, &known_names()).unwrap(),
            LoadedToolConfig::Current {
                enabled: HashSet::from(["Clipboard".to_string()]),
                orphaned: vec![],
            }
        );
    }

    #[test]
    fn poisoned_policy_lock_fails_closed() {
        let registry = test_registry();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = registry.enabled_names.write().unwrap();
            panic!("poison policy lock");
        }));
        assert!(!registry.is_enabled("Clipboard"));
    }
}
