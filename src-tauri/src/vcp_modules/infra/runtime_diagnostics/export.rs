use super::{diagnostics_dir, MAX_FILE_BYTES};
use chrono::Utc;
use serde::Serialize;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const MAX_BUNDLE_SOURCE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticSummary {
    generated_at: String,
    app_version: String,
    target_os: &'static str,
    target_arch: &'static str,
    android_exit_diagnostics: serde_json::Value,
    collection_warnings: Vec<String>,
}

fn collect_files(root: &Path, prefix: &str, files: &mut Vec<(PathBuf, String)>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let archive_name = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if file_type.is_dir() {
            collect_files(&path, &archive_name, files);
        } else if file_type.is_file() {
            files.push((path, archive_name));
        }
    }
}

fn read_file_tail(path: &Path, max_bytes: u64) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let len = file.metadata().map_err(|error| error.to_string())?.len();
    if len > max_bytes {
        file.seek(SeekFrom::End(-(max_bytes as i64)))
            .map_err(|error| error.to_string())?;
    }
    let mut bytes = Vec::with_capacity(len.min(max_bytes) as usize);
    file.read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

fn add_bytes(zip: &mut ZipWriter<File>, name: &str, bytes: &[u8]) -> Result<(), String> {
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file(name, options)
        .map_err(|error| format!("无法创建诊断包条目 {name}: {error}"))?;
    zip.write_all(bytes)
        .map_err(|error| format!("无法写入诊断包条目 {name}: {error}"))
}

fn build_bundle(
    export_path: &Path,
    log_dir: Option<PathBuf>,
    diagnostic_dir: PathBuf,
    summary: DiagnosticSummary,
) -> Result<(), String> {
    let file = File::create(export_path).map_err(|error| format!("无法创建诊断包: {error}"))?;
    let mut zip = ZipWriter::new(file);
    let mut sources = Vec::new();

    if let Some(log_dir) = log_dir.filter(|path| path.exists()) {
        collect_files(&log_dir, "logs", &mut sources);
    }
    if diagnostic_dir.exists() {
        collect_files(&diagnostic_dir, "diagnostics", &mut sources);
    }

    sources.sort_by(|left, right| left.1.cmp(&right.1));
    let mut included_bytes = 0_u64;
    let mut warnings = summary.collection_warnings.clone();
    for (path, archive_name) in sources {
        if included_bytes >= MAX_BUNDLE_SOURCE_BYTES {
            warnings.push("日志总体积超过 16MB，较旧条目未继续收集".to_string());
            break;
        }

        match read_file_tail(&path, MAX_FILE_BYTES) {
            Ok(bytes) => {
                included_bytes += bytes.len() as u64;
                add_bytes(&mut zip, &archive_name, &bytes)?;
            }
            Err(error) => warnings.push(format!("读取 {} 失败: {}", path.display(), error)),
        }
    }

    let summary = DiagnosticSummary {
        collection_warnings: warnings,
        ..summary
    };
    let summary_bytes = serde_json::to_vec_pretty(&summary)
        .map_err(|error| format!("无法生成诊断摘要: {error}"))?;
    add_bytes(&mut zip, "summary.json", &summary_bytes)?;

    #[cfg(target_os = "linux")]
    if let Ok(process_status) = fs::read("/proc/self/status") {
        add_bytes(&mut zip, "process-status.txt", &process_status)?;
    }

    zip.finish()
        .map_err(|error| format!("无法完成诊断包: {error}"))?;
    Ok(())
}

#[tauri::command]
pub async fn export_runtime_diagnostics(app: AppHandle) -> Result<String, String> {
    let diagnostic_dir = diagnostics_dir(&app)?;
    let log_dir = app.path().app_log_dir().ok();
    let export_dir = app
        .path()
        .app_cache_dir()
        .map_err(|error| format!("无法定位缓存目录: {error}"))?
        .join("diagnostic-exports");
    fs::create_dir_all(&export_dir).map_err(|error| format!("无法创建导出目录: {error}"))?;

    let android_exit_diagnostics =
        match tauri_plugin_vcp_mobile::system::get_process_exit_diagnostics(app.clone()) {
            Ok(value) => serde_json::to_value(value).unwrap_or_else(|_| serde_json::json!({})),
            Err(error) => serde_json::json!({ "supported": false, "error": error }),
        };
    let summary = DiagnosticSummary {
        generated_at: Utc::now().to_rfc3339(),
        app_version: app.package_info().version.to_string(),
        target_os: std::env::consts::OS,
        target_arch: std::env::consts::ARCH,
        android_exit_diagnostics,
        collection_warnings: Vec::new(),
    };
    let export_path = export_dir.join(format!(
        "vcp-mobile-diagnostics-{}.zip",
        Utc::now().format("%Y%m%d-%H%M%S")
    ));
    let build_path = export_path.clone();

    tauri::async_runtime::spawn_blocking(move || {
        build_bundle(&build_path, log_dir, diagnostic_dir, summary)
    })
    .await
    .map_err(|error| format!("诊断包任务执行失败: {error}"))??;

    Ok(export_path.to_string_lossy().to_string())
}
