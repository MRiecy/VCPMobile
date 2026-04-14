use serde::Serialize;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{AppHandle, Emitter, Runtime};

pub struct SyncMetrics {
    pub total_operations: AtomicU64,
    pub completed: AtomicU64,
    pub failed: AtomicU64,
    pub retries: AtomicU64,
}

impl SyncMetrics {
    pub fn new() -> Self {
        Self {
            total_operations: AtomicU64::new(0),
            completed: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            retries: AtomicU64::new(0),
        }
    }

    pub fn record_start(&self) {
        self.total_operations.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_success(&self) {
        self.completed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_failure(&self) {
        self.failed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_retry(&self) {
        self.retries.fetch_add(1, Ordering::Relaxed);
    }

    pub fn emit_to_frontend<R: Runtime>(&self, app_handle: &AppHandle<R>) {
        let metrics = json!({
            "total": self.total_operations.load(Ordering::Relaxed),
            "completed": self.completed.load(Ordering::Relaxed),
            "failed": self.failed.load(Ordering::Relaxed),
            "retries": self.retries.load(Ordering::Relaxed),
        });
        let _ = app_handle.emit("vcp-sync-metrics", metrics);
    }
}

#[derive(Serialize)]
pub struct SyncProgress {
    pub current_op: String,
    pub completed: u64,
    pub total: u64,
}
