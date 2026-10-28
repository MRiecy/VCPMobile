//! Pure HTTP endpoint rules shared by VCP Chat and its companion services.
//! Chat query parameters stay on Chat/model requests. Companion service URLs
//! deliberately use only the resolved deployment prefix; authentication belongs
//! to their request owners.

use super::settings_manager::ChatEndpointMode;
use url::Url;

const STANDARD_CHAT_SUFFIX: &str = "/v1/chat/completions";
const VCP_TOOLS_CHAT_SUFFIX: &str = "/v1/chatvcp/completions";
const MODELS_SUFFIX: &str = "/v1/models";
pub const MODEL_DISCOVERY_UNAVAILABLE: &str = "MODEL_DISCOVERY_UNAVAILABLE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRequestPurpose {
    Interactive,
    Auxiliary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpEndpointError {
    Missing,
    Invalid(String),
    UnavailableRaw,
}

impl std::fmt::Display for HttpEndpointError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => formatter.write_str("尚未配置 VCP Server URL，请在 设置 → 服务器连接 中填写。"),
            Self::Invalid(message) => formatter.write_str(message),
            Self::UnavailableRaw => formatter.write_str(
                "原始 URL 无法推导 VCP 服务根地址；请在 设置 → 服务器连接 中使用服务器根地址、/v1 或已知聊天端点。",
            ),
        }
    }
}

impl std::error::Error for HttpEndpointError {}

struct ValidatedHttpEndpoint {
    url: Url,
    explicit_default_port: Option<u16>,
}

fn explicit_port_from_raw(raw_url: &str) -> Option<u16> {
    let (_, after_scheme) = raw_url.split_once("://")?;
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let port = if authority.starts_with('[') {
        let closing_bracket = authority.find(']')?;
        authority.get(closing_bracket + 1..)?.strip_prefix(':')?
    } else {
        authority.rsplit_once(':')?.1
    };
    port.parse().ok()
}

fn validate_http_endpoint(raw_url: &str) -> Result<ValidatedHttpEndpoint, HttpEndpointError> {
    if raw_url.chars().any(char::is_control) {
        return Err(HttpEndpointError::Invalid("URL 不能包含控制字符".into()));
    }
    if raw_url.trim().is_empty() {
        return Err(HttpEndpointError::Missing);
    }

    let url = Url::parse(raw_url)
        .map_err(|error| HttpEndpointError::Invalid(format!("URL 解析失败: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(HttpEndpointError::Invalid(
            "URL 仅支持 HTTP 或 HTTPS".into(),
        ));
    }
    if url.host_str().is_none() {
        return Err(HttpEndpointError::Invalid("URL 必须包含主机名".into()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(HttpEndpointError::Invalid(
            "URL 不能包含用户名或密码".into(),
        ));
    }
    if url.fragment().is_some() {
        return Err(HttpEndpointError::Invalid("URL 不能包含 fragment".into()));
    }
    let explicit_default_port = explicit_port_from_raw(raw_url)
        .filter(|port| url.port().is_none() && url.port_or_known_default() == Some(*port));
    Ok(ValidatedHttpEndpoint {
        url,
        explicit_default_port,
    })
}

fn strip_known_chat_suffix(path: &str) -> Option<&str> {
    let path = path.trim_end_matches('/');
    [STANDARD_CHAT_SUFFIX, VCP_TOOLS_CHAT_SUFFIX]
        .into_iter()
        .find_map(|suffix| path.strip_suffix(suffix))
}

fn known_service_prefix(path: &str) -> Option<&str> {
    let path = path.trim_end_matches('/');
    if path.is_empty() {
        Some("")
    } else {
        strip_known_chat_suffix(path).or_else(|| path.strip_suffix("/v1"))
    }
}

fn service_prefix(path: &str) -> &str {
    let path = path.trim_end_matches('/');
    known_service_prefix(path)
        .unwrap_or(path)
        .trim_end_matches('/')
}

fn derive_known_api_endpoint(endpoint: ValidatedHttpEndpoint, suffix: &str) -> String {
    let ValidatedHttpEndpoint {
        mut url,
        explicit_default_port,
    } = endpoint;
    let prefix = service_prefix(url.path());
    url.set_path(&format!("{prefix}{suffix}"));
    let mut derived = url.to_string();
    if let Some(port) = explicit_default_port {
        if let Some(after_scheme) = derived.find("://").map(|index| index + 3) {
            let authority_len = derived[after_scheme..]
                .find(['/', '?', '#'])
                .unwrap_or(derived.len() - after_scheme);
            derived.insert_str(after_scheme + authority_len, &format!(":{port}"));
        }
    }
    derived
}

/// Resolve a deployment directory for VCP companion services. Their URLs do not
/// inherit Chat query parameters. Unknown Raw paths are complete endpoints, so
/// they cannot be treated as directories by guessing.
pub fn resolve_service_base(raw: &str, mode: ChatEndpointMode) -> Result<Url, HttpEndpointError> {
    let mut url = validate_http_endpoint(raw)?.url;
    if mode == ChatEndpointMode::Raw && known_service_prefix(url.path()).is_none() {
        return Err(HttpEndpointError::UnavailableRaw);
    }
    let prefix = service_prefix(url.path());
    let directory = format!("{prefix}/");
    url.set_path(&directory);
    url.set_query(None);
    Ok(url)
}

/// An explicit origin-only operation for resources deployed at the server root.
/// It does not change the prefix rules used by Chat or companion API requests.
pub fn resolve_http_origin(raw: &str) -> Result<Url, HttpEndpointError> {
    let mut url = validate_http_endpoint(raw)?.url;
    url.set_path("/");
    url.set_query(None);
    Ok(url)
}

/// Append raw path segments, encoding each once without replacing the existing
/// deployment prefix. Reject navigation segments before mutating the address.
pub fn append_url_segments(url: &mut Url, segments: &[&str]) -> Result<(), HttpEndpointError> {
    if segments
        .iter()
        .any(|segment| matches!(*segment, "." | ".."))
    {
        return Err(HttpEndpointError::Invalid(
            "URL 路径段不能是 . 或 ..".into(),
        ));
    }
    let mut path = url
        .path_segments_mut()
        .map_err(|_| HttpEndpointError::Invalid("URL 不能作为分层 HTTP 地址".into()))?;
    path.pop_if_empty();
    for segment in segments {
        path.push(segment);
    }
    Ok(())
}

pub fn resolve_chat_endpoint(
    raw_url: &str,
    mode: ChatEndpointMode,
    purpose: ChatRequestPurpose,
) -> Result<String, String> {
    let endpoint = validate_http_endpoint(raw_url).map_err(|error| error.to_string())?;
    if mode == ChatEndpointMode::Raw {
        // Raw requests retain the exact configured endpoint after validation.
        return Ok(raw_url.to_string());
    }

    let suffix = match (mode, purpose) {
        (ChatEndpointMode::VcpTools, ChatRequestPurpose::Interactive) => VCP_TOOLS_CHAT_SUFFIX,
        _ => STANDARD_CHAT_SUFFIX,
    };
    Ok(derive_known_api_endpoint(endpoint, suffix))
}

pub fn resolve_model_discovery_endpoint(
    raw_url: &str,
    mode: ChatEndpointMode,
) -> Result<Option<String>, String> {
    let endpoint = validate_http_endpoint(raw_url).map_err(|error| error.to_string())?;
    // Preserve the existing, narrower Raw discovery contract: only an explicit
    // known Chat endpoint enables automatic models discovery.
    if mode == ChatEndpointMode::Raw && strip_known_chat_suffix(endpoint.url.path()).is_none() {
        return Ok(None);
    }
    Ok(Some(derive_known_api_endpoint(endpoint, MODELS_SUFFIX)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_bases_share_chat_suffixes_and_preserve_deployment_prefixes() {
        for (input, expected) in [
            ("https://host", "https://host/"),
            ("https://host/", "https://host/"),
            ("https://host/proxy", "https://host/proxy/"),
            ("https://host/proxy/", "https://host/proxy/"),
            ("https://host/proxy/v1", "https://host/proxy/"),
            (
                "https://host/proxy/v1/chat/completions",
                "https://host/proxy/",
            ),
            (
                "https://host/proxy/v1/chatvcp/completions/",
                "https://host/proxy/",
            ),
            (
                "https://host/proxy%20space/v1/chatvcp/completions?tenant=mobile",
                "https://host/proxy%20space/",
            ),
            (
                "http://[::1]:6005/mount/v1/chat/completions",
                "http://[::1]:6005/mount/",
            ),
            (
                "https://host/proxy/v1/chat/completions-extra",
                "https://host/proxy/v1/chat/completions-extra/",
            ),
            (
                "https://host/proxy/v1/chatvcp/completions/child",
                "https://host/proxy/v1/chatvcp/completions/child/",
            ),
            ("https://host/proxy/not-v1", "https://host/proxy/not-v1/"),
        ] {
            for mode in [ChatEndpointMode::Standard, ChatEndpointMode::VcpTools] {
                assert_eq!(
                    resolve_service_base(input, mode).unwrap().as_str(),
                    expected
                );
            }
        }
    }

    #[test]
    fn raw_service_base_is_explicit_and_discovery_keeps_its_narrower_contract() {
        for (input, expected) in [
            ("https://host", "https://host/"),
            ("https://host/v1", "https://host/"),
            ("https://host/proxy/v1/", "https://host/proxy/"),
            (
                "https://host/proxy/v1/chat/completions",
                "https://host/proxy/",
            ),
            (
                "https://host/proxy/v1/chatvcp/completions/?token=chat",
                "https://host/proxy/",
            ),
        ] {
            assert_eq!(
                resolve_service_base(input, ChatEndpointMode::Raw)
                    .unwrap()
                    .as_str(),
                expected
            );
        }
        for input in [
            "https://host/proxy",
            "https://host/custom/gateway",
            "https://host/v1/chatvcp/completions-extra",
            "https://host/v1/chat/completions/child",
        ] {
            assert_eq!(
                resolve_service_base(input, ChatEndpointMode::Raw),
                Err(HttpEndpointError::UnavailableRaw)
            );
            assert_eq!(
                resolve_chat_endpoint(input, ChatEndpointMode::Raw, ChatRequestPurpose::Auxiliary)
                    .unwrap(),
                input
            );
        }
        assert_eq!(
            resolve_model_discovery_endpoint("https://host/v1", ChatEndpointMode::Raw).unwrap(),
            None
        );
    }

    #[test]
    fn chat_queries_and_auxiliary_routes_do_not_leak_into_companion_requests() {
        let input = "https://host/proxy/v1/chatvcp/completions?tenant=mobile&token=chat-only";
        assert_eq!(
            resolve_chat_endpoint(
                input,
                ChatEndpointMode::VcpTools,
                ChatRequestPurpose::Auxiliary
            )
            .unwrap(),
            "https://host/proxy/v1/chat/completions?tenant=mobile&token=chat-only"
        );
        let mut url = resolve_service_base(input, ChatEndpointMode::VcpTools).unwrap();
        append_url_segments(&mut url, &["admin_api", "server-log"]).unwrap();
        let request = reqwest::Client::new()
            .get(url)
            .query(&[("lines", "25")])
            .build()
            .unwrap();
        assert_eq!(
            request.url().as_str(),
            "https://host/proxy/admin_api/server-log?lines=25"
        );
        assert!(request
            .headers()
            .get(reqwest::header::AUTHORIZATION)
            .is_none());
    }

    #[test]
    fn origin_selection_is_explicit_and_preserves_ipv6_and_port() {
        let input = "https://[2001:db8::1]:8443/proxy/v1/chatvcp/completions?token=chat";
        assert_eq!(
            resolve_http_origin(input).unwrap().as_str(),
            "https://[2001:db8::1]:8443/"
        );
        assert_eq!(
            resolve_service_base(input, ChatEndpointMode::VcpTools)
                .unwrap()
                .as_str(),
            "https://[2001:db8::1]:8443/proxy/"
        );
        assert_eq!(
            resolve_http_origin("https://host/custom/gateway")
                .unwrap()
                .as_str(),
            "https://host/"
        );
    }

    #[test]
    fn raw_path_segments_are_encoded_once_without_rewriting_the_prefix() {
        let mut url =
            resolve_service_base("https://host/proxy%20space", ChatEndpointMode::Standard).unwrap();
        append_url_segments(&mut url, &["admin_api", "dir/name", "雪 %2F"]).unwrap();
        assert_eq!(
            url.path(),
            "/proxy%20space/admin_api/dir%2Fname/%E9%9B%AA%20%252F"
        );
        let before = url.clone();
        for segment in [".", ".."] {
            assert!(append_url_segments(&mut url, &["ignored", segment]).is_err());
            assert_eq!(
                url, before,
                "a rejected segment must not partially append the path"
            );
        }
    }

    #[test]
    fn companion_urls_reject_invalid_inputs_without_echoing_secrets() {
        assert_eq!(
            resolve_service_base("", ChatEndpointMode::Standard),
            Err(HttpEndpointError::Missing)
        );
        for input in [
            "https://user:secret@host/v1",
            "https://host/proxy#secret",
            "https://host/proxy\n",
            "ftp://host/proxy",
            "https://host:65536/proxy",
        ] {
            let error = resolve_service_base(input, ChatEndpointMode::Standard).unwrap_err();
            assert!(!error.to_string().contains("secret"));
            assert!(resolve_http_origin(input).is_err());
        }
    }

    #[test]
    fn endpoint_modes_cover_base_full_prefix_port_and_query_urls() {
        let cases = [
            (
                "https://example.invalid",
                "https://example.invalid/v1/chat/completions",
                "https://example.invalid/v1/chatvcp/completions",
            ),
            (
                "https://example.invalid/v1/chat/completions",
                "https://example.invalid/v1/chat/completions",
                "https://example.invalid/v1/chatvcp/completions",
            ),
            (
                "http://example.invalid:6005/proxy?tenant=mobile",
                "http://example.invalid:6005/proxy/v1/chat/completions?tenant=mobile",
                "http://example.invalid:6005/proxy/v1/chatvcp/completions?tenant=mobile",
            ),
            (
                "https://example.invalid:443/proxy",
                "https://example.invalid:443/proxy/v1/chat/completions",
                "https://example.invalid:443/proxy/v1/chatvcp/completions",
            ),
            (
                "https://example.invalid/proxy%20space?tenant=mobile",
                "https://example.invalid/proxy%20space/v1/chat/completions?tenant=mobile",
                "https://example.invalid/proxy%20space/v1/chatvcp/completions?tenant=mobile",
            ),
            (
                "https://example.invalid/proxy/v1",
                "https://example.invalid/proxy/v1/chat/completions",
                "https://example.invalid/proxy/v1/chatvcp/completions",
            ),
            (
                "https://example.invalid/proxy/v1/chatvcp/completions/?key=value",
                "https://example.invalid/proxy/v1/chat/completions?key=value",
                "https://example.invalid/proxy/v1/chatvcp/completions?key=value",
            ),
        ];

        for (raw, standard, vcp_tools) in cases {
            assert_eq!(
                resolve_chat_endpoint(
                    raw,
                    ChatEndpointMode::Standard,
                    ChatRequestPurpose::Interactive,
                )
                .expect("standard endpoint"),
                standard
            );
            assert_eq!(
                resolve_chat_endpoint(
                    raw,
                    ChatEndpointMode::VcpTools,
                    ChatRequestPurpose::Interactive,
                )
                .expect("VCP tools endpoint"),
                vcp_tools
            );
            assert_eq!(
                resolve_chat_endpoint(raw, ChatEndpointMode::Raw, ChatRequestPurpose::Interactive,)
                    .expect("raw endpoint"),
                raw
            );
        }
    }

    #[test]
    fn model_discovery_is_prefix_aware_and_conservative_in_raw_mode() {
        assert_eq!(
            resolve_model_discovery_endpoint(
                "https://example.invalid/proxy?tenant=mobile",
                ChatEndpointMode::VcpTools,
            )
            .expect("derived discovery"),
            Some("https://example.invalid/proxy/v1/models?tenant=mobile".to_string())
        );
        assert_eq!(
            resolve_model_discovery_endpoint(
                "https://example.invalid/proxy/v1/chat/completions?tenant=mobile",
                ChatEndpointMode::Raw,
            )
            .expect("safe raw discovery"),
            Some("https://example.invalid/proxy/v1/models?tenant=mobile".to_string())
        );
        assert_eq!(
            resolve_model_discovery_endpoint(
                "https://example.invalid/custom/gateway",
                ChatEndpointMode::Raw,
            )
            .expect("unsafe raw discovery"),
            None
        );
    }

    #[test]
    fn endpoint_validation_rejects_unsafe_or_non_http_urls() {
        for raw in [
            "ftp://example.invalid/v1/chat/completions",
            "https://user:secret@example.invalid/v1/chat/completions",
            "https://example.invalid/v1/chat/completions#fragment",
            "https://example.invalid/v1/chat/completions\n",
            "https://",
        ] {
            assert!(
                resolve_chat_endpoint(raw, ChatEndpointMode::Raw, ChatRequestPurpose::Interactive,)
                    .is_err(),
                "unexpectedly accepted {raw}"
            );
        }
    }
}
