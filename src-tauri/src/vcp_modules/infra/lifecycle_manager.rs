use log::info;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::RwLock;

use crate::vcp_modules::db_manager::{init_db, DbState};
use crate::vcp_modules::emoticon_manager::{
    internal_load_library, refresh_emoticon_library_internal, EmoticonManagerState,
};
use crate::vcp_modules::infra::local_server::{self, ServerHandle};
use crate::vcp_modules::model_manager::{init_model_manager, ModelManagerState};
use crate::vcp_modules::settings_manager::{read_settings, SettingsState};
use crate::vcp_modules::sync_service::init_sync_service;
use crate::vcp_modules::vcp_log_service::init_vcp_log_connection_internal;

#[derive(Debug, Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CoreStatus {
    Initializing,
    Ready,
    // Syncing,
    Error,
}

pub struct LifecycleState {
    pub status: Arc<RwLock<CoreStatus>>,
    pub last_error: Arc<RwLock<Option<String>>>,
    /// 划词助手本地服务器句柄：用于根据设置动态启停
    pub local_server_handle: Arc<tokio::sync::Mutex<Option<ServerHandle>>>,
}

impl LifecycleState {
    pub fn new() -> Self {
        Self {
            status: Arc::new(RwLock::new(CoreStatus::Initializing)),
            last_error: Arc::new(RwLock::new(None)),
            local_server_handle: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }
}

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

    // 3.5 根据设置决定是否启动划词助手本地服务器 (Beta)
    {
        let enable = settings.enable_assistant;
        log::info!(
            "[Lifecycle] enableAssistant={}, reconciling local server...",
            enable
        );
        reconcile_local_server(&handle, &lifecycle, enable).await;
    }

    // 3.6 根据设置决定是否启动分布式节点 (自动重连)
    {
        let enable_dist = settings.distributed_enabled;
        log::info!(
            "[Lifecycle] distributedEnabled={}, reconciling distributed node...",
            enable_dist
        );
        reconcile_distributed_node(&handle, enable_dist, false).await;
    }

    // 初始化同步服务
    let sync_state = init_sync_service(handle.clone());
    handle.manage(sync_state);

    // 4. 服务级后台初始化 (P2 - 非阻塞)
    {
        let h = handle.clone();
        let s_url = settings.vcp_log_url.clone();
        let s_key = settings.vcp_log_key.clone();

        tokio::spawn(async move {
            let emoticon_state = h.state::<EmoticonManagerState>();
            if let Ok(lib) = internal_load_library(&h).await {
                *emoticon_state.library.lock().await = lib;
                info!("[Lifecycle] Emoticon library loaded from DB.");
            }

            // Best-effort refresh from server (does not block startup)
            match refresh_emoticon_library_internal(&h).await {
                Ok(count) => info!(
                    "[Lifecycle] Emoticon library auto-refreshed: {} items",
                    count
                ),
                Err(e) => info!("[Lifecycle] Emoticon auto-refresh skipped: {}", e),
            }

            // 自动连接 VCP Log
            if !s_url.is_empty() && !s_key.is_empty() {
                info!("[Lifecycle] Auto-connecting VCP Log...");
                let _ =
                    init_vcp_log_connection_internal(h.clone(), s_url.clone(), s_key.clone()).await;
                info!("[Lifecycle] Auto-connecting VCP Info...");
                let _ = super::vcp_info_service::init_vcp_info_connection(h.clone(), s_url, s_key)
                    .await;
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
            use crate::vcp_modules::sync_executor::delete_executor::DeleteExecutor;
            let _ = DeleteExecutor::cleanup_old_deleted_records(&h, 30).await;

            loop {
                tokio::time::sleep(Duration::from_secs(86400)).await;
                let _ = DeleteExecutor::cleanup_old_deleted_records(&h, 30).await;
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

    // 6. 后台静默检查前端热更新（完全非阻塞）
    {
        let h = handle.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            info!("[FrontendUpdate] Starting background check...");

            match crate::vcp_modules::frontend_update_manager::check_for_frontend_update(h.clone())
                .await
            {
                Ok(info) => {
                    if info.has_update {
                        if let Some(url) = info.download_url {
                            info!(
                                "[FrontendUpdate] New version available: {}, downloading...",
                                info.remote_version
                            );
                            match crate::vcp_modules::frontend_update_manager::download_frontend_update_inner(
                                &h,
                                &url,
                                None,
                            )
                            .await
                            {
                                Ok(zip_path) => {
                                    if let Err(e) = crate::vcp_modules::frontend_update_manager::apply_frontend_update(
                                        h.clone(),
                                        zip_path,
                                        info.remote_version.clone(),
                                    )
                                    .await
                                    {
                                        log::error!("[FrontendUpdate] Apply failed: {}", e);
                                    } else {
                                        info!(
                                            "[FrontendUpdate] Version {} downloaded and applied. Will take effect on next cold start.",
                                            info.remote_version
                                        );
                                    }
                                }
                                Err(e) => {
                                    log::error!("[FrontendUpdate] Download failed: {}", e);
                                }
                            }
                        }
                    } else {
                        info!("[FrontendUpdate] No frontend update available.");
                    }
                }
                Err(e) => {
                    log::error!("[FrontendUpdate] Check failed: {}", e);
                }
            }
        });
    }

    Ok(())
}

#[derive(Debug, Serialize, Clone)]
pub struct SystemSnapshot {
    pub core: CoreStatus,
    pub log: String,
    pub sync: String,
    pub distributed: String,
}

#[tauri::command]
pub async fn get_system_snapshot(
    state: State<'_, LifecycleState>,
    app: AppHandle,
) -> Result<SystemSnapshot, String> {
    let core = *state.status.read().await;

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
        log,
        sync,
        distributed,
    })
}

/// 前端保存设置后调用，即时生效启用/停用划词助手本地服务器
#[tauri::command]
pub async fn reconcile_local_server_cmd(
    app_handle: AppHandle,
    state: State<'_, LifecycleState>,
    enable: bool,
) -> Result<bool, String> {
    log::info!(
        "[Lifecycle] reconcile_local_server_cmd called: enable={}",
        enable
    );
    let lifecycle = &*state;
    reconcile_local_server(&app_handle, lifecycle, enable).await;
    Ok(enable)
}

#[tauri::command]
pub async fn reconcile_distributed_node_cmd(
    app_handle: AppHandle,
    enable: bool,
) -> Result<bool, String> {
    log::info!(
        "[Lifecycle] reconcile_distributed_node_cmd called: enable={}",
        enable
    );
    reconcile_distributed_node(&app_handle, enable, false).await;
    Ok(enable)
}

#[tauri::command]
pub async fn get_core_status(state: State<'_, LifecycleState>) -> Result<CoreStatus, String> {
    Ok(*state.status.read().await)
}

#[tauri::command]
pub async fn get_last_error(state: State<'_, LifecycleState>) -> Result<Option<String>, String> {
    Ok(state.last_error.read().await.clone())
}

pub static APP_IN_FOREGROUND: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

lazy_static::lazy_static! {
    static ref LOG_LINGER_CANCEL: tokio::sync::Mutex<Option<tokio_util::sync::CancellationToken>> = tokio::sync::Mutex::new(None);
    static ref DIST_LINGER_CANCEL: tokio::sync::Mutex<Option<tokio_util::sync::CancellationToken>> = tokio::sync::Mutex::new(None);
    static ref IS_LINGER_DISCONNECTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
}

#[tauri::command]
pub async fn set_app_foreground_state(app: AppHandle, is_foreground: bool) {
    set_app_foreground_state_internal(app, is_foreground).await;
}

pub async fn set_app_foreground_state_internal(app: AppHandle, is_foreground: bool) {
    let was_foreground = APP_IN_FOREGROUND.swap(is_foreground, std::sync::atomic::Ordering::SeqCst);
    if was_foreground == is_foreground {
        return;
    }
    log::info!(
        "[Lifecycle] App foreground state transitioned: {} -> {}",
        was_foreground,
        is_foreground
    );

    // 1. 调整心跳频率
    crate::vcp_modules::infra::vcp_log_service::handle_foreground_state_change(&app, is_foreground)
        .await;

    if !is_foreground {
        // --- 进入后台 ---
        // 1.1 取消旧倒计时任务
        {
            let mut cancel_lock = LOG_LINGER_CANCEL.lock().await;
            if let Some(token) = cancel_lock.take() {
                token.cancel();
            }
        }
        {
            let mut cancel_lock = DIST_LINGER_CANCEL.lock().await;
            if let Some(token) = cancel_lock.take() {
                token.cancel();
            }
        }
        IS_LINGER_DISCONNECTED.store(false, std::sync::atomic::Ordering::SeqCst);

        // 1.2 开启 VCPLog/Info (5分钟延迟断连任务)
        // 优化：将延迟断连时间从 10 分钟缩短至 5 分钟。
        // 原因：在后台缩短活跃连接持有时间，可显著降低后台长连接维持的心跳能耗，并防御 aggressive 系统省电机制的杀进程风险。
        let log_token = tokio_util::sync::CancellationToken::new();
        {
            let mut cancel_lock = LOG_LINGER_CANCEL.lock().await;
            *cancel_lock = Some(log_token.clone());
        }
        let app_clone = app.clone();
        crate::vcp_modules::infra::utils::spawn_linger_task(
            Duration::from_secs(300),
            log_token,
            move || async move {
                log::info!(
                    "[Lifecycle] Background linger expired (10m). Disconnecting VCPLog/Info."
                );
                let _ = crate::vcp_modules::infra::vcp_log_service::disconnect_log_connections(
                    &app_clone,
                )
                .await;
                IS_LINGER_DISCONNECTED.store(true, std::sync::atomic::Ordering::SeqCst);
            },
        );

        // 1.3 开启 Distributed (5分钟保活冷却任务)
        let settings_state = app.state::<SettingsState>();
        if let Ok(settings) = read_settings(app.clone(), settings_state).await {
            if settings.distributed_enabled {
                log::info!("[Lifecycle] Distributed enabled. Activating FGS keepalive.");
                let _ = tauri_plugin_vcp_mobile::stream::set_keepalive_mode_inner(&app, true);

                let dist_token = tokio_util::sync::CancellationToken::new();
                {
                    let mut cancel_lock = DIST_LINGER_CANCEL.lock().await;
                    *cancel_lock = Some(dist_token.clone());
                }
                let app_clone = app.clone();
                crate::vcp_modules::infra::utils::spawn_linger_task(
                    Duration::from_secs(300),
                    dist_token,
                    move || async move {
                        log::info!("[Lifecycle] Background distributed linger expired (5m). Deactivating keepalive.");
                        let _ = tauri_plugin_vcp_mobile::stream::set_keepalive_mode_inner(
                            &app_clone, false,
                        );
                    },
                );
            }
        }
    } else {
        // --- 返回前台 ---
        // 2.1 立即取消所有倒计时
        {
            let mut cancel_lock = LOG_LINGER_CANCEL.lock().await;
            if let Some(token) = cancel_lock.take() {
                token.cancel();
            }
        }
        {
            let mut cancel_lock = DIST_LINGER_CANCEL.lock().await;
            if let Some(token) = cancel_lock.take() {
                token.cancel();
            }
        }

        // 2.2 恢复分布式保活状态 (前台关闭保活通知)
        let settings_state = app.state::<SettingsState>();
        if let Ok(settings) = read_settings(app.clone(), settings_state).await {
            if settings.distributed_enabled {
                let _ = tauri_plugin_vcp_mobile::stream::set_keepalive_mode_inner(&app, false);
            }
        }

        // 2.3 若此前已冷断开，一键拉起恢复
        if IS_LINGER_DISCONNECTED.swap(false, std::sync::atomic::Ordering::SeqCst) {
            let settings_state = app.state::<SettingsState>();
            if let Ok(settings) = read_settings(app.clone(), settings_state).await {
                let log_url = settings.vcp_log_url;
                let log_key = settings.vcp_log_key;
                if !log_url.trim().is_empty() && !log_key.trim().is_empty() {
                    log::info!("[Lifecycle] App returned to foreground. Reconnecting VCPLog/Info.");
                    let _ = crate::vcp_modules::infra::vcp_log_service::reconnect_log_connections(
                        &app, log_url, log_key,
                    )
                    .await;
                }
            }
        }
    }
}
