//! VCP 管理接口的共享请求构造。
//!
//! - 地址解析与路由派生由 `vcp_http_endpoint` 统一处理；
//! - 认证：HTTP Basic（admin_username / admin_password）；
//! - Client：统一走 `http_clients::HttpProfile::AdminApi` 共享画像。

use super::http_clients::{client, HttpProfile};
use super::vcp_http_endpoint::{append_url_segments, resolve_service_base};
use crate::vcp_modules::settings_manager::Settings;
use reqwest::{Method, RequestBuilder, Url};

/// 校验 admin_api 所需的配置齐备（Server URL + 管理员凭据）。
pub fn ensure_admin_config(settings: &Settings, feature_label: &str) -> Result<(), String> {
    resolve_service_base(&settings.server.url, settings.server.endpoint_mode)
        .map_err(|error| error.to_string())?;
    if settings.server.admin_username.trim().is_empty() || settings.server.admin_password.is_empty()
    {
        return Err(format!(
            "{feature_label}需要管理员用户名与密码，请在 设置 → 用户身份 中填写"
        ));
    }
    Ok(())
}

/// 构造指向 `/admin_api/<suffix...>` 的 URL。
pub fn admin_url(settings: &Settings, suffix: &[&str]) -> Result<Url, String> {
    let mut url = resolve_service_base(&settings.server.url, settings.server.endpoint_mode)
        .map_err(|error| error.to_string())?;
    append_url_segments(&mut url, &["admin_api"]).map_err(|error| error.to_string())?;
    append_url_segments(&mut url, suffix).map_err(|error| error.to_string())?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcp_modules::settings_manager::{ChatEndpointMode, ServerConfig};
    use axum::http::{HeaderMap, Uri};
    use axum::{Json, Router};
    use serde_json::{json, Value};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn admin_request_sends_one_basic_header_and_preserves_only_operation_query() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new().fallback(|uri: Uri, headers: HeaderMap| async move {
            let authorization: Vec<_> = headers
                .get_all(reqwest::header::AUTHORIZATION)
                .iter()
                .map(|value| value.to_str().unwrap().to_owned())
                .collect();
            Json(json!({ "uri": uri.to_string(), "authorization": authorization }))
        });
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let settings = Settings {
            server: ServerConfig {
                url: format!("http://{address}/proxy/v1/chatvcp/completions?chatOnly=secret"),
                endpoint_mode: ChatEndpointMode::VcpTools,
                api_key: "unused-bearer".into(),
                admin_username: "user".into(),
                admin_password: "pass".into(),
                ..Default::default()
            },
            ..Default::default()
        };

        let response: Value = admin_request(
            &settings,
            Method::GET,
            &["forum", "post", "topic/with space"],
        )
        .unwrap()
        .query(&[("cursor", "next/part")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
        server.abort();

        assert_eq!(
            response["uri"],
            "/proxy/admin_api/forum/post/topic%2Fwith%20space?cursor=next%2Fpart"
        );
        assert_eq!(response["authorization"], json!(["Basic dXNlcjpwYXNz"]));
    }
}
