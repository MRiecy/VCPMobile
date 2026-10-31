use log::info;
use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::vcp_modules::db_manager::{init_db, DbState};
use crate::vcp_modules::emoticon_manager::{
    internal_load_library, refresh_emoticon_library_internal, EmoticonManagerState,
};
use crate::vcp_modules::model_manager::{init_model_manager, ModelManagerState};
use crate::vcp_modules::settings_manager::{read_config, SettingsState};
use crate::vcp_modules::sync_service::init_sync_service;
use crate::vcp_modules::vcp_log_service::reconnect_log_connections;

// Re-export submodules to preserve public API compatibility
pub use crate::vcp_modules::infra::lifecycle_controller::is_app_in_foreground;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use crate::vcp_modules::infra::lifecycle_controller::{
    reserve_lifecycle_transition, set_app_foreground_state_for_epoch,
};
pub use crate::vcp_modules::infra::lifecycle_reconciler::reconcile_distributed_node;
pub use crate::vcp_modules::infra::lifecycle_state::{CoreStatus, LifecycleState};

async fn persist_delete_cleanup_marker(db_state: &DbState, now: i64) -> Result<(), String> {
    let mut tx = db_state
        .write_transaction("maintenance.cleanup-marker")
        .await?;
    sqlx::query("INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('delete_executor_last_cleanup', ?, ?)")
        .bind(now.to_string())
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|error| error.to_string())?;
    tx.commit().await.map_err(|error| error.to_string())?;
    Ok(())
}

/// 核心启动逻辑：线性化管理所有服务的初始化顺序
pub async fn bootstrap(app: &AppHandle) -> Result<(), String> {
    let lifecycle = app.state::<LifecycleState>();
    let handle = app.clone();

    info!("[Lifecycle] Starting bootstrap sequence...");
    crate::vcp_modules::boot_trace::boot_mark(&handle, "rs:bootstrap_begin");

    // 发射初始状态
    let _ = handle.emit(
        "vcp-system-event",
        serde_json::json!({
            "type": "vcp-core-status",
            "status": "initializing",
            "message": "核心引擎初始化中...",
            "source": "Core"
        }),
    );

    // 1. 数据库初始化 (P0 - 绝对基础)
    match init_db(&handle).await {
        Ok((p, path)) => {
            handle.manage(DbState::new(p, path));
        }
        Err(e) => {
            let err_msg = format!("数据库初始化失败: {}", e);
            *lifecycle.last_error.write().await = Some(err_msg.clone());
            *lifecycle.status_message.write().await = err_msg.clone();
            *lifecycle.status.write().await = CoreStatus::Error;

            // 发射致命错误
            let _ = handle.emit(
                "vcp-system-event",
                serde_json::json!({
                    "type": "vcp-core-status",
                    "status": "error",
                    "message": &err_msg,
                    "source": "Core"
                }),
            );
            return Err(err_msg);
        }
    }

    let db_state = handle.state::<DbState>();
    crate::vcp_modules::boot_trace::boot_mark(&handle, "rs:init_db_done");
    let attachment_cleanup = async {
        crate::vcp_modules::file_manager::reconcile_attachment_storage(&handle, &db_state).await?;
        crate::vcp_modules::maintenance_manager::reclaim_orphaned_attachments(&handle, &db_state).await
    }.await;
    match attachment_cleanup {
        Ok(report) if report.reclaimed > 0 || report.ghost_files > 0 => {
            info!(
                "[Lifecycle] Rechecked attachment candidates and removed {} indexed CAS entries and {} orphan files.",
                report.reclaimed, report.ghost_files
            );
        }
        Ok(_) => {}
        Err(error) => {
            log::warn!("[Lifecycle] Attachment path repair/GC skipped after error: {error}");
        }
    }
    crate::vcp_modules::boot_trace::boot_mark(&handle, "rs:attachment_gc_done");

    // 2. 基础状态管理注册已在 lib.rs 中的 setup 阶段提前同步完成，此处无需重复注册以避免覆盖已有缓存。

    // 3. 配置预加载 (P1 - 前端强依赖)
    // 将配置读取前置，确保前端 Ready 后 fetchSettings 必然成功
    let settings_state = handle.state::<SettingsState>();
    let settings = match read_config(handle.clone(), settings_state).await {
        Ok(s) => s,
        Err(e) => {
            let err_msg = format!("基础配置读取失败: {}", e);
            *lifecycle.last_error.write().await = Some(err_msg.clone());
            *lifecycle.status_message.write().await = err_msg.clone();
            *lifecycle.status.write().await = CoreStatus::Error;
            let _ = handle.emit(
                "vcp-system-event",
                serde_json::json!({
                    "type": "vcp-core-status",
                    "status": "error",
                    "message": &err_msg,
                    "source": "Core"
                }),
            );
            return Err(err_msg);
        }
    };
    crate::vcp_modules::boot_trace::boot_mark(&handle, "rs:read_settings_done");

    // Tool authorization is an offline policy and must be ready before any catalog read or
    // network reconciliation. Distributed being disabled must not hide persisted grants or
    // migration warnings from the plugin center.
    {
        let distributed_state = handle.state::<crate::distributed::DistributedState>();
        let tool_config_status = distributed_state
            .registry
            .ensure_enabled_config_loaded(&handle)
            .await;
        if tool_config_status.state != crate::distributed::tool_registry::ToolConfigState::Ready {
            log::warn!(
                "[Lifecycle] Distributed tool authorization loaded fail-closed: {:?}",
                tool_config_status
            );
        }
    }
    crate::vcp_modules::boot_trace::boot_mark(&handle, "rs:tool_auth_done");

    // 3.6 根据设置决定是否启动分布式节点 (自动重连)
    {
        let enable_dist = settings.distributed.enabled;
        log::info!(
            "[Lifecycle] distributedEnabled={}, reconciling distributed node...",
            enable_dist
        );
        let settings_state = handle.state::<SettingsState>();
        let _runtime_guard = settings_state.lock_runtime_reconcile().await;
        reconcile_distributed_node(&handle, false).await;
    }
    crate::vcp_modules::boot_trace::boot_mark(&handle, "rs:distributed_reconcile_done");

    // 初始化同步服务
    let sync_state = init_sync_service(handle.clone());
    handle.manage(sync_state);
    crate::vcp_modules::boot_trace::boot_mark(&handle, "rs:sync_init_done");

    // 4. 服务级后台初始化 (P2 - 非阻塞)
    {
        let h = handle.clone();

        tokio::spawn(async move {
            let emoticon_state = h.state::<EmoticonManagerState>();
            if let Ok(lib) = internal_load_library(&h).await {
                *emoticon_state.library.lock().await = lib;
                info!("[Lifecycle] Emoticon library loaded from DB.");
            }

            // Best-effort refresh from server (does not block startup)
            match refresh_emoticon_library_internal(&h, false).await {
                Ok(count) => info!(
                    "[Lifecycle] Emoticon library auto-refreshed: {} items",
                    count
                ),
                Err(e) => info!("[Lifecycle] Emoticon auto-refresh skipped: {}", e),
            }
        });
    }

    // 通知连接独立启动，表情库的网络刷新不能延迟实时通道。
    {
        let h = handle.clone();

        tokio::spawn(async move {
            // 自动连接 VCP Log。执行前在 Settings runtime owner 下重读，避免启动
            // 后台任务用旧快照覆盖用户刚保存的新连接参数。
            let settings_state = h.state::<SettingsState>();
            let _runtime_guard = settings_state.lock_runtime_reconcile().await;
            if let Ok(latest) = read_config(h.clone(), h.state()).await {
                if let Err(error) = reconnect_log_connections(
                    &h,
                    latest.log.url.clone(),
                    latest.log.key.clone(),
                )
                .await
                {
                    log::warn!("[Lifecycle] Cannot restore VCPLog/Info connections: {error}");
                }
            }
        });
    }

    {
        let h = handle.clone();
        tokio::spawn(async move {
            let model_state = h.state::<ModelManagerState>();
            init_model_manager(&h, &model_state).await;
            info!("[Lifecycle] Model manager initialized in background.");
        });
    }

    // DeleteExecutor 定时清理（原在 sync_service.rs 常驻循环中，现移至此处）
    {
        let h = handle.clone();
        tokio::spawn(async move {
            // 启动延时 10 秒后执行首航清理，完美避开冷启动黄金 IO 密集期
            tokio::time::sleep(Duration::from_secs(10)).await;

            let db_state = h.state::<DbState>();
            let pool = &db_state.pool;

            let mut should_cleanup = true;
            {
                use sqlx::Row;
                if let Ok(Some(row)) = sqlx::query(
                    "SELECT value FROM settings WHERE key = 'delete_executor_last_cleanup'",
                )
                .fetch_optional(pool)
                .await
                {
                    let last_cleanup_str: String = row.get("value");
                    if let Ok(last_cleanup) = last_cleanup_str.parse::<i64>() {
                        let now = crate::vcp_modules::infra::utils::now_millis();
                        // 24h = 86_400_000 ms
                        if now - last_cleanup < 86_400_000 {
                            log::info!("[Lifecycle] DeleteExecutor last cleanup ran at {} (less than 24h ago). Skipping startup cleanup.", last_cleanup);
                            should_cleanup = false;
                        }
                    }
                }
            }

            if should_cleanup {
                use crate::vcp_modules::sync_executor::delete_executor::DeleteExecutor;
                if DeleteExecutor::cleanup_old_deleted_records(&h, 30)
                    .await
                    .is_ok()
                {
                    let now = crate::vcp_modules::infra::utils::now_millis();
                    if let Err(error) = persist_delete_cleanup_marker(&db_state, now).await {
                        log::warn!("[Lifecycle] Failed to persist cleanup marker: {error}");
                    }
                }
            }

            loop {
                tokio::time::sleep(Duration::from_secs(86400)).await;
                use crate::vcp_modules::sync_executor::delete_executor::DeleteExecutor;
                if DeleteExecutor::cleanup_old_deleted_records(&h, 30)
                    .await
                    .is_ok()
                {
                    let now = crate::vcp_modules::infra::utils::now_millis();
                    if let Err(error) = persist_delete_cleanup_marker(&db_state, now).await {
                        log::warn!("[Lifecycle] Failed to persist cleanup marker: {error}");
                    }
                }
            }
        });
    }

    // 5. 标记为就绪
    *lifecycle.status_message.write().await = "核心引擎已就绪".to_string();
    *lifecycle.last_error.write().await = None;
    *lifecycle.status.write().await = CoreStatus::Ready;

    // 发射 Ready 信号
    let _ = handle.emit(
        "vcp-system-event",
        serde_json::json!({
            "type": "vcp-core-status",
            "status": "ready",
            "message": "核心引擎已就绪",
            "source": "Core"
        }),
    );

    info!("[Lifecycle] Bootstrap complete. Core is READY.");
    crate::vcp_modules::boot_trace::boot_mark(&handle, "rs:core_ready");

    // 原生 Channel 在状态 owner 就绪后安装；订阅时回放当前状态，补齐冷启动期间的变化。
    // 独立任务等待真实注册结果，不阻塞 bootstrap，也不将原生响应延迟视为失败。
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let handle_net = handle.clone();
        tauri::async_runtime::spawn(async move {
            let event_handle = handle_net.clone();
            if let Err(error) =
                tauri_plugin_vcp_mobile::events::subscribe(&handle_net, move |event| {
                    use tauri_plugin_vcp_mobile::events::NativeEvent;
                    // Native 回调只提交内存事实/安排任务，不在 UIKit/JNI 线程等待业务或原生命令。
                    match event {
                        NativeEvent::Lifecycle(event) => {
                            crate::vcp_modules::infra::lifecycle_controller::enqueue_native_lifecycle_event(
                                event_handle.clone(), event,
                            );
                        }
                        NativeEvent::Network { connected } => {
                            #[cfg(target_os = "ios")]
                            {
                                // Vue reuses its existing READY/active/in-flight recovery gate.
                                // This is an observed path fact, not a guarantee of Internet reachability.
                                let _ = event_handle.emit(
                                    "vcp-network-changed",
                                    serde_json::json!({ "connected": connected }),
                                );
                            }
                            #[cfg(target_os = "android")]
                            if connected {
                                let app = event_handle.clone();
                                tauri::async_runtime::spawn(async move {
                                    if let Some(state) =
                                        app.try_state::<crate::distributed::DistributedState>()
                                    {
                                        state.client.read().await.trigger_reconnect().await;
                                    }
                                });
                            }
                        }
                        NativeEvent::NotificationClick(payload) => {
                            let _ = event_handle.emit("vcp-notification-click", payload);
                        }
                        NativeEvent::BackgroundExecutionFailure { reason } => {
                            tauri_plugin_vcp_mobile::events::report_service_failure(
                                &event_handle, "guardian", &reason,
                            );
                        }
                        NativeEvent::VoiceAudio(event) => {
                            if let Some(event) = tauri_plugin_vcp_mobile::voice::handle_native_event(&event_handle, event) {
                                crate::vcp_modules::chat::voice_service::handle_audio_event(&event_handle, &event);
                            }
                        }
                        NativeEvent::VoiceDocument { document_id } => {
                            crate::vcp_modules::chat::voice_service::invalidate_document(
                                &event_handle, Some(&document_id),
                            );
                        }
                        NativeEvent::VoicePlayback(event) => {
                            crate::vcp_modules::chat::voice_service::handle_playback_event(&event_handle, event);
                        }
                        NativeEvent::Other {} => {}
                    }
                })
                .await
            {
                tauri_plugin_vcp_mobile::events::report_service_failure(
                    &handle_net,
                    "events",
                    &error,
                );
            }
        });
    }

    Ok(())
}

#[derive(Debug, Serialize, Clone)]
pub struct SystemSnapshot {
    pub core: CoreStatus,
    pub message: String,
    pub log: String,
    #[serde(rename = "logConfigurationError")]
    pub log_configuration_error: Option<String>,
    pub sync: String,
    pub lifecycle: Option<crate::vcp_modules::infra::lifecycle_state::LifecycleSnapshot>,
    #[serde(rename = "databaseRecovery")]
    pub database_recovery:
        Option<crate::vcp_modules::infra::lifecycle_state::DatabaseRecoveryNotice>,
    #[serde(rename = "backgroundServiceFailures")]
    pub background_service_failures: Vec<tauri_plugin_vcp_mobile::events::ServiceFailure>,
}

#[tauri::command]
pub async fn get_system_snapshot(
    state: State<'_, LifecycleState>,
    app: AppHandle,
) -> Result<SystemSnapshot, String> {
    let core = *state.status.read().await;
    let message = state.status_message.read().await.clone();
    let database_recovery = state.database_recovery.read().await.clone();

    // 获取 VCPLog 状态
    let log = crate::vcp_modules::vcp_log_service::get_vcp_log_status_internal().await;
    // Startup can finish before the WebView subscribes to events. Diagnose
    // persisted endpoint errors in its existing hydration snapshot as well.
    // Earlier phases must not touch configuration before database registration.
    let log_configuration_error = if core == CoreStatus::Ready {
        let settings_state = app
            .try_state::<SettingsState>()
            .ok_or_else(|| "核心已就绪但配置状态尚未注册".to_string())?;
        let db_state = app
            .try_state::<DbState>()
            .ok_or_else(|| "核心已就绪但数据库状态尚未注册".to_string())?;
        let settings = settings_state.read_config(db_state.inner()).await?;
        super::vcp_ws_endpoint::compile_log_connection(&settings.log.url, &settings.log.key)
            .err()
            .map(|issue| issue.to_string())
    } else {
        None
    };

    // 获取 Sync 状态
    let sync = match app.try_state::<crate::vcp_modules::sync_service::SyncState>() {
        Some(s) => s.connection_status.read().await.clone(),
        None => "closed".to_string(),
    };

    Ok(SystemSnapshot {
        core,
        message,
        log,
        log_configuration_error,
        sync,
        lifecycle: state
            .lifecycle_snapshot
            .read()
            .await
            .map(|value| value.as_snapshot()),
        database_recovery,
        background_service_failures:
            tauri_plugin_vcp_mobile::events::collect_service_failure_notices(&app),
    })
}

#[tauri::command]
pub async fn reconcile_distributed_node_cmd(app_handle: AppHandle) -> Result<bool, String> {
    let settings_state = app_handle.state::<SettingsState>();
    let _runtime_guard = settings_state.lock_runtime_reconcile().await;
    let settings = read_config(app_handle.clone(), app_handle.state())
        .await
        .map_err(|e| e.to_string())?;
    log::info!(
        "[Lifecycle] reconcile_distributed_node_cmd called: enable={}",
        settings.distributed.enabled
    );
    // A changed configuration already schedules its own replacement. Explicit
    // Connect also admits an unchanged offline node without restarting a fresh
    // connection that the post-commit owner has just created.
    reconcile_distributed_node(&app_handle, false).await;
    if settings.distributed.enabled {
        if let Some(distributed) = app_handle.try_state::<crate::distributed::DistributedState>() {
            let client = distributed.client.read().await;
            let status = client.get_status().await;
            if !status.connected && status.last_error.is_some() {
                client.trigger_reconnect().await;
            }
        }
    }
    Ok(settings.distributed.enabled)
}

#[tauri::command]
pub async fn get_core_status(state: State<'_, LifecycleState>) -> Result<CoreStatus, String> {
    Ok(*state.status.read().await)
}

#[tauri::command]
pub async fn get_last_error(state: State<'_, LifecycleState>) -> Result<Option<String>, String> {
    Ok(state.last_error.read().await.clone())
}

#[tauri::command]
pub async fn restart_or_exit_app(
    #[allow(unused_variables)] app: tauri::AppHandle,
) -> Result<(), String> {
    log::info!("[Lifecycle] Requesting application restart/exit...");

    #[cfg(target_os = "android")]
    {
        use tokio::sync::oneshot;
        let (tx, rx) = oneshot::channel();

        let window = app
            .get_webview_window("main")
            .ok_or("main window not found")?;
        let res = window.as_ref().with_webview(move |webview| {
            webview.jni_handle().exec(move |env, activity, _webview| {
                match restart_android_app(env, &activity) {
                    Ok(_) => {
                        let _ = tx.send(());
                    }
                    Err(e) => {
                        log::error!("[Lifecycle] Restart JNI failed: {}", e);
                        std::process::exit(0);
                    }
                }
            });
        });

        if let Err(e) = res {
            log::error!("[Lifecycle] with_webview failed: {:?}", e);
            std::process::exit(0);
        }

        // 等待 JNI 线程成功调用并完成 startActivity Binder IPC
        if let Ok(_) = rx.await {
            // 给系统 Binder 一点时间刷盘并调度拉起新进程，然后自杀
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }
        std::process::exit(0);
    }

    #[cfg(target_os = "ios")]
    {
        Err(
            "unsupported: iOS does not support restarting or exiting the application by command"
                .to_string(),
        )
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        std::process::exit(0);
    }
}

#[cfg(target_os = "android")]
fn restart_android_app(
    env: &mut jni::JNIEnv<'_>,
    activity: &jni::objects::JObject<'_>,
) -> Result<(), String> {
    use jni::objects::JValue;

    // 1. Get Context
    let context = env
        .call_method(
            activity,
            "getApplicationContext",
            "()Landroid/content/Context;",
            &[],
        )
        .map_err(|e| format!("getApplicationContext failed: {:?}", e))?
        .l()
        .map_err(|e| format!("getApplicationContext returned non-object: {:?}", e))?;

    // 2. Get PackageManager
    let pm = env
        .call_method(
            &context,
            "getPackageManager",
            "()Landroid/content/pm/PackageManager;",
            &[],
        )
        .map_err(|e| format!("getPackageManager failed: {:?}", e))?
        .l()
        .map_err(|e| format!("getPackageManager returned non-object: {:?}", e))?;

    // 3. Get PackageName
    let package_name = env
        .call_method(&context, "getPackageName", "()Ljava/lang/String;", &[])
        .map_err(|e| format!("getPackageName failed: {:?}", e))?
        .l()
        .map_err(|e| format!("getPackageName returned non-object: {:?}", e))?;

    // 4. Get Launch Intent
    let intent = env
        .call_method(
            &pm,
            "getLaunchIntentForPackage",
            "(Ljava/lang/String;)Landroid/content/Intent;",
            &[JValue::Object(&package_name)],
        )
        .map_err(|e| format!("getLaunchIntentForPackage failed: {:?}", e))?
        .l()
        .map_err(|e| format!("getLaunchIntentForPackage returned non-object: {:?}", e))?;

    if intent.is_null() {
        return Err("Launch intent not found".to_string());
    }

    // 5. Start Activity
    env.call_method(
        &context,
        "startActivity",
        "(Landroid/content/Intent;)V",
        &[JValue::Object(&intent)],
    )
    .map_err(|e| format!("startActivity failed: {:?}", e))?;

    Ok(())
}
