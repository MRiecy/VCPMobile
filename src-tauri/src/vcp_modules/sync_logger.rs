use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Debug = 0,
    Info = 1,
    Error = 2,
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
}

impl SyncLogger {
    pub fn new_session(log_level: LogLevel, log_dir: Option<PathBuf>) -> Self {
        println!("[Sync] Session started");

        let (log_file, log_path) = if let Some(dir) = log_dir {
            if let Ok(()) = fs::create_dir_all(&dir) {
                let filename = format!("{}_sync.log", chrono::Local::now().format("%Y%m%d_%H%M%S"));
                let path = dir.join(&filename);
                match OpenOptions::new().create(true).append(true).open(&path) {
                    Ok(file) => {
                        println!("[SyncLogger] Logging to {:?}", path);
                        (Some(file), Some(path))
                    }
                    Err(e) => {
                        println!("[SyncLogger] Failed to create log file: {}", e);
                        (None, None)
                    }
                }
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        Self {
            log_level,
            phases: HashMap::new(),
            error_aggregator: ErrorAggregator::new(),
            log_file,
            log_path,
        }
    }

    pub fn log(&mut self, level: LogLevel, phase: &str, message: &str) {
        if level < self.log_level {
            return;
        }

        let line = format!(
            "[{}] [{:?}] [{}] {}",
            chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z"),
            level,
            phase,
            message
        );
        println!("[Sync] [{}] {}", phase, message);

        if let Some(ref mut file) = self.log_file {
            let _ = writeln!(file, "{}", line);
            let _ = file.flush();
        }
    }

    pub fn log_path(&self) -> Option<&PathBuf> {
        self.log_path.as_ref()
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

    pub fn update_phase_expected(&mut self, phase: &str, delta: u32) {
        if let Some(metrics) = self.phases.get(phase) {
            metrics.expected_count.fetch_add(delta, Ordering::SeqCst);
        }
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
        println!("[Sync] Session ended");
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
