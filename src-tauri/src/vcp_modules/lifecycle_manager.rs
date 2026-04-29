use log::info;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::RwLock;

use crate::vcp_modules::agent_service::AgentConfigState;
use crate::vcp_modules::db_manager::{init_db, DbState};
use crate::vcp_modules::emoticon_manager::{
    internal_load_library, refresh_emoticon_library_internal, EmoticonManagerState,
};
use crate::vcp_modules::group_service::GroupManagerState;
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
}

impl LifecycleState {
    pub fn new() -> Self {
        Self {
            status: Arc::new(RwLock::new(CoreStatus::Initializing)),
            last_error: Arc::new(RwLock::new(None)),
        }
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
        Ok(p) => {
            handle.manage(DbState { pool: p.clone() });
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

    // 2. 基础状态管理注册
    handle.manage(AgentConfigState::new());
    handle.manage(GroupManagerState::new());
    handle.manage(SettingsState::new());
    handle.manage(ModelManagerState::new());
    handle.manage(EmoticonManagerState::default());

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
                let _ = init_vcp_log_connection_internal(h.clone(), s_url, s_key).await;
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

    Ok(())
}

#[derive(Debug, Serialize, Clone)]
pub struct SystemSnapshot {
    pub core: CoreStatus,
    pub log: String,
    pub sync: String,
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

    Ok(SystemSnapshot { core, log, sync })
}

#[tauri::command]
pub async fn get_core_status(state: State<'_, LifecycleState>) -> Result<CoreStatus, String> {
    Ok(*state.status.read().await)
}

#[tauri::command]
pub async fn get_last_error(state: State<'_, LifecycleState>) -> Result<Option<String>, String> {
    Ok(state.last_error.read().await.clone())
}
