use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

mod export;

pub use export::export_runtime_diagnostics;

const MAX_FIELD_CHARS: usize = 16_000;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
static FRONTEND_LOG_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendDiagnosticEvent {
    pub level: String,
    pub source: String,
    pub message: String,
    pub stack: Option<String>,
    pub location: Option<String>,
    pub window_label: Option<String>,
    pub timestamp: Option<i64>,
}

fn truncate(value: String) -> String {
    if value.chars().count() <= MAX_FIELD_CHARS {
        return value;
    }

    let mut truncated = value.chars().take(MAX_FIELD_CHARS).collect::<String>();
    truncated.push_str("\n…[已截断]");
    truncated
}

fn diagnostics_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法定位应用数据目录: {error}"))?
        .join("diagnostics");
    fs::create_dir_all(&dir).map_err(|error| format!("无法创建诊断目录: {error}"))?;
    Ok(dir)
}

fn rotate_if_needed(path: &Path, max_bytes: u64) {
    if path.metadata().map(|value| value.len()).unwrap_or(0) < max_bytes {
        return;
    }

    let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
        return;
    };
    let backup = path.with_file_name(format!("{file_name}.1"));
    let _ = fs::remove_file(&backup);
    let _ = fs::rename(path, backup);
}

fn append_json_line(path: &Path, event: &FrontendDiagnosticEvent) -> Result<(), String> {
    let _guard = FRONTEND_LOG_LOCK
        .lock()
        .map_err(|_| "前端诊断日志写入锁已损坏".to_string())?;
    rotate_if_needed(path, MAX_FILE_BYTES);

    let mut line =
        serde_json::to_vec(event).map_err(|error| format!("无法序列化前端诊断事件: {error}"))?;
    line.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("无法打开前端诊断日志: {error}"))?;
    file.write_all(&line)
        .map_err(|error| format!("无法写入前端诊断日志: {error}"))?;
    file.flush()
        .map_err(|error| format!("无法刷新前端诊断日志: {error}"))
}

#[tauri::command]
pub fn record_frontend_diagnostic(
    app: AppHandle,
    mut event: FrontendDiagnosticEvent,
) -> Result<(), String> {
    event.level = truncate(event.level);
    event.source = truncate(event.source);
    event.message = truncate(event.message);
    event.stack = event.stack.map(truncate);
    event.location = event.location.map(truncate);
    event.window_label = event.window_label.map(truncate);
    event
        .timestamp
        .get_or_insert_with(|| Utc::now().timestamp_millis());

    if event.level.eq_ignore_ascii_case("warn") {
        log::warn!("[FrontendDiagnostic][{}] {}", event.source, event.message);
    } else {
        log::error!("[FrontendDiagnostic][{}] {}", event.source, event.message);
    }

    let path = diagnostics_dir(&app)?.join("frontend-errors.jsonl");
    append_json_line(&path, &event)
}

pub fn install_panic_hook(app: AppHandle) {
    let Ok(dir) = diagnostics_dir(&app) else {
        return;
    };
    let panic_path = dir.join("rust-panic.log");
    let previous_hook = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |panic_info| {
        let payload = panic_info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| {
                panic_info
                    .payload()
                    .downcast_ref::<String>()
                    .map(String::as_str)
            })
            .unwrap_or("未知 panic 载荷");
        let location = panic_info
            .location()
            .map(|value| format!("{}:{}:{}", value.file(), value.line(), value.column()))
            .unwrap_or_else(|| "未知位置".to_string());
        let backtrace = std::backtrace::Backtrace::force_capture();
        let report = format!(
            "\n===== {} =====\n位置: {}\n信息: {}\n回溯:\n{}\n",
            Utc::now().to_rfc3339(),
            location,
            payload,
            backtrace
        );

        rotate_if_needed(&panic_path, MAX_FILE_BYTES);
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&panic_path)
        {
            let _ = file.write_all(report.as_bytes());
            let _ = file.flush();
        }
        log::error!("[RustPanic] {} at {}", payload, location);
        previous_hook(panic_info);
    }));
}
