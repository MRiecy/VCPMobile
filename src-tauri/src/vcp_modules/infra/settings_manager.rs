//! Typed configuration domains, one durable owner, and post-commit runtime effects.

mod backup;
mod migration;
mod repository;
pub mod schema;
pub mod types;

pub use backup::{export_config_domains, export_group_preferences, parse_config_backup};
pub use repository::SettingsState;
pub use types::*;

use crate::vcp_modules::db_manager::DbState;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

/// Back-end readers share one coherent immutable snapshot across domain boundaries.
pub async fn read_config<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, SettingsState>,
) -> Result<Arc<Settings>, String> {
    state
        .read_config(app_handle.state::<DbState>().inner())
        .await
}

pub async fn read_config_snapshot<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, SettingsState>,
) -> Result<ConfigSnapshot, String> {
    state
        .read_snapshot(app_handle.state::<DbState>().inner())
        .await
}

#[tauri::command]
pub async fn read_settings<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, SettingsState>,
) -> Result<ConfigSnapshot, String> {
    read_config_snapshot(app_handle, state).await
}

#[tauri::command]
pub async fn update_settings<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, SettingsState>,
    request: ConfigUpdate,
) -> Result<ConfigSaveResult, String> {
    let app = app_handle.clone();
    state
        .apply(
            app_handle.state::<DbState>().inner(),
            request,
            move |old, snapshot| settings_committed(app, old, snapshot),
        )
        .await
}

#[tauri::command]
pub async fn import_settings_backup<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, SettingsState>,
    request: ConfigUpdate,
    group_preferences: serde_json::Value,
) -> Result<ConfigSaveResult, String> {
    let preferences =
        crate::vcp_modules::group::group_service::parse_group_local_settings_backup(&group_preferences)?;
    let group_ids: Vec<_> = preferences.keys().cloned().collect();
    let app = app_handle.clone();
    state
        .import_backup(
            app_handle.state::<DbState>().inner(),
            request,
            preferences,
            move |old, snapshot| {
                if let Some(groups) = app.try_state::<crate::vcp_modules::group::group_service::GroupManagerState>() {
                    groups.invalidate_cache();
                }
                if let Some(turns) = app.try_state::<crate::vcp_modules::vcp_client::ActiveGroupTurns>() {
                    for id in &group_ids {
                        turns.notify_group_changed(id);
                    }
                }
                settings_committed(app, old, snapshot);
            },
        )
        .await
}

fn settings_committed<R: Runtime>(
    app: AppHandle<R>,
    old: Arc<Settings>,
    snapshot: Arc<ConfigSnapshot>,
) {
    let _ = app.emit("vcp-config-changed", snapshot.as_ref());
    let changes = classify_runtime_settings_changes(Some(&old), &snapshot.data);
    let flags =
        u8::from(changes.vcp_log_changed) | (u8::from(changes.distributed_reconcile_required) << 1);
    if flags != 0 {
        // The mock/host exporter need not install the concrete mobile handle.
        if let Some(concrete) = app.try_state::<tauri::AppHandle>() {
            let concrete_app = concrete.inner().clone();
            let state = app.state::<SettingsState>();
            let generation = state.reserve_runtime_reconcile(flags);
            tauri::async_runtime::spawn(reconcile_current_runtime_settings(
                concrete_app,
                generation,
            ));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuntimeSettingsChanges {
    vcp_log_changed: bool,
    distributed_reconcile_required: bool,
}

fn classify_runtime_settings_changes(
    old: Option<&Settings>,
    settings: &Settings,
) -> RuntimeSettingsChanges {
    let vcp_log_changed = old.map_or_else(
        || !settings.log.url.is_empty() || !settings.log.key.is_empty(),
        |old| old.log.url != settings.log.url || old.log.key != settings.log.key,
    );
    let distributed_reconcile_required = old.map_or(settings.distributed.enabled, |old| {
        let enabled_changed = old.distributed.enabled != settings.distributed.enabled;
        let params_changed = old.distributed.ws_url != settings.distributed.ws_url
            || old.distributed.vcp_key != settings.distributed.vcp_key
            || old.distributed.device_name != settings.distributed.device_name
            || old.log.url != settings.log.url
            || old.log.key != settings.log.key;
        enabled_changed || (params_changed && settings.distributed.enabled)
    });
    RuntimeSettingsChanges {
        vcp_log_changed,
        distributed_reconcile_required,
    }
}

async fn reconcile_current_runtime_settings(app_handle: AppHandle, generation: u64) {
    let state = app_handle.state::<SettingsState>();
    let _runtime_guard = state.lock_runtime_reconcile().await;
    if !state.is_runtime_generation_current(generation) {
        log::debug!("[Settings] Skipping stale runtime reconciliation generation={generation}");
        return;
    }

    let changes = state.take_runtime_changes();
    let settings = match read_config(app_handle.clone(), app_handle.state()).await {
        Ok(settings) => settings,
        Err(error) => {
            log::error!("[Settings] Runtime reconciliation could not read settings: {error}");
            return;
        }
    };

    if changes & 1 != 0 {
        if let Err(error) = crate::vcp_modules::vcp_log_service::init_vcp_log_connection_internal(
            app_handle.clone(),
            settings.log.url.clone(),
            settings.log.key.clone(),
        )
        .await
        {
            log::error!("[Settings] VCPLog runtime reconciliation failed: {error}");
        }
        if let Err(error) = crate::vcp_modules::vcp_info_service::init_vcp_info_connection_internal(
            app_handle.clone(),
            settings.log.url.clone(),
            settings.log.key.clone(),
        )
        .await
        {
            log::error!("[Settings] VCPInfo runtime reconciliation failed: {error}");
        }
    }
    if changes & 2 != 0 {
        crate::vcp_modules::infra::lifecycle_manager::reconcile_distributed_node(&app_handle, true)
            .await;
    }
}
