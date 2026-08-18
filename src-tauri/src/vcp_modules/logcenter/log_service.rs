//! 日志中心远端服务：admin_api server-log 协议的认证代理 + 边界防护。
//!
//! 职责边界：只做「带认证的 HTTP 代理 + 大小/编码防护」。增量状态机
//! （offset / trailingFragment / 行裁剪）全部在前端 Store（见
//! `src/features/logcenter/logCenterStore.ts`），本模块不持有任何会话状态。

use crate::vcp_modules::infra::admin_api;
use crate::vcp_modules::settings_manager::{read_settings, SettingsState};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::{AppHandle, Runtime, State};

/// 全量/增量拉取的总超时（本地网络通常 <1s，弱网留足余量）。
const FETCH_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
/// 清空操作总超时。
const CLEAR_TOTAL_TIMEOUT: Duration = Duration::from_secs(15);
/// 响应体上限：后端全量最多返回尾部 2MB 文本，JSON 转义后放寬到 8 MiB；
/// 超过即视为异常响应，拒绝读入内存。
const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;

/// `GET /admin_api/server-log` 的响应契约（与 VCPToolBox routes/admin/logs.js 对齐）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogFetchResult {
    /// 日志文本（全量快照或增量切片；轮转重载通知时为空）。
    #[serde(default)]
    pub content: String,
    /// 新的字节偏移（= 当前 fileSize）。
    #[serde(default)]
    pub offset: u64,
    /// 服务器端日志文件绝对路径（仅用于展示）。
    #[serde(default)]
    pub path: String,
    /// 服务器端日志文件大小。
    #[serde(default)]
    pub file_size: u64,
    /// true 表示检测到日志轮转/截断，前端必须丢弃本地缓冲并全量重拉。
    #[serde(default)]
    pub need_full_reload: bool,
}

/// 剥除响应首尾可能存在 UTF-8 替换字符（U+FFFD）。
///
/// 后端按**字节** offset 切片后做 lossy UTF-8 解码：offset 落在多字节字符
/// 中间时，切片首/尾会产生 U+FFFD。前端的半行拼接（trailingFragment）
/// 只能拼「行」边界，拼不回「字符」边界，因此在 Rust 侧剥除。
fn strip_boundary_replacement_chars(content: &str) -> &str {
    content.trim_matches('\u{FFFD}')
}

async fn read_capped_text(resp: reqwest::Response) -> Result<String, String> {
    if let Some(len) = resp.content_length() {
        if len > MAX_RESPONSE_BYTES {
            return Err(format!("日志响应体异常庞大（{len} 字节），已拒绝读取"));
        }
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取日志响应失败: {e}"))?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(format!(
            "日志响应体超过上限（{} 字节），已拒绝解析",
            bytes.len()
        ));
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| "服务器返回了非 UTF-8 响应".to_string())
}

/// 拉取服务器日志。
///
/// - `incremental=false`：全量模式，后端最多返回文件尾部 2MB；
/// - `incremental=true` + `offset`：增量模式，返回 offset 之后的新增字节；
///   若响应 `needFullReload=true`（日志轮转/截断），前端须重置 offset 全量重拉。
#[tauri::command]
pub async fn logcenter_fetch<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    incremental: bool,
    offset: u64,
) -> Result<LogFetchResult, String> {
    let settings = read_settings(app_handle, settings_state).await?;
    admin_api::ensure_admin_config(&settings, "日志中心")?;

    let mut request = admin_api::admin_request(&settings, Method::GET, &["server-log"])?;
    if incremental {
        request = request.query(&[
            ("incremental", "true".to_string()),
            ("offset", offset.to_string()),
        ]);
    }
    let resp = request
        .timeout(FETCH_TOTAL_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("日志拉取请求失败: {e}"))?;

    let status = resp.status();
    if status != StatusCode::OK {
        // 后端错误响应同样带可读 content 字段，尽力透传给前端展示。
        let body = read_capped_text(resp).await.unwrap_or_default();
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
            if let Some(content) = value.get("content").and_then(|c| c.as_str()) {
                return Err(format!("服务器返回 {status}: {content}"));
            }
        }
        return Err(match status {
            StatusCode::NOT_FOUND => "服务器日志文件不存在（可能尚未创建）".to_string(),
            StatusCode::SERVICE_UNAVAILABLE => {
                "服务器日志路径暂不可用（可能仍在初始化）".to_string()
            }
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                "管理员凭据校验失败，请检查 设置 中的管理员账号与密码".to_string()
            }
            _ => format!("日志拉取失败: HTTP {status}"),
        });
    }

    let body = read_capped_text(resp).await?;
    let mut result: LogFetchResult =
        serde_json::from_str(&body).map_err(|_| "服务器返回了不符合契约的 JSON".to_string())?;
    let stripped = strip_boundary_replacement_chars(&result.content);
    if stripped.len() != result.content.len() {
        result.content = stripped.to_string();
    }
    Ok(result)
}

/// 清空服务器日志文件本体（危险操作，影响所有客户端）。
/// 前端必须二次确认后才可调用；成功后前端应重置本地 offset 并全量重拉。
#[tauri::command]
pub async fn logcenter_clear_server<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
) -> Result<String, String> {
    let settings = read_settings(app_handle, settings_state).await?;
    admin_api::ensure_admin_config(&settings, "日志中心")?;

    let resp = admin_api::admin_request(&settings, Method::POST, &["server-log", "clear"])?
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body("{}")
        .timeout(CLEAR_TOTAL_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("清空日志请求失败: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("清空服务器日志失败: HTTP {}", resp.status()));
    }
    Ok("服务器日志已清空".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_replacement_chars_at_boundaries() {
        assert_eq!(strip_boundary_replacement_chars("\u{FFFD}abc"), "abc");
        assert_eq!(strip_boundary_replacement_chars("abc\u{FFFD}"), "abc");
        assert_eq!(
            strip_boundary_replacement_chars("\u{FFFD}\u{FFFD}abc\u{FFFD}"),
            "abc"
        );
        assert_eq!(strip_boundary_replacement_chars("abc"), "abc");
        assert_eq!(strip_boundary_replacement_chars(""), "");
    }

    #[test]
    fn strip_preserves_interior_replacement_chars() {
        // 日志内容中间出现的 U+FFFD 可能本就是日志内容的一部分，不剥除。
        assert_eq!(
            strip_boundary_replacement_chars("ab\u{FFFD}cd"),
            "ab\u{FFFD}cd"
        );
    }

    #[test]
    fn fetch_result_deserializes_rotation_notice() {
        let json = r#"{"needFullReload":true,"path":"/srv/DebugLog/ServerLog.txt","offset":0}"#;
        let result: LogFetchResult = serde_json::from_str(json).unwrap();
        assert!(result.need_full_reload);
        assert_eq!(result.content, "");
        assert_eq!(result.offset, 0);
    }

    #[test]
    fn fetch_result_deserializes_full_payload() {
        let json = r#"{
            "content": "[2026-08-18 10:00:00] [INFO] boot\n",
            "offset": 42,
            "path": "/srv/DebugLog/ServerLog.txt",
            "fileSize": 42,
            "needFullReload": false
        }"#;
        let result: LogFetchResult = serde_json::from_str(json).unwrap();
        assert!(!result.need_full_reload);
        assert_eq!(result.offset, 42);
        assert_eq!(result.file_size, 42);
        assert!(result.content.contains("[INFO]"));
    }
}
