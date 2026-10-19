//! The only reader of the former settings/global representation.
//!
//! The marker, domain rows, original archive, maintenance state and removal of
//! global share one SQLite transaction. Once committed, runtime never reads or
//! writes the old representation again.

use super::schema::{domain_defaults, set_value_at_path};
use super::types::{ConfigDomain, SYNC_LOG_LEVELS};
use crate::vcp_modules::db_manager::DbState;
use serde_json::Value;
use std::collections::BTreeMap;

const MIGRATION_KEY: &str = "global_domains_v1";

pub(super) async fn ensure_migrated(db: &DbState) -> Result<(), String> {
    let complete: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM config_metadata WHERE key = ?)")
            .bind(MIGRATION_KEY)
            .fetch_one(&db.pool)
            .await
            .map_err(|error| format!("读取配置搬运状态失败: {error}"))?;
    if complete {
        return Ok(());
    }

    let mut transaction = db.write_transaction("settings.migrate-domains").await?;
    // Another SettingsState/process may have completed migration while this
    // writer waited. SQLite's write transaction is the actual migration owner.
    let complete: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM config_metadata WHERE key = ?)")
            .bind(MIGRATION_KEY)
            .fetch_one(&mut *transaction)
            .await
            .map_err(|error| format!("读取配置搬运状态失败: {error}"))?;
    if complete {
        transaction.commit().await?;
        return Ok(());
    }

    let legacy: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'global'")
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|error| format!("读取原有配置失败: {error}"))?;
    let now = crate::vcp_modules::infra::utils::now_millis();
    let parsed = legacy
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    let domains = match legacy.as_ref() {
        None => default_domains(),
        Some(_) => match parsed.as_ref().filter(|value| value.is_object()) {
            Some(value) => split_legacy(value)?,
            // A malformed original is archived intact. Only domains that
            // actually owned legacy fields remain unavailable until repaired.
            // Newly introduced domains have no damaged predecessor to inherit.
            None => ConfigDomain::ALL
                .into_iter()
                .map(|domain| {
                    let value = if legacy_fields().iter().any(|(owner, _, _)| *owner == domain) {
                        Value::Null
                    } else {
                        domain_defaults(domain)
                    };
                    (domain, value)
                })
                .collect(),
        },
    };

    if let Some(original) = legacy.as_ref() {
        sqlx::query(
            "INSERT INTO config_recovery (domain, schema_version, revision, value, reason, created_at) \
             VALUES ('legacy.global', 0, 0, ?, 'global-migration', ?)",
        )
        .bind(original)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| format!("归档原有配置失败: {error}"))?;
    }
    if let Some(last_maintenance) = parsed
        .as_ref()
        .and_then(|value| value.get("lastAutomaticMaintenance"))
        .filter(|value| value.as_i64().is_some())
    {
        sqlx::query(
            "INSERT OR IGNORE INTO settings (key, value, updated_at) \
             VALUES ('last_automatic_maintenance', ?, ?)",
        )
        .bind(last_maintenance.to_string())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| format!("搬运维护任务状态失败: {error}"))?;
    }
    for (domain, value) in domains {
        sqlx::query(
            "INSERT OR IGNORE INTO config_domains (domain, schema_version, revision, value, updated_at) \
             VALUES (?, ?, 1, ?, ?)",
        )
        .bind(domain.as_str())
        .bind(i64::from(domain.schema_version()))
        .bind(value.to_string())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| format!("搬运配置领域失败: {error}"))?;
    }
    sqlx::query("INSERT INTO config_metadata (key, value) VALUES (?, 'complete')")
        .bind(MIGRATION_KEY)
        .execute(&mut *transaction)
        .await
        .map_err(|error| format!("记录配置搬运完成状态失败: {error}"))?;
    sqlx::query("DELETE FROM settings WHERE key = 'global'")
        .execute(&mut *transaction)
        .await
        .map_err(|error| format!("移除已搬运配置失败: {error}"))?;
    transaction.commit().await
}

fn default_domains() -> BTreeMap<ConfigDomain, Value> {
    ConfigDomain::ALL
        .into_iter()
        .map(|domain| (domain, domain_defaults(domain)))
        .collect()
}

/// Pure name mapping shared by one-time database migration and v1 backup import.
/// Each boundary owns its own missing-value/default policy.
pub(super) fn legacy_fields() -> &'static [(ConfigDomain, &'static str, &'static str)] {
    use ConfigDomain::*;
    // Active field/default/validation definitions belong to schema.rs and types.
    const FIELDS: &[(ConfigDomain, &str, &str)] = &[
        (Identity, "userName", "userName"),
        (Server, "vcpServerUrl", "url"),
        (Server, "vcpApiKey", "apiKey"),
        (Server, "chatEndpointMode", "endpointMode"),
        (Server, "adminUsername", "adminUsername"),
        (Server, "adminPassword", "adminPassword"),
        (Server, "fileKey", "fileKey"),
        (Log, "vcpLogUrl", "url"),
        (Log, "vcpLogKey", "key"),
        (Sync, "syncServerUrl", "serverUrl"),
        (Sync, "syncHttpUrl", "httpUrl"),
        (Sync, "syncToken", "token"),
        (Sync, "syncLogLevel", "logLevel"),
        (Sync, "syncPrerenderEnabled", "prerenderEnabled"),
        (Distributed, "distributedEnabled", "enabled"),
        (Distributed, "distributedWsUrl", "wsUrl"),
        (Distributed, "distributedVcpKey", "vcpKey"),
        (Distributed, "distributedDeviceName", "deviceName"),
        (Conversation, "topicSummaryModel", "topicSummaryModel"),
        (Voice, "voiceApiBaseUrl", "connection.apiBaseUrl"),
        (Voice, "voiceApiKey", "connection.apiKey"),
        (Voice, "voiceAsrStream", "asr.stream"),
        (Voice, "voiceInputEncoding", "asr.inputEncoding"),
        (Voice, "voiceLanguage", "asr.language"),
        (Voice, "voiceTtsStream", "tts.stream"),
        (Voice, "voiceTtsVoice", "tts.voice"),
        (Voice, "voiceTtsMode", "tts.mode"),
        (Voice, "voiceTtsStylePrompt", "tts.stylePrompt"),
        (Voice, "voiceTtsCloneSampleId", "tts.cloneSampleId"),
        (Voice, "voiceAudioTagRules", "guidance.audioTagRules"),
        (Voice, "voiceAutoGuidance", "guidance.autoGuidance"),
        (Voice, "voiceGuidanceRules", "guidance.guidanceRules"),
        (Voice, "voiceVadSensitivity", "vad.sensitivity"),
        (Voice, "voiceVadEndSilenceMs", "vad.endSilenceMs"),
        (Voice, "voiceVadPlaybackOnsetMs", "vad.playbackOnsetMs"),
        (Navigation, "agentOrder", "agentOrder"),
        (Navigation, "groupOrder", "groupOrder"),
    ];
    FIELDS
}

fn split_legacy(legacy: &Value) -> Result<BTreeMap<ConfigDomain, Value>, String> {
    use ConfigDomain::*;
    let mut values = default_domains();
    for (domain, old_name, path) in legacy_fields() {
        if let (Some(value), Some(target)) = (legacy.get(*old_name), values.get_mut(domain)) {
            set_value_at_path(target, path, value.clone())?;
        }
    }
    if let Some(level) = legacy.get("syncLogLevel").and_then(Value::as_str) {
        // The former logger accepted case/whitespace and WARNING. Preserve that
        // meaning only at the migration boundary; new data uses canonical IDs.
        let normalized = level.trim().to_ascii_uppercase();
        let normalized = if normalized == "WARNING" {
            "WARN"
        } else {
            &normalized
        };
        if SYNC_LOG_LEVELS.contains(&normalized) {
            if let Some(sync) = values.get_mut(&Sync) {
                set_value_at_path(sync, "logLevel", Value::String(normalized.into()))?;
            }
        }
    }
    if legacy.get("chatEndpointMode").is_none() {
        // Preserve the actual historical endpoint behavior only while moving
        // existing data; new installations use the single Rust default.
        let endpoint = if legacy
            .get("enableVcpToolInjection")
            .and_then(Value::as_bool)
            == Some(false)
        {
            "standard"
        } else {
            "vcpTools"
        };
        if let Some(server) = values.get_mut(&Server) {
            set_value_at_path(server, "endpointMode", Value::String(endpoint.into()))?;
        }
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn legacy_log_levels_keep_existing_meaning_without_hiding_invalid_input() {
        for (old, canonical) in [
            (" debug ", "DEBUG"),
            ("WARNING", "WARN"),
            ("trace", "TRACE"),
        ] {
            let domains = split_legacy(&json!({"syncLogLevel":old})).unwrap();
            assert_eq!(domains[&ConfigDomain::Sync]["logLevel"], canonical);
        }
        let invalid = split_legacy(&json!({"syncLogLevel":" custom "})).unwrap();
        assert_eq!(invalid[&ConfigDomain::Sync]["logLevel"], " custom ");
    }

    #[test]
    fn database_migration_keeps_its_historical_missing_endpoint_policy() {
        let missing = split_legacy(&json!({})).unwrap();
        assert_eq!(missing[&ConfigDomain::Server]["endpointMode"], "vcpTools");
        let disabled = split_legacy(&json!({"enableVcpToolInjection":false})).unwrap();
        assert_eq!(disabled[&ConfigDomain::Server]["endpointMode"], "standard");
    }
}
