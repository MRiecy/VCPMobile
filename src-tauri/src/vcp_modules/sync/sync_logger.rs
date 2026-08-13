use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Trace = 0,
    Debug = 1,
    Info = 2,
    Warning = 3,
    Error = 4,
}

impl LogLevel {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_uppercase().as_str() {
            "TRACE" => Some(Self::Trace),
            "DEBUG" => Some(Self::Debug),
            "INFO" => Some(Self::Info),
            "WARN" | "WARNING" => Some(Self::Warning),
            "ERROR" => Some(Self::Error),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warning => "WARN",
            Self::Error => "ERROR",
        }
    }

    fn rust_level(self) -> log::Level {
        match self {
            Self::Trace => log::Level::Trace,
            Self::Debug => log::Level::Debug,
            Self::Info => log::Level::Info,
            Self::Warning => log::Level::Warn,
            Self::Error => log::Level::Error,
        }
    }
}

pub(crate) fn redact_sync_diagnostic(value: &str) -> String {
    static BEARER: OnceLock<Regex> = OnceLock::new();
    static SECRET_FIELD: OnceLock<Regex> = OnceLock::new();
    static SECRET_QUERY: OnceLock<Regex> = OnceLock::new();

    let bearer = BEARER.get_or_init(|| {
        Regex::new(r#"(?i)(\bBearer\s+)(?:\"[^\"\r\n]*\"|'[^'\r\n]*'|[^\s,;]+)"#)
            .expect("static bearer redaction regex must compile")
    });
    let secret_field = SECRET_FIELD.get_or_init(|| {
        Regex::new(
            r#"(?i)(\b(?:token|x[_-]?sync[_-]?token|sync[_-]?token|access[_-]?token|api[_-]?key|vcp[_-]?key|secret|password)\b\s*[:=]\s*)(?:\"[^\"\r\n]*\"|'[^'\r\n]*'|[^\s,;&#]+)"#,
        )
        .expect("static secret field redaction regex must compile")
    });
    let secret_query = SECRET_QUERY.get_or_init(|| {
        Regex::new(
            r#"(?i)([?&](?:token|sync(?:[_-]|%5f)?token|access(?:[_-]|%5f)?token|api(?:[_-]|%5f)?key|vcp(?:[_-]|%5f)?key|secret|password)=)[^&#\s]*"#,
        )
        .expect("static secret query redaction regex must compile")
    });

    let redacted = bearer.replace_all(value, "${1}[redacted]");
    let redacted = secret_field.replace_all(&redacted, "${1}[redacted]");
    secret_query
        .replace_all(&redacted, "${1}[redacted]")
        .into_owned()
}

pub struct SyncPhaseMetrics {
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
pub struct ErrorDetail;

pub struct ErrorAggregator {
    errors: HashMap<String, Vec<ErrorDetail>>,
}

impl ErrorAggregator {
    pub fn new() -> Self {
        Self {
            errors: HashMap::new(),
        }
    }

    pub fn add_error(&mut self, phase: &str, _id: &str, _error: &str, _retryable: bool) {
        const MAX_ERRORS_PER_PHASE: usize = 1000;
        let vec = self.errors.entry(phase.to_string()).or_default();
        if vec.len() >= MAX_ERRORS_PER_PHASE {
            vec.remove(0);
        }
        vec.push(ErrorDetail);
    }
}

pub struct SyncLogger {
    log_level: LogLevel,
    phases: HashMap<String, Arc<SyncPhaseMetrics>>,
    error_aggregator: ErrorAggregator,
    log_file: Option<std::fs::File>,
    log_path: Option<PathBuf>,
    initialization_error: Option<String>,
}

impl SyncLogger {
    pub fn new_session(log_level: LogLevel, log_dir: Option<PathBuf>, session_id: u64) -> Self {
        log::info!("[Sync] Session started");

        let (log_file, log_path, initialization_error) = if let Some(dir) = log_dir {
            match fs::create_dir_all(&dir) {
                Ok(()) => {
                    let filename = format!(
                        "{}_{}_sync.log",
                        chrono::Local::now().format("%Y%m%d_%H%M%S_%3f"),
                        session_id
                    );
                    let path = dir.join(&filename);
                    match OpenOptions::new().create_new(true).write(true).open(&path) {
                        Ok(file) => {
                            log::info!("[SyncLogger] Logging to {:?}", path);
                            (Some(file), Some(path), None)
                        }
                        Err(e) => {
                            let detail = redact_sync_diagnostic(&format!(
                                "Failed to create sync log file at {}: {e}",
                                path.display()
                            ));
                            log::error!("[SyncLogger] {detail}");
                            (None, None, Some(detail))
                        }
                    }
                }
                Err(e) => {
                    let detail = redact_sync_diagnostic(&format!(
                        "Failed to create sync log directory {}: {e}",
                        dir.display()
                    ));
                    log::error!("[SyncLogger] {detail}");
                    (None, None, Some(detail))
                }
            }
        } else {
            let detail = "Application log directory is unavailable".to_string();
            log::error!("[SyncLogger] {detail}");
            (None, None, Some(detail))
        };

        let mut logger = Self {
            log_level,
            phases: HashMap::new(),
            error_aggregator: ErrorAggregator::new(),
            log_file,
            log_path,
            initialization_error,
        };
        logger.log_direct(
            LogLevel::Info,
            "session",
            &format!("Session started (session_id={session_id})"),
        );
        logger
    }

    pub fn log_direct(&mut self, level: LogLevel, phase: &str, message: &str) {
        let safe_phase = redact_sync_diagnostic(phase);
        let safe_message = redact_sync_diagnostic(message);
        let line = format!(
            "[{}] [{}] [{}] {}",
            chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z"),
            level.as_str(),
            safe_phase,
            safe_message
        );
        log::log!(
            level.rust_level(),
            "[Sync] [{}] {}",
            safe_phase,
            safe_message
        );

        if let Some(ref mut file) = self.log_file {
            let _ = writeln!(file, "{}", line);
            let _ = file.flush();
        }
    }

    pub fn log(&mut self, level: LogLevel, phase: &str, message: &str) {
        if level < self.log_level {
            return;
        }

        self.log_direct(level, phase, message);
    }

    pub fn log_path(&self) -> Option<&PathBuf> {
        self.log_path.as_ref()
    }

    pub fn initialization_error(&self) -> Option<&str> {
        self.initialization_error.as_deref()
    }

    pub fn start_phase(&mut self, phase: &str, expected: u32) {
        let metrics = Arc::new(SyncPhaseMetrics {
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

    pub fn set_phase_expected(&mut self, phase: &str, expected: u32) {
        if let Some(metrics) = self.phases.get(phase) {
            metrics.expected_count.store(expected, Ordering::SeqCst);
        }
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

    pub fn complete_phase(&mut self, phase: &str) -> Option<PhaseSummary> {
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

    pub fn end_session(&mut self) {
        log::info!("[Sync] Session ended");
        if let Some(ref mut file) = self.log_file {
            let _ = writeln!(
                file,
                "[{}] [INFO] [sync] Session ended",
                chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z")
            );
            let _ = file.flush();
        }
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
    fn parses_all_configured_log_levels_case_insensitively() {
        assert_eq!(LogLevel::parse("trace"), Some(LogLevel::Trace));
        assert_eq!(LogLevel::parse("DEBUG"), Some(LogLevel::Debug));
        assert_eq!(LogLevel::parse(" info "), Some(LogLevel::Info));
        assert_eq!(LogLevel::parse("warning"), Some(LogLevel::Warning));
        assert_eq!(LogLevel::parse("ERROR"), Some(LogLevel::Error));
        assert_eq!(LogLevel::parse("verbose"), None);
    }

    #[test]
    fn redacts_secrets_but_keeps_diagnostic_locations_and_ids() {
        let input = concat!(
            "Authorization: Bearer top-secret; ",
            "syncToken=alpha, X-Sync-Token: beta, ",
            "ws://192.168.1.9:5890/sync?token=gamma&sync%5Ftoken=delta&topic=topic-7 ",
            "/data/user/0/com.vcp.avatar/logs/session.log"
        );
        let output = redact_sync_diagnostic(input);

        assert!(!output.contains("top-secret"));
        assert!(!output.contains("alpha"));
        assert!(!output.contains("beta"));
        assert!(!output.contains("gamma"));
        assert!(!output.contains("delta"));
        assert!(output.contains("192.168.1.9:5890"));
        assert!(output.contains("topic-7"));
        assert!(output.contains("/data/user/0/com.vcp.avatar/logs/session.log"));
    }

    #[test]
    fn creates_a_unique_session_file_with_standard_levels() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut logger = SyncLogger::new_session(LogLevel::Trace, Some(dir.path().into()), 42);
        logger.log(LogLevel::Warning, "network", "retrying");
        logger.end_session();

        let path = logger.log_path().expect("log path");
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("_42_sync.log"));
        let content = std::fs::read_to_string(path).expect("read log");
        assert!(content.contains("[INFO] [session] Session started (session_id=42)"));
        assert!(content.contains("[WARN] [network] retrying"));
    }

    #[test]
    fn reports_log_creation_failure_without_preventing_logger_use() {
        let dir = tempfile::tempdir().expect("tempdir");
        let blocked_path = dir.path().join("not-a-directory");
        std::fs::write(&blocked_path, b"occupied").expect("create blocking file");

        let mut logger = SyncLogger::new_session(LogLevel::Info, Some(blocked_path), 9);
        assert!(logger.log_path().is_none());
        assert!(logger.initialization_error().is_some());
        logger.log(LogLevel::Info, "sync", "continues without a file");
    }
}
