//! Shared field policy for persistence, edits, recovery, backup and TS export.
//! The field inventory is derived from typed defaults, not a parallel key list.

use super::types::*;
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

static DEFAULT_DOMAINS: LazyLock<Value> = LazyLock::new(|| serde_json::json!(Settings::default()));
static FIELDS: LazyLock<BTreeMap<ConfigDomain, Vec<FieldDescriptor>>> = LazyLock::new(|| {
    ConfigDomain::ALL
        .into_iter()
        .map(|domain| {
            let mut fields = Vec::new();
            collect_fields(domain, "", &DEFAULT_DOMAINS[domain.as_str()], &mut fields);
            (domain, fields)
        })
        .collect()
});

pub fn domain_defaults(domain: ConfigDomain) -> Value {
    DEFAULT_DOMAINS[domain.as_str()].clone()
}

pub fn domain_fields(domain: ConfigDomain) -> Vec<FieldDescriptor> {
    FIELDS.get(&domain).cloned().unwrap_or_default()
}

fn serialized_options<T: Serialize>(values: &[T]) -> Vec<String> {
    values
        .iter()
        .filter_map(|value| {
            serde_json::to_value(value)
                .ok()?
                .as_str()
                .map(str::to_owned)
        })
        .collect()
}

fn collect_fields(
    domain: ConfigDomain,
    prefix: &str,
    value: &Value,
    fields: &mut Vec<FieldDescriptor>,
) {
    if let Some(object) = value.as_object() {
        for (key, value) in object {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            collect_fields(domain, &path, value, fields);
        }
        return;
    }
    let kind = match value {
        Value::Bool(_) => FieldKind::Boolean,
        Value::Number(_) => FieldKind::Integer,
        Value::Array(_) => FieldKind::StringArray,
        _ => FieldKind::String,
    };
    let mut field = FieldDescriptor {
        domain,
        path: prefix.into(),
        kind,
        backup: !matches!(domain, ConfigDomain::Navigation)
            && !(domain == ConfigDomain::Sync && matches!(prefix, "logLevel" | "prerenderEnabled")),
        secret: matches!(
            (domain, prefix),
            (ConfigDomain::Server, "apiKey" | "adminPassword" | "fileKey")
                | (ConfigDomain::Log, "key")
                | (ConfigDomain::Sync, "token")
                | (ConfigDomain::Distributed, "vcpKey")
                | (ConfigDomain::Voice, "connection.apiKey")
                | (ConfigDomain::Jev, "typesafe.apiKey" | "openrouter.apiKey" | "proxyUrl")
        ),
        validation_group: prefix.into(),
        effect: match domain {
            ConfigDomain::Log | ConfigDomain::Distributed => ConfigEffect::Reconnect,
            ConfigDomain::Sync => ConfigEffect::NextSync,
            ConfigDomain::Identity
            | ConfigDomain::Server
            | ConfigDomain::Conversation
            | ConfigDomain::Jev => ConfigEffect::NextRequest,
            ConfigDomain::Navigation => ConfigEffect::Immediate,
            ConfigDomain::Voice if prefix.starts_with("vad.") => ConfigEffect::NextCall,
            ConfigDomain::Voice if prefix.starts_with("connection.") => ConfigEffect::NextVoiceTask,
            ConfigDomain::Voice if prefix.starts_with("asr.") => ConfigEffect::NextInput,
            ConfigDomain::Voice => ConfigEffect::NextNarration,
        },
        options: Vec::new(),
        required: false,
        min: None,
        max: None,
        max_chars: None,
    };
    match (domain, prefix) {
        (ConfigDomain::Jev, "provider") => field.options = serialized_options(&JevProvider::ALL),
        (ConfigDomain::Jev, "timeoutMs") => {
            field.min = Some(1_000);
            field.max = Some(300_000);
        }
        (ConfigDomain::Jev, "maxRetries") => {
            field.min = Some(0);
            field.max = Some(10);
        }
        (ConfigDomain::Jev, "retryBaseDelayMs") => {
            field.min = Some(1);
            field.max = Some(30_000);
        }
        (
            ConfigDomain::Jev,
            "typesafe.apiUrl" | "openrouter.apiUrl" | "typesafe.model" | "openrouter.model",
        ) => {
            field.required = true;
            field.max_chars = Some(2_048);
        }
        (ConfigDomain::Jev, "typesafe.apiKey" | "openrouter.apiKey") => {
            field.max_chars = Some(32_768)
        }
        (ConfigDomain::Jev, "proxyUrl" | "httpReferer" | "appTitle") => {
            field.max_chars = Some(2_048)
        }
        (ConfigDomain::Server, "endpointMode") => {
            field.options = serialized_options(&ChatEndpointMode::ALL)
        }
        (ConfigDomain::Sync, "logLevel") => {
            field.options = SYNC_LOG_LEVELS.iter().map(|v| (*v).into()).collect()
        }
        (ConfigDomain::Voice, "asr.language") => {
            field.options = VOICE_LANGUAGES.iter().map(|v| (*v).into()).collect()
        }
        (ConfigDomain::Voice, "asr.inputEncoding") => {
            field.options = serialized_options(&VoiceInputEncoding::ALL)
        }
        (ConfigDomain::Voice, "tts.voice") => {
            field.options = VOICE_TTS_VOICES.iter().map(|v| (*v).into()).collect()
        }
        (ConfigDomain::Voice, "tts.mode") => field.options = serialized_options(&VoiceTtsMode::ALL),
        (ConfigDomain::Voice, "vad.sensitivity") => {
            field.options = serialized_options(&VoiceVadSensitivity::ALL)
        }
        (ConfigDomain::Voice, "vad.endSilenceMs") => {
            field.min = Some(VoiceVadConfig::MIN_DURATION_MS);
            field.max = Some(VoiceVadConfig::MAX_END_SILENCE_MS);
        }
        (ConfigDomain::Voice, "vad.playbackOnsetMs") => {
            field.min = Some(VoiceVadConfig::MIN_DURATION_MS);
            field.max = Some(VoiceVadConfig::MAX_PLAYBACK_ONSET_MS);
        }
        (ConfigDomain::Voice, "guidance.audioTagRules") => {
            field.max_chars = Some(VOICE_RULE_MAX_CHARS);
            field.required = true;
        }
        (ConfigDomain::Voice, "guidance.guidanceRules") => {
            field.max_chars = Some(VOICE_RULE_MAX_CHARS);
            field.validation_group = "automaticGuidance".into();
        }
        (ConfigDomain::Voice, "guidance.autoGuidance") => {
            field.validation_group = "automaticGuidance".into()
        }
        (ConfigDomain::Voice, "tts.stylePrompt") => {
            field.max_chars = Some(VOICE_GUIDANCE_MAX_CHARS)
        }
        _ => {}
    }
    match (domain, prefix) {
        (ConfigDomain::Jev, "typesafe.apiUrl" | "typesafe.apiKey" | "typesafe.model") => {
            field.validation_group = "typesafeConnection".into();
        }
        (ConfigDomain::Jev, "openrouter.apiUrl" | "openrouter.apiKey" | "openrouter.model") => {
            field.validation_group = "openrouterConnection".into();
        }
        (ConfigDomain::Server, "url" | "apiKey")
        | (ConfigDomain::Log, "url" | "key")
        | (ConfigDomain::Sync, "serverUrl" | "httpUrl" | "token")
        | (ConfigDomain::Distributed, "enabled" | "wsUrl" | "vcpKey" | "deviceName")
        | (ConfigDomain::Voice, "connection.apiBaseUrl" | "connection.apiKey") => {
            field.validation_group = "connection".into();
        }
        (ConfigDomain::Server, "adminUsername" | "adminPassword") => {
            field.validation_group = "adminCredentials".into();
        }
        _ => {}
    }
    fields.push(field);
}

pub fn affected_fields(domain: ConfigDomain, changed: &[&str]) -> Vec<String> {
    let fields = domain_fields(domain);
    let groups: BTreeSet<_> = fields
        .iter()
        .filter(|field| changed.contains(&field.path.as_str()))
        .map(|field| field.validation_group.as_str())
        .collect();
    fields
        .iter()
        .filter(|field| groups.contains(field.validation_group.as_str()))
        .map(|field| field.path.clone())
        .collect()
}

pub fn value_at_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() {
        return Some(value);
    }
    path.split('.')
        .try_fold(value, |current, key| current.as_object()?.get(key))
}

pub fn set_value_at_path(value: &mut Value, path: &str, replacement: Value) -> Result<(), String> {
    if path.is_empty() || path.split('.').any(str::is_empty) {
        return Err("配置字段路径无效".into());
    }
    let mut segments = path.split('.').peekable();
    let mut current = value;
    while let Some(segment) = segments.next() {
        if !current.is_object() {
            *current = Value::Object(Map::new());
        }
        let object = current
            .as_object_mut()
            .ok_or_else(|| "配置字段必须属于对象".to_string())?;
        if segments.peek().is_none() {
            object.insert(segment.into(), replacement);
            return Ok(());
        }
        current = object
            .entry(segment)
            .or_insert_with(|| Value::Object(Map::new()));
    }
    Err("配置字段路径无效".into())
}

fn issue(domain: ConfigDomain, field: &str, code: &str, message: impl Into<String>) -> FieldIssue {
    FieldIssue {
        domain,
        field: field.into(),
        code: code.into(),
        message: message.into(),
    }
}

fn validate_descriptor(field: &FieldDescriptor, value: &Value) -> Vec<FieldIssue> {
    let valid_type = match field.kind {
        FieldKind::String => value.is_string(),
        FieldKind::Boolean => value.is_boolean(),
        FieldKind::Integer => value
            .as_u64()
            .is_some_and(|value| value <= u64::from(u32::MAX)),
        FieldKind::StringArray => value
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string)),
    };
    let error = |code, message| vec![issue(field.domain, &field.path, code, message)];
    if !valid_type {
        return error("invalidType", "配置值的类型不正确".to_string());
    }
    if let Some(text) = value.as_str() {
        if !field.options.is_empty() && !field.options.iter().any(|choice| choice == text) {
            return error("invalidOption", "请选择受支持的配置选项".to_string());
        }
        if field.required && text.trim().is_empty() {
            return error("required", "请填写此项配置".to_string());
        }
        if let Some(limit) = field.max_chars {
            if text.chars().count() > limit {
                return error("tooLong", format!("此项配置不能超过 {limit} 字"));
            }
        }
    }
    if let Some(number) = value.as_u64() {
        if field.min.is_some_and(|min| number < u64::from(min))
            || field.max.is_some_and(|max| number > u64::from(max))
        {
            return error(
                "outOfRange",
                format!(
                    "请输入 {}–{} 范围内的整数",
                    field.min.unwrap_or(0),
                    field.max.unwrap_or(u32::MAX)
                ),
            );
        }
    }
    Vec::new()
}

/// Validates a supplied leaf without inventing values for absent peers. Backup
/// import uses this; cross-field rules run against the actual merged commit.
pub fn validate_field(domain: ConfigDomain, path: &str, value: &Value) -> Vec<FieldIssue> {
    match FIELDS
        .get(&domain)
        .and_then(|fields| fields.iter().find(|field| field.path == path))
    {
        Some(field) => validate_descriptor(field, value),
        None => vec![issue(domain, path, "unknownField", "此配置字段不存在")],
    }
}

fn source_leaf<'a>(raw: &'a Value, path: &str) -> Result<Option<&'a Value>, ()> {
    let mut current = raw;
    for key in path.split('.') {
        let object = current.as_object().ok_or(())?;
        let Some(next) = object.get(key) else {
            return Ok(None);
        };
        current = next;
    }
    Ok(Some(current))
}

/// Returns an in-memory usable view and local diagnostics. The repository keeps
/// the original raw JSON; this function never serializes repairs back to disk.
pub fn project_domain(domain: ConfigDomain, raw: &Value) -> (Value, Vec<FieldIssue>) {
    let defaults = domain_defaults(domain);
    let mut effective = defaults.clone();
    if !raw.is_object() {
        return (
            effective,
            vec![issue(domain, "", "invalidDomain", "此领域配置不是有效对象")],
        );
    }
    let mut issues = Vec::new();
    for field in domain_fields(domain) {
        match source_leaf(raw, &field.path) {
            Ok(Some(value)) => {
                let errors = validate_descriptor(&field, value);
                if errors.is_empty() {
                    // The descriptor came from this exact typed default tree.
                    if set_value_at_path(&mut effective, &field.path, value.clone()).is_err() {
                        issues.push(issue(
                            domain,
                            &field.path,
                            "invalidPath",
                            "配置字段路径无效",
                        ));
                    }
                } else {
                    issues.extend(errors);
                }
            }
            Ok(None) => {} // Missing newly introduced fields use the same Default.
            Err(()) => issues.push(issue(
                domain,
                &field.path,
                "invalidGroup",
                "所属配置分组的类型不正确",
            )),
        }
    }
    // An endpoint and its credentials form one recovery unit. In particular,
    // never send a saved third-party key to the default MiMo endpoint after a
    // malformed URL was replaced. Administrative auth stays separate from chat.
    let fields = domain_fields(domain);
    let broken_connections: BTreeSet<_> = fields
        .iter()
        .filter(|field| {
            matches!(
                field.validation_group.as_str(),
                "connection" | "adminCredentials" | "typesafeConnection" | "openrouterConnection"
            ) && issues.iter().any(|issue| issue.field == field.path)
        })
        .map(|field| field.validation_group.as_str())
        .collect();
    for field in fields
        .iter()
        .filter(|field| broken_connections.contains(field.validation_group.as_str()))
    {
        if let Some(default) = value_at_path(&defaults, &field.path) {
            let _ = set_value_at_path(&mut effective, &field.path, default.clone());
        }
        if !issues.iter().any(|issue| issue.field == field.path) {
            issues.push(issue(
                domain,
                &field.path,
                "dependentRecovery",
                "关联连接配置有误，此项暂不应用；原始值仍已保留",
            ));
        }
    }
    if domain == ConfigDomain::Voice
        && value_at_path(&effective, "guidance.autoGuidance").and_then(Value::as_bool) == Some(true)
        && value_at_path(&effective, "guidance.guidanceRules")
            .and_then(Value::as_str)
            .is_some_and(|text| text.trim().is_empty())
    {
        issues.push(issue(
            domain,
            "guidance.guidanceRules",
            "required",
            "开启自动风格指导时，请填写生成规则",
        ));
        if let Some(default) = value_at_path(&defaults, "guidance.guidanceRules") {
            let _ = set_value_at_path(&mut effective, "guidance.guidanceRules", default.clone());
        }
    }
    (effective, issues)
}

pub fn validate_domain(domain: ConfigDomain, value: &Value) -> Vec<FieldIssue> {
    project_domain(domain, value).1
}

pub fn settings_to_domains(settings: &Settings) -> Result<BTreeMap<ConfigDomain, Value>, String> {
    let serialized = serde_json::to_value(settings).map_err(|_| "无法序列化配置".to_string())?;
    ConfigDomain::ALL
        .into_iter()
        .map(|domain| {
            serialized
                .get(domain.as_str())
                .cloned()
                .map(|value| (domain, value))
                .ok_or_else(|| "配置领域定义不完整".to_string())
        })
        .collect()
}

pub fn settings_from_domains(domains: &BTreeMap<ConfigDomain, Value>) -> Result<Settings, String> {
    let object: Map<_, _> = ConfigDomain::ALL
        .into_iter()
        .map(|domain| {
            (
                domain.as_str().into(),
                domains
                    .get(&domain)
                    .cloned()
                    .unwrap_or_else(|| domain_defaults(domain)),
            )
        })
        .collect();
    serde_json::from_value(Value::Object(object))
        .map_err(|_| "配置领域快照的类型不正确".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_defaults_and_missing_fields_share_one_source() {
        let defaults = Settings::default();
        let missing: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(defaults, missing);
        let domains = settings_to_domains(&defaults).unwrap();
        assert_eq!(settings_from_domains(&domains).unwrap(), defaults);
        for domain in ConfigDomain::ALL {
            assert!(validate_domain(domain, &domain_defaults(domain)).is_empty());
            assert_eq!(
                project_domain(domain, &serde_json::json!({})).0,
                domain_defaults(domain)
            );
        }
    }

    #[test]
    fn jev_provider_recovery_keeps_credentials_bound_to_their_endpoint() {
        let raw = serde_json::json!({
            "provider":"openrouter",
            "typesafe":{"apiKey":"official-secret"},
            "openrouter":{"apiUrl":false,"apiKey":"router-secret"},
            "maxRetries":0,
        });
        let (effective, issues) = project_domain(ConfigDomain::Jev, &raw);
        assert_eq!(effective["typesafe"]["apiKey"], "official-secret");
        assert_eq!(effective["openrouter"], domain_defaults(ConfigDomain::Jev)["openrouter"]);
        assert_eq!(effective["maxRetries"], 0);
        assert_eq!(effective["provider"], "openrouter");
        assert!(!serde_json::to_string(&issues).unwrap().contains("secret"));
        assert!(validate_field(ConfigDomain::Jev, "timeoutMs", &serde_json::json!(999)).iter().any(|issue| issue.code == "outOfRange"));
        assert!(validate_field(ConfigDomain::Jev, "maxRetries", &serde_json::json!(11)).iter().any(|issue| issue.code == "outOfRange"));
    }

    #[test]
    fn bad_fields_do_not_discard_valid_siblings_or_mutate_input() {
        let raw = serde_json::json!({"connection":{"apiKey":"secret"}, "tts":{"mode":"design", "stylePrompt":"轻快"}, "vad":{"endSilenceMs":1}});
        let original = raw.clone();
        let (effective, issues) = project_domain(ConfigDomain::Voice, &raw);
        assert_eq!(effective["connection"]["apiKey"], "secret");
        assert_eq!(effective["tts"]["stylePrompt"], "轻快");
        assert_eq!(effective["tts"]["mode"], "builtin");
        assert_eq!(issues.len(), 2);
        assert_eq!(raw, original);
        assert!(!serde_json::to_string(&issues).unwrap().contains("secret"));
    }

    #[test]
    fn invalid_nested_group_recovers_only_its_leaves() {
        let (effective, issues) = project_domain(
            ConfigDomain::Voice,
            &serde_json::json!({"connection":false,"asr":{"language":"en"}}),
        );
        assert_eq!(effective["asr"]["language"], "en");
        assert_eq!(issues.len(), 2);
        assert!(issues
            .iter()
            .all(|issue| issue.field.starts_with("connection.")));
        assert!(issues.iter().all(|issue| issue.code == "invalidGroup"));
    }

    #[test]
    fn dependent_rule_validation_is_local_and_uses_unicode_lengths() {
        let disabled = serde_json::json!({"guidance":{"autoGuidance":false,"guidanceRules":""}});
        assert!(validate_domain(ConfigDomain::Voice, &disabled).is_empty());
        let enabled = serde_json::json!({"guidance":{"autoGuidance":true,"guidanceRules":""}});
        assert_eq!(
            validate_domain(ConfigDomain::Voice, &enabled)[0].field,
            "guidance.guidanceRules"
        );
        assert_eq!(
            affected_fields(ConfigDomain::Voice, &["guidance.autoGuidance"]),
            vec!["guidance.autoGuidance", "guidance.guidanceRules"]
        );
        assert!(validate_field(
            ConfigDomain::Voice,
            "tts.stylePrompt",
            &Value::String("字".repeat(VOICE_GUIDANCE_MAX_CHARS))
        )
        .is_empty());
        assert_eq!(
            validate_field(
                ConfigDomain::Voice,
                "tts.stylePrompt",
                &Value::String("字".repeat(VOICE_GUIDANCE_MAX_CHARS + 1))
            )[0]
            .code,
            "tooLong"
        );
    }

    #[test]
    fn a_bad_voice_endpoint_never_reuses_the_key_at_the_default_provider() {
        let raw = serde_json::json!({"connection":{"apiBaseUrl":5,"apiKey":"third-party-secret"}, "tts":{"stylePrompt":"轻快"}});
        let (effective, issues) = project_domain(ConfigDomain::Voice, &raw);
        assert_eq!(
            effective["connection"],
            domain_defaults(ConfigDomain::Voice)["connection"]
        );
        assert_eq!(effective["connection"]["apiKey"], "");
        assert_eq!(effective["tts"]["stylePrompt"], "轻快");
        assert_eq!(issues.len(), 2);
        assert_eq!(raw["connection"]["apiKey"], "third-party-secret");
    }

    #[test]
    fn connection_recovery_does_not_turn_off_unrelated_server_capabilities() {
        let raw = serde_json::json!({"url":"https://example.test", "apiKey":"chat-secret", "adminUsername":"admin", "adminPassword":false});
        let (effective, issues) = project_domain(ConfigDomain::Server, &raw);
        assert_eq!(effective["url"], "https://example.test");
        assert_eq!(effective["apiKey"], "chat-secret");
        assert_eq!(effective["adminUsername"], "");
        assert_eq!(effective["adminPassword"], "");
        assert_eq!(issues.len(), 2);
        let (distributed, _) = project_domain(
            ConfigDomain::Distributed,
            &serde_json::json!({"enabled":true,"wsUrl":[],"vcpKey":"secret"}),
        );
        assert_eq!(distributed["enabled"], false);
        assert_eq!(distributed["vcpKey"], "");
    }
}
