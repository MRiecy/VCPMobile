use log::info;
use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::vcp_modules::db_manager::{init_db, DbState};
use crate::vcp_modules::emoticon_manager::{
    internal_load_library, refresh_emoticon_library_internal, EmoticonManagerState,
};
use crate::vcp_modules::model_manager::{init_model_manager, ModelManagerState};
use crate::vcp_modules::settings_manager::{read_settings, SettingsState};
use crate::vcp_modules::sync_service::init_sync_service;
use crate::vcp_modules::vcp_log_service::init_vcp_log_connection_internal;

// Re-export submodules to preserve public API compatibility
#[allow(unused_imports)]
pub use crate::vcp_modules::infra::lifecycle_controller::{
    is_app_in_foreground, reserve_lifecycle_transition, set_app_foreground_state,
    set_app_foreground_state_for_epoch, set_app_foreground_state_internal,
};
pub use crate::vcp_modules::infra::lifecycle_reconciler::{
    reconcile_distributed_node, reconcile_local_server,
};
pub use crate::vcp_modules::infra::lifecycle_state::{CoreStatus, LifecycleState};

/// 核心启动逻辑：线性化管理所有服务的初始化顺序
pub async fn bootstrap(app: &AppHandle) -> Result<(), String> {
    let lifecycle = app.state::<LifecycleState>();
    let handle = app.clone();

    info!("[Lifecycle] Starting bootstrap sequence...");

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
    let _pool = match init_db(&handle).await {
        Ok((p, path)) => {
            handle.manage(DbState {
                pool: p.clone(),
                path,
            });

            // 运行数据库解压升级迁移 (若有旧版压缩数据，将在此处展示进度并安全拦截启动流程)
            match crate::vcp_modules::db_manager::decompress_database_migration(&handle).await {
                Ok(true) => {
                    log::info!("[Lifecycle] Decompress database migration completed. Halting boot sequence for restart.");
                    return Ok(());
                }
                Ok(false) => {
                    // 不需要解压迁移，继续引导
                }
                Err(e) => {
                    let err_msg = format!("数据库解压迁移失败: {}", e);
                    *lifecycle.last_error.write().await = Some(err_msg.clone());
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
            }

            p
        }
        Err(e) => {
            let err_msg = format!("数据库初始化失败: {}", e);
            *lifecycle.last_error.write().await = Some(err_msg.clone());
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
    };

    // 2. 基础状态管理注册已在 lib.rs 中的 setup 阶段提前同步完成，此处无需重复注册以避免覆盖已有缓存。

    // 3. 配置预加载 (P1 - 前端强依赖)
    // 将配置读取前置，确保前端 Ready 后 fetchSettings 必然成功
    let settings_state = handle.state::<SettingsState>();
    let settings = match read_settings(handle.clone(), settings_state).await {
        Ok(s) => s,
        Err(e) => {
            let err_msg = format!("基础配置读取失败: {}", e);
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

    // 3.6 根据设置决定是否启动分布式节点 (自动重连)
    {
        let enable_dist = settings.distributed_enabled;
        log::info!(
            "[Lifecycle] distributedEnabled={}, reconciling distributed node...",
            enable_dist
        );
        let settings_state = handle.state::<SettingsState>();
        let _runtime_guard = settings_state.lock_runtime_reconcile().await;
        reconcile_distributed_node(&handle, false).await;
    }

    // 初始化同步服务
    let sync_state = init_sync_service(handle.clone());
    handle.manage(sync_state);

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

            // 自动连接 VCP Log。执行前在 Settings runtime owner 下重读，避免启动
            // 后台任务用旧快照覆盖用户刚保存的新连接参数。
            let settings_state = h.state::<SettingsState>();
            let _runtime_guard = settings_state.lock_runtime_reconcile().await;
            if let Ok(latest) = read_settings(h.clone(), h.state()).await {
                if !latest.vcp_log_url.is_empty() && !latest.vcp_log_key.is_empty() {
                    info!("[Lifecycle] Auto-connecting VCP Log...");
                    let _ = init_vcp_log_connection_internal(
                        h.clone(),
                        latest.vcp_log_url.clone(),
                        latest.vcp_log_key.clone(),
                    )
                    .await;
                    info!("[Lifecycle] Auto-connecting VCP Info...");
                    let _ = crate::vcp_modules::vcp_info_service::init_vcp_info_connection(
                        h.clone(),
                        latest.vcp_log_url,
                        latest.vcp_log_key,
                    )
                    .await;
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
                    let _ = sqlx::query("INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('delete_executor_last_cleanup', ?, ?)")
                        .bind(now.to_string())
                        .bind(now)
                        .execute(pool)
                        .await;
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
                    let _ = sqlx::query("INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('delete_executor_last_cleanup', ?, ?)")
                        .bind(now.to_string())
                        .bind(now)
                        .execute(pool)
                        .await;
                }
            }
        });
    }

    // 5. 标记为就绪
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

    // 6. 核心就绪后，安全地激活安卓原生网络监听，彻底规避冷启动 JNI WebView 未就绪的死锁与崩塌
    let handle_net = handle.clone();
    tauri::async_runtime::spawn(async move {
        log::info!("[Lifecycle] Activating Android native network status monitoring...");
        if let Err(e) = tauri_plugin_vcp_mobile::system::start_network_monitoring(handle_net) {
            log::error!(
                "[Lifecycle] Failed to start native network status monitoring: {}",
                e
            );
        }
    });

    Ok(())
}

#[derive(Debug, Serialize, Clone)]
pub struct SystemSnapshot {
    pub core: CoreStatus,
    pub message: String,
    pub log: String,
    pub sync: String,
    pub distributed: String,
    #[serde(rename = "databaseRecovery")]
    pub database_recovery:
        Option<crate::vcp_modules::infra::lifecycle_state::DatabaseRecoveryNotice>,
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

    // 获取 Sync 状态
    let sync = match app.try_state::<crate::vcp_modules::sync_service::SyncState>() {
        Some(s) => s.connection_status.read().await.clone(),
        None => "closed".to_string(),
    };

    // 获取分布式连接状态
    let distributed = match app.try_state::<crate::distributed::DistributedState>() {
        Some(s) => {
            let client = s.client.read().await;
            let status = client.get_status().await;
            serde_json::to_value(status.state)
                .unwrap_or_else(|_| serde_json::json!("disconnected"))
                .as_str()
                .unwrap_or("disconnected")
                .to_string()
        }
        None => "disconnected".to_string(),
    };

    Ok(SystemSnapshot {
        core,
        message,
        log,
        sync,
        distributed,
        database_recovery,
    })
}

/// 前端保存设置后调用，即时生效启用/停用划词助手本地服务器 - 暂时停用该功能
#[tauri::command]
#[allow(dead_code)] // DORMANT ASSET: implementation is retained but absent from generate_handler!.
pub async fn reconcile_local_server_cmd(
    app_handle: AppHandle,
    state: State<'_, LifecycleState>,
    enable: bool,
) -> Result<bool, String> {
    log::info!(
        "[Lifecycle] reconcile_local_server_cmd called (temporarily disabled): enable={}",
        enable
    );
    let lifecycle = &*state;
    reconcile_local_server(&app_handle, lifecycle, false).await;
    Ok(false)
}

#[tauri::command]
pub async fn reconcile_distributed_node_cmd(app_handle: AppHandle) -> Result<bool, String> {
    let settings_state = app_handle.state::<SettingsState>();
    let _runtime_guard = settings_state.lock_runtime_reconcile().await;
    let settings = read_settings(app_handle.clone(), app_handle.state())
        .await
        .map_err(|e| e.to_string())?;
    log::info!(
        "[Lifecycle] reconcile_distributed_node_cmd called: enable={}",
        settings.distributed_enabled
    );
    reconcile_distributed_node(&app_handle, true).await;
    Ok(settings.distributed_enabled)
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

    #[cfg(not(target_os = "android"))]
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
