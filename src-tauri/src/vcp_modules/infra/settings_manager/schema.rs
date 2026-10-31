//! Typed field normalization for persistence, edits, backup and TS export.
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

/// Decode stored configuration without letting one unrepresentable JSON number
/// discard its valid siblings. Object depth follows the current typed defaults;
/// ordinary leaf decoding stays strict and never coerces a value's type.
pub(crate) fn parse_config_json(raw: &str, defaults: &Value) -> Value {
    let Some(defaults) = defaults.as_object() else {
        return serde_json::from_str(raw).unwrap_or(Value::Null);
    };
    let Ok(fields) = serde_json::from_str::<BTreeMap<String, &serde_json::value::RawValue>>(raw)
    else {
        return Value::Null;
    };
    Value::Object(
        fields
            .into_iter()
            .map(|(key, raw)| {
                let value = match defaults.get(&key) {
                    Some(default) => parse_config_json(raw.get(), default),
                    None => serde_json::from_str(raw.get()).unwrap_or(Value::Null),
                };
                (key, value)
            })
            .collect(),
    )
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
        backup: !(domain == ConfigDomain::Sync
            && matches!(prefix, "logLevel" | "prerenderEnabled")),
        secret: matches!(
            (domain, prefix),
            (ConfigDomain::Server, "apiKey" | "adminPassword" | "fileKey")
                | (ConfigDomain::Log, "key")
                | (ConfigDomain::Sync, "token")
                | (ConfigDomain::Distributed, "vcpKey")
                | (ConfigDomain::Voice, "connection.apiKey")
                | (
                    ConfigDomain::Jev,
                    "typesafe.apiKey" | "openrouter.apiKey" | "proxyUrl"
                )
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
        preset: match (domain, prefix) {
            (ConfigDomain::Voice, "connection.apiBaseUrl") => Some(VOICE_API_BASE_URL_PRESET),
            (ConfigDomain::Jev, "typesafe.apiUrl") => Some(JevProvider::Typesafe.api_url_preset()),
            (ConfigDomain::Jev, "typesafe.model") => Some(JevProvider::Typesafe.model_preset()),
            (ConfigDomain::Jev, "openrouter.apiUrl") => {
                Some(JevProvider::Openrouter.api_url_preset())
            }
            (ConfigDomain::Jev, "openrouter.model") => Some(JevProvider::Openrouter.model_preset()),
            (ConfigDomain::Jev, "appTitle") => Some(JEV_APP_TITLE_PRESET),
            _ => None,
        }
        .map(str::to_owned),
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
        (ConfigDomain::Voice, "guidance.rules") => {
            field.max_chars = Some(VOICE_RULE_MAX_CHARS);
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

fn descriptor_accepts(field: &FieldDescriptor, value: &Value) -> bool {
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
    if !valid_type {
        return false;
    }
    if let Some(text) = value.as_str() {
        if (!field.options.is_empty() && !field.options.iter().any(|choice| choice == text))
            || (field.required && text.trim().is_empty())
            || field
                .max_chars
                .is_some_and(|limit| text.chars().count() > limit)
        {
            return false;
        }
    }
    if let Some(number) = value.as_u64() {
        if field.min.is_some_and(|min| number < u64::from(min))
            || field.max.is_some_and(|max| number > u64::from(max))
        {
            return false;
        }
    }
    true
}

/// Normalize a known leaf. Callers that receive requests validate path identity
/// first; full/partial tree normalization drops unknown keys altogether.
pub fn normalize_field_value(domain: ConfigDomain, path: &str, value: &Value) -> Value {
    let Some(field) = FIELDS
        .get(&domain)
        .and_then(|fields| fields.iter().find(|field| field.path == path))
    else {
        return Value::Null;
    };
    if descriptor_accepts(field, value) {
        value.clone()
    } else {
        value_at_path(&DEFAULT_DOMAINS[domain.as_str()], path)
            .cloned()
            .unwrap_or(Value::Null)
    }
}

fn normalize_tree(
    domain: ConfigDomain,
    raw: &Value,
    default: &Value,
    path: &str,
    partial: bool,
) -> Value {
    let Some(fields) = default.as_object() else {
        return normalize_field_value(domain, path, raw);
    };
    let Some(source) = raw.as_object() else {
        return default.clone();
    };
    let mut normalized = Map::new();
    for (key, default) in fields {
        let next_path = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        match source.get(key) {
            Some(value) => {
                normalized.insert(
                    key.clone(),
                    normalize_tree(domain, value, default, &next_path, partial),
                );
            }
            None if !partial => {
                normalized.insert(key.clone(), default.clone());
            }
            None => {}
        }
    }
    Value::Object(normalized)
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

fn is_connection_group(group: &str) -> bool {
    matches!(
        group,
        "connection" | "adminCredentials" | "typesafeConnection" | "openrouterConnection"
    )
}

fn is_connection_endpoint(field: &FieldDescriptor) -> bool {
    matches!(
        field.path.as_str(),
        "url"
            | "serverUrl"
            | "httpUrl"
            | "wsUrl"
            | "connection.apiBaseUrl"
            | "typesafe.apiUrl"
            | "openrouter.apiUrl"
    )
}

fn has_nonempty_default_endpoint(domain: ConfigDomain, field: &FieldDescriptor) -> bool {
    is_connection_endpoint(field)
        && (field
            .preset
            .as_deref()
            .is_some_and(|value| !value.is_empty())
            || value_at_path(&DEFAULT_DOMAINS[domain.as_str()], &field.path)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty()))
}

fn normalize_dependencies(
    domain: ConfigDomain,
    raw: &Value,
    normalized: &mut Value,
    partial: bool,
) {
    let fields = domain_fields(domain);
    let invalid_connections: BTreeSet<_> = fields
        .iter()
        .filter(|field| is_connection_group(&field.validation_group))
        .filter(|field| {
            is_connection_endpoint(field)
                || (field.validation_group == "adminCredentials" && field.path == "adminUsername")
        })
        .filter(|field| match source_leaf(raw, &field.path) {
            Ok(Some(value)) => !descriptor_accepts(field, value),
            // An absent endpoint in complete stored data must not redirect a
            // retained Key to a nonempty default endpoint. A partial Key edit
            // keeps its absent endpoint absent until merged with current data.
            Ok(None) => !partial && has_nonempty_default_endpoint(domain, field),
            // A malformed containing object already defaults as a whole tree.
            Err(()) => false,
        })
        .map(|field| field.validation_group.as_str())
        .collect();
    for field in fields.iter().filter(|field| {
        field.secret && invalid_connections.contains(field.validation_group.as_str())
    }) {
        if let Some(default) = value_at_path(&DEFAULT_DOMAINS[domain.as_str()], &field.path) {
            let _ = set_value_at_path(normalized, &field.path, default.clone());
        }
    }
}

/// Produce the complete current typed domain. Invalid/missing leaves default,
/// invalid groups default as a group, and unknown keys are not retained.
pub fn normalize_domain(domain: ConfigDomain, raw: &Value) -> Value {
    let mut normalized = normalize_tree(domain, raw, &DEFAULT_DOMAINS[domain.as_str()], "", false);
    normalize_dependencies(domain, raw, &mut normalized, false);
    normalized
}

/// Preserve partial presence. An explicitly supplied invalid group becomes its
/// default group; absent peers remain absent except credentials paired with an
/// invalid endpoint or administrative identity.
pub fn normalize_domain_patch(domain: ConfigDomain, raw: &Value) -> Value {
    let mut normalized = normalize_tree(domain, raw, &DEFAULT_DOMAINS[domain.as_str()], "", true);
    normalize_dependencies(domain, raw, &mut normalized, true);
    normalized
}

/// Flat command fields use the same partial policy, including coupled endpoint
/// and credential defaults. Other valid fields in that group remain unchanged.
/// Unknown request paths must be rejected before this.
pub fn normalize_changes(domain: ConfigDomain, changes: &Map<String, Value>) -> Map<String, Value> {
    let mut nested = Value::Object(Map::new());
    for field in domain_fields(domain) {
        if let Some(value) = changes.get(&field.path) {
            let _ = set_value_at_path(&mut nested, &field.path, value.clone());
        }
    }
    let normalized = normalize_domain_patch(domain, &nested);
    domain_fields(domain)
        .into_iter()
        .filter_map(|field| {
            value_at_path(&normalized, &field.path)
                .cloned()
                .map(|value| (field.path, value))
        })
        .collect()
}

pub fn settings_to_domains(settings: &Settings) -> Result<BTreeMap<ConfigDomain, Value>, String> {
    let serialized = serde_json::to_value(settings).map_err(|_| "无法序列化配置".to_string())?;
    ConfigDomain::ALL
        .into_iter()
        .map(|domain| {
            serialized
                .get(domain.as_str())
                .map(|value| (domain, normalize_domain(domain, value)))
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
                normalize_domain(domain, domains.get(&domain).unwrap_or(&Value::Null)),
            )
        })
        .collect();
    serde_json::from_value(Value::Object(object))
        .map_err(|_| "配置领域快照的类型不正确".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stored_json_number_overflow_defaults_only_the_affected_field() {
        let defaults = domain_defaults(ConfigDomain::Voice);
        let raw = parse_config_json(
            r#"{
            "connection":{"apiBaseUrl":"https://voice.example/v1","apiKey":"keep-key"},
            "vad":{"endSilenceMs":1e400,"playbackOnsetMs":100},
            "asr":{"stream":false,"language":"en"},
            "tts":{"voice":"Mia"},"guidance":{"rules":"1e400 is literal text"}
        }"#,
            &defaults,
        );
        let voice = normalize_domain(ConfigDomain::Voice, &raw);
        assert_eq!(
            voice["vad"]["endSilenceMs"],
            defaults["vad"]["endSilenceMs"]
        );
        assert_eq!(voice["vad"]["playbackOnsetMs"], 100);
        assert_eq!(voice["asr"]["stream"], false);
        assert_eq!(voice["asr"]["language"], "en");
        assert_eq!(voice["connection"]["apiKey"], "keep-key");
        assert_eq!(voice["tts"]["voice"], "Mia");
        assert_eq!(voice["guidance"]["rules"], "1e400 is literal text");
        assert_eq!(
            parse_config_json(r#"{"asr":{"language":"en"},"vad":broken}"#, &defaults),
            Value::Null
        );
    }

    #[test]
    fn stored_json_arrays_and_numbers_are_not_coerced() {
        let defaults = domain_defaults(ConfigDomain::Navigation);
        let parsed = parse_config_json(
            r#"{"agentOrder":["a",1e400],"groupOrder":["b","1"]}"#,
            &defaults,
        );
        let navigation = normalize_domain(ConfigDomain::Navigation, &parsed);
        assert_eq!(navigation["agentOrder"], defaults["agentOrder"]);
        assert_eq!(navigation["groupOrder"], json!(["b", "1"]));
        let defaults = domain_defaults(ConfigDomain::Jev);
        for number in ["-1", "1.5", "1000.0", "\"1000\"", "4294967296", "1e400"] {
            let parsed = parse_config_json(
                &format!(r#"{{"timeoutMs":{number},"maxRetries":3}}"#),
                &defaults,
            );
            let jev = normalize_domain(ConfigDomain::Jev, &parsed);
            assert_eq!(jev["timeoutMs"], defaults["timeoutMs"], "{number}");
            assert_eq!(jev["maxRetries"], 3);
        }
    }

    #[test]
    fn missing_domain_values_default_but_empty_patches_stay_empty() {
        for domain in ConfigDomain::ALL {
            assert_eq!(
                normalize_domain(domain, &Value::Null),
                domain_defaults(domain)
            );
            assert_eq!(
                normalize_domain(domain, &json!({})),
                domain_defaults(domain)
            );
            assert_eq!(normalize_domain_patch(domain, &json!({})), json!({}));
        }
    }

    #[test]
    fn invalid_leaves_groups_and_unknown_keys_do_not_discard_valid_siblings() {
        let raw = json!({
            "asr":{"language":"en","stream":false,"inputEncoding":"unsupported","unknown":42},
            "tts":false,
            "vad":{"endSilenceMs":1,"playbackOnsetMs":100},
            "unknownRoot":{"keep":false}
        });
        let voice = normalize_domain(ConfigDomain::Voice, &raw);
        assert_eq!(voice["asr"]["language"], "en");
        assert_eq!(voice["asr"]["stream"], false);
        assert_eq!(voice["asr"]["inputEncoding"], "wav");
        assert_eq!(voice["tts"], domain_defaults(ConfigDomain::Voice)["tts"]);
        assert_eq!(
            voice["vad"]["endSilenceMs"],
            domain_defaults(ConfigDomain::Voice)["vad"]["endSilenceMs"]
        );
        assert!(voice.get("unknownRoot").is_none());
        assert!(voice["asr"].get("unknown").is_none());
        let typed: VoiceConfig = serde_json::from_value(voice).unwrap();
        assert_eq!(typed.asr.language, "en");
    }

    #[test]
    fn malformed_field_matrix_always_deserializes_into_current_rust_types() {
        for domain in ConfigDomain::ALL {
            for field in domain_fields(domain) {
                let bad = match field.kind {
                    FieldKind::Boolean => json!("true"),
                    FieldKind::Integer => json!(u64::MAX),
                    FieldKind::String => json!(false),
                    FieldKind::StringArray => json!([false]),
                };
                let mut raw = domain_defaults(domain);
                set_value_at_path(&mut raw, &field.path, bad).unwrap();
                let normalized = normalize_domain(domain, &raw);
                assert_eq!(
                    value_at_path(&normalized, &field.path),
                    value_at_path(&domain_defaults(domain), &field.path)
                );
                let mut domains = settings_to_domains(&Settings::default()).unwrap();
                domains.insert(domain, normalized);
                assert!(settings_from_domains(&domains).is_ok());
            }
        }
    }

    #[test]
    fn endpoint_defaulting_also_defaults_its_paired_credentials() {
        let raw = json!({"connection":{"apiBaseUrl":false,"apiKey":"third-party-key"},"tts":{"voice":"Mia"}});
        let voice = normalize_domain(ConfigDomain::Voice, &raw);
        assert_eq!(
            voice["connection"],
            domain_defaults(ConfigDomain::Voice)["connection"]
        );
        assert_eq!(voice["tts"]["voice"], "Mia");
        let missing_endpoint = normalize_domain(
            ConfigDomain::Voice,
            &json!({"connection":{"apiKey":"third-party-key"}}),
        );
        assert_eq!(missing_endpoint["connection"]["apiKey"], "");
        let invalid_provider = normalize_domain_patch(
            ConfigDomain::Jev,
            &json!({"openrouter":{"apiUrl":42,"apiKey":"router-key","model":"chosen-model"}}),
        );
        assert_eq!(
            invalid_provider["openrouter"]["apiUrl"],
            domain_defaults(ConfigDomain::Jev)["openrouter"]["apiUrl"]
        );
        assert_eq!(invalid_provider["openrouter"]["apiKey"], "");
        assert_eq!(invalid_provider["openrouter"]["model"], "chosen-model");
        assert!(invalid_provider.get("typesafe").is_none());
        assert_eq!(
            normalize_domain_patch(
                ConfigDomain::Voice,
                &json!({"connection":{"apiKey":"new-key"}})
            ),
            json!({"connection":{"apiKey":"new-key"}})
        );
    }

    #[test]
    fn invalid_model_or_device_name_preserves_valid_connection_siblings() {
        let jev = normalize_domain(
            ConfigDomain::Jev,
            &json!({
                "openrouter":{"apiUrl":"https://custom.example/decisions","apiKey":"keep-key","model":false}
            }),
        );
        assert_eq!(
            jev["openrouter"]["apiUrl"],
            "https://custom.example/decisions"
        );
        assert_eq!(jev["openrouter"]["apiKey"], "keep-key");
        assert_eq!(
            jev["openrouter"]["model"],
            domain_defaults(ConfigDomain::Jev)["openrouter"]["model"]
        );
        let changes = json!({"openrouter.model":false})
            .as_object()
            .unwrap()
            .clone();
        let normalized = normalize_changes(ConfigDomain::Jev, &changes);
        assert_eq!(normalized.len(), 1);
        assert_eq!(
            normalized["openrouter.model"],
            domain_defaults(ConfigDomain::Jev)["openrouter"]["model"]
        );

        let distributed = normalize_domain(
            ConfigDomain::Distributed,
            &json!({
                "enabled":true,"wsUrl":"wss://custom.example/ws","vcpKey":"keep-key","deviceName":false
            }),
        );
        assert_eq!(distributed["enabled"], true);
        assert_eq!(distributed["wsUrl"], "wss://custom.example/ws");
        assert_eq!(distributed["vcpKey"], "keep-key");
        assert_eq!(distributed["deviceName"], "VCPMobile");
    }

    #[test]
    fn invalid_credentials_do_not_reset_valid_targets_or_admin_names() {
        let voice = normalize_domain(
            ConfigDomain::Voice,
            &json!({
                "connection":{"apiBaseUrl":"https://custom.example/v1","apiKey":false}
            }),
        );
        assert_eq!(
            voice["connection"]["apiBaseUrl"],
            "https://custom.example/v1"
        );
        assert_eq!(voice["connection"]["apiKey"], "");
        let invalid_password = normalize_domain_patch(
            ConfigDomain::Server,
            &json!({
                "adminUsername":"keep-user","adminPassword":false
            }),
        );
        assert_eq!(
            invalid_password,
            json!({"adminUsername":"keep-user","adminPassword":""})
        );
        let invalid_username = normalize_domain_patch(
            ConfigDomain::Server,
            &json!({
                "adminUsername":false,"adminPassword":"old-password"
            }),
        );
        assert_eq!(
            invalid_username,
            json!({"adminUsername":"","adminPassword":""})
        );
    }

    #[test]
    fn partial_normalization_keeps_absence_and_defaults_only_supplied_invalid_groups() {
        let partial = normalize_domain_patch(
            ConfigDomain::Voice,
            &json!({"asr":{"stream":null,"language":"en"},"tts":false,"unknown":1}),
        );
        assert_eq!(partial["asr"], json!({"stream":true,"language":"en"}));
        assert_eq!(partial["tts"], domain_defaults(ConfigDomain::Voice)["tts"]);
        assert!(partial.get("connection").is_none());
        assert!(partial.get("guidance").is_none());
        assert_eq!(
            normalize_domain_patch(ConfigDomain::Voice, &json!({"guidance":{"rules":""}})),
            json!({"guidance":{"rules":""}})
        );
        for rules in ["", "  \n ", "自定义风格规则"] {
            let normalized =
                normalize_domain(ConfigDomain::Voice, &json!({"guidance":{"rules":rules}}));
            assert_eq!(normalized["guidance"]["rules"], rules);
        }
        let missing = normalize_domain(ConfigDomain::Voice, &json!({}));
        assert_eq!(missing["guidance"]["rules"], DEFAULT_VOICE_GUIDANCE_RULES);
    }

    #[test]
    fn flat_changes_keep_connection_reset_intent_and_partial_presence() {
        let changes = json!({"connection.apiBaseUrl":false,"guidance.rules":"保留"})
            .as_object()
            .unwrap()
            .clone();
        let normalized = normalize_changes(ConfigDomain::Voice, &changes);
        assert_eq!(normalized["connection.apiKey"], "");
        assert_eq!(
            normalized["connection.apiBaseUrl"],
            VoiceConnectionConfig::default().api_base_url
        );
        assert_eq!(normalized["guidance.rules"], "保留");
        assert!(!normalized.contains_key("tts.mode"));
        assert_eq!(
            normalize_domain(ConfigDomain::Voice, &json!({"tts":{"mode":false}}))["tts"]["mode"],
            "builtin"
        );
    }

    #[test]
    fn empty_defaults_preserve_explicit_connection_values() {
        for domain in [ConfigDomain::Voice, ConfigDomain::Jev] {
            for field in domain_fields(domain)
                .into_iter()
                .filter(|field| field.preset.is_some())
            {
                let preset = field.preset.as_deref().unwrap();
                assert_eq!(
                    normalize_field_value(domain, &field.path, &json!(preset)),
                    preset
                );
                assert_eq!(
                    normalize_field_value(domain, &field.path, &json!("custom-value")),
                    "custom-value"
                );
                assert_eq!(
                    value_at_path(&domain_defaults(domain), &field.path).unwrap(),
                    ""
                );
                if !is_connection_endpoint(&field) {
                    continue;
                }
                let key_path = match field.path.as_str() {
                    "connection.apiBaseUrl" => "connection.apiKey",
                    "typesafe.apiUrl" => "typesafe.apiKey",
                    "openrouter.apiUrl" => "openrouter.apiKey",
                    _ => unreachable!(),
                };
                for input in ["", " \t ", preset, "https://custom.example/api"] {
                    let mut raw = json!({});
                    set_value_at_path(&mut raw, &field.path, json!(input)).unwrap();
                    set_value_at_path(&mut raw, key_path, json!("retained-key")).unwrap();
                    let normalized = normalize_domain(domain, &raw);
                    assert_eq!(
                        value_at_path(&normalized, key_path).unwrap(),
                        "retained-key"
                    );
                    assert_eq!(value_at_path(&normalized, &field.path).unwrap(), input);
                }
                let mut missing = json!({});
                set_value_at_path(&mut missing, key_path, json!("retained-key")).unwrap();
                assert_eq!(
                    value_at_path(&normalize_domain(domain, &missing), key_path).unwrap(),
                    ""
                );
                set_value_at_path(&mut missing, &field.path, json!(false)).unwrap();
                assert_eq!(
                    value_at_path(&normalize_domain(domain, &missing), key_path).unwrap(),
                    ""
                );
            }
        }
    }
}
