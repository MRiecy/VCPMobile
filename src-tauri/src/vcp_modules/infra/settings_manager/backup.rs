use super::schema;
use super::{read_config_snapshot, ConfigDomain, ConfigSnapshot, SettingsState};
use serde::Serialize;
use serde_json::{Map, Value};
use tauri::{AppHandle, Manager, Runtime, State};
use std::collections::BTreeMap;
use crate::vcp_modules::group::{
    group_service,
    group_types::{GroupLocalSettings, GroupLocalSettingsPatch},
};
use tokio::io::AsyncReadExt;

const MAX_CONFIG_BACKUP_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedConfigBackup {
    domains: Map<String, Value>,
    preferences: Map<String, Value>,
    group_preferences: BTreeMap<String, GroupLocalSettingsPatch>,
    exported_at: String,
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
        return Ok(preferences);
    };
    for (group, fields) in [
        ("theme", &["themeKey", "mode"][..]),
        ("chat", &[
            "presentationMode", "smoothStreamingEnabled",
            "messageHapticsEnabled", "collapseToolsWhileStreaming",
        ][..]),
    ] {
        let Some(value) = source.get(group) else { continue; };
        if let Some(object) = value.as_object() {
            let filtered: Map<String, Value> = fields.iter()
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
    for field in schema::domain_fields(domain).into_iter().filter(|field| field.backup) {
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
        group_preferences: BTreeMap::new(),
        exported_at: raw["exportedAt"].as_str().unwrap_or_default().to_string(),
    })
}

fn parse_backup_bytes(bytes: &[u8]) -> Result<ParsedConfigBackup, String> {
    if bytes.len() as u64 > MAX_CONFIG_BACKUP_BYTES {
        return Err("备份文件过大（超过 1 MiB），已拒绝导入".into());
    }
    let raw: Value =
        serde_json::from_slice(bytes).map_err(|_| "备份文件不是合法的 JSON".to_string())?;
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
        else { continue; };
        let parsed = parse_domain_fields(domain, raw_domain)?;
        if parsed.as_object().is_some_and(|object| !object.is_empty()) {
            domains.insert(name.clone(), parsed);
        }
    }
    Ok(ParsedConfigBackup {
        domains,
        preferences: parse_backup_preferences(raw.get("preferences"))?,
        group_preferences: match raw.get("groupPreferences") {
            Some(value) => group_service::parse_group_local_settings_backup(value)?,
            None => BTreeMap::new(),
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
    group_service::export_group_local_settings(&app.state::<crate::vcp_modules::db_manager::DbState>()).await
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
        })).unwrap();
        let parsed = parse_backup_bytes(&bytes).unwrap();
        let mut normalized = patches;
        normalized["group-a"]["chatMode"] = json!("sequential");
        assert_eq!(serde_json::to_value(parsed.group_preferences).unwrap(), normalized);
        assert!(parsed.domains.is_empty());
    }

    #[test]
    fn export_uses_schema_scope_and_round_trips_the_current_format() {
        let mut snapshot = snapshot();
        snapshot.data.server.api_key = "kept-plaintext".into();
        let domains = backup_domains(&snapshot).unwrap();
        assert!(!domains.contains_key("navigation"));
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
    fn voice_mode_import_defaults_only_when_the_field_is_present_and_invalid() {
        for mode in [json!("unknown-mode"), json!(""), Value::Null, json!(false), json!(42), json!({})] {
            let parsed = parse_backup_bytes(&backup(json!({"voice":{"tts":{"mode":mode,"stream":false}}}))).unwrap();
            assert_eq!(parsed.domains["voice"], json!({"tts":{"mode":"none","stream":false}}));
            let legacy = parse_backup_bytes(&v1_backup(json!({"voiceTtsMode":mode,"voiceTtsStream":false}))).unwrap();
            assert_eq!(legacy.domains["voice"], json!({"tts":{"mode":"none","stream":false}}));
        }
        for mode in ["none", "builtin", "clone"] {
            let parsed = parse_backup_bytes(&backup(json!({"voice":{"tts":{"mode":mode}}}))).unwrap();
            assert_eq!(parsed.domains["voice"]["tts"]["mode"], mode);
        }
        let partial = parse_backup_bytes(&backup(json!({"voice":{"tts":{"stream":false}}}))).unwrap();
        assert_eq!(partial.domains["voice"], json!({"tts":{"stream":false}}));
        let legacy_partial = parse_backup_bytes(&v1_backup(json!({"voiceTtsStream":false}))).unwrap();
        assert_eq!(legacy_partial.domains["voice"], json!({"tts":{"stream":false}}));
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
        }))).unwrap();
        let voice = schema::domain_defaults(ConfigDomain::Voice);
        assert_eq!(parsed.domains["voice"], json!({
            "asr":{"inputEncoding":voice["asr"]["inputEncoding"],"language":"en"},
            "tts":{"voice":voice["tts"]["voice"],"stream":false},
            "vad":{"endSilenceMs":voice["vad"]["endSilenceMs"]}
        }));
        assert_eq!(parsed.domains["server"], json!({"url":"https://saved.example","apiKey":""}));
        assert!(!serde_json::to_string(&parsed).unwrap().contains("private-token"));
    }

    #[test]
    fn unknown_and_nonportable_fields_are_dropped_while_bad_provided_groups_default() {
        let parsed = parse_backup_bytes(&backup(json!({
            "unknown-domain":{"private-token":true},
            "voice":{"unknown-field":true,"tts":null,"asr":{"language":"en"}},
            "navigation":{"agentOrder":["not-imported"]},
            "sync":{"logLevel":"DEBUG","token":"keep-token"}
        }))).unwrap();
        assert_eq!(parsed.domains["voice"], json!({
            "tts":schema::domain_defaults(ConfigDomain::Voice)["tts"],
            "asr":{"language":"en"}
        }));
        assert_eq!(parsed.domains["sync"], json!({"token":"keep-token"}));
        assert!(!parsed.domains.contains_key("navigation"));
        assert!(!parsed.domains.contains_key("unknown-domain"));
        let invalid_domain = parse_backup_bytes(&backup(json!({"server":false}))).unwrap();
        assert_eq!(invalid_domain.domains["server"], schema::domain_defaults(ConfigDomain::Server));
    }

    #[test]
    fn partial_rule_import_preserves_explicit_empty_and_custom_rules() {
        for rules in ["", "  \n ", "自定义风格规则"] {
            let parsed =
                parse_backup_bytes(&backup(json!({"voice":{"guidance":{"rules":rules}}})))
                    .unwrap();
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
        }))).unwrap();
        assert_eq!(parsed.domains["server"], json!({"endpointMode":"standard","apiKey":"retained-key"}));
        assert_eq!(parsed.domains["distributed"], json!({"enabled":false}));
        assert_eq!(parsed.domains["voice"], json!({"asr":{"language":"auto"},"tts":{"voice":"Mia"}}));
        assert!(!serde_json::to_string(&parsed).unwrap().contains("private-token"));
    }

    #[test]
    fn absent_preferences_stay_absent_but_a_bad_root_defaults_both_frontend_owners() {
        let missing = parse_backup_bytes(&backup(json!({}))).unwrap();
        assert!(missing.preferences.is_empty());
        for preferences in [Value::Null, json!(false), json!([]), json!("invalid")] {
            let bytes = serde_json::to_vec(&json!({
                "app":"vcp-mobile", "kind":"vcp-mobile-settings-backup", "version":2,
                "domains":{}, "preferences":preferences,
            })).unwrap();
            let parsed = parse_backup_bytes(&bytes).unwrap();
            assert_eq!(Value::Object(parsed.preferences), json!({"theme":null,"chat":null}));
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
        })).unwrap();
        let parsed = parse_backup_bytes(&bytes).unwrap();
        assert_eq!(Value::Object(parsed.preferences), json!({
            "theme":null,"chat":{"messageHapticsEnabled":"invalid"}
        }));
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
}
