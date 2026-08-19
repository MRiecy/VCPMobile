//! clawEmail 邮箱远端服务：`/admin_api/claw-mail/*` 的认证代理。
//!
//! 职责边界：认证、超时、响应大小上限、查询参数拼装、错误语义化
//! （503=插件未加载 / 401=凭据错误 / 429=防爆破限流带 Retry-After 语境）。
//! 邮件 JSON 以 Value 透传（契约解释权在前端 Store 的归一化层——
//! from/to/date 等字段上游形态不稳定，由前端宽容解析）。
//!
//! 上游事实（详见 plan/vcpmobile-more-tools-research/10 篇）：
//! - 响应包裹为 `{status:'success', ...}`（与 forum 的 `{success:true}` 不同）；
//! - 邮件本体在 ClawEmail 云端，列表/详情实时穿透，无本地存储；
//! - 标读唯一途径是读详情时带 markRead=true；垃圾箱为软删除。

use crate::vcp_modules::infra::admin_api;
use crate::vcp_modules::settings_manager::{read_settings, Settings, SettingsState};
use reqwest::{RequestBuilder, StatusCode};
use serde_json::Value;
use std::time::Duration;
use tauri::{AppHandle, Runtime, State};

/// 常规读/写总超时。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// state?refresh=true 会触发服务端 pollOnce 穿透云端，给足余量。
const REFRESH_TIMEOUT: Duration = Duration::from_secs(60);
/// 列表/状态响应体上限。
const MAX_LIST_BYTES: u64 = 4 * 1024 * 1024;
/// 详情响应体上限（附件解析文本截断 16000 字符/个，base64 内联图另计）。
const MAX_DETAIL_BYTES: u64 = 8 * 1024 * 1024;

fn build_url(settings: &Settings, suffix: &[&str]) -> Result<String, String> {
    Ok(admin_api::admin_url(settings, suffix)?.to_string())
}

/// mailId 校验：非空、无控制字符与路径分隔符（它是云端 opaque id，走路径段编码）。
fn validate_mail_id(mail_id: &str) -> Result<&str, String> {
    let trimmed = mail_id.trim();
    if trimmed.is_empty()
        || trimmed.chars().any(char::is_control)
        || trimmed.contains('/')
        || trimmed.contains('\\')
    {
        return Err("mailId 格式无效".to_string());
    }
    Ok(trimmed)
}

/// 可选 mailbox（mail1-4）/ user（完整地址）寻址参数校验。
fn validate_addressing(mailbox: &Option<String>, user: &Option<String>) -> Result<(), String> {
    for (label, value) in [("mailbox", mailbox), ("user", user)] {
        if let Some(v) = value {
            if v.trim().is_empty() || v.chars().any(char::is_control) {
                return Err(format!("{label} 参数无效"));
            }
        }
    }
    Ok(())
}

async fn send_json(
    request: RequestBuilder,
    timeout: Duration,
    max_bytes: u64,
) -> Result<Value, String> {
    let resp = request
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| format!("邮箱请求失败: {e}"))?;

    let status = resp.status();
    if let Some(len) = resp.content_length() {
        if len > max_bytes {
            return Err(format!("邮箱响应体异常庞大（{len} 字节），已拒绝读取"));
        }
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取邮箱响应失败: {e}"))?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!(
            "邮箱响应体超过上限（{} 字节），已拒绝解析",
            bytes.len()
        ));
    }

    let body: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "服务器返回了不符合契约的 JSON".to_string())?;

    if !status.is_success() {
        // clawMail 错误包裹：{status:'error', error:'...'}
        let detail = body
            .get("error")
            .and_then(|e| e.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                "管理员凭据校验失败，请检查 设置 中的管理员账号与密码".to_string()
            }
            StatusCode::TOO_MANY_REQUESTS => {
                "请求过于频繁，已被服务器临时限制，请稍后再试".to_string()
            }
            StatusCode::SERVICE_UNAVAILABLE => format!("PLUGIN_UNAVAILABLE:{detail}"),
            _ => format!("邮箱操作失败: {detail}"),
        });
    }
    Ok(body)
}

async fn settings_of<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
) -> Result<Settings, String> {
    let settings = read_settings(app_handle, settings_state).await?;
    admin_api::ensure_admin_config(&settings, "邮箱")?;
    Ok(settings)
}

/// 邮箱状态（mailboxes / users 摘要缓存 / wsStates / lastError）。
/// refresh=true 触发服务端 pollOnce 穿透刷新。
#[tauri::command]
pub async fn mail_state<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    refresh: Option<bool>,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let url = build_url(&settings, &["claw-mail", "state"])?;
    let refresh = refresh.unwrap_or(false);
    send_json(
        admin_api::client_get(&settings, &url)?.query(&[("refresh", if refresh { "true" } else { "false" })]),
        if refresh { REFRESH_TIMEOUT } else { DEFAULT_TIMEOUT },
        MAX_LIST_BYTES,
    )
    .await
}

/// 邮件列表（offset 分页 + 仅未读 + 文件夹过滤）。
#[tauri::command]
pub async fn mail_list<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    mailbox: Option<String>,
    user: Option<String>,
    limit: Option<u32>,
    start: Option<u32>,
    unread_only: Option<bool>,
    fid: Option<String>,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    validate_addressing(&mailbox, &user)?;
    if let Some(fid_value) = &fid {
        if fid_value.trim().is_empty() || fid_value.chars().any(char::is_control) {
            return Err("fid 参数无效".to_string());
        }
    }
    let limit = limit.unwrap_or(20).clamp(1, 100);
    let start = start.unwrap_or(0);

    let url = build_url(&settings, &["claw-mail", "messages"])?;
    let mut query: Vec<(&str, String)> = vec![
        ("limit", limit.to_string()),
        ("start", start.to_string()),
    ];
    if let Some(mailbox) = mailbox.as_deref() {
        query.push(("mailbox", mailbox.trim().to_string()));
    }
    if let Some(user) = user.as_deref() {
        query.push(("user", user.trim().to_string()));
    }
    if unread_only.unwrap_or(false) {
        query.push(("unreadOnly", "true".to_string()));
    }
    if let Some(fid) = fid.as_deref() {
        query.push(("fid", fid.trim().to_string()));
    }

    send_json(
        admin_api::client_get(&settings, &url)?.query(&query),
        DEFAULT_TIMEOUT,
        MAX_LIST_BYTES,
    )
    .await
}

/// 邮件详情（markdown + content[]；markRead 默认 false——读不副作用，
/// 用户显式操作才标读）。
#[tauri::command]
pub async fn mail_read<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    mail_id: String,
    mailbox: Option<String>,
    user: Option<String>,
    mark_read: Option<bool>,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let mail_id = validate_mail_id(&mail_id)?;
    validate_addressing(&mailbox, &user)?;

    let url = build_url(&settings, &["claw-mail", "messages", mail_id])?;
    let mut query: Vec<(&str, String)> = Vec::new();
    if let Some(mailbox) = mailbox.as_deref() {
        query.push(("mailbox", mailbox.trim().to_string()));
    }
    if let Some(user) = user.as_deref() {
        query.push(("user", user.trim().to_string()));
    }
    if mark_read.unwrap_or(false) {
        query.push(("markRead", "true".to_string()));
    }

    send_json(
        admin_api::client_get(&settings, &url)?.query(&query),
        DEFAULT_TIMEOUT,
        MAX_DETAIL_BYTES,
    )
    .await
}

/// 移入垃圾箱（软删除；confirm 由后端路由强制 true）。
#[tauri::command]
pub async fn mail_trash<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    mail_id: String,
    mailbox: Option<String>,
    user: Option<String>,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let mail_id = validate_mail_id(&mail_id)?;
    validate_addressing(&mailbox, &user)?;

    let url = build_url(&settings, &["claw-mail", "messages", mail_id, "trash"])?;
    send_json(
        admin_api::client_post_json(
            &settings,
            &url,
            &serde_json::json!({
                "mailbox": mailbox.as_deref().map(str::trim),
                "user": user.as_deref().map(str::trim),
            }),
        )?,
        DEFAULT_TIMEOUT,
        MAX_DETAIL_BYTES,
    )
    .await
}

// ===== V1.1/V2 端点（依赖上游补丁；404/非 2xx 走统一错误语义） =====

/// 发送新邮件。
#[tauri::command]
pub async fn mail_send<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    mailbox: Option<String>,
    user: Option<String>,
    to: String,
    cc: Option<String>,
    bcc: Option<String>,
    subject: String,
    body: String,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    validate_addressing(&mailbox, &user)?;
    if to.trim().is_empty() {
        return Err("收件人不能为空".to_string());
    }
    if subject.trim().is_empty() {
        return Err("主题不能为空".to_string());
    }
    if body.trim().is_empty() {
        return Err("正文不能为空".to_string());
    }

    let url = build_url(&settings, &["claw-mail", "messages"])?;
    send_json(
        admin_api::client_post_json(
            &settings,
            &url,
            &serde_json::json!({
                "mailbox": mailbox.as_deref().map(str::trim),
                "user": user.as_deref().map(str::trim),
                "to": to.trim(),
                "cc": cc.as_deref().map(str::trim),
                "bcc": bcc.as_deref().map(str::trim),
                "subject": subject.trim(),
                "body": body,
            }),
        )?,
        DEFAULT_TIMEOUT,
        MAX_DETAIL_BYTES,
    )
    .await
}

/// 回复邮件（服务端自动带原邮件上下文并标读原邮件）。
#[tauri::command]
pub async fn mail_reply<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    mail_id: String,
    mailbox: Option<String>,
    user: Option<String>,
    body: String,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let mail_id = validate_mail_id(&mail_id)?;
    validate_addressing(&mailbox, &user)?;
    if body.trim().is_empty() {
        return Err("回复正文不能为空".to_string());
    }

    let url = build_url(&settings, &["claw-mail", "messages", mail_id, "reply"])?;
    send_json(
        admin_api::client_post_json(
            &settings,
            &url,
            &serde_json::json!({
                "mailbox": mailbox.as_deref().map(str::trim),
                "user": user.as_deref().map(str::trim),
                "body": body,
            }),
        )?,
        DEFAULT_TIMEOUT,
        MAX_DETAIL_BYTES,
    )
    .await
}

/// 文件夹列表。
#[tauri::command]
pub async fn mail_folders<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    mailbox: Option<String>,
    user: Option<String>,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    validate_addressing(&mailbox, &user)?;
    let url = build_url(&settings, &["claw-mail", "folders"])?;
    let mut query: Vec<(&str, String)> = Vec::new();
    if let Some(mailbox) = mailbox.as_deref() {
        query.push(("mailbox", mailbox.trim().to_string()));
    }
    if let Some(user) = user.as_deref() {
        query.push(("user", user.trim().to_string()));
    }
    send_json(
        admin_api::client_get(&settings, &url)?.query(&query),
        DEFAULT_TIMEOUT,
        MAX_LIST_BYTES,
    )
    .await
}

/// 标记已读/未读。
#[tauri::command]
pub async fn mail_mark<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    mail_id: String,
    read: bool,
    mailbox: Option<String>,
    user: Option<String>,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let mail_id = validate_mail_id(&mail_id)?;
    validate_addressing(&mailbox, &user)?;

    let url = build_url(&settings, &["claw-mail", "messages", mail_id, "read"])?;
    send_json(
        admin_api::client_post_json(
            &settings,
            &url,
            &serde_json::json!({
                "read": read,
                "mailbox": mailbox.as_deref().map(str::trim),
                "user": user.as_deref().map(str::trim),
            }),
        )?,
        DEFAULT_TIMEOUT,
        MAX_DETAIL_BYTES,
    )
    .await
}

/// 移动到指定文件夹（target = 文件夹 id；从「已删除」恢复 = 移到收件箱 id）。
#[tauri::command]
pub async fn mail_move<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    mail_id: String,
    target: String,
    mailbox: Option<String>,
    user: Option<String>,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let mail_id = validate_mail_id(&mail_id)?;
    validate_addressing(&mailbox, &user)?;
    let target = target.trim();
    if target.is_empty() || target.chars().count() > 128 || target.chars().any(char::is_control) {
        return Err("目标文件夹 id 无效".to_string());
    }

    let url = build_url(&settings, &["claw-mail", "messages", mail_id, "move"])?;
    send_json(
        admin_api::client_post_json(
            &settings,
            &url,
            &serde_json::json!({
                "target": target,
                "mailbox": mailbox.as_deref().map(str::trim),
                "user": user.as_deref().map(str::trim),
            }),
        )?,
        DEFAULT_TIMEOUT,
        MAX_DETAIL_BYTES,
    )
    .await
}

/// 搜索邮件（keyword 必填；fts=true 全文搜索）。
#[tauri::command]
pub async fn mail_search<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    keyword: String,
    fts: Option<bool>,
    mailbox: Option<String>,
    user: Option<String>,
    limit: Option<u32>,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    validate_addressing(&mailbox, &user)?;
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Err("搜索关键词不能为空".to_string());
    }
    let limit = limit.unwrap_or(20).clamp(1, 100);

    let url = build_url(&settings, &["claw-mail", "search"])?;
    let mut query: Vec<(&str, String)> = vec![
        ("keyword", keyword.to_string()),
        ("limit", limit.to_string()),
    ];
    if fts.unwrap_or(false) {
        query.push(("fts", "true".to_string()));
    }
    if let Some(mailbox) = mailbox.as_deref() {
        query.push(("mailbox", mailbox.trim().to_string()));
    }
    if let Some(user) = user.as_deref() {
        query.push(("user", user.trim().to_string()));
    }
    send_json(
        admin_api::client_get(&settings, &url)?.query(&query),
        DEFAULT_TIMEOUT,
        MAX_LIST_BYTES,
    )
    .await
}

/// 附件下载上限（base64 经 IPC 回传，限制 16 MiB 原始字节）。
const MAX_ATTACHMENT_BYTES: usize = 16 * 1024 * 1024;

/// 下载附件（base64 回传；前端负责落盘/预览）。
#[tauri::command]
pub async fn mail_attachment<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    mail_id: String,
    part_id: String,
    mailbox: Option<String>,
    user: Option<String>,
) -> Result<Value, String> {
    let settings = settings_of(app_handle, settings_state).await?;
    let mail_id = validate_mail_id(&mail_id)?;
    let part_id = part_id.trim();
    if part_id.is_empty() || part_id.chars().any(char::is_control) {
        return Err("partId 参数无效".to_string());
    }
    validate_addressing(&mailbox, &user)?;

    let url = build_url(
        &settings,
        &["claw-mail", "messages", mail_id, "attachments", part_id],
    )?;
    let mut query: Vec<(&str, String)> = Vec::new();
    if let Some(mailbox) = mailbox.as_deref() {
        query.push(("mailbox", mailbox.trim().to_string()));
    }
    if let Some(user) = user.as_deref() {
        query.push(("user", user.trim().to_string()));
    }

    let resp = admin_api::client_get(&settings, &url)?
        .query(&query)
        .timeout(DEFAULT_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("附件下载失败: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                "管理员凭据校验失败，请检查 设置 中的管理员账号与密码".to_string()
            }
            StatusCode::NOT_FOUND => "附件不存在或服务器未打补丁（404）".to_string(),
            _ => format!("附件下载失败: HTTP {status}"),
        });
    }

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let filename = resp
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split("filename*=UTF-8''").nth(1).map(str::to_string))
        .and_then(|v| urlencoding_decode(&v))
        .unwrap_or_else(|| format!("attachment-{part_id}.bin"));

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取附件失败: {e}"))?;
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "附件超过 16 MiB 上限（{} 字节），请在网页端下载",
            bytes.len()
        ));
    }

    use base64::Engine;
    Ok(serde_json::json!({
        "filename": filename,
        "contentType": content_type,
        "size": bytes.len(),
        "dataBase64": base64::engine::general_purpose::STANDARD.encode(&bytes),
    }))
}

/// 极简 percent-decode（Content-Disposition filename* 用）。
fn urlencoding_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = input.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_mail_id_accepts_opaque_ids() {
        assert!(validate_mail_id("msg_abc123").is_ok());
        assert!(validate_mail_id("1724000000000.abcdef@claw").is_ok());
    }

    #[test]
    fn validate_mail_id_rejects_unsafe_input() {
        assert!(validate_mail_id("").is_err());
        assert!(validate_mail_id("  ").is_err());
        assert!(validate_mail_id("a/b").is_err());
        assert!(validate_mail_id("a\\b").is_err());
        assert!(validate_mail_id("a\u{0007}b").is_err());
    }

    #[test]
    fn validate_addressing_rejects_blank_and_control_chars() {
        assert!(validate_addressing(&Some("mail1".to_string()), &None).is_ok());
        assert!(validate_addressing(&Some("  ".to_string()), &None).is_err());
        assert!(validate_addressing(&None, &Some("a\u{0001}".to_string())).is_err());
        assert!(validate_addressing(&None, &None).is_ok());
    }
}
