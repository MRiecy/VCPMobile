//! OTA APK 下载机：Range 断点续传、停滞判死、SHA-256 校验。
//!
//! 本模块只做单文件下载与校验的纯机制，不持有任何会话状态；
//! 重试编排与状态机跃迁由 `update_manager` 负责。

use futures_util::StreamExt;
use reqwest::{Client, StatusCode, Url};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 单个 chunk 的停滞判死时间：弱网下 30 秒无任何字节到达即放弃本次尝试。
pub const STALL_TIMEOUT: Duration = Duration::from_secs(30);

const HASH_READ_CHUNK: usize = 256 * 1024;

/// 单次下载尝试的结果。
#[derive(Debug, PartialEq, Eq)]
pub enum DownloadAttempt {
    /// 文件完整写入并通过字节数核对。
    Completed,
    /// 用户取消；`.part` 保留供后续续传。
    Cancelled,
    /// 可重试的失败（网络错误、停滞、不完整等）；`.part` 保留。
    Retryable(String),
    /// 服务端不接受续传起点；调用方应删除 `.part` 从头再来。
    RestartFromScratch,
}

/// 从 `Content-Range: bytes <start>-<end>/<total>` 中提取续传起点。
pub fn parse_content_range_start(value: &str) -> Option<u64> {
    let range = value.trim().strip_prefix("bytes ")?;
    let (start, _) = range.split_once('-')?;
    start.trim().parse().ok()
}

async fn file_len(path: &Path) -> u64 {
    tokio::fs::metadata(path)
        .await
        .map(|m| m.len())
        .unwrap_or(0)
}

/// 单尝试下载：把 `url` 的内容写入 `part_path`。
///
/// - `offset > 0` 时发送 `Range: bytes=<offset>-` 续传；服务端忽略 Range
///   （返回 200）时自动截断文件从头写；
/// - 每个 chunk 受 [`STALL_TIMEOUT`] 停滞判死保护；
/// - `cancel` 置位时在 chunk 边界安全退出并保留 `.part`；
/// - `on_progress` 携带累计字节数（含续传起点）。
///
/// 返回 `Err` 仅用于文件系统等不可重试的致命错误。
pub async fn download_once(
    client: &Client,
    url: &Url,
    part_path: &Path,
    max_bytes: u64,
    cancel: &AtomicBool,
    on_progress: &mut (dyn FnMut(u64) + Send),
) -> Result<DownloadAttempt, String> {
    let offset = file_len(part_path).await;

    let mut request = client.get(url.clone()).header("User-Agent", "VCPMobile");
    if offset > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
    }
    let res = match request.send().await {
        Ok(res) => res,
        Err(error) => return Ok(DownloadAttempt::Retryable(format!("下载请求失败: {error}"))),
    };

    let status = res.status();
    if status == StatusCode::RANGE_NOT_SATISFIABLE {
        // 本地 .part 已不落后于服务端（或已完整）：从头再来由调用方决定。
        return Ok(DownloadAttempt::RestartFromScratch);
    }
    if !status.is_success() {
        if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
            return Ok(DownloadAttempt::Retryable(format!(
                "下载失败 ({})",
                status.as_u16()
            )));
        }
        return Err(format!("下载失败 ({})", status.as_u16()));
    }

    // 续传对齐：206 时 Content-Range 起点必须等于本地字节数；
    // 200 表示服务端忽略 Range，截断重下。
    let mut effective_offset = offset;
    if status == StatusCode::PARTIAL_CONTENT {
        let start = res
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_content_range_start);
        match start {
            Some(start) if start == offset => {}
            _ => return Ok(DownloadAttempt::RestartFromScratch),
        }
    } else {
        effective_offset = 0;
    }

    let mut file = if effective_offset > 0 {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(part_path)
            .await
            .map_err(|e| format!("打开更新临时文件失败: {e}"))?
    } else {
        tokio::fs::File::create(part_path)
            .await
            .map_err(|e| format!("创建更新临时文件失败: {e}"))?
    };

    let mut downloaded = effective_offset;
    let mut stream = res.bytes_stream();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(DownloadAttempt::Cancelled);
        }
        let chunk = match tokio::time::timeout(STALL_TIMEOUT, stream.next()).await {
            Err(_) => {
                return Ok(DownloadAttempt::Retryable(format!(
                    "网络停滞超过 {} 秒",
                    STALL_TIMEOUT.as_secs()
                )))
            }
            Ok(None) => break,
            Ok(Some(Err(error))) => {
                return Ok(DownloadAttempt::Retryable(format!("下载流错误: {error}")))
            }
            Ok(Some(Ok(chunk))) => chunk,
        };
        downloaded = downloaded
            .checked_add(chunk.len() as u64)
            .filter(|next| *next <= max_bytes)
            .ok_or_else(|| format!("更新包超过 {} MiB 下载上限", max_bytes / 1024 / 1024))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("写入更新临时文件失败: {e}"))?;
        on_progress(downloaded);
    }

    file.flush()
        .await
        .map_err(|e| format!("刷新更新临时文件失败: {e}"))?;
    file.sync_all()
        .await
        .map_err(|e| format!("同步更新临时文件失败: {e}"))?;
    Ok(DownloadAttempt::Completed)
}

/// 流式计算文件 SHA-256 并与期望值（小写十六进制）比对。
pub async fn verify_file_sha256(path: &Path, expected_hex: &str) -> Result<bool, String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| format!("打开更新包校验失败: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; HASH_READ_CHUNK];
    loop {
        let n = file
            .read(&mut buf)
            .await
            .map_err(|e| format!("读取更新包校验失败: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual = format!("{:x}", hasher.finalize());
    Ok(actual == expected_hex.to_ascii_lowercase())
}
