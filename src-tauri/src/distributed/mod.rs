// distributed/mod.rs
// Self-contained distributed node module.
// Does NOT depend on any vcp_modules/ code.
// To remove: delete this directory + 3 references in lib.rs → cargo check passes.

pub mod client;
pub mod tool_registry;
pub mod tools;
pub mod types;

use std::sync::Arc;

use client::DistributedClient;
use tauri::State;
use tokio::sync::RwLock;

/// Managed state for the distributed node. Registered via app.manage() in lib.rs.
pub struct DistributedState {
    pub client: RwLock<DistributedClient>,
    pub registry: Arc<tool_registry::ToolRegistry>,
}

impl DistributedState {
    pub fn new() -> Self {
        let registry = Arc::new(tools::build_registry());
        Self {
            client: RwLock::new(DistributedClient::new()),
            registry,
        }
    }
}

// ============================================================
// Tauri commands — entry points registered in lib.rs
// ============================================================

/// Get current distributed node status.
#[tauri::command]
pub async fn get_distributed_status(
    state: State<'_, DistributedState>,
) -> Result<types::DistributedStatus, String> {
    let client = state.client.read().await;
    Ok(client.get_status().await)
}

/// Get all registered tools metadata for frontend display.
#[tauri::command]
pub async fn get_registered_tools_metadata(
    state: State<'_, DistributedState>,
) -> Result<Vec<serde_json::Value>, String> {
    Ok(state.registry.get_tools_metadata())
}

/// Update disabled tools list and re-register if connected.
#[tauri::command]
pub async fn update_disabled_tools(
    app: tauri::AppHandle,
    state: State<'_, DistributedState>,
    disabled_names: Vec<String>,
) -> Result<(), String> {
    let changed = state.registry.update_disabled(disabled_names);

    if changed {
        let _ = state.registry.save_disabled_config(&app);
        let client = state.client.read().await;
        if client.is_connected().await {
            client.re_register_tools().await;
        }
    }
    Ok(())
}

/// Execute a distributed tool by name.
#[tauri::command]
pub async fn execute_distributed_tool(
    app: tauri::AppHandle,
    state: State<'_, DistributedState>,
    name: String,
) -> Result<String, String> {
    let res = state
        .registry
        .execute(&name, serde_json::Value::Null, &app)
        .await?;
    match res {
        serde_json::Value::String(s) => Ok(s),
        other => Ok(other.to_string()),
    }
}

/// Trigger immediate reconnect of the distributed client.
#[tauri::command]
pub async fn reconnect_distributed_client(
    state: State<'_, DistributedState>,
) -> Result<(), String> {
    let client = state.client.read().await;
    client.trigger_reconnect().await;
    Ok(())
}
