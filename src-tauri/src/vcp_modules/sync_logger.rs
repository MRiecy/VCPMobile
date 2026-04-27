use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[allow(dead_code)]
pub enum LogLevel {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}

impl LogLevel {
    #[allow(dead_code)]
    pub fn from_str(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "DEBUG" => LogLevel::Debug,
            "WARN" | "WARNING" => LogLevel::Warn,
            "ERROR" => LogLevel::Error,
            _ => LogLevel::Info,
        }
    }
}

#[allow(dead_code)]
pub struct SyncPhaseMetrics {
    pub phase_name: String,
    pub started_at: Instant,
    pub expected_count: AtomicU32,
    pub success_count: AtomicU32,
    pub error_count: AtomicU32,
}

#[allow(dead_code)]
pub struct PhaseSummary {
    pub phase: String,
    pub expected: u32,
    pub success: u32,
    pub errors: u32,
    pub duration_ms: u64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ErrorDetail {
    pub id: String,
    pub error: String,
    pub timestamp: u64,
    pub retryable: bool,
}

pub struct ErrorAggregator {
    errors: HashMap<String, Vec<ErrorDetail>>,
}

impl ErrorAggregator {
    pub fn new() -> Self {
        Self {
            errors: HashMap::new(),
        }
    }

    pub fn add_error(&mut self, phase: &str, id: &str, error: &str, retryable: bool) {
        let detail = ErrorDetail {
            id: id.to_string(),
            error: error.to_string(),
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
            retryable,
        };

        self.errors
            .entry(phase.to_string())
            .or_default()
            .push(detail);
    }

    #[allow(dead_code)]
    pub fn get_summary(&self, phase: &str) -> Option<ErrorSummary> {
        let errors = self.errors.get(phase)?;

        let retryable_count = errors.iter().filter(|e| e.retryable).count();
        let non_retryable_count = errors.len() - retryable_count;

        Some(ErrorSummary {
            phase: phase.to_string(),
            total: errors.len(),
            retryable: retryable_count,
            non_retryable: non_retryable_count,
            details: errors.clone(),
        })
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ErrorSummary {
    pub phase: String,
    pub total: usize,
    pub retryable: usize,
    pub non_retryable: usize,
    pub details: Vec<ErrorDetail>,
}

pub struct SyncLogger {
    session_id: String,
    log_level: LogLevel,
    phases: HashMap<String, Arc<SyncPhaseMetrics>>,
    error_aggregator: ErrorAggregator,
}

impl SyncLogger {
    pub fn new_session(log_level: LogLevel) -> Self {
        let session_id = format!(
            "sync_{}_{}",
            chrono::Utc::now().timestamp_millis(),
            &rand::random::<u32>().to_string()[..8]
        );

        println!("[Sync] Session started");

        Self {
            session_id,
            log_level,
            phases: HashMap::new(),
            error_aggregator: ErrorAggregator::new(),
        }
    }

    pub fn log(&self, level: LogLevel, phase: &str, message: &str) {
        if level < self.log_level {
            return;
        }

        println!("[Sync] [{}] {}", phase, message);
    }

    pub fn start_phase(&mut self, phase: &str, expected: u32) {
        let metrics = Arc::new(SyncPhaseMetrics {
            phase_name: phase.to_string(),
            started_at: Instant::now(),
            expected_count: AtomicU32::new(expected),
            success_count: AtomicU32::new(0),
            error_count: AtomicU32::new(0),
        });

        self.phases.insert(phase.to_string(), metrics);

        self.log(
            LogLevel::Info,
            phase,
            &format!("Phase started (expected={})", expected),
        );
    }

    pub fn log_operation(
        &mut self,
        phase: &str,
        data_type: &str,
        id: &str,
        success: bool,
        detail: Option<&str>,
    ) {
        if let Some(metrics) = self.phases.get(phase) {
            if success {
                metrics.success_count.fetch_add(1, Ordering::SeqCst);
            } else {
                metrics.error_count.fetch_add(1, Ordering::SeqCst);
            }
        }

        let level = if success {
            LogLevel::Debug
        } else {
            LogLevel::Error
        };
        let status = if success { "success" } else { "error" };
        let msg = match detail {
            Some(d) => format!("{}:{} - {} ({})", data_type, id, status, d),
            None => format!("{}:{} - {}", data_type, id, status),
        };

        self.log(level, phase, &msg);

        if !success {
            if let Some(d) = detail {
                let retryable = d.contains("database is locked");
                self.error_aggregator.add_error(phase, id, d, retryable);
            }
        }
    }

    pub fn complete_phase(&self, phase: &str) -> Option<PhaseSummary> {
        let metrics = self.phases.get(phase)?;

        let duration = metrics.started_at.elapsed().as_millis() as u64;
        let expected = metrics.expected_count.load(Ordering::SeqCst);
        let success = metrics.success_count.load(Ordering::SeqCst);
        let errors = metrics.error_count.load(Ordering::SeqCst);

        self.log(
            LogLevel::Info,
            phase,
            &format!(
                "Phase completed in {}ms | expected={}, success={}, errors={}",
                duration, expected, success, errors
            ),
        );

        Some(PhaseSummary {
            phase: phase.to_string(),
            expected,
            success,
            errors,
            duration_ms: duration,
        })
    }

    #[allow(dead_code)]
    pub fn get_error_summary(&self, phase: &str) -> Option<ErrorSummary> {
        self.error_aggregator.get_summary(phase)
    }

    #[allow(dead_code)]
    pub fn get_session_id(&self) -> &str {
        &self.session_id
    }

    pub fn end_session(&self) {
        println!("[Sync] Session ended");
    }

    #[allow(dead_code)]
    pub fn emit_to_vcp_log<R: tauri::Runtime>(
        &self,
        app_handle: &AppHandle<R>,
        level: LogLevel,
        phase: &str,
        message: &str,
    ) {
        // Only emit ERROR level to vcp-log by default (per user preference)
        if level != LogLevel::Error {
            return;
        }

        let _ = app_handle.emit(
            "vcp-log",
            serde_json::json!({
                "level": match level {
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
                },
                "category": "sync",
                "phase": phase,
                "message": message,
                "sessionId": self.session_id,
                "timestamp": chrono::Utc::now().to_rfc3339()
            }),
        );
    }

    // Convenience method for logging with vcp-log emission
    #[allow(dead_code)]
    pub fn log_with_vcp<R: tauri::Runtime>(
        &mut self,
        app_handle: &AppHandle<R>,
        level: LogLevel,
        phase: &str,
        message: &str,
    ) {
        // Console logging
        self.log(level, phase, message);

        // VCP-log emission (only for errors)
        self.emit_to_vcp_log(app_handle, level, phase, message);
    }
}

impl Default for ErrorAggregator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level_filtering() {
        let logger = SyncLogger::new_session(LogLevel::Info);

        assert!(logger.log_level >= LogLevel::Info);
    }

    #[test]
    fn test_session_id_format() {
        let logger = SyncLogger::new_session(LogLevel::Info);
        assert!(logger.session_id.starts_with("sync_"));
        assert!(logger.session_id.contains("_"));
    }

    #[test]
    fn test_phase_tracking() {
        let mut logger = SyncLogger::new_session(LogLevel::Info);
        logger.start_phase("metadata", 10);

        logger.log_operation("metadata", "agent", "agent_001", true, None);
        logger.log_operation(
            "metadata",
            "group",
            "group_001",
            false,
            Some("database locked"),
        );

        let summary = logger.complete_phase("metadata").unwrap();
        assert_eq!(summary.expected, 10);
        assert_eq!(summary.success, 1);
        assert_eq!(summary.errors, 1);
    }

    #[test]
    fn test_error_aggregation() {
        let mut logger = SyncLogger::new_session(LogLevel::Info);
        logger.start_phase("metadata", 5);

        logger.log_operation(
            "metadata",
            "agent",
            "agent_001",
            false,
            Some("database is locked"),
        );
        logger.log_operation("metadata", "agent", "agent_002", false, Some("not found"));

        let summary = logger.get_error_summary("metadata").unwrap();
        assert_eq!(summary.total, 2);
        assert_eq!(summary.retryable, 1);
        assert_eq!(summary.non_retryable, 1);
    }

    #[test]
    fn test_log_level_from_str() {
        assert_eq!(LogLevel::from_str("debug"), LogLevel::Debug);
        assert_eq!(LogLevel::from_str("INFO"), LogLevel::Info);
        assert_eq!(LogLevel::from_str("warn"), LogLevel::Warn);
        assert_eq!(LogLevel::from_str("ERROR"), LogLevel::Error);
        assert_eq!(LogLevel::from_str("unknown"), LogLevel::Info);
    }
}
