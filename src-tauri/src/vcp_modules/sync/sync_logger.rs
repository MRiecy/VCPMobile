use crate::vcp_modules::db_manager::DbWriteMetric;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use regex::Regex;
use tokio::sync::{broadcast, oneshot};
use tokio::task::JoinHandle;

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

pub struct SyncLogger {
    log_level: LogLevel,
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

/// Session-local bridge from the persistence metric bus to the shareable sync log.
///
/// The broadcast receive and synchronous file write happen on this independent task, after the
/// database lease has released its writer guard. Lag or logger failure is diagnostic loss only and
/// can never change a database or sync result.
pub(crate) struct DbWriteMetricLogBridge {
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl DbWriteMetricLogBridge {
    pub(crate) fn start(
        receiver: broadcast::Receiver<DbWriteMetric>,
        logger: Arc<Mutex<SyncLogger>>,
    ) -> Self {
        let (stop, stop_rx) = oneshot::channel();
        let task = tokio::spawn(run_db_write_metric_log_bridge(receiver, logger, stop_rx));
        Self {
            stop: Some(stop),
            task,
        }
    }

    pub(crate) async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Err(error) = self.task.await {
            log::warn!("[SyncLogger] DbWrite metric bridge failed: {error}");
        }
    }
}

async fn run_db_write_metric_log_bridge(
    mut receiver: broadcast::Receiver<DbWriteMetric>,
    logger: Arc<Mutex<SyncLogger>>,
    mut stop: oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            biased;
            _ = &mut stop => {
                drain_db_write_metrics(&mut receiver, &logger);
                return;
            }
            received = receiver.recv() => {
                match received {
                    Ok(metric) => {
                        if !write_db_write_metric(&logger, metric) {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        if !write_db_write_metric_lag(&logger, skipped) {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        }
    }
}

fn drain_db_write_metrics(
    receiver: &mut broadcast::Receiver<DbWriteMetric>,
    logger: &Arc<Mutex<SyncLogger>>,
) {
    // Freeze the session cutoff before draining. A process-wide producer may continue publishing
    // ordinary writes after sync stops; chasing those new metrics could otherwise keep stop_sync
    // alive forever. Lagged entries count against this fixed snapshot because they were part of
    // the observed backlog even though the bounded ring has already overwritten them.
    let mut remaining = receiver.len();
    while remaining > 0 {
        match receiver.try_recv() {
            Ok(metric) => {
                remaining -= 1;
                if !write_db_write_metric(logger, metric) {
                    return;
                }
            }
            Err(broadcast::error::TryRecvError::Lagged(skipped)) => {
                let skipped_count = usize::try_from(skipped).unwrap_or(usize::MAX);
                remaining = remaining.saturating_sub(skipped_count);
                if !write_db_write_metric_lag(logger, skipped) {
                    return;
                }
            }
            Err(broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed) => {
                return
            }
        }
    }
}

fn write_db_write_metric(logger: &Arc<Mutex<SyncLogger>>, metric: DbWriteMetric) -> bool {
    let level = if metric.is_failure() {
        LogLevel::Error
    } else if metric.is_slow() {
        LogLevel::Warning
    } else {
        LogLevel::Debug
    };
    let message = if metric.is_slow() && !metric.is_failure() {
        format!("slow {metric}")
    } else {
        metric.to_string()
    };
    write_db_write_log(logger, level, &message)
}

fn write_db_write_metric_lag(logger: &Arc<Mutex<SyncLogger>>, skipped: u64) -> bool {
    write_db_write_log(
        logger,
        LogLevel::Warning,
        &format!("metric stream lagged; skipped={skipped}"),
    )
}

fn write_db_write_log(logger: &Arc<Mutex<SyncLogger>>, level: LogLevel, message: &str) -> bool {
    match logger.lock() {
        Ok(mut logger) => {
            logger.log(level, "db_write", message);
            true
        }
        Err(_) => {
            log::error!("[SyncLogger] DbWrite metric logger lock is poisoned");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_metric(operation: &'static str, outcome: &'static str) -> DbWriteMetric {
        DbWriteMetric {
            operation,
            outcome,
            wait_duration: std::time::Duration::from_millis(12),
            begin_duration: Some(std::time::Duration::from_millis(3)),
            hold_duration: std::time::Duration::from_millis(25),
            finish_duration: Some(std::time::Duration::from_millis(4)),
        }
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

    #[tokio::test]
    async fn db_write_bridge_records_debug_metrics_before_the_session_end() {
        let temp_dir = tempfile::tempdir().expect("create sync metric log directory");
        let logger = Arc::new(Mutex::new(SyncLogger::new_session(
            LogLevel::Debug,
            Some(temp_dir.path().to_path_buf()),
            77,
        )));
        let log_path = logger
            .lock()
            .expect("lock metric logger")
            .log_path()
            .cloned()
            .expect("metric logger path");
        let (sender, receiver) = broadcast::channel(8);
        let bridge = DbWriteMetricLogBridge::start(receiver, logger.clone());

        sender
            .send(test_metric("sync.queue", "committed"))
            .expect("send metric to active bridge");
        bridge.shutdown().await;
        logger
            .lock()
            .expect("lock metric logger for shutdown")
            .end_session();

        let contents = std::fs::read_to_string(log_path).expect("read sync metric log");
        assert!(contents.contains("[DEBUG] [db_write] operation=sync.queue outcome=committed"));
        assert!(contents.contains("wait_ms=12.000 begin_ms=3.000 hold_ms=25.000 finish_ms=4.000"));
        assert!(contents.contains("Session ended"));
        assert!(
            contents.find("operation=sync.queue").expect("metric line")
                < contents.find("Session ended").expect("session end line")
        );
    }

    #[tokio::test]
    async fn db_write_bridge_respects_the_sync_log_level() {
        let temp_dir = tempfile::tempdir().expect("create filtered metric log directory");
        let logger = Arc::new(Mutex::new(SyncLogger::new_session(
            LogLevel::Error,
            Some(temp_dir.path().to_path_buf()),
            78,
        )));
        let log_path = logger
            .lock()
            .expect("lock filtered metric logger")
            .log_path()
            .cloned()
            .expect("filtered metric logger path");
        let (sender, receiver) = broadcast::channel(8);
        let bridge = DbWriteMetricLogBridge::start(receiver, logger.clone());

        sender
            .send(test_metric("test.filtered-commit", "committed"))
            .expect("send filtered normal metric");
        sender
            .send(test_metric("test.visible-failure", "transaction_failed"))
            .expect("send visible failure metric");
        bridge.shutdown().await;
        logger
            .lock()
            .expect("lock filtered logger for shutdown")
            .end_session();

        let contents = std::fs::read_to_string(log_path).expect("read filtered metric log");
        assert!(!contents.contains("test.filtered-commit"));
        assert!(contents.contains("[ERROR] [db_write] operation=test.visible-failure"));
    }

    #[tokio::test]
    async fn db_write_bridge_shutdown_ignores_metrics_after_its_fixed_cutoff() {
        let temp_dir = tempfile::tempdir().expect("create shutdown metric log directory");
        let logger = Arc::new(Mutex::new(SyncLogger::new_session(
            LogLevel::Error,
            Some(temp_dir.path().to_path_buf()),
            79,
        )));
        let (sender, receiver) = broadcast::channel(8);
        let bridge = DbWriteMetricLogBridge::start(receiver, logger.clone());
        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let producer_running = running.clone();
        let produced = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let producer_count = produced.clone();
        let producer = std::thread::spawn(move || {
            let metric = test_metric("test.continuous-producer", "committed");
            while producer_running.load(std::sync::atomic::Ordering::Relaxed) {
                if sender.send(metric.clone()).is_ok() {
                    producer_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
        });

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while produced.load(std::sync::atomic::Ordering::Relaxed) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("continuous producer never published a metric");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let shutdown =
            tokio::time::timeout(std::time::Duration::from_secs(1), bridge.shutdown()).await;
        running.store(false, std::sync::atomic::Ordering::Relaxed);
        producer.join().expect("join continuous metric producer");

        shutdown.expect("metric bridge chased post-cutoff writes during shutdown");
    }
}
