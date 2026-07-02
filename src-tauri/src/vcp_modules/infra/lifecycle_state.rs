use serde::Serialize;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::vcp_modules::infra::local_server::ServerHandle;

#[derive(Debug, Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CoreStatus {
    Initializing,
    Decompressing,
    #[serde(rename = "decompression-complete")]
    DecompressionComplete,
    Optimizing,
    Ready,
    Error,
}

pub struct LingerController {
    pub log_cancel: Mutex<Option<CancellationToken>>,
    pub dist_cancel: Mutex<Option<CancellationToken>>,
    pub is_log_disconnected: AtomicBool,
    pub is_dist_disconnected: AtomicBool,
}

impl LingerController {
    pub fn new() -> Self {
        Self {
            log_cancel: Mutex::new(None),
            dist_cancel: Mutex::new(None),
            is_log_disconnected: AtomicBool::new(false),
            is_dist_disconnected: AtomicBool::new(false),
        }
    }
}

pub struct LifecycleState {
    pub status: Arc<RwLock<CoreStatus>>,
    pub status_message: Arc<RwLock<String>>,
    pub last_error: Arc<RwLock<Option<String>>>,
    /// 划词助手本地服务器句柄：用于根据设置动态启停
    pub local_server_handle: Arc<tokio::sync::Mutex<Option<ServerHandle>>>,
    /// 应用前台状态，统一替代原裸静态全局变量
    pub is_foreground: Arc<AtomicBool>,
    /// 统一后台 Linger 延时断连任务状态与控制器
    pub linger: Arc<LingerController>,
}

impl LifecycleState {
    pub fn new() -> Self {
        Self {
            status: Arc::new(RwLock::new(CoreStatus::Initializing)),
            status_message: Arc::new(RwLock::new("核心引擎初始化中...".to_string())),
            last_error: Arc::new(RwLock::new(None)),
            local_server_handle: Arc::new(tokio::sync::Mutex::new(None)),
            is_foreground: Arc::new(AtomicBool::new(true)),
            linger: Arc::new(LingerController::new()),
        }
    }
}
