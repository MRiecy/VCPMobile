//! admin_api 共享助手（供 logcenter / taskcenter 等新模块使用）。
//!
//! 约定来自 diary_service 的同款实现（本期按 S0 裁决 diary 模块不迁移，
//! 两处实现暂时并存；新模块一律使用本模块，不要再复制第三份）：
//! - Base URL 规范化：剥掉 `/v1/chat/completions` 后缀、拒绝内嵌凭据；
//! - 认证：HTTP Basic（admin_username / admin_password）；
//! - Client：统一走 `http_clients::HttpProfile::AdminApi` 共享画像。

use super::http_clients::{client, HttpProfile};
use crate::vcp_modules::settings_manager::Settings;
use reqwest::{Method, RequestBuilder, Url};

/// 校验 admin_api 所需的配置齐备（Server URL + 管理员凭据）。
pub fn ensure_admin_config(settings: &Settings, feature_label: &str) -> Result<(), String> {
    normalize_server_base(&settings.server.url)?;
    if settings.server.admin_username.trim().is_empty() || settings.server.admin_password.is_empty() {
        return Err(format!(
            "{feature_label}需要管理员用户名与密码，请在 设置 → 用户档案 或 设置 → 数据同步 中填写"
        ));
    }
    Ok(())
}

/// 规范化 VCP Server URL 为服务根地址（以 `/` 结尾）。
pub fn normalize_server_base(raw: &str) -> Result<Url, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("尚未配置 VCP Server URL".to_string());
    }
    if trimmed.chars().any(char::is_control) {
        return Err("VCP Server URL 含控制字符".to_string());
    }

    let mut url = Url::parse(trimmed).map_err(|_| "VCP Server URL 格式无效".to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("VCP Server URL 必须是无内嵌凭据的 HTTP(S) 地址".to_string());
    }
    url.set_query(None);
    url.set_fragment(None);

    let path = url.path().trim_end_matches('/');
    let base_path = path.strip_suffix("/v1/chat/completions").unwrap_or(path);
    let normalized_path = if base_path.is_empty() {
        "/".to_string()
    } else {
        format!("{}/", base_path.trim_end_matches('/'))
    };
    url.set_path(&normalized_path);
    Ok(url)
}

/// 在服务根地址上拼接路径段（每段自动百分号编码）。
pub fn append_url_segments(url: &mut Url, segments: &[&str]) -> Result<(), String> {
    let mut path = url
        .path_segments_mut()
        .map_err(|_| "VCP Server URL 不能作为分层 HTTP 地址".to_string())?;
    path.pop_if_empty();
    for segment in segments {
        path.push(segment);
    }
    Ok(())
}

/// 构造指向 `/admin_api/<suffix...>` 的 URL。
pub fn admin_url(settings: &Settings, suffix: &[&str]) -> Result<Url, String> {
    let mut url = normalize_server_base(&settings.server.url)?;
    append_url_segments(&mut url, &["admin_api"])?;
    append_url_segments(&mut url, suffix)?;
    Ok(url)
}

/// 构造带 Basic Auth 的 admin_api 请求（共享 AdminApi 画像 Client）。
/// 超时等请求级策略由调用方在返回的 `RequestBuilder` 上继续设置。
pub fn admin_request(
    settings: &Settings,
    method: Method,
    suffix: &[&str],
) -> Result<RequestBuilder, String> {
    let url = admin_url(settings, suffix)?;
    client_request(settings, method, url.as_str())
}

/// 以完整 URL 字符串构造带 Basic Auth 的请求（调用方已自行拼好路径）。
pub fn client_request(
    settings: &Settings,
    method: Method,
    url: &str,
) -> Result<RequestBuilder, String> {
    let parsed = Url::parse(url).map_err(|_| "内部错误：admin_api URL 解析失败".to_string())?;
    Ok(client(HttpProfile::AdminApi)
        .request(method, parsed)
        .basic_auth(
            &settings.server.admin_username,
            Some(&settings.server.admin_password),
        )
        .header(reqwest::header::ACCEPT, "application/json"))
}

/// GET 便捷函数。
pub fn client_get(settings: &Settings, url: &str) -> Result<RequestBuilder, String> {
    client_request(settings, Method::GET, url)
}

/// POST JSON 便捷函数。
pub fn client_post_json(
    settings: &Settings,
    url: &str,
    body: &serde_json::Value,
) -> Result<RequestBuilder, String> {
    Ok(client_request(settings, Method::POST, url)?.json(body))
}
