use super::schema;
use super::{read_config_snapshot, ConfigDomain, ConfigSnapshot, SettingsState};
use crate::vcp_modules::agent::{agent_service, agent_types::AgentLocalSettingsPatch};
use crate::vcp_modules::chat::context_injection::TarvenRule;
use crate::vcp_modules::group::{
    group_service,
    group_types::{GroupLocalSettings, GroupLocalSettingsPatch},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::io::AsyncReadExt;

const MAX_CONFIG_BACKUP_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StandaloneBackupData {
    #[serde(default)]
    pub model_favorites: Vec<String>,
    #[serde(default)]
    pub tarven_rules: Vec<TarvenRule>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ParsedConfigBackup {
    pub domains: Map<String, Value>,
    pub preferences: Map<String, Value>,
    pub agent_preferences: BTreeMap<String, AgentLocalSettingsPatch>,
    pub group_preferences: BTreeMap<String, GroupLocalSettingsPatch>,
    pub standalone_data: StandaloneBackupData,
    pub exported_at: String,
}

pub fn parse_standalone_data_backup(
    value: &Value,
) -> Result<(Vec<String>, Vec<TarvenRule>), String> {
    let Some(obj) = value.as_object() else {
        return Ok((Vec::new(), Vec::new()));
    };
    let model_favorites: Vec<String> = match obj.get("modelFavorites") {
        Some(v) => serde_json::from_value(v.clone()).unwrap_or_default(),
        None => Vec::new(),
    };
    let tarven_rules: Vec<TarvenRule> = match obj.get("tarvenRules") {
        Some(v) => serde_json::from_value(v.clone()).unwrap_or_default(),
        None => Vec::new(),
    };
    Ok((model_favorites, tarven_rules))
}

// Preferences keep their frontend owners. Preserve supplied values for their
// typed normalization without inventing a second default table in Rust.
fn parse_backup_preferences(raw: Option<&Value>) -> Result<Map<String, Value>, String> {
    let mut preferences = Map::new();
    let Some(raw) = raw else {
        return Ok(preferences);
    };
    let Some(source) = raw.as_object() else {
        preferences.insert("theme".into(), Value::Null);
        preferences.insert("chat".into(), Value::Null);
        preferences.insert("topic".into(), Value::Null);
        preferences.insert("diary".into(), Value::Null);
        preferences.insert("client".into(), Value::Null);
        return Ok(preferences);
    };
    for (group, fields) in [
        ("theme", &["themeKey", "mode"][..]),
        (
            "chat",
            &[
                "presentationMode",
                "smoothStreamingEnabled",
                "messageHapticsEnabled",
                "collapseToolsWhileStreaming",
            ][..],
        ),
        ("topic", &["sortMode", "pinnedTopicKeys"][..]),
        (
            "diary",
            &["hiddenFolders", "folderOrder", "collapsedCategories"][..],
        ),
        ("client", &["autoCheckUpdate"][..]),
    ] {
        let Some(value) = source.get(group) else {
            continue;
        };
        if let Some(object) = value.as_object() {
            let filtered: Map<String, Value> = fields
                .iter()
                .filter_map(|key| object.get(*key).map(|value| ((*key).into(), value.clone())))
                .collect();
            if !filtered.is_empty() {
                preferences.insert(group.into(), filtered.into());
            }
        } else {
            preferences.insert(group.into(), value.clone());
        }
    }
    Ok(preferences)
}

fn parse_domain_fields(domain: ConfigDomain, raw: &Value) -> Result<Value, String> {
    let normalized = schema::normalize_domain_patch(domain, raw);
    let mut parsed = Value::Object(Map::new());
    for field in schema::domain_fields(domain)
        .into_iter()
        .filter(|field| field.backup)
    {
        if let Some(value) = schema::value_at_path(&normalized, &field.path) {
            schema::set_value_at_path(&mut parsed, &field.path, value.clone())?;
        }
    }
    Ok(parsed)
}

/// One-way v1 import keeps only explicitly provided portable fields. Unlike
/// database migration, absent old fields never acquire defaults here.
fn parse_v1_backup(raw: &Value) -> Result<ParsedConfigBackup, String> {
    let source = raw["settings"]
        .as_object()
        .ok_or_else(|| "备份文件缺少 settings 配置内容".to_string())?;
    let mut domains = Map::new();
    for (domain, old_name, path) in super::migration::legacy_fields() {
        let Some(value) = source.get(*old_name) else {
            continue;
        };
        if !schema::domain_fields(*domain)
            .iter()
            .any(|field| field.path == *path && field.backup)
        {
            continue;
        }
        let target = domains
            .entry(domain.as_str().to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        schema::set_value_at_path(target, path, value.clone())?;
    }
    for domain in ConfigDomain::ALL {
        if let Some(value) = domains.get_mut(domain.as_str()) {
            *value = parse_domain_fields(domain, value)?;
        }
    }
    Ok(ParsedConfigBackup {
        domains,
        // v1 never owned UI preferences; even unexpected preferences are ignored.
        preferences: Map::new(),
        agent_preferences: BTreeMap::new(),
        group_preferences: BTreeMap::new(),
        standalone_data: StandaloneBackupData::default(),
        exported_at: raw["exportedAt"].as_str().unwrap_or_default().to_string(),
    })
}

fn parse_backup_bytes(bytes: &[u8]) -> Result<ParsedConfigBackup, String> {
    if bytes.len() as u64 > MAX_CONFIG_BACKUP_BYTES {
        return Err("备份文件过大（超过 1 MiB），已拒绝导入".into());
    }
    // Validate the envelope separately from configuration content: one numeric
    // overflow in a preference must not discard unrelated, valid settings.
    let source: BTreeMap<String, &serde_json::value::RawValue> =
        serde_json::from_slice(bytes).map_err(|_| "备份文件不是合法的 JSON".to_string())?;
    let mut raw = Map::new();
    for key in ["app", "kind", "version", "exportedAt"] {
        if let Some(value) = source.get(key) {
            raw.insert(
                key.into(),
                serde_json::from_str(value.get())
                    .map_err(|_| format!("备份字段 {key} 类型不正确"))?,
            );
        }
    }
    for (key, shape) in [
        ("domains", serde_json::json!(super::Settings::default())),
        ("settings", serde_json::json!({})),
        // Only the shape belongs here; defaults remain with the frontend owner.
        (
            "preferences",
            serde_json::json!({"theme":{},"chat":{},"topic":{},"diary":{},"client":{}}),
        ),
    ] {
        if let Some(value) = source.get(key) {
            raw.insert(key.into(), schema::parse_config_json(value.get(), &shape));
        }
    }
    if let Some(value) = source.get("agentPreferences") {
        let agents =
            serde_json::from_str::<BTreeMap<String, &serde_json::value::RawValue>>(value.get());
        let shape = serde_json::json!(
            crate::vcp_modules::agent::agent_types::AgentLocalPreferences::default()
        );
        raw.insert(
            "agentPreferences".into(),
            match agents {
                Ok(agents) => Value::Object(
                    agents
                        .into_iter()
                        .map(|(id, value)| (id, schema::parse_config_json(value.get(), &shape)))
                        .collect(),
                ),
                Err(_) => Value::Null,
            },
        );
    }
    if let Some(value) = source.get("groupPreferences") {
        let groups =
            serde_json::from_str::<BTreeMap<String, &serde_json::value::RawValue>>(value.get());
        let shape = serde_json::json!(GroupLocalSettings::default());
        raw.insert(
            "groupPreferences".into(),
            match groups {
                Ok(groups) => Value::Object(
                    groups
                        .into_iter()
                        .map(|(id, value)| (id, schema::parse_config_json(value.get(), &shape)))
                        .collect(),
                ),
                Err(_) => Value::Null,
            },
        );
    }
    if let Some(value) = source.get("standaloneData") {
        if let Ok(parsed) = serde_json::from_str::<Value>(value.get()) {
            raw.insert("standaloneData".into(), parsed);
        }
    }
    let raw = Value::Object(raw);
    if raw["app"] != "vcp-mobile" || raw["kind"] != "vcp-mobile-settings-backup" {
        return Err("这不是 VCP Mobile 的配置备份文件".into());
    }
    match raw["version"].as_u64() {
        Some(1) => return parse_v1_backup(&raw),
        Some(2) => {}
        _ => return Err("不支持的备份版本".into()),
    }
    if raw
        .get("exportedAt")
        .is_some_and(|value| !value.is_string())
    {
        return Err("备份字段 exportedAt 类型不正确".into());
    }
    let source = raw["domains"]
        .as_object()
        .ok_or_else(|| "备份文件缺少 domains 配置内容".to_string())?;
    let mut domains = Map::new();
    for (name, raw_domain) in source {
        let Some(domain) = ConfigDomain::ALL
            .into_iter()
            .find(|domain| domain.as_str() == name)
        else {
            continue;
        };
        let parsed = parse_domain_fields(domain, raw_domain)?;
        if parsed.as_object().is_some_and(|object| !object.is_empty()) {
            domains.insert(name.clone(), parsed);
        }
    }
    Ok(ParsedConfigBackup {
        domains,
        preferences: parse_backup_preferences(raw.get("preferences"))?,
        agent_preferences: match raw.get("agentPreferences") {
            Some(value) => agent_service::parse_agent_local_settings_backup(value)?,
            None => BTreeMap::new(),
        },
        group_preferences: match raw.get("groupPreferences") {
            Some(value) => group_service::parse_group_local_settings_backup(value)?,
            None => BTreeMap::new(),
        },
        standalone_data: match raw.get("standaloneData") {
            Some(value) => {
                let (model_favorites, tarven_rules) = parse_standalone_data_backup(value)?;
                StandaloneBackupData {
                    model_favorites,
                    tarven_rules,
                }
            }
            None => StandaloneBackupData::default(),
        },
        exported_at: raw["exportedAt"].as_str().unwrap_or_default().to_string(),
    })
}

/// Native selection hands over a temporary path; no external file bytes enter Vue.
#[tauri::command]
pub async fn parse_config_backup(path: String) -> Result<ParsedConfigBackup, String> {
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|error| error.to_string())?;
    let metadata = file.metadata().await.map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("请选择配置备份文件".into());
    }
    if metadata.len() > MAX_CONFIG_BACKUP_BYTES {
        return Err("备份文件过大（超过 1 MiB），已拒绝导入".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BACKUP_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| error.to_string())?;
    parse_backup_bytes(&bytes)
}

/// Portable fields come from one committed, typed configuration snapshot.
#[tauri::command]
pub async fn export_config_domains<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, SettingsState>,
) -> Result<Map<String, Value>, String> {
    let snapshot = read_config_snapshot(app, state).await?;
    backup_domains(&snapshot)
}

fn backup_domains(snapshot: &ConfigSnapshot) -> Result<Map<String, Value>, String> {
    let values = schema::settings_to_domains(&snapshot.data)?;
    let mut domains = Map::new();
    for domain in ConfigDomain::ALL {
        let fields = schema::domain_fields(domain)
            .into_iter()
            .filter(|field| field.backup)
            .collect::<Vec<_>>();
        if fields.is_empty() {
            continue;
        }
        let value = values
            .get(&domain)
            .ok_or_else(|| "配置快照缺少领域".to_string())?;
        let mut filtered = Value::Object(Map::new());
        for field in fields {
            let value = schema::value_at_path(value, &field.path)
                .ok_or_else(|| "配置快照缺少字段".to_string())?;
            schema::set_value_at_path(&mut filtered, &field.path, value.clone())?;
        }
        domains.insert(domain.as_str().to_owned(), filtered);
    }
    Ok(domains)
}

#[tauri::command]
pub async fn export_group_preferences<R: Runtime>(
    app: AppHandle<R>,
) -> Result<BTreeMap<String, GroupLocalSettings>, String> {
    group_service::export_group_local_settings(
        &app.state::<crate::vcp_modules::db_manager::DbState>(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::super::{DomainStatus, Settings};
    use super::*;
    use serde_json::json;

    fn backup(domains: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({"app":"vcp-mobile","kind":"vcp-mobile-settings-backup","version":2,"domains":domains})).unwrap()
    }

    fn v1_backup(settings: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({"app":"vcp-mobile","kind":"vcp-mobile-settings-backup","version":1,"settings":settings})).unwrap()
    }

    fn snapshot() -> ConfigSnapshot {
        ConfigSnapshot {
            data: Settings::default(),
            domains: ConfigDomain::ALL
                .into_iter()
                .map(|domain| {
                    (
                        domain,
                        DomainStatus {
                            revision: "1".into(),
                            schema_version: domain.schema_version(),
                        },
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn group_backup_parser_preserves_only_supplied_nested_fields_and_explicit_empty_values() {
        let patches = json!({
            "group-a": {
                "chatMode":null,
                "enableContextMessageWindow":false,
                "modeSettings":{
                    "sequential":{"speakerOrder":[]},
                    "jev":{"navigatorPrompt":"","memberStyles":{}}
                }
            }
        });
        let bytes = serde_json::to_vec(&json!({
            "app":"vcp-mobile", "kind":"vcp-mobile-settings-backup", "version":2,
            "domains":{}, "groupPreferences":patches,
        }))
        .unwrap();
        let parsed = parse_backup_bytes(&bytes).unwrap();
        let mut normalized = patches;
        normalized["group-a"]["chatMode"] = json!("sequential");
        assert_eq!(
            serde_json::to_value(parsed.group_preferences).unwrap(),
            normalized
        );
        assert!(parsed.domains.is_empty());
    }

    #[test]
    fn export_uses_schema_scope_and_round_trips_the_current_format() {
        let mut snapshot = snapshot();
        snapshot.data.server.api_key = "kept-plaintext".into();
        let domains = backup_domains(&snapshot).unwrap();
        assert!(domains.contains_key("navigation"));
        assert_eq!(
            domains["navigation"],
            serde_json::json!({"agentOrder": [], "groupOrder": []})
        );
        assert!(domains["sync"].get("logLevel").is_none());
        assert!(domains["sync"].get("prerenderEnabled").is_none());
        assert_eq!(domains["server"]["apiKey"], "kept-plaintext");
        let parsed = parse_backup_bytes(&backup(domains.clone().into())).unwrap();
        assert_eq!(parsed.domains, domains);
    }

    #[test]
    fn partial_domains_keep_only_present_leaves_and_keep_plaintext_credentials() {
        let parsed = parse_backup_bytes(&backup(json!({"server":{"apiKey":"secret"},"voice":{"asr":{"inputEncoding":"mp3_64"},"vad":{"endSilenceMs":777}}}))).unwrap();
        assert_eq!(parsed.domains["server"], json!({"apiKey":"secret"}));
        assert_eq!(
            parsed.domains["voice"],
            json!({"asr":{"inputEncoding":"mp3_64"},"vad":{"endSilenceMs":777}})
        );
        assert!(!parsed.domains.contains_key("identity"));
    }

    #[test]
    fn overflowing_backup_numbers_recover_only_their_configuration_fields() {
        let parsed = parse_backup_bytes(br#"{
            "app":"vcp-mobile","kind":"vcp-mobile-settings-backup","version":2,
            "domains":{"jev":{"timeoutMs":1e400,"maxRetries":0},"voice":{"tts":{"stream":false,"voice":"Mia"}}},
            "preferences":{"chat":{"messageHapticsEnabled":1e400,"smoothStreamingEnabled":false}},
            "groupPreferences":{"g":{"chatMode":"jev","modeSettings":{"jev":{"maxSpeakersPerRound":1e400,"continueThreshold":0.7,"memberStyles":{"a":"keep","b":1e400}}}}}
        }"#).unwrap();
        assert_eq!(
            parsed.domains["jev"],
            json!({"timeoutMs":Settings::default().jev.timeout_ms,"maxRetries":0})
        );
        assert_eq!(
            parsed.domains["voice"],
            json!({"tts":{"stream":false,"voice":"Mia"}})
        );
        assert_eq!(
            parsed.preferences["chat"],
            json!({"messageHapticsEnabled":null,"smoothStreamingEnabled":false})
        );
        let groups = serde_json::to_value(parsed.group_preferences).unwrap();
        assert_eq!(groups["g"]["chatMode"], "jev");
        let defaults = GroupLocalSettings::default();
        assert_eq!(
            groups["g"]["modeSettings"]["jev"],
            json!({
                "maxSpeakersPerRound":defaults.mode_settings.jev.max_speakers_per_round,
                "continueThreshold":0.7,"memberStyles":{"a":"keep","b":""}
            })
        );
        assert!(!parsed.domains.contains_key("server"));
    }

    #[test]
    fn overflow_in_v1_configuration_does_not_reset_valid_present_values() {
        let parsed = parse_backup_bytes(
            br#"{
            "app":"vcp-mobile","kind":"vcp-mobile-settings-backup","version":1,
            "settings":{"voiceVadEndSilenceMs":1e400,"voiceTtsStream":false,"vcpApiKey":"keep"}
        }"#,
        )
        .unwrap();
        assert_eq!(
            parsed.domains["voice"],
            json!({
                "vad":{"endSilenceMs":Settings::default().voice.vad.end_silence_ms},
                "tts":{"stream":false}
            })
        );
        assert_eq!(parsed.domains["server"], json!({"apiKey":"keep"}));
        assert!(!parsed.domains.contains_key("identity"));
    }

    #[test]
    fn envelope_errors_never_acquire_default_metadata() {
        for raw in [
            r#"{"app":"vcp-mobile","kind":"vcp-mobile-settings-backup","version":1e400,"domains":{}}"#,
            r#"{"app":"vcp-mobile","kind":"vcp-mobile-settings-backup","version":2,"domains":{},"exportedAt":1e400}"#,
            r#"{"app":"vcp-mobile","kind":"vcp-mobile-settings-backup","version":2,"domains":{"voice":{"tts":broken}}}"#,
        ] {
            assert!(parse_backup_bytes(raw.as_bytes()).is_err());
        }
        let parsed = parse_backup_bytes(br#"{"app":"vcp-mobile","kind":"vcp-mobile-settings-backup","version":2,"domains":{},"unknownMetadata":1e400}"#).unwrap();
        assert!(parsed.domains.is_empty());
        assert!(parsed.exported_at.is_empty());
    }

    #[test]
    fn voice_mode_import_defaults_only_when_the_field_is_present_and_invalid() {
        for mode in [
            json!("unknown-mode"),
            json!(""),
            Value::Null,
            json!(false),
            json!(42),
            json!({}),
        ] {
            let parsed = parse_backup_bytes(&backup(
                json!({"voice":{"tts":{"mode":mode,"stream":false}}}),
            ))
            .unwrap();
            assert_eq!(
                parsed.domains["voice"],
                json!({"tts":{"mode":"builtin","stream":false}})
            );
            let legacy = parse_backup_bytes(&v1_backup(
                json!({"voiceTtsMode":mode,"voiceTtsStream":false}),
            ))
            .unwrap();
            assert_eq!(
                legacy.domains["voice"],
                json!({"tts":{"mode":"builtin","stream":false}})
            );
        }
        for mode in ["builtin", "clone"] {
            let parsed =
                parse_backup_bytes(&backup(json!({"voice":{"tts":{"mode":mode}}}))).unwrap();
            assert_eq!(parsed.domains["voice"]["tts"]["mode"], mode);
        }
        let partial =
            parse_backup_bytes(&backup(json!({"voice":{"tts":{"stream":false}}}))).unwrap();
        assert_eq!(partial.domains["voice"], json!({"tts":{"stream":false}}));
        let legacy_partial =
            parse_backup_bytes(&v1_backup(json!({"voiceTtsStream":false}))).unwrap();
        assert_eq!(
            legacy_partial.domains["voice"],
            json!({"tts":{"stream":false}})
        );
    }

    #[test]
    fn invalid_fields_use_current_defaults_and_keep_valid_siblings() {
        let parsed = parse_backup_bytes(&backup(json!({
            "voice":{
                "asr":{"inputEncoding":"private-token","language":"en"},
                "tts":{"voice":"private-token","stream":false},
                "vad":{"endSilenceMs":"private-token"}
            },
            "server":{"url":"https://saved.example","apiKey":42}
        })))
        .unwrap();
        let voice = schema::domain_defaults(ConfigDomain::Voice);
        assert_eq!(
            parsed.domains["voice"],
            json!({
                "asr":{"inputEncoding":voice["asr"]["inputEncoding"],"language":"en"},
                "tts":{"voice":voice["tts"]["voice"],"stream":false},
                "vad":{"endSilenceMs":voice["vad"]["endSilenceMs"]}
            })
        );
        assert_eq!(
            parsed.domains["server"],
            json!({"url":"https://saved.example","apiKey":""})
        );
        assert!(!serde_json::to_string(&parsed)
            .unwrap()
            .contains("private-token"));
    }

    #[test]
    fn unknown_and_nonportable_fields_are_dropped_while_bad_provided_groups_default() {
        let parsed = parse_backup_bytes(&backup(json!({
            "unknown-domain":{"private-token":true},
            "voice":{"unknown-field":true,"tts":null,"asr":{"language":"en"}},
            "navigation":{"agentOrder":["not-imported"]},
            "sync":{"logLevel":"DEBUG","token":"keep-token"}
        })))
        .unwrap();
        assert_eq!(
            parsed.domains["voice"],
            json!({
                "tts":schema::domain_defaults(ConfigDomain::Voice)["tts"],
                "asr":{"language":"en"}
            })
        );
        assert_eq!(parsed.domains["sync"], json!({"token":"keep-token"}));
        assert_eq!(
            parsed.domains["navigation"],
            serde_json::json!({"agentOrder": ["not-imported"]})
        );
        assert!(!parsed.domains.contains_key("unknown-domain"));
        let invalid_domain = parse_backup_bytes(&backup(json!({"server":false}))).unwrap();
        assert_eq!(
            invalid_domain.domains["server"],
            schema::domain_defaults(ConfigDomain::Server)
        );
    }

    #[test]
    fn partial_rule_import_preserves_explicit_empty_and_custom_rules() {
        for rules in ["", "  \n ", "自定义风格规则"] {
            let parsed =
                parse_backup_bytes(&backup(json!({"voice":{"guidance":{"rules":rules}}}))).unwrap();
            assert_eq!(parsed.domains["voice"], json!({"guidance":{"rules":rules}}));
        }
    }

    #[test]
    fn malformed_files_and_unknown_formats_still_fail_before_field_normalization() {
        assert!(parse_backup_bytes(b"{broken-json").is_err());
        for raw in [
            json!({"app":"other-app","kind":"vcp-mobile-settings-backup","version":2,"domains":{}}),
            json!({"app":"vcp-mobile","kind":"unknown-format","version":2,"domains":{}}),
            json!({"app":"vcp-mobile","kind":"vcp-mobile-settings-backup","version":999,"domains":{}}),
            json!({"app":"vcp-mobile","kind":"vcp-mobile-settings-backup","version":2,"domains":false}),
        ] {
            assert!(parse_backup_bytes(&serde_json::to_vec(&raw).unwrap()).is_err());
        }
        let known = json!({"app":"vcp-mobile","kind":"vcp-mobile-settings-backup","version":2,
            "unknownMetadata":true,"domains":{"identity":{"userName":"保留"}}});
        let parsed = parse_backup_bytes(&serde_json::to_vec(&known).unwrap()).unwrap();
        assert_eq!(parsed.domains["identity"], json!({"userName":"保留"}));
    }

    #[test]
    fn v1_sparse_import_never_fills_missing_fields_or_restores_preferences() {
        let bytes = serde_json::to_vec(&json!({"app":"vcp-mobile","kind":"vcp-mobile-settings-backup","version":1,"settings":{"vcpApiKey":"old-key"},"preferences":{"theme":{"mode":"light"},"chat":{"messageHapticsEnabled":false}}})).unwrap();
        let parsed = parse_backup_bytes(&bytes).unwrap();
        assert_eq!(
            parsed.domains,
            json!({"server":{"apiKey":"old-key"}})
                .as_object()
                .unwrap()
                .clone()
        );
        assert!(parsed.preferences.is_empty());
    }

    #[test]
    fn v1_keeps_false_and_empty_values_and_defaults_explicit_invalid_fields() {
        let parsed = parse_backup_bytes(&v1_backup(json!({
            "vcpApiKey":"", "userName":null, "distributedEnabled":false,
            "voiceAsrStream":false, "voiceTtsCloneSampleId":"", "unknownField":"ignored",
            "syncLogLevel":"not-imported", "agentOrder":["not-imported"],
        })))
        .unwrap();
        assert_eq!(
            Value::Object(parsed.domains),
            json!({
                "identity":{"userName":schema::domain_defaults(ConfigDomain::Identity)["userName"]},
                "navigation":{"agentOrder":["not-imported"]},
                "server":{"apiKey":""}, "distributed":{"enabled":false},
                "voice":{"asr":{"stream":false},"tts":{"cloneSampleId":""}},
            })
        );
    }

    #[test]
    fn v1_invalid_values_default_without_losing_valid_credentials_or_other_fields() {
        let parsed = parse_backup_bytes(&v1_backup(json!({
            "chatEndpointMode":"private-token", "vcpApiKey":"retained-key",
            "distributedEnabled":"private-token", "voiceLanguage":"private-token",
            "voiceTtsVoice":"Mia", "enableVcpToolInjection":true,
        })))
        .unwrap();
        assert_eq!(
            parsed.domains["server"],
            json!({"endpointMode":"standard","apiKey":"retained-key"})
        );
        assert_eq!(parsed.domains["distributed"], json!({"enabled":false}));
        assert_eq!(
            parsed.domains["voice"],
            json!({"asr":{"language":"auto"},"tts":{"voice":"Mia"}})
        );
        assert!(!serde_json::to_string(&parsed)
            .unwrap()
            .contains("private-token"));
    }

    #[test]
    fn absent_preferences_stay_absent_but_a_bad_root_defaults_both_frontend_owners() {
        let missing = parse_backup_bytes(&backup(json!({}))).unwrap();
        assert!(missing.preferences.is_empty());
        for preferences in [Value::Null, json!(false), json!([]), json!("invalid")] {
            let bytes = serde_json::to_vec(&json!({
                "app":"vcp-mobile", "kind":"vcp-mobile-settings-backup", "version":2,
                "domains":{}, "preferences":preferences,
            }))
            .unwrap();
            let parsed = parse_backup_bytes(&bytes).unwrap();
            assert_eq!(
                Value::Object(parsed.preferences),
                json!({"theme":null,"chat":null,"topic":null,"diary":null,"client":null})
            );
        }
    }

    #[test]
    fn preference_values_keep_presence_for_their_frontend_default_owner() {
        let bytes = serde_json::to_vec(&json!({
            "app":"vcp-mobile", "kind":"vcp-mobile-settings-backup", "version":2, "domains":{},
            "preferences":{
                "theme":null,
                "chat":{"messageHapticsEnabled":"invalid","unknownField":true},
                "unknownGroup":{"anything":true}
            }
        }))
        .unwrap();
        let parsed = parse_backup_bytes(&bytes).unwrap();
        assert_eq!(
            Value::Object(parsed.preferences),
            json!({
                "theme":null,"chat":{"messageHapticsEnabled":"invalid"}
            })
        );
    }

    #[test]
    fn preferences_remain_independent_partial_values() {
        let bytes = serde_json::to_vec(&json!({"app":"vcp-mobile","kind":"vcp-mobile-settings-backup","version":2,"domains":{},"preferences":{"theme":{"mode":"system"},"chat":{"messageHapticsEnabled":false}}})).unwrap();
        let parsed = parse_backup_bytes(&bytes).unwrap();
        assert!(parsed.domains.is_empty());
        assert_eq!(
            Value::Object(parsed.preferences),
            json!({"theme":{"mode":"system"},"chat":{"messageHapticsEnabled":false}})
        );
    }

    #[test]
    fn v2_full_backup_parses_all_standalone_and_preferences_data() {
        let v2_data = json!({
            "app": "vcp-mobile",
            "kind": "vcp-mobile-settings-backup",
            "version": 2,
            "exportedAt": "2026-10-31T12:00:00Z",
            "domains": {
                "navigation": {
                    "agentOrder": ["agent-1", "agent-2"],
                    "groupOrder": ["group-1"]
                }
            },
            "preferences": {
                "theme": { "mode": "dark", "themeKey": "cyberpunk" },
                "chat": { "presentationMode": "bubble", "smoothStreamingEnabled": true },
                "topic": { "sortMode": "updated", "pinnedTopicKeys": ["t-1"] },
                "diary": { "hiddenFolders": ["secret"], "folderOrder": ["f-1"], "collapsedCategories": [] },
                "client": { "autoCheckUpdate": true }
            },
            "agentPreferences": {
                "agent-1": {
                    "mobileSystemPrompt": "移动端专用提示词",
                    "useTemperature": true,
                    "voice": {
                        "enabled": true,
                        "mode": "builtin",
                        "voice": "Mia",
                        "stream": true,
                        "cloneSampleId": ""
                    }
                }
            },
            "groupPreferences": {
                "group-1": {
                    "chatMode": "sequential",
                    "enableContextMessageWindow": true,
                    "contextMessageWindowSize": 50
                }
            },
            "standaloneData": {
                "modelFavorites": ["gemini-2.5-flash", "claude-3-7-sonnet"],
                "tarvenRules": [
                    {
                        "id": "system_meta_injection",
                        "name": "系统元数据注入",
                        "ruleType": "system_meta_injection",
                        "isEnabled": false,
                        "content": "包含当前系统时间",
                        "scope": "global",
                        "wrap": false,
                        "sortOrder": -100
                    }
                ]
            }
        });

        let bytes = serde_json::to_vec(&v2_data).unwrap();
        let parsed = parse_backup_bytes(&bytes).unwrap();

        // 1. Domains: navigation exported and parsed
        assert_eq!(
            parsed.domains["navigation"]["agentOrder"],
            json!(["agent-1", "agent-2"])
        );
        assert_eq!(
            parsed.domains["navigation"]["groupOrder"],
            json!(["group-1"])
        );

        // 2. Preferences
        assert_eq!(parsed.preferences["theme"]["themeKey"], "cyberpunk");
        assert_eq!(parsed.preferences["topic"]["sortMode"], "updated");
        assert_eq!(
            parsed.preferences["topic"]["pinnedTopicKeys"],
            json!(["t-1"])
        );
        assert_eq!(
            parsed.preferences["diary"]["hiddenFolders"],
            json!(["secret"])
        );
        assert_eq!(parsed.preferences["client"]["autoCheckUpdate"], true);

        // 3. Agent preferences
        assert!(parsed.agent_preferences.contains_key("agent-1"));
        let agent_patch = &parsed.agent_preferences["agent-1"];
        assert_eq!(agent_patch.0["mobileSystemPrompt"], "移动端专用提示词");
        assert_eq!(agent_patch.0["useTemperature"], true);

        // 4. Group preferences
        assert!(parsed.group_preferences.contains_key("group-1"));

        // 5. Standalone data
        assert_eq!(
            parsed.standalone_data.model_favorites,
            vec!["gemini-2.5-flash", "claude-3-7-sonnet"]
        );
        assert_eq!(parsed.standalone_data.tarven_rules.len(), 1);
        assert_eq!(
            parsed.standalone_data.tarven_rules[0].id,
            "system_meta_injection"
        );
        assert!(!parsed.standalone_data.tarven_rules[0].is_enabled);
    }
}
