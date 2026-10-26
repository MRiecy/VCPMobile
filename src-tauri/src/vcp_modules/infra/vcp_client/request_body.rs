use serde_json::Value;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

pub(super) const MAX_REQUEST_BYTES: u64 = 128 * 1024 * 1024;
pub(super) static PREPARATION_SLOT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// Rust owns only the prepared path. Android atomically renames it when the
/// helper accepts the request, so a lost reply cannot make Rust delete its body.
pub(super) struct PreparedBody {
    path: PathBuf,
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub token: String,
    pub length: u64,
}

impl Drop for PreparedBody {
    fn drop(&mut self) {
        let path = self.path.clone();
        tauri::async_runtime::spawn(async move {
            for file in [path.with_extension("part"), path] {
                if let Err(error) = tokio::fs::remove_file(&file).await {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        log::warn!("[VCPClient] Request body cleanup failed: {error}");
                    }
                }
            }
        });
    }
}

impl PreparedBody {
    pub async fn into_http_body(self) -> Result<reqwest::Body, String> {
        tokio::task::spawn_blocking(move || {
            let file = std::fs::File::open(&self.path)
                .map_err(|error| format!("无法打开模型请求正文：{error}"))?;
            if file.metadata().map_err(|error| error.to_string())?.len() != self.length {
                return Err("模型请求正文大小发生变化".into());
            }
            // SAFETY: this private, uniquely named file is immutable after
            // publication. Its sole cleanup owner moves into the mapping and
            // cannot remove it until every reqwest redirect/retry clone drops.
            let mapping = unsafe { memmap2::Mmap::map(&file) }
                .map_err(|error| format!("无法映射模型请求正文：{error}"))?;
            Ok(reqwest::Body::from(bytes::Bytes::from_owner(MappedBody {
                mapping,
                _file: file,
                _prepared: self,
            })))
        })
        .await
        .map_err(|error| format!("无法准备模型上传正文：{error}"))?
    }
}

// Bytes keeps this owner alive without a whole-body heap copy. Unlike an
// unrepeatable ReaderStream, it preserves reqwest's existing 307/308 behavior.
// Field order releases the mapping and file before scheduling path cleanup.
struct MappedBody {
    mapping: memmap2::Mmap,
    _file: std::fs::File,
    _prepared: PreparedBody,
}

impl AsRef<[u8]> for MappedBody {
    fn as_ref(&self) -> &[u8] {
        &self.mapping
    }
}

struct LimitedWriter<W> {
    output: W,
    length: u64,
    limit: u64,
    cancellation: CancellationToken,
}

impl<W: Write> Write for LimitedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.cancellation.is_cancelled() {
            return Err(std::io::Error::other("模型请求准备已取消"));
        }
        if bytes.len() as u64 > self.limit.saturating_sub(self.length) {
            return Err(std::io::Error::other(
                "本次请求正文超过 128 MiB，请减少附件或缩短历史",
            ));
        }
        let written = self.output.write(bytes)?;
        self.length += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.output.flush()
    }
}

pub(super) async fn prepare(
    cache_dir: PathBuf,
    value: Value,
    cancellation: CancellationToken,
    permit: tokio::sync::SemaphorePermit<'static>,
) -> Result<PreparedBody, String> {
    // Keep both cancellation and the preparation slot with the actual writer,
    // including when the caller disappears while this blocking work drains.
    let token = cancellation.child_token();
    let _cancel_on_drop = token.clone().drop_guard();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let directory = cache_dir.join("request-bodies").join("prepared");
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        // Only the main process writes prepared files. Never sweep helper-owned
        // files: its process may still be uploading after the main process died.
        static CLEANED: std::sync::Once = std::sync::Once::new();
        CLEANED.call_once(|| {
            if let Ok(entries) = std::fs::read_dir(&directory) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if matches!(
                        path.extension().and_then(|value| value.to_str()),
                        Some("body" | "part")
                    ) {
                        let _ = std::fs::remove_file(path);
                    }
                }
            }
        });
        let body_token = uuid::Uuid::new_v4().to_string();
        let mut body = PreparedBody {
            path: directory.join(format!("{body_token}.body")),
            token: body_token,
            length: 0,
        };
        let partial = body.path.with_extension("part");
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)
            .map_err(|error| format!("无法创建模型请求正文：{error}"))?;
        let mut writer = LimitedWriter {
            output: BufWriter::new(file),
            length: 0,
            limit: MAX_REQUEST_BYTES,
            cancellation: token.clone(),
        };
        serde_json::to_writer(&mut writer, &value)
            .map_err(|error| format!("模型请求正文准备失败：{error}"))?;
        writer
            .flush()
            .map_err(|error| format!("写入模型请求正文失败：{error}"))?;
        body.length = writer.length;
        drop(writer);
        if token.is_cancelled() {
            return Err("模型请求准备已取消".into());
        }
        std::fs::rename(partial, &body.path).map_err(|error| error.to_string())?;
        Ok(body)
    })
    .await
    .map_err(|error| format!("模型请求正文准备失败：{error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn final_budget_counts_utf8_and_json_escaping_before_writing() {
        let value = json!({"text": "中文\n\"😀"});
        let expected = serde_json::to_vec(&value).unwrap();
        let mut writer = LimitedWriter {
            output: Vec::new(),
            length: 0,
            limit: expected.len() as u64,
            cancellation: CancellationToken::new(),
        };
        serde_json::to_writer(&mut writer, &value).unwrap();
        assert_eq!(writer.output, expected);
        writer.output.clear();
        writer.length = 0;
        writer.limit -= 1;
        assert!(serde_json::to_writer(&mut writer, &value).is_err());
        assert!(writer.output.len() <= writer.limit as usize);
    }

    #[test]
    fn cancelled_writer_does_not_append_bytes() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let mut writer = LimitedWriter {
            output: Vec::new(),
            length: 0,
            limit: MAX_REQUEST_BYTES,
            cancellation,
        };
        assert!(writer.write_all(b"body").is_err());
        assert!(writer.output.is_empty());
    }
}
