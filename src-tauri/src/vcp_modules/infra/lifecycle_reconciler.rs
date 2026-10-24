use crate::vcp_modules::settings_manager::{read_config, SettingsState};
use tauri::{AppHandle, Manager};

/// 根据设置决定启动或停止分布式节点连接
pub async fn reconcile_distributed_node(app_handle: &AppHandle, force_reconnect: bool) {
    let distributed_state = match app_handle.try_state::<crate::distributed::DistributedState>() {
        Some(s) => s,
        None => {
            log::warn!("[Lifecycle] DistributedState not registered, skipping reconciliation");
            return;
        }
    };
    let _reconciliation = distributed_state.reconciliation.lock().await;
    let client = distributed_state.client.read().await;

    // 读取全局 settings，获取连接参数
    let settings_state = app_handle.state::<SettingsState>();
    let settings = match read_config(app_handle.clone(), settings_state).await {
        Ok(s) => s,
        Err(e) => {
            log::error!(
                "[Lifecycle] Failed to read settings for distributed reconnect: {}",
                e
            );
            return;
        }
    };

    // iOS owns no background execution resource for this node. Background
    // settings writes must not recreate a session after lifecycle suspension.
    let distributed_enabled = settings.distributed.enabled
        && (!cfg!(target_os = "ios")
            || crate::vcp_modules::infra::lifecycle_manager::is_app_in_foreground(app_handle));

    let mut is_running = client.is_running().await;
    if force_reconnect && is_running {
        log::info!("[Lifecycle] Connection settings changed, stopping existing connection for reconnect...");
        client.stop(app_handle).await;
        is_running = false;
    }

    match (distributed_enabled, is_running) {
        (true, false) => {
            if !cfg!(any(target_os = "android", target_os = "ios")) {
                log::info!(
                    "[Lifecycle] Distributed node is a mobile runtime; host startup skipped."
                );
                return;
            }
            let ws_url = if !settings.log.url.is_empty() {
                settings.log.url.clone()
            } else {
                settings.distributed.ws_url.clone()
            };
            let vcp_key = if !settings.log.key.is_empty() {
                settings.log.key.clone()
            } else {
                settings.distributed.vcp_key.clone()
            };
            let device_name = "VCPMobile".to_string();
            log::info!(
                "[Lifecycle] distributedEnabled=true, starting distributed node connection..."
            );
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
