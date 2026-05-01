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
// Tauri commands — the 4 entry points registered in lib.rs
// ============================================================

/// Phase 2 sensor bridge: frontend pushes sensor data into the shared store.
/// Called by SensorCollector.vue via invoke("update_sensor_data", { key, value }).
#[tauri::command]
pub fn update_sensor_data(key: String, value: String) {
    tools::frontend_bridge::update_sensor(&key, value);
}

/// Start the distributed node connection.
#[tauri::command]
pub async fn start_distributed_node(
    app: tauri::AppHandle,
    state: State<'_, DistributedState>,
    ws_url: String,
    vcp_key: String,
    device_name: String,
) -> Result<(), String> {
    let registry = state.registry.clone();
    let client = state.client.read().await;
    client
        .start(app, ws_url, vcp_key, device_name, registry)
        .await
}

/// Stop the distributed node connection.
#[tauri::command]
pub async fn stop_distributed_node(
    state: State<'_, DistributedState>,
) -> Result<(), String> {
    let client = state.client.read().await;
    client.stop().await;
    Ok(())
}

/// Get current distributed node status.
#[tauri::command]
pub async fn get_distributed_status(
    state: State<'_, DistributedState>,
) -> Result<types::DistributedStatus, String> {
    let client = state.client.read().await;
    Ok(client.get_status().await)
}
