use crate::vcp_modules::infra::lifecycle_state::LifecycleState;
use crate::vcp_modules::infra::local_server;
use crate::vcp_modules::settings_manager::{read_settings, SettingsState};
use tauri::{AppHandle, Manager};

/// 根据设置决定启动或停止划词助手本地服务器
pub async fn reconcile_local_server(
    app_handle: &AppHandle,
    lifecycle: &LifecycleState,
    enable_assistant: bool,
) {
    let mut handle_lock = lifecycle.local_server_handle.lock().await;
    let has_server = handle_lock.is_some();

    match (enable_assistant, has_server) {
        (true, false) => {
            log::info!("[Lifecycle] enableAssistant=true, starting local server...");
            *handle_lock = Some(local_server::start_server(app_handle.clone()));
        }
        (false, true) => {
            log::info!("[Lifecycle] enableAssistant=false, stopping local server...");
            if let Some(h) = handle_lock.take() {
                h.shutdown().await;
            }
        }
        _ => {
            // 无需变更
        }
    }
}

/// 根据设置决定启动或停止分布式节点连接
pub async fn reconcile_distributed_node(
    app_handle: &AppHandle,
    distributed_enabled: bool,
    force_reconnect: bool,
) {
    let distributed_state = match app_handle.try_state::<crate::distributed::DistributedState>() {
        Some(s) => s,
        None => {
            log::warn!("[Lifecycle] DistributedState not registered, skipping reconciliation");
            return;
        }
    };
    let client = distributed_state.client.read().await;

    // 读取全局 settings，获取连接参数
    let settings_state = app_handle.state::<SettingsState>();
    let settings = match read_settings(app_handle.clone(), settings_state).await {
        Ok(s) => s,
        Err(e) => {
            log::error!(
                "[Lifecycle] Failed to read settings for distributed reconnect: {}",
                e
            );
            return;
        }
    };

    let ws_url = settings.distributed_ws_url.clone();
    let vcp_key = settings.distributed_vcp_key.clone();
    let device_name = if settings.distributed_device_name.is_empty() {
        "VCPMobile".to_string()
    } else {
        settings.distributed_device_name.clone()
    };

    let mut is_running = client.is_running().await;
    if force_reconnect && is_running {
        log::info!("[Lifecycle] Connection settings changed, stopping existing connection for reconnect...");
        client.stop(app_handle).await;
        is_running = false;
    }

    match (distributed_enabled, is_running) {
        (true, false) => {
            if ws_url.is_empty() || vcp_key.is_empty() {
                log::warn!("[Lifecycle] distributedEnabled=true but ws_url/vcp_key is empty, skipping auto-connect");
                return;
            }
            log::info!(
                "[Lifecycle] distributedEnabled=true, starting distributed node connection..."
            );
            distributed_state.registry.load_disabled_config(app_handle);
            if let Err(e) = client
                .start(
                    app_handle.clone(),
                    ws_url,
                    vcp_key,
                    device_name,
                    distributed_state.registry.clone(),
                )
                .await
            {
                log::error!("[Lifecycle] Auto-start distributed node failed: {}", e);
            }
        }
        (false, true) => {
            log::info!(
                "[Lifecycle] distributedEnabled=false, stopping distributed node connection..."
            );
            client.stop(app_handle).await;
        }
        _ => {}
    }
}
