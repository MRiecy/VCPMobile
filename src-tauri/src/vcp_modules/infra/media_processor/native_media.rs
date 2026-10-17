use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Runtime};
use tokio_util::sync::CancellationToken;

pub(super) const AUDIO_MAX_BYTES: u64 = 16 * 1024 * 1024;
pub(super) const VIDEO_MAX_BYTES: u64 = 12_000_000;
const CACHE_SCHEMA_VERSION: u8 = 2;

pub(crate) fn check_cancelled(token: &CancellationToken) -> Result<(), String> {
    if token.is_cancelled() {
        Err("附件处理已取消".into())
    } else {
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedMedia<T> {
    schema_version: u8,
    kind: String,
    payload: T,
}

fn cache_path<R: Runtime>(app: &AppHandle<R>, source: &Path) -> Option<PathBuf> {
    let hash = source.file_stem()?.to_str()?;
    if hash.is_empty() {
        return None;
    }
    // Keep the CAS hash filename so attachment deletion and cache eviction keep
    // using their existing owner. The envelope rejects pre-normalization caches.
    Some(
        crate::vcp_modules::infra::file_manager::get_multimodal_cache_dir(app)
            .ok()?
            .join(format!("{hash}.json")),
    )
}

pub(super) async fn read_cache<R: Runtime, T: DeserializeOwned + Send + 'static>(
    app: &AppHandle<R>,
    source: &Path,
    kind: &'static str,
    max_cache_bytes: u64,
) -> Option<T> {
    let path = cache_path(app, source)?;
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(path).ok()?;
        if file.metadata().ok()?.len() > max_cache_bytes {
            return None;
        }
        let cached: CachedMedia<T> = serde_json::from_reader(BufReader::new(file)).ok()?;
        (cached.schema_version == CACHE_SCHEMA_VERSION && cached.kind == kind)
            .then_some(cached.payload)
    })
    .await
    .ok()
    .flatten()
}

pub(super) async fn write_cache<R: Runtime, T: Serialize + Send + 'static>(
    app: &AppHandle<R>,
    source: &Path,
    kind: &'static str,
    payload: T,
) -> Result<T, String> {
    let path = cache_path(app, source);
    let app = app.clone();
    tokio::task::spawn_blocking(move || {
        let cached = CachedMedia {
            schema_version: CACHE_SCHEMA_VERSION,
            kind: kind.to_string(),
            payload,
        };
        if let Some(path) = path {
            let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
            let write_result = (|| -> Result<(), String> {
                let parent = path.parent().ok_or("媒体缓存目录不可用")?;
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                let mut writer = BufWriter::new(
                    std::fs::File::create(&temporary).map_err(|error| error.to_string())?,
                );
                serde_json::to_writer(&mut writer, &cached).map_err(|error| error.to_string())?;
                writer.flush().map_err(|error| error.to_string())?;
                drop(writer);
                std::fs::rename(&temporary, &path).map_err(|error| error.to_string())
            })();
            if let Err(error) = write_result {
                let _ = std::fs::remove_file(&temporary);
                log::warn!("[MediaProcessor] Could not cache {kind} result: {error}");
            } else {
                crate::vcp_modules::infra::file_manager::evict_multimodal_cache_if_needed(
                    &app,
                    300 * 1024 * 1024,
                    150 * 1024 * 1024,
                );
            }
        }
        cached.payload
    })
    .await
    .map_err(|error| format!("媒体缓存任务失败：{error}"))
}

pub(super) fn read_output_file(
    output_dir: &Path,
    path: &Path,
    max_bytes: u64,
) -> Result<Vec<u8>, String> {
    let directory = std::fs::canonicalize(output_dir).map_err(|error| error.to_string())?;
    let path = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    if path.parent() != Some(directory.as_path()) {
        return Err("媒体处理结果不属于本次临时目录".into());
    }
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > max_bytes {
        return Err("媒体处理结果为空或超过编码预算".into());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 != metadata.len() || bytes.len() as u64 > max_bytes {
        return Err("媒体处理结果的文件大小发生变化".into());
    }
    Ok(bytes)
}

#[cfg(any(target_os = "android", target_os = "ios"))]
async fn native_call<R: Runtime, T: DeserializeOwned + Send + 'static>(
    app: &AppHandle<R>,
    command: &'static str,
    payload: serde_json::Value,
) -> Result<T, String> {
    use tauri::Manager;
    let handle = app
        .state::<tauri_plugin_vcp_mobile::VcpMobileState<R>>()
        .mobile_plugin_handle()?;
    // Tauri's native receiver must survive until its callback. In particular,
    // dropping run_mobile_plugin_async on cancellation would drop that receiver.
    tokio::task::spawn_blocking(move || {
        handle
            .run_mobile_plugin::<T>(command, payload)
            .map_err(|error| format!("{command}: {error}"))
    })
    .await
    .map_err(|error| format!("媒体原生桥接任务失败：{error}"))?
}

/// This resource worker owns only codecs and temporary files. The existing chat
/// request token decides cancellation; its caller remains the sole finalizer.
/// If that caller disappears, the worker still drains the native callback and
/// removes its files before exiting, rather than orphaning a codec operation.
pub(super) async fn run_native_media<R, T, U, F>(
    app: &AppHandle<R>,
    source: &Path,
    command: &'static str,
    max_bytes: u64,
    cancellation: &CancellationToken,
    consume: F,
) -> Result<U, String>
where
    R: Runtime,
    T: DeserializeOwned + Send + 'static,
    U: Send + 'static,
    F: FnOnce(T, &Path, &CancellationToken) -> Result<U, String> + Send + 'static,
{
    check_cancelled(cancellation)?;
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let operation_id = uuid::Uuid::new_v4().to_string();
        let output_dir = crate::vcp_modules::infra::file_manager::get_cache_root_dir(app)?
            .join("media-work")
            .join(&operation_id);
        let token = cancellation.child_token();
        let _cancel_on_drop = token.clone().drop_guard();
        let app = app.clone();
        let source = source.to_path_buf();
        tokio::spawn(async move {
            tokio::fs::create_dir_all(&output_dir)
                .await
                .map_err(|error| format!("无法创建媒体临时目录：{error}"))?;
            let mut cancellation_sent = false;
            let result = async {
                // Admission is settled before cancellation can race a queued
                // process command. A cancelled/absent admission cannot start.
                native_call::<_, serde_json::Value>(
                    &app,
                    "prepareMediaOperation",
                    serde_json::json!({"operationId": operation_id}),
                )
                .await?;
                if token.is_cancelled() {
                    cancellation_sent = true;
                    let _ = native_call::<_, serde_json::Value>(
                        &app,
                        "cancelMediaOperation",
                        serde_json::json!({"operationId": operation_id}),
                    )
                    .await;
                    return Err("附件处理已取消".into());
                }
                let processing = native_call::<_, T>(
                    &app,
                    command,
                    serde_json::json!({
                        "operationId": operation_id,
                        "path": source,
                        "outputDir": output_dir,
                        "maxBytes": max_bytes,
                    }),
                );
                tokio::pin!(processing);
                let output = tokio::select! {
                    biased;
                    _ = token.cancelled() => {
                        cancellation_sent = true;
                        if let Err(error) = native_call::<_, serde_json::Value>(
                            &app,
                            "cancelMediaOperation",
                            serde_json::json!({"operationId": operation_id}),
                        ).await {
                            log::warn!("[MediaProcessor] Native cancellation failed: {error}");
                        }
                        // Poll the original call even if cancellation won before
                        // its first poll; the revoked admission rejects it safely.
                        let _ = processing.await;
                        return Err("附件处理已取消".into());
                    }
                    result = &mut processing => result?,
                };
                check_cancelled(&token)?;
                let consume_dir = output_dir.clone();
                let consume_token = token.clone();
                let result = tokio::task::spawn_blocking(move || {
                    consume(output, &consume_dir, &consume_token)
                })
                .await
                .map_err(|error| format!("读取媒体处理结果失败：{error}"))?;
                check_cancelled(&token)?;
                result
            }
            .await;
            if result.is_err() && !cancellation_sent {
                // A bridge/dispatch error may leave a prepared slot that never
                // reached the codec. Cancellation is idempotent after settlement.
                let _ = native_call::<_, serde_json::Value>(
                    &app,
                    "cancelMediaOperation",
                    serde_json::json!({"operationId": operation_id}),
                )
                .await;
            }
            if let Err(error) = tokio::fs::remove_dir_all(&output_dir).await {
                if error.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("[MediaProcessor] Temporary media cleanup failed: {error}");
                }
            }
            result
        })
        .await
        .map_err(|error| format!("媒体处理任务失败：{error}"))?
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = (app, source, command, max_bytes, consume);
        Err("音视频附件处理仅在 Android/iOS 中可用".into())
    }
}
