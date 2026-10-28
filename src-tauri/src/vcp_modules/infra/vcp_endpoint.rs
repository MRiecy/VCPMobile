//! Shared, offline compilation of the VCP path-authenticated WebSocket channels.
//! A successful result is the address used by the transport, not a connectivity check.

use super::settings_manager::Settings;
use std::cell::Cell;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EndpointField {
    Url,
    Key,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EndpointIssue {
    pub field: EndpointField,
    pub code: &'static str,
    pub message: &'static str,
}

impl std::fmt::Display for EndpointIssue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message)
    }
}

fn url_issue(code: &'static str, message: &'static str) -> EndpointIssue {
    EndpointIssue {
        field: EndpointField::Url,
        code,
        message,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BaseKind {
    Log,
    Distributed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LogEndpoints {
    pub log: Url,
    pub info: Url,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DistributedEndpoint {
    pub base: Url,
    pub url: Url,
}

pub(crate) struct ResolvedDistributedInput<'a> {
    pub url: &'a str,
    pub key: &'a str,
    pub url_from_log: bool,
    pub key_from_log: bool,
}

impl ResolvedDistributedInput<'_> {
    pub fn base_kind(&self) -> BaseKind {
        if self.url_from_log {
            BaseKind::Log
        } else {
            BaseKind::Distributed
        }
    }
}

/// Preserve the existing per-field priority, including its empty-string semantics.
pub(crate) fn resolve_distributed_connection(settings: &Settings) -> ResolvedDistributedInput<'_> {
    let url_from_log = !settings.log.url.is_empty();
    let key_from_log = !settings.log.key.is_empty();
    ResolvedDistributedInput {
        url: if url_from_log {
            &settings.log.url
        } else {
            &settings.distributed.ws_url
        },
        key: if key_from_log {
            &settings.log.key
        } else {
            &settings.distributed.vcp_key
        },
        url_from_log,
        key_from_log,
    }
}

fn parse_base(raw: &str, kind: BaseKind) -> Result<Url, EndpointIssue> {
    if raw
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(url_issue(
            "wsUrlWhitespace",
            "WebSocket 地址不能包含空白或控制字符。",
        ));
    }
    if raw.contains('\\') {
        return Err(url_issue(
            "wsUrlBackslash",
            "WebSocket 地址不能包含反斜杠。",
        ));
    }
    let Some((scheme, remainder)) = raw.split_once("://") else {
        return Err(url_issue(
            "wsUrlScheme",
            "WebSocket 地址必须明确使用 ws:// 或 wss://。",
        ));
    };
    if !scheme.eq_ignore_ascii_case("ws") && !scheme.eq_ignore_ascii_case("wss") {
        return Err(url_issue(
            "wsUrlScheme",
            "WebSocket 地址必须明确使用 ws:// 或 wss://。",
        ));
    }
    let authority = remainder.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() {
        return Err(url_issue(
            "wsUrlHost",
            "WebSocket 地址必须包含主机名或 IP 地址。",
        ));
    }
    if authority.contains('@') {
        return Err(url_issue(
            "wsUrlCredentials",
            "WebSocket 地址不能包含用户名或密码，请使用独立的 Key 字段。",
        ));
    }
    // URL parsing otherwise silently collapses literal and percent-encoded dot segments.
    let raw_path = remainder
        .get(authority.len()..)
        .unwrap_or_default()
        .split(['?', '#'])
        .next()
        .unwrap_or_default();
    if raw_path.split('/').any(|segment| {
        let dots = segment.to_ascii_lowercase().replace("%2e", ".");
        dots == "." || dots == ".."
    }) {
        return Err(url_issue(
            "wsUrlPath",
            "WebSocket 地址不能包含会被折叠的 . 或 .. 路径段。",
        ));
    }
    let violation = Cell::new(false);
    let on_violation = |_| violation.set(true);
    let mut url = Url::options()
        .syntax_violation_callback(Some(&on_violation))
        .parse(raw)
        .map_err(|error| match error {
            url::ParseError::InvalidPort => {
                url_issue("wsUrlPort", "WebSocket 端口格式或范围无效。")
            }
            url::ParseError::EmptyHost => {
                url_issue("wsUrlHost", "WebSocket 地址必须包含主机名或 IP 地址。")
            }
            _ => url_issue(
                "wsUrlInvalid",
                "WebSocket 地址格式无效，请检查主机、IP 地址和端口。",
            ),
        })?;
    if violation.get() {
        return Err(url_issue(
            "wsUrlInvalid",
            "WebSocket 地址包含需要自动修正的语法，请检查地址。",
        ));
    }
    if !url.has_host() || url.cannot_be_a_base() {
        return Err(url_issue(
            "wsUrlHost",
            "WebSocket 地址必须包含主机名或 IP 地址。",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(url_issue(
            "wsUrlCredentials",
            "WebSocket 地址不能包含用户名或密码，请使用独立的 Key 字段。",
        ));
    }
    if url.fragment().is_some() {
        return Err(url_issue(
            "wsUrlFragment",
            "WebSocket 地址不能包含 #fragment。",
        ));
    }
    if url.query().is_some() {
        return Err(url_issue(
            "wsUrlQuery",
            "WebSocket 服务器地址不支持查询参数，请填写基础地址。",
        ));
    }
    if url.path().contains("VCP_Key=") {
        return Err(url_issue(
            "wsUrlEmbeddedKey",
            "WebSocket 地址不能内嵌 VCP_Key，请使用独立的 Key 字段。",
        ));
    }
    match kind {
        BaseKind::Log => {
            if !matches!(url.path(), "/" | "/VCPlog" | "/VCPlog/") {
                return Err(url_issue(
                    "wsUrlPath",
                    "VCP WebSocket 地址只支持服务器根地址或 /VCPlog 路径。",
                ));
            }
            // The supported Log endpoint spelling identifies the same server for all channels.
            url.set_path("/");
        }
        BaseKind::Distributed => {
            let path = url.path().trim_end_matches('/').to_owned();
            url.set_path(&path);
        }
    }
    Ok(url)
}

fn validate_key(key: &str) -> Result<(), EndpointIssue> {
    // Deliberately simple input policy: no Node legacy-parser branches or implicit encoding.
    // Base64 punctuation (+ / =) and literal percent sequences remain usable.
    if key.bytes().any(|byte| {
        !byte.is_ascii_graphic()
            || matches!(
                byte,
                b'#' | b'?' | b'\\' | b'"' | b'\'' | b'`' | b'<' | b'>' | b'{' | b'}' | b'^' | b'|'
            )
    }) {
        return Err(EndpointIssue {
            field: EndpointField::Key,
            code: "wsKeyCharacters",
            message: "WebSocket Key 仅支持无空白的可打印 ASCII 字符，且不能包含 # ? \\ \" ' ` < > { } ^ |。",
        });
    }
    Ok(())
}

fn channel_url(base: &Url, channel: &str, key: &str) -> Result<Url, EndpointIssue> {
    let mut url = base.clone();
    let expected_path = format!(
        "{}/{channel}/VCP_Key={key}",
        base.path().trim_end_matches('/')
    );
    url.set_path(&expected_path);
    if url.path() != expected_path {
        return Err(EndpointIssue {
            field: EndpointField::Key,
            code: "wsKeyPath",
            message: "WebSocket Key 包含会被编码或折叠的路径内容，无法原样传递。",
        });
    }
    validate_request(&url)?;
    Ok(url)
}

fn validate_request(url: &Url) -> Result<(), EndpointIssue> {
    url.as_str()
        .into_client_request()
        .map(|_| ())
        .map_err(|_| url_issue("wsUrlRequest", "WebSocket 地址无法构造有效的连接请求。"))
}

pub(crate) fn compile_log_connection(
    raw: &str,
    key: &str,
) -> Result<Option<LogEndpoints>, EndpointIssue> {
    if raw.trim().is_empty() || key.trim().is_empty() {
        return Ok(None);
    }
    let base = parse_base(raw, BaseKind::Log)?;
    validate_key(key)?;
    let log = channel_url(&base, "VCPlog", key)?;
    let info = channel_url(&base, "vcpinfo", key)?;
    for attempt in log_connection_attempts(&log) {
        validate_request(&attempt)?;
    }
    Ok(Some(LogEndpoints { log, info }))
}

/// Keep the existing device-name attempt followed by the plain endpoint fallback.
pub(crate) fn log_connection_attempts(log: &Url) -> [Url; 2] {
    let mut identified = log.clone();
    identified
        .query_pairs_mut()
        .append_pair("deviceName", "VCPChat-Mobile");
    [identified, log.clone()]
}

pub(crate) fn compile_distributed_connection(
    raw: &str,
    key: &str,
    kind: BaseKind,
) -> Result<Option<DistributedEndpoint>, EndpointIssue> {
    if raw.trim().is_empty() || key.trim().is_empty() {
        return Ok(None);
    }
    let base = parse_base(raw, kind)?;
    validate_key(key)?;
    let url = channel_url(&base, "vcp-distributed-server", key)?;
    Ok(Some(DistributedEndpoint { base, url }))
}

/// Never include the path-authentication Key in connection diagnostics.
pub(crate) fn endpoint_log_label(url: &Url) -> String {
    url.origin().ascii_serialization()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_addresses_without_echoing_credentials() {
        for address in [
            "example.invalid",
            "http://example.invalid",
            "ws:example.invalid",
            "ws:///example.invalid",
            "ws://",
            "ws://host:65536",
            "ws://host:no",
            "ws://host\n",
            " ws://host",
            "ws://ho\tst",
            "ws://user:secret@host",
            "ws://@host",
            "ws://host/#",
            "ws://host/#fragment",
            "ws://host/?",
            "ws://host/?deviceName=phone",
            "ws://host/prefix",
            "ws://host/VCPlog-other",
            "ws://host/VCPlog/VCP_Key=secret",
            "ws://host/../VCPlog",
            "ws://host/%2e/VCPlog",
            "ws:\\host",
        ] {
            let error = compile_log_connection(address, "secret").unwrap_err();
            assert_eq!(error.field, EndpointField::Url, "{address}");
            assert!(!error.message.contains("secret"));
        }
    }

    #[test]
    fn supports_explicit_schemes_hosts_ports_and_log_endpoint_spelling() {
        for address in [
            "ws://example.invalid",
            "WS://Example.invalid:80/",
            "wss://localhost:23456",
            "ws://192.168.1.1:5800",
            "wss://[2001:db8::1]:8443",
            "ws://host/VCPlog",
            "ws://host/VCPlog/",
        ] {
            let endpoints = compile_log_connection(address, "a+b/c=d&x%23")
                .unwrap()
                .unwrap();
            assert_eq!(endpoints.log.path(), "/VCPlog/VCP_Key=a+b/c=d&x%23");
            assert_eq!(endpoints.info.path(), "/vcpinfo/VCP_Key=a+b/c=d&x%23");
            assert!(endpoints.log.query().is_none());
            let attempts = log_connection_attempts(&endpoints.log);
            assert_eq!(attempts[0].query(), Some("deviceName=VCPChat-Mobile"));
            assert_eq!(attempts[1], endpoints.log);
            assert!(!endpoint_log_label(&endpoints.log).contains("a+b"));
        }
    }

    #[test]
    fn keys_use_a_simple_policy_and_never_change_on_the_wire() {
        for key in [
            "a#b",
            "a?b",
            "a\\b",
            "a b",
            "a\nb",
            "中文",
            "a'b",
            "a\"b",
            "a`b",
            "a<b",
            "a>b",
            "a{b",
            "a}b",
            "a^b",
            "a|b",
            "a/../b",
            "a/%2e%2e/b",
        ] {
            let error = compile_log_connection("ws://host", key).unwrap_err();
            assert_eq!(error.field, EndpointField::Key, "{key}");
        }
        for key in [
            "abc_123-456",
            "a+b/c==",
            "a%23b",
            "a%b",
            "a@b",
            "a&b",
            "a:b;c!d$e(f)g*h",
        ] {
            let endpoints = compile_log_connection("ws://host", key).unwrap().unwrap();
            let request = endpoints.log.as_str().into_client_request().unwrap();
            assert_eq!(
                request.uri().path().strip_prefix("/VCPlog/VCP_Key="),
                Some(key)
            );
        }
    }

    #[test]
    fn unconfigured_connections_do_not_validate_the_other_field() {
        assert_eq!(compile_log_connection("invalid", "  ").unwrap(), None);
        assert_eq!(compile_log_connection("", "#").unwrap(), None);
        assert_eq!(
            compile_distributed_connection("invalid", "", BaseKind::Distributed).unwrap(),
            None
        );
    }

    #[test]
    fn distributed_prefixes_and_log_origins_are_explicit() {
        let own = compile_distributed_connection("ws://host/proxy/", "key", BaseKind::Distributed)
            .unwrap()
            .unwrap();
        assert_eq!(own.url.path(), "/proxy/vcp-distributed-server/VCP_Key=key");
        let inherited = compile_distributed_connection("ws://host/VCPlog/", "key", BaseKind::Log)
            .unwrap()
            .unwrap();
        assert_eq!(inherited.url.path(), "/vcp-distributed-server/VCP_Key=key");
        for address in [
            "ws://host/proxy?x=1",
            "ws://host/a/../b",
            "ws://host/VCP_Key=old",
        ] {
            assert!(compile_distributed_connection(address, "key", BaseKind::Distributed).is_err());
        }
    }

    #[test]
    fn distributed_resolution_preserves_individual_field_sources() {
        let mut settings = Settings::default();
        settings.log.url = "ws://log-host".into();
        settings.distributed.ws_url = "ws://node-host/proxy".into();
        settings.distributed.vcp_key = "node-key".into();
        let input = resolve_distributed_connection(&settings);
        assert_eq!(input.url, "ws://log-host");
        assert_eq!(input.key, "node-key");
        assert!(input.url_from_log);
        assert!(!input.key_from_log);
        settings.log.url.clear();
        settings.log.key = "log-key".into();
        let input = resolve_distributed_connection(&settings);
        assert_eq!(input.url, "ws://node-host/proxy");
        assert_eq!(input.key, "log-key");
        assert_eq!(input.base_kind(), BaseKind::Distributed);
        assert!(input.key_from_log);
    }
}
