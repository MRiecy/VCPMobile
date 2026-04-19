use log::info;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::RwLock;

use crate::vcp_modules::agent_service::AgentConfigState;
use crate::vcp_modules::db_manager::{init_db, DbState};
use crate::vcp_modules::emoticon_manager::{internal_generate_library, EmoticonManagerState};
use crate::vcp_modules::group_service::GroupManagerState;
use crate::vcp_modules::model_manager::{init_model_manager, ModelManagerState};
use crate::vcp_modules::settings_manager::SettingsState;
use crate::vcp_modules::sync_service::init_sync_service;

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

    // 1. 数据库初始化 (所有服务的基础)
    let _pool = match init_db(&handle).await {
        Ok(p) => {
            handle.manage(DbState { pool: p.clone() });
            p
        }
        Err(e) => {
            let err_msg = format!("Database init failed: {}", e);
            *lifecycle.last_error.write().await = Some(err_msg.clone());
            *lifecycle.status.write().await = CoreStatus::Error;
            return Err(err_msg);
        }
    };

    // 2. 基础状态管理注册
    handle.manage(AgentConfigState::new());
    handle.manage(GroupManagerState::new());
    handle.manage(SettingsState::new());
    handle.manage(ModelManagerState::new());
    handle.manage(EmoticonManagerState::default());

    // 初始化同步服务
    let sync_state = init_sync_service(handle.clone());
    handle.manage(sync_state);

    // 3. 服务级后台初始化 (解耦阻塞)
    // 我们将这些任务放入后台执行，不再使用 join! 等待。
    // 这样前端能立刻收到 Ready 信号，而不需要等待表情包库扫完。
    {
        let h = handle.clone();
        tokio::spawn(async move {
            let emoticon_state = h.state::<EmoticonManagerState>();
            let settings_state = h.state::<SettingsState>();
            if let Ok(lib) = internal_generate_library(&h, &settings_state).await {
                *emoticon_state.library.lock().await = lib;
                info!("[Lifecycle] Emoticon library loaded in background.");
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

    // 4. 群组初始化 (如有需要可在此添加)

    // 5. 标记为就绪 (不再等待后台任务)
    *lifecycle.status.write().await = CoreStatus::Ready;
    let _ = handle.emit("vcp-core-ready", ());
    info!("[Lifecycle] Bootstrap complete. Core is READY (Non-blocking).");

    Ok(())
}

#[tauri::command]
pub async fn get_core_status(state: State<'_, LifecycleState>) -> Result<CoreStatus, String> {
    Ok(*state.status.read().await)
}

#[tauri::command]
pub async fn get_last_error(state: State<'_, LifecycleState>) -> Result<Option<String>, String> {
    Ok(state.last_error.read().await.clone())
}
