use serde::Serialize;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri_plugin_vcp_mobile::events::LifecyclePhase;
#[cfg(any(target_os = "android", target_os = "ios"))]
use tauri_plugin_vcp_mobile::events::NativeLifecycleEvent;
#[cfg(not(target_os = "ios"))]
use tokio::sync::Mutex;
use tokio::sync::RwLock;
#[cfg(not(target_os = "ios"))]
use tokio_util::sync::CancellationToken;

use crate::vcp_modules::infra::local_server::ServerHandle;

#[derive(Debug, Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CoreStatus {
    Initializing,
    Optimizing,
    Ready,
    Error,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseRecoveryNotice {
    pub message: String,
    pub archive_path: String,
    pub recovered_at: i64,
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleSnapshot {
    pub phase: LifecyclePhase,
    pub native_sequence: Option<u64>,
    pub snapshot: bool,
    pub transition_epoch: u64,
}

impl LifecycleSnapshot {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    pub fn from_native(event: NativeLifecycleEvent, transition_epoch: u64) -> Self {
        Self {
            phase: event.phase,
            native_sequence: Some(event.native_sequence),
            snapshot: event.snapshot,
            transition_epoch,
        }
    }

    pub fn as_snapshot(mut self) -> Self {
        self.snapshot = true;
        self
    }
}

#[derive(Default)]
#[cfg(any(target_os = "android", target_os = "ios", test))]
pub struct NativeLifecycleOrdering {
    last_sequence: Option<u64>,
}

#[cfg(any(target_os = "android", target_os = "ios", test))]
impl NativeLifecycleOrdering {
    /// Accept the first snapshot even when the native source has not transitioned (0).
    pub fn accept(&mut self, sequence: u64) -> bool {
        if self.last_sequence.is_some_and(|last| sequence <= last) {
            return false;
        }
        self.last_sequence = Some(sequence);
        true
    }
}

#[cfg(not(target_os = "ios"))]
pub struct LingerController {
    pub log_cancel: Mutex<Option<CancellationToken>>,
    pub dist_cancel: Mutex<Option<CancellationToken>>,
    pub is_log_disconnected: AtomicBool,
    pub is_dist_disconnected: AtomicBool,
}

#[cfg(not(target_os = "ios"))]
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
    /// 启动早期的数据库恢复结果。保留到前端读取系统快照，避免一次性事件先于监听器发出。
    pub database_recovery: Arc<RwLock<Option<DatabaseRecoveryNotice>>>,
    /// 划词助手本地服务器句柄：用于根据设置动态启停
    #[allow(dead_code)]
    // DORMANT ASSET: no production lifecycle path currently reads this handle.
    pub local_server_handle: Arc<tokio::sync::Mutex<Option<ServerHandle>>>,
    /// 应用前台状态，统一替代原裸静态全局变量
    pub is_foreground: Arc<AtomicBool>,
    /// None until an actual native/host event has reached the authoritative owner.
    pub lifecycle_snapshot: RwLock<Option<LifecycleSnapshot>>,
    /// Native delivery ordering is checked before allocating a Rust transition epoch.
    #[cfg(any(target_os = "android", target_os = "ios"))]
    pub native_lifecycle_ordering: std::sync::Mutex<NativeLifecycleOrdering>,
    /// 统一后台 Linger 延时断连任务状态与控制器
    #[cfg(not(target_os = "ios"))]
    pub linger: Arc<LingerController>,
}

impl LifecycleState {
    pub fn new() -> Self {
        Self {
            status: Arc::new(RwLock::new(CoreStatus::Initializing)),
            status_message: Arc::new(RwLock::new("核心引擎初始化中...".to_string())),
            last_error: Arc::new(RwLock::new(None)),
            database_recovery: Arc::new(RwLock::new(None)),
            local_server_handle: Arc::new(tokio::sync::Mutex::new(None)),
            // iOS services wait for an actual active observation. An initial
            // inactive snapshot must not suppress the first active transition.
            is_foreground: Arc::new(AtomicBool::new(!cfg!(target_os = "ios"))),
            lifecycle_snapshot: RwLock::new(None),
            #[cfg(any(target_os = "android", target_os = "ios"))]
            native_lifecycle_ordering: std::sync::Mutex::new(NativeLifecycleOrdering::default()),
            #[cfg(not(target_os = "ios"))]
            linger: Arc::new(LingerController::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_replay_and_out_of_order_events_cannot_reserve_a_new_transition() {
        let mut ordering = NativeLifecycleOrdering::default();
        assert!(ordering.accept(0));
        assert!(!ordering.accept(0));
        assert!(ordering.accept(4));
        assert!(!ordering.accept(3));
        assert!(!ordering.accept(4));
        assert!(ordering.accept(5));
    }
}
