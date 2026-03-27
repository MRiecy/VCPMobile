use log::{error, info};
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::RwLock;

use crate::vcp_modules::agent_config_manager::AgentConfigState;
use crate::vcp_modules::app_settings_manager::AppSettingsState;
use crate::vcp_modules::db_manager::{init_db, DbState};
use crate::vcp_modules::emoticon_manager::{internal_generate_library, EmoticonManagerState};
use crate::vcp_modules::file_watcher::{init_watcher, WatcherState};
use crate::vcp_modules::group_manager::{load_all_groups, GroupManagerState};
use crate::vcp_modules::index_service::full_scan;
use crate::vcp_modules::model_manager::{init_model_manager, ModelManagerState};

#[derive(Debug, Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CoreStatus {
    Initializing,
    Ready,
    Syncing,
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
    let pool = match init_db(&handle).await {
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
    handle.manage(AppSettingsState::new());
    handle.manage(EmoticonManagerState::new());
    handle.manage(WatcherState::default());
    handle.manage(ModelManagerState::new());

    // 3. 服务级并行初始化 (这些服务彼此依赖较少)
    let emoticon_task = {
        let h = handle.clone();
        tokio::spawn(async move {
            let emoticon_state = h.state::<EmoticonManagerState>();
            let settings_state = h.state::<AppSettingsState>();
            if let Ok(lib) = internal_generate_library(&h, &settings_state).await {
                *emoticon_state.library.lock().await = lib;
                info!("[Lifecycle] Emoticon library loaded.");
            }
        })
    };

    let model_task = {
        let h = handle.clone();
        tokio::spawn(async move {
            let model_state = h.state::<ModelManagerState>();
            init_model_manager(&h, &model_state).await;
            info!("[Lifecycle] Model manager initialized.");
        })
    };

    // 4. 群组与索引初始化 (存在 DB 写入，顺序执行)
    let group_state = handle.state::<GroupManagerState>();
    if let Err(e) = load_all_groups(&handle, &group_state, &pool).await {
        error!("[Lifecycle] Group load failed: {}", e);
    }

    if let Err(e) = init_watcher(handle.clone()) {
        error!("[Lifecycle] Watcher init failed: {}", e);
    }

    // 5. 全量扫描 (建立最终一致性)
    info!("[Lifecycle] Running initial full scan...");
    if let Err(e) = full_scan(&handle, &pool).await {
        error!("[Lifecycle] Initial scan failed: {}", e);
    }

    // 等待并行任务完成 (非阻塞)
    let _ = tokio::join!(emoticon_task, model_task);

    // 6. 标记为就绪
    *lifecycle.status.write().await = CoreStatus::Ready;
    let _ = handle.emit("vcp-core-ready", ());
    info!("[Lifecycle] Bootstrap complete. Core is READY.");

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
