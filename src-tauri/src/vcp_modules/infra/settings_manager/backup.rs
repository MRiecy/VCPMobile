use super::schema;
use super::{read_config_snapshot, ConfigDomain, ConfigSnapshot, DomainHealth, SettingsState};
use serde::Serialize;
use serde_json::{Map, Value};
use tauri::{AppHandle, Runtime, State};
use tokio::io::AsyncReadExt;

const MAX_CONFIG_BACKUP_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedConfigBackup {
    domains: Map<String, Value>,
    preferences: Map<String, Value>,
    exported_at: String,
}

// Preferences keep their existing localStorage owners. Their shape is separate
// from the SQLite domain schema and they are not part of its transaction.
fn parse_backup_preferences(raw: Option<&Value>) -> Result<Map<String, Value>, String> {
    let mut preferences = Map::new();
    let Some(raw) = raw else {
        return Ok(preferences);
    };
    let source = raw
        .as_object()
        .ok_or_else(|| "备份字段 preferences 类型不正确".to_string())?;
    if source
        .keys()
        .any(|key| !matches!(key.as_str(), "theme" | "chat"))
    {
        return Err("备份 preferences 包含未知偏好领域".into());
    }
    if let Some(theme) = source.get("theme") {
        let source = theme
            .as_object()
            .ok_or_else(|| "备份字段 preferences.theme 类型不正确".to_string())?;
        if source
            .keys()
            .any(|key| !matches!(key.as_str(), "themeKey" | "mode"))
        {
            return Err("备份 preferences.theme 包含未知字段".into());
        }
        let mut theme = Map::new();
        if let Some(key) = source.get("themeKey") {
            if !key.is_string() {
                return Err("备份字段 preferences.theme.themeKey 类型不正确".into());
            }
            // Availability belongs to the bundled frontend theme registry.
            theme.insert("themeKey".into(), key.clone());
        }
        if let Some(mode) = source.get("mode") {
            if !matches!(mode.as_str(), Some("light" | "dark" | "system")) {
                return Err("备份字段 preferences.theme.mode 类型不正确".into());
            }
            theme.insert("mode".into(), mode.clone());
        }
        if !theme.is_empty() {
            preferences.insert("theme".into(), theme.into());
        }
    }
    if let Some(chat) = source.get("chat") {
        let source = chat
            .as_object()
            .ok_or_else(|| "备份字段 preferences.chat 类型不正确".to_string())?;
        if source.keys().any(|key| {
            !matches!(
                key.as_str(),
                "presentationMode"
                    | "smoothStreamingEnabled"
                    | "messageHapticsEnabled"
                    | "collapseToolsWhileStreaming"
            )
        }) {
            return Err("备份 preferences.chat 包含未知字段".into());
        }
        let mut chat = Map::new();
        if let Some(mode) = source.get("presentationMode") {
            if !matches!(mode.as_str(), Some("bubble" | "panel" | "immersive")) {
                return Err("备份字段 preferences.chat.presentationMode 类型不正确".into());
            }
            chat.insert("presentationMode".into(), mode.clone());
        }
        for key in [
            "smoothStreamingEnabled",
            "messageHapticsEnabled",
            "collapseToolsWhileStreaming",
        ] {
            if let Some(value) = source.get(key) {
                if !value.is_boolean() {
                    return Err(format!("备份字段 preferences.chat.{key} 类型不正确"));
                }
                chat.insert(key.into(), value.clone());
            }
        }
        if !chat.is_empty() {
            preferences.insert("chat".into(), chat.into());
        }
    }
    Ok(preferences)
}

fn parse_domain_fields(domain: ConfigDomain, raw: &Value, prefix: &str) -> Result<Value, String> {
    let source = raw.as_object().ok_or_else(|| {
        format!(
            "备份字段 {}{} 类型不正确",
            domain.as_str(),
            if prefix.is_empty() {
                String::new()
            } else {
                format!(".{prefix}")
            }
        )
    })?;
    let descriptors = schema::domain_fields(domain);
    let mut parsed = Map::new();
    for (key, value) in source {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        if let Some(field) = descriptors.iter().find(|field| field.path == path) {
            if !field.backup {
                return Err(format!(
                    "备份字段 {}.{} 不属于可恢复配置",
                    domain.as_str(),
                    field.path
                ));
            }
            let issues = schema::validate_field(domain, &path, value);
            if !issues.is_empty() {
                return Err(issues
                    .into_iter()
                    .map(|issue| format!("{}.{}: {}", domain.as_str(), issue.field, issue.message))
                    .collect::<Vec<_>>()
                    .join("\n"));
            }
            parsed.insert(key.clone(), value.clone());
        } else if descriptors
            .iter()
            .any(|field| field.path.starts_with(&format!("{path}.")))
        {
            let nested = parse_domain_fields(domain, value, &path)?;
            if nested.as_object().is_some_and(|object| !object.is_empty()) {
                parsed.insert(key.clone(), nested);
            }
        } else {
            // Never echo arbitrary unknown keys or values from an imported file.
            return Err(format!(
                "备份 {}{} 包含未知字段",
                domain.as_str(),
                if prefix.is_empty() {
                    String::new()
                } else {
                    format!(".{prefix}")
                }
            ));
        }
    }
    Ok(parsed.into())
}

/// One-way v1 import keeps only explicitly provided portable fields. Unlike
/// database migration, absent old fields never acquire defaults here.
fn parse_v1_backup(raw: &Value) -> Result<ParsedConfigBackup, String> {
    let source = raw["settings"]
        .as_object()
        .ok_or_else(|| "备份文件缺少 settings 配置内容".to_string())?;
    let mut domains = Map::new();
    for (domain, old_name, path) in super::migration::legacy_fields() {
        let Some(value) = source.get(*old_name).filter(|value| !value.is_null()) else {
            continue;
        };
        if !schema::domain_fields(*domain)
            .iter()
            .any(|field| field.path == *path && field.backup)
        {
            continue;
        }
        let issues = schema::validate_field(*domain, path, value);
        if !issues.is_empty() {
            return Err(issues
                .into_iter()
                .map(|issue| format!("{}.{}: {}", domain.as_str(), issue.field, issue.message))
                .collect::<Vec<_>>()
                .join("\n"));
        }
        let target = domains
            .entry(domain.as_str().to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        schema::set_value_at_path(target, path, value.clone())?;
    }
    if source.get("chatEndpointMode").is_none_or(Value::is_null) {
        if let Some(value) = source
            .get("enableVcpToolInjection")
            .filter(|value| !value.is_null())
        {
            let enabled = value
                .as_bool()
                .ok_or_else(|| "备份字段 enableVcpToolInjection 类型不正确".to_string())?;
            let server = domains
                .entry(ConfigDomain::Server.as_str().to_owned())
                .or_insert_with(|| Value::Object(Map::new()));
            schema::set_value_at_path(
                server,
                "endpointMode",
                if enabled { "vcpTools" } else { "standard" }.into(),
            )?;
        }
    }
    Ok(ParsedConfigBackup {
        domains,
        // v1 never owned UI preferences; even unexpected preferences are ignored.
        preferences: Map::new(),
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
    if raw.as_object().is_some_and(|object| {
        object.keys().any(|key| {
            !matches!(
                key.as_str(),
                "app" | "kind" | "version" | "exportedAt" | "domains" | "preferences"
            )
        })
    }) {
        return Err("备份文件包含未知顶层字段".into());
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
        let domain = ConfigDomain::ALL
            .into_iter()
            .find(|domain| domain.as_str() == name)
            .ok_or_else(|| "备份 domains 包含未知配置领域".to_string())?;
        let parsed = parse_domain_fields(domain, raw_domain, "")?;
        if parsed.as_object().is_some_and(|object| !object.is_empty()) {
            domains.insert(name.clone(), parsed);
        }
    }
    Ok(ParsedConfigBackup {
        domains,
        preferences: parse_backup_preferences(raw.get("preferences"))?,
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

/// Portable fields are projected from one committed snapshot. Recovered defaults
/// are not exported as if they were the user's saved configuration.
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
        if let Some(status) = snapshot.domains.get(&domain) {
            if matches!(status.health, DomainHealth::Unavailable) {
                return Err(format!("{} 配置需要修复后再导出", domain.as_str()));
            }
            for issue in &status.issues {
                if fields.iter().any(|field| {
                    issue.field.is_empty()
                        || field.path == issue.field
                        || field.path.starts_with(&format!("{}.", issue.field))
                }) {
                    return Err(format!(
                        "{}.{}: {}；修复后再导出",
                        domain.as_str(),
                        issue.field,
                        issue.message
                    ));
                }
            }
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

#[cfg(test)]
mod tests {
    use super::super::{DomainStatus, FieldIssue, Settings};
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
                            issues: Vec::new(),
                            health: DomainHealth::Ready,
                        },
                    )
                })
                .collect(),
        }
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
    fn export_refuses_recovered_portable_values_but_ignores_unrelated_local_issues() {
        let mut snapshot = snapshot();
        let sync = snapshot.domains.get_mut(&ConfigDomain::Sync).unwrap();
        sync.health = DomainHealth::Recovered;
        sync.issues.push(FieldIssue {
            domain: ConfigDomain::Sync,
            field: "logLevel".into(),
            code: "invalid".into(),
            message: "日志等级无效".into(),
        });
        assert!(backup_domains(&snapshot).is_ok());
        let voice = snapshot.domains.get_mut(&ConfigDomain::Voice).unwrap();
        voice.health = DomainHealth::Recovered;
        voice.issues.push(FieldIssue {
            domain: ConfigDomain::Voice,
            field: "tts.mode".into(),
            code: "invalid".into(),
            message: "音色模式无效".into(),
        });
        let error = backup_domains(&snapshot).unwrap_err();
        assert!(error.contains("voice.tts.mode"));
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
    fn clone_id_and_guidance_round_trip_without_audio_or_manual_field_lists() {
        let values = json!({"voice":{"tts":{"mode":"clone","cloneSampleId":"sample-id","stylePrompt":"自然表达"},"guidance":{"autoGuidance":false,"audioTagRules":"使用合适标签","guidanceRules":""}}});
        let parsed = parse_backup_bytes(&backup(values.clone())).unwrap();
        assert_eq!(Value::Object(parsed.domains), values);
    }

    #[test]
    fn invalid_known_fields_report_path_without_echoing_values() {
        for values in [
            json!({"voice":{"asr":{"inputEncoding":"private-token"}}}),
            json!({"voice":{"tts":{"mode":"private-token"}}}),
            json!({"voice":{"vad":{"endSilenceMs":"private-token"}}}),
            json!({"server":{"apiKey":42}}),
        ] {
            let error = parse_backup_bytes(&backup(values)).unwrap_err();
            assert!(!error.contains("private-token"));
            assert!(error.contains("voice.") || error.contains("server."));
        }
    }

    #[test]
    fn unknown_nonportable_and_wrong_shape_data_are_not_silently_discarded() {
        for values in [
            json!({"private-token":{}}),
            json!({"voice":{"private-token":true}}),
            json!({"voice":{"tts":null}}),
            json!({"navigation":{"agentOrder":[]}}),
            json!({"sync":{"logLevel":"DEBUG"}}),
        ] {
            let error = parse_backup_bytes(&backup(values)).unwrap_err();
            assert!(!error.contains("private-token"));
        }
    }

    #[test]
    fn partial_rule_import_defers_cross_field_checks_to_committed_state() {
        let parsed =
            parse_backup_bytes(&backup(json!({"voice":{"guidance":{"autoGuidance":true}}})))
                .unwrap();
        assert_eq!(
            parsed.domains["voice"],
            json!({"guidance":{"autoGuidance":true}})
        );
    }

    #[test]
    fn old_flat_v2_and_future_formats_are_not_a_second_active_protocol() {
        for version in [2, 3] {
            let bytes = serde_json::to_vec(&json!({"app":"vcp-mobile","kind":"vcp-mobile-settings-backup","version":version,"settings":{"vcpApiKey":"secret"}})).unwrap();
            assert!(parse_backup_bytes(&bytes).is_err());
        }
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
    fn v1_keeps_false_and_empty_values_while_skipping_null_and_unknown_fields() {
        let parsed = parse_backup_bytes(&v1_backup(json!({
            "vcpApiKey":"", "userName":null, "distributedEnabled":false,
            "voiceAsrStream":false, "voiceTtsStylePrompt":"", "unknownField":"ignored",
            "syncLogLevel":"not-imported", "agentOrder":["not-imported"],
        })))
        .unwrap();
        assert_eq!(
            Value::Object(parsed.domains),
            json!({
                "server":{"apiKey":""}, "distributed":{"enabled":false},
                "voice":{"asr":{"stream":false},"tts":{"stylePrompt":""}},
            })
        );
    }

    #[test]
    fn v1_legacy_endpoint_flag_is_used_only_without_a_present_mode() {
        for (enabled, expected) in [(true, "vcpTools"), (false, "standard")] {
            let parsed = parse_backup_bytes(&v1_backup(
                json!({"chatEndpointMode":null,"enableVcpToolInjection":enabled}),
            ))
            .unwrap();
            assert_eq!(parsed.domains["server"]["endpointMode"], expected);
        }
        let explicit = parse_backup_bytes(&v1_backup(
            json!({"chatEndpointMode":"raw","enableVcpToolInjection":"ignored"}),
        ))
        .unwrap();
        assert_eq!(explicit.domains["server"]["endpointMode"], "raw");
        let absent = parse_backup_bytes(&v1_backup(json!({}))).unwrap();
        assert!(absent.domains.is_empty());
        let null = parse_backup_bytes(&v1_backup(
            json!({"chatEndpointMode":null,"enableVcpToolInjection":null}),
        ))
        .unwrap();
        assert!(null.domains.is_empty());
    }

    #[test]
    fn v1_invalid_known_values_fail_without_echoing_private_input() {
        for fields in [
            json!({"chatEndpointMode":"private-token","enableVcpToolInjection":true}),
            json!({"chatEndpointMode":false,"enableVcpToolInjection":true}),
            json!({"vcpApiKey":{"secret":"private-token"}}),
            json!({"distributedEnabled":"private-token"}),
            json!({"voiceLanguage":"private-token"}),
            json!({"enableVcpToolInjection":"private-token"}),
        ] {
            let error = parse_backup_bytes(&v1_backup(fields)).unwrap_err();
            assert!(!error.contains("private-token"));
            assert!(!error.is_empty());
        }
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
