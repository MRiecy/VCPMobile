use rand::Rng;
use serde::Serialize;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::time::sleep;

pub struct RetryPolicy {
    pub max_retries: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
    pub jitter_factor: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 200,
            max_delay_ms: 5000,
            jitter_factor: 0.1,
        }
    }
}

impl RetryPolicy {
    pub fn default_clone(&self) -> Self {
        Self {
            max_retries: self.max_retries,
            base_delay_ms: self.base_delay_ms,
            max_delay_ms: self.max_delay_ms,
            jitter_factor: self.jitter_factor,
        }
    }

    pub fn calculate_delay(&self, attempt: u32) -> Duration {
        let exponential_delay = self.base_delay_ms * 2u64.pow(attempt);
        let capped_delay = exponential_delay.min(self.max_delay_ms);

        let jitter_range = (capped_delay as f64 * self.jitter_factor) as u64;
        let jitter = if jitter_range > 0 {
            rand::thread_rng().gen_range(0..=jitter_range)
        } else {
            0
        };

        Duration::from_millis(capped_delay + jitter)
    }

    pub async fn execute_with_retry<T, E, F, Fut>(
        &self,
        operation: F,
        is_retryable: impl Fn(&E) -> bool,
    ) -> Result<T, E>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T, E>>,
    {
        let mut last_error = None;

        for attempt in 0..=self.max_retries {
            match operation().await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    if !is_retryable(&e) || attempt == self.max_retries {
                        return Err(e);
                    }

                    let delay = self.calculate_delay(attempt);
                    println!("[SyncRetry] Attempt {} failed, retrying in {:?}", attempt + 1, delay);
                    sleep(delay).await;
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.expect("At least one error occurred"))
    }
}

pub fn is_network_retryable(error: &str) -> bool {
    let e = error.to_lowercase();
    e.contains("timeout")
        || e.contains("connection reset")
        || e.contains("connection refused")
        || e.contains("502")
        || e.contains("503")
        || e.contains("504")
        || e.contains("429")
}

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

pub async fn query_avatar_color(pool: &sqlx::SqlitePool, agent_id: &str) -> String {
    if agent_id.is_empty() {
        return "rgb(128, 128, 128)".to_string();
    }

    sqlx::query_scalar::<sqlx::Sqlite, Option<String>>(
        "SELECT dominant_color FROM avatars WHERE owner_id = ? AND owner_type = 'agent' AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .flatten()
    .unwrap_or_else(|| "rgb(128, 128, 128)".to_string())
}
