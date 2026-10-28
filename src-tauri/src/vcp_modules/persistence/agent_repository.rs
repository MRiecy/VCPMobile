//! Agent edits compare their actual baseline inside the shared SQLite write
//! transaction. A local voice edit cannot write back stale synchronized fields.

use super::db_manager::DbState;
use crate::vcp_modules::{
    agent_types::{AgentConfig, AgentVoiceConfig},
    settings_manager::VOICE_TTS_VOICES,
    sync_dto::AgentSyncDTO,
    sync_hash::HashAggregator,
};
use sqlx::{sqlite::SqliteRow, Row, Sqlite};

pub(crate) fn decode_voice_config(value: &str) -> AgentVoiceConfig {
    let defaults = serde_json::json!(AgentVoiceConfig::default());
    AgentVoiceConfig::from_stored(&crate::vcp_modules::settings_manager::schema::parse_config_json(value, &defaults))
}

fn stored_field<T>(row: &SqliteRow, column: &str, default: T, valid: impl FnOnce(&T) -> bool) -> Result<(T, bool), String>
where T: for<'r> sqlx::Decode<'r, Sqlite> + sqlx::Type<Sqlite> {
    match row.try_get::<Option<T>, _>(column) {
        Ok(Some(value)) if valid(&value) => Ok((value, false)),
        Ok(_) | Err(sqlx::Error::ColumnDecode { .. }) => Ok((default, true)),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) struct DecodedAgent {
    pub config: AgentConfig,
    repaired: bool,
    shared_repaired: bool,
}

/// A row's configuration leaves may recover; its identity and query failures
/// never do. Valid siblings retain their exact stored value.
pub(crate) fn decode_agent_row(row: &SqliteRow, agent_id: &str) -> Result<DecodedAgent, String> {
    let defaults = AgentConfig::default();
    let mut repaired = false;
    let mut shared_repaired = false;
    macro_rules! field {
        ($column:literal, $default:expr, $valid:expr, $shared:expr) => {{
            let (value, changed) = stored_field(row, $column, $default, $valid)?;
            repaired |= changed;
            shared_repaired |= changed && $shared;
            value
        }};
    }
    let name = field!("name", defaults.name, |_| true, true);
    let system_prompt = field!("system_prompt", defaults.system_prompt, |_| true, true);
    let mobile_system_prompt = field!("mobile_system_prompt", defaults.mobile_system_prompt, |_| true, false);
    let model = field!("model", defaults.model, |_| true, true);
    let temperature = field!("temperature", defaults.temperature, |value: &f64| value.is_finite() && (0.0..=2.0).contains(value), true);
    // The UI/consumer defines no narrower token range than the i32 contract.
    let context_token_limit = field!("context_token_limit", i64::from(defaults.context_token_limit), |value| i32::try_from(*value).is_ok(), true) as i32;
    let max_output_tokens = field!("max_output_tokens", i64::from(defaults.max_output_tokens), |value| i32::try_from(*value).is_ok(), true) as i32;
    let stream_output = field!("stream_output", i64::from(defaults.stream_output), |value| matches!(*value, 0 | 1), true) == 1;
    let use_temperature = field!("use_temperature", i64::from(defaults.use_temperature), |value| matches!(*value, 0 | 1), false) == 1;
    let (raw, raw_repaired) = stored_field(row, "voice_config", String::new(), |_| true)?;
    let voice = decode_voice_config(&raw);
    repaired |= raw_repaired || serde_json::from_str::<serde_json::Value>(&raw).ok().as_ref()
        != Some(&serde_json::json!(voice));
    Ok(DecodedAgent {
        config: AgentConfig {
            id: agent_id.to_owned(), name, system_prompt, mobile_system_prompt, model, temperature,
            context_token_limit, max_output_tokens, stream_output, use_temperature, voice,
            avatar_calculated_color: row.try_get::<Option<String>, _>("dominant_color").ok().flatten(),
            topics: Vec::new(),
        }, repaired, shared_repaired,
    })
}

async fn stored_agent_rows<'a, E>(executor: E, agent_id: Option<&str>) -> Result<Vec<SqliteRow>, String>
where E: sqlx::Executor<'a, Database = Sqlite> {
    sqlx::query("SELECT * FROM agents WHERE owner_type='agent' AND deleted_at IS NULL AND (? IS NULL OR agent_id=?)")
        .bind(agent_id).bind(agent_id).fetch_all(executor).await.map_err(|error| error.to_string())
}

pub(crate) async fn normalize_agent_configs<F>(db: &DbState, agent_id: Option<&str>, on_repaired: F) -> Result<bool, String>
where F: FnOnce() + Send + 'static {
    let rows = stored_agent_rows(&db.pool, agent_id).await?;
    let mut needed = false;
    for row in &rows { needed |= decode_agent_row(row, "")?.repaired; }
    if !needed { return Ok(false); }
    let db = db.clone();
    let agent_id = agent_id.map(str::to_owned);
    // Re-read in the real write owner so a late repair cannot overwrite newer
    // synchronized data; dropping the read caller cannot abandon a commit.
    tokio::spawn(async move {
        let mut tx = db.write_transaction("identity.agent.normalize").await?;
        let mut changed = false;
        for row in stored_agent_rows(&mut *tx, agent_id.as_deref()).await? {
            let id: String = row.try_get("agent_id").map_err(|error| error.to_string())?;
            let decoded = decode_agent_row(&row, &id)?;
            if !decoded.repaired { continue; }
            let config = decoded.config;
            let hash = HashAggregator::compute_agent_config_hash(&AgentSyncDTO::from(&config));
            sqlx::query("UPDATE agents SET name=?,system_prompt=?,mobile_system_prompt=?,model=?,temperature=?,
                context_token_limit=?,max_output_tokens=?,stream_output=?,use_temperature=?,voice_config=?,
                config_hash=CASE WHEN ? THEN ? ELSE config_hash END,
                updated_at=CASE WHEN ? THEN ? ELSE updated_at END
                WHERE owner_type='agent' AND agent_id=? AND deleted_at IS NULL")
                .bind(config.name).bind(config.system_prompt).bind(config.mobile_system_prompt).bind(config.model)
                .bind(config.temperature).bind(config.context_token_limit).bind(config.max_output_tokens)
                .bind(config.stream_output).bind(config.use_temperature)
                .bind(serde_json::to_string(&config.voice).map_err(|error| error.to_string())?)
                .bind(decoded.shared_repaired).bind(hash).bind(decoded.shared_repaired)
                .bind(crate::vcp_modules::infra::utils::now_millis()).bind(id)
                .execute(&mut *tx).await.map_err(|error| error.to_string())?;
            changed = true;
        }
        tx.commit().await.map_err(|error| error.to_string())?;
        if changed { on_repaired(); }
        Ok(changed)
    }).await.map_err(|error| format!("助手配置修复任务异常：{error}"))?
}

fn merge_agent_edit(
    current: &mut AgentConfig,
    draft: &AgentConfig,
    baseline: &AgentConfig,
) -> Result<(), String> {
    if draft.id.is_empty() || draft.id != baseline.id || draft.id != current.id {
        return Err("助手设置保存目标与编辑基准不一致".into());
    }
    let mut conflicts = Vec::new();
    macro_rules! merge {
        ($label:literal, $($field:ident).+) => {
            if draft.$($field).+ != baseline.$($field).+ {
                if current.$($field).+ != baseline.$($field).+
                    && current.$($field).+ != draft.$($field).+
                {
                    conflicts.push($label);
                } else {
                    current.$($field).+ = draft.$($field).+.clone();
                }
            }
        };
    }
    merge!("名称", name);
    merge!("移动端提示词", mobile_system_prompt);
    merge!("模型", model);
    merge!("温度", temperature);
    merge!("上下文 Token 上限", context_token_limit);
    merge!("最大输出 Token", max_output_tokens);
    merge!("流式输出", stream_output);
    merge!("发送温度参数", use_temperature);
    // Mode and its selected handle form one edit: merging their pieces can
    // combine two visits into a voice selection that neither editor chose.
    merge!("语音设置", voice);
    if !conflicts.is_empty() {
        return Err(format!(
            "以下助手设置已被其他操作修改：{}。请重新打开设置后重试。",
            conflicts.join("、")
        ));
    }
    if !VOICE_TTS_VOICES.contains(&current.voice.voice.as_str()) {
        return Err("请选择有效的预置音色".into());
    }
    Ok(())
}

/// Voice handles are device-local selections. Missing/deleted library entries
/// remain stored and are resolved as unavailable by the narration owner.
pub(crate) async fn save_agent_edit(
    db: &DbState,
    draft: &AgentConfig,
    baseline: &AgentConfig,
) -> Result<(), String> {
    let mut tx = db.write_transaction("identity.agent.save").await?;
    let row = sqlx::query(
        "SELECT name, system_prompt, mobile_system_prompt, model, temperature,
                context_token_limit, max_output_tokens, stream_output,
                use_temperature, voice_config
         FROM agents WHERE owner_type = 'agent' AND agent_id = ? AND deleted_at IS NULL",
    )
    .bind(&draft.id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "助手已不存在，无法保存设置".to_string())?;
    let mut current = decode_agent_row(&row, &draft.id)?.config;
    merge_agent_edit(&mut current, draft, baseline)?;
    let config_hash = HashAggregator::compute_agent_config_hash(&AgentSyncDTO::from(&current));
    let voice = serde_json::to_string(&current.voice).map_err(|error| error.to_string())?;
    // system_prompt and topics are never editable through this mobile form.
    // Shared updated_at advances only when the sync hash actually changes.
    sqlx::query(
        "UPDATE agents SET name = ?, system_prompt = ?, mobile_system_prompt = ?, model = ?, temperature = ?,
            context_token_limit = ?, max_output_tokens = ?, stream_output = ?,
            use_temperature = ?, voice_config = ?,
            updated_at = CASE WHEN config_hash IS NOT ? THEN ? ELSE updated_at END,
            config_hash = ?
         WHERE owner_type = 'agent' AND agent_id = ? AND deleted_at IS NULL",
    )
    .bind(&current.name)
    .bind(&current.system_prompt)
    .bind(&current.mobile_system_prompt)
    .bind(&current.model)
    .bind(current.temperature)
    .bind(current.context_token_limit)
    .bind(current.max_output_tokens)
    .bind(current.stream_output)
    .bind(current.use_temperature)
    .bind(voice)
    .bind(&config_hash)
    .bind(crate::vcp_modules::infra::utils::now_millis())
    .bind(&config_hash)
    .bind(&current.id)
    .execute(&mut *tx)
    .await
    .map_err(|error| error.to_string())?;
    tx.commit().await.map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcp_modules::agent_service::create_default_config;

    #[test]
    fn voice_recovers_only_invalid_typed_leaves_and_keeps_valid_neighbors() {
        let value = decode_voice_config(r#"{"enabled":false,"mode":"invalid","voice":"苏打","stream":false,"cloneSampleId":"../outside","unknown":true}"#);
        assert!(!value.enabled);
        assert!(!value.stream);
        assert_eq!(value.voice, "苏打");
        assert_eq!(value.mode, AgentVoiceConfig::default().mode);
        assert_eq!(value.clone_sample_id, "../outside");
        assert_eq!(decode_voice_config(r#"{"cloneSampleId":42}"#).clone_sample_id, AgentVoiceConfig::default().clone_sample_id);
        let value = decode_voice_config(r#"{"enabled":"false","mode":"clone","voice":1e400,"stream":false,"cloneSampleId":"0123456789abcdef0123456789abcdef"}"#);
        assert_eq!(value.enabled, AgentVoiceConfig::default().enabled);
        assert_eq!(value.voice, AgentVoiceConfig::default().voice);
        assert!(!value.stream);
        assert_eq!(value.mode, crate::vcp_modules::settings_manager::VoiceTtsMode::Clone);
        assert_eq!(value.clone_sample_id, "0123456789abcdef0123456789abcdef");
        for raw in ["broken", "null", "[]", "42"] { assert_eq!(decode_voice_config(raw), AgentVoiceConfig::default()); }
        let empty: AgentConfig = serde_json::from_str("{}").unwrap();
        let new = create_default_config("new");
        for config in [empty, new, AgentConfig::default()] {
            assert_eq!(config.temperature, 1.0);
            assert_eq!(config.max_output_tokens, 64000);
            assert!(!config.use_temperature);
        }
    }

    #[tokio::test]
    async fn sql_null_bad_types_and_out_of_range_integers_recover_without_coercion() {
        let db = database().await;
        let row = sqlx::query("SELECT NULL AS name,'valid prompt' AS system_prompt, X'0102' AS mobile_system_prompt,
            'chosen model' AS model,'0.7' AS temperature,2147483648 AS context_token_limit,
            12345 AS max_output_tokens,2 AS stream_output,0 AS use_temperature,42 AS voice_config")
            .fetch_one(&db.pool).await.unwrap();
        let decoded = decode_agent_row(&row, "a").unwrap();
        let config = decoded.config;
        assert!(decoded.repaired);
        assert_eq!(config.name, AgentConfig::default().name);
        assert_eq!(config.system_prompt, "valid prompt");
        assert!(config.mobile_system_prompt.is_empty());
        assert_eq!(config.model, "chosen model");
        assert_eq!(config.temperature, 1.0);
        assert_eq!(config.context_token_limit, AgentConfig::default().context_token_limit);
        assert_eq!(config.max_output_tokens, 12345);
        assert!(config.stream_output);
        assert!(!config.use_temperature);
        assert_eq!(config.voice, AgentVoiceConfig::default());
        let missing = sqlx::query("SELECT 'name' AS name").fetch_one(&db.pool).await.unwrap();
        assert!(decode_agent_row(&missing, "a").is_err());
    }

    #[tokio::test]
    async fn repairing_voice_persists_valid_siblings_without_touching_sync_metadata() {
        let db = database().await;
        seed(&db).await;
        let before: (String, i64) = sqlx::query_as("SELECT config_hash,updated_at FROM agents WHERE agent_id='a'")
            .fetch_one(&db.pool).await.unwrap();
        sqlx::query("UPDATE agents SET voice_config=? WHERE agent_id='a'")
            .bind(r#"{"enabled":false,"mode":"unknown","voice":"苏打","stream":false}"#)
            .execute(&db.pool).await.unwrap();
        assert!(normalize_agent_configs(&db, Some("a"), || {}).await.unwrap());
        assert!(!normalize_agent_configs(&db, Some("a"), || {}).await.unwrap());
        let row = sqlx::query("SELECT voice_config,config_hash,updated_at FROM agents WHERE agent_id='a'")
            .fetch_one(&db.pool).await.unwrap();
        let voice = decode_voice_config(row.get("voice_config"));
        assert!(!voice.enabled);
        assert!(!voice.stream);
        assert_eq!(voice.voice, "苏打");
        assert_eq!(row.get::<String, _>("config_hash"), before.0);
        assert_eq!(row.get::<i64, _>("updated_at"), before.1);
    }

    #[tokio::test]
    async fn malformed_stored_json_is_repaired_but_missing_database_schema_is_an_error() {
        let db = database().await;
        let baseline = seed(&db).await;
        // Simulate a damaged persisted value; normal writers retain the JSON
        // CHECK constraint and cannot introduce this malformed representation.
        sqlx::query("PRAGMA ignore_check_constraints=ON").execute(&db.pool).await.unwrap();
        sqlx::query("UPDATE agents SET voice_config='{broken' WHERE agent_id='a'").execute(&db.pool).await.unwrap();
        sqlx::query("PRAGMA ignore_check_constraints=OFF").execute(&db.pool).await.unwrap();
        assert!(normalize_agent_configs(&db, Some("a"), || {}).await.unwrap());
        let row = sqlx::query("SELECT * FROM agents WHERE agent_id='a'").fetch_one(&db.pool).await.unwrap();
        assert_eq!(serde_json::from_str::<AgentVoiceConfig>(row.get("voice_config")).unwrap(), AgentVoiceConfig::default());
        assert_eq!(row.get::<String, _>("model"), baseline.model);
        sqlx::query("DROP TABLE agents").execute(&db.pool).await.unwrap();
        assert!(normalize_agent_configs(&db, None, || {}).await.is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_the_waiter_does_not_drop_post_commit_cache_publication() {
        let db = database().await;
        seed(&db).await;
        let (entered, committed) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let (published, publication) = tokio::sync::oneshot::channel();
        let owner_db = db.clone();
        let waiter = tokio::spawn(async move {
            normalize_agent_configs(&owner_db, Some("a"), move || {
                entered.send(()).unwrap();
                released.recv().unwrap();
                published.send(()).unwrap();
            }).await
        });
        committed.await.unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        release.send(()).unwrap();
        publication.await.unwrap();
        let stored: String = sqlx::query_scalar("SELECT voice_config FROM agents WHERE agent_id='a'")
            .fetch_one(&db.pool).await.unwrap();
        assert_eq!(serde_json::from_str::<AgentVoiceConfig>(&stored).unwrap(), AgentVoiceConfig::default());
    }

    #[tokio::test]
    async fn repairing_shared_fields_rehashes_only_the_current_row_and_preserves_tombstones() {
        let db = database().await;
        let baseline = seed(&db).await;
        sqlx::query("UPDATE agents SET temperature=5, context_token_limit=2147483648, stream_output=-1, max_output_tokens=12345 WHERE agent_id='a'")
            .execute(&db.pool).await.unwrap();
        sqlx::query("INSERT INTO agents(owner_type,agent_id,name,model,temperature,updated_at,deleted_at) VALUES('agent','deleted','keep','model',5,1,1)")
            .execute(&db.pool).await.unwrap();
        assert!(normalize_agent_configs(&db, None, || {}).await.unwrap());
        let row = sqlx::query("SELECT * FROM agents WHERE agent_id='a'").fetch_one(&db.pool).await.unwrap();
        let canonical = decode_agent_row(&row, "a").unwrap().config;
        assert_eq!(canonical.name, baseline.name);
        assert_eq!(canonical.max_output_tokens, 12345);
        assert_eq!(canonical.temperature, 1.0);
        assert_eq!(canonical.context_token_limit, AgentConfig::default().context_token_limit);
        assert!(canonical.stream_output);
        assert_eq!(row.get::<String, _>("config_hash"), HashAggregator::compute_agent_config_hash(&AgentSyncDTO::from(&canonical)));
        assert!(row.get::<i64, _>("updated_at") > 91);
        let deleted: f64 = sqlx::query_scalar("SELECT temperature FROM agents WHERE agent_id='deleted'").fetch_one(&db.pool).await.unwrap();
        assert_eq!(deleted, 5.0);
        assert!(!normalize_agent_configs(&db, None, || {}).await.unwrap());
    }

    async fn database() -> DbState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1)
            .connect("sqlite::memory:").await.expect("database");
        for migration in [
            include_str!("../../../migrations/0100_baseline_v2.sql"),
            include_str!("../../../migrations/0103_config_domains.sql"),
            include_str!("../../../migrations/0105_agent_voice_config.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await.expect("migration");
        }
        DbState::new(pool, std::path::PathBuf::from("agent-voice-test.sqlite"))
    }

    async fn seed(db: &DbState) -> AgentConfig {
        let mut baseline = create_default_config("a");
        baseline.system_prompt = "private desktop prompt".into();
        let hash = HashAggregator::compute_agent_config_hash(&AgentSyncDTO::from(&baseline));
        sqlx::query(
            "INSERT INTO agents (owner_type, agent_id, name, system_prompt, model, temperature,
                context_token_limit, max_output_tokens, stream_output, use_temperature,
                config_hash, updated_at) VALUES ('agent', 'a', ?, ?, ?, ?, ?, ?, ?, ?, ?, 91)",
        )
        .bind(&baseline.name).bind(&baseline.system_prompt).bind(&baseline.model)
        .bind(baseline.temperature).bind(baseline.context_token_limit)
        .bind(baseline.max_output_tokens).bind(baseline.stream_output)
        .bind(baseline.use_temperature).bind(hash)
        .execute(&db.pool).await.expect("seed");
        // This is the form's actual IPC view; it never receives the shared prompt.
        baseline.system_prompt.clear();
        baseline
    }

    #[tokio::test]
    async fn voice_migration_preserves_existing_agents_with_independent_defaults() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1)
            .connect("sqlite::memory:").await.expect("database");
        sqlx::raw_sql(include_str!("../../../migrations/0100_baseline_v2.sql"))
            .execute(&pool).await.expect("existing schema");
        sqlx::query("INSERT INTO agents(owner_type,agent_id,name,model,updated_at) VALUES('agent','existing','Existing','model',91)")
            .execute(&pool).await.expect("existing agent");
        sqlx::raw_sql(include_str!("../../../migrations/0105_agent_voice_config.sql"))
            .execute(&pool).await.expect("upgrade");
        let row = sqlx::query("SELECT name,updated_at,voice_config FROM agents WHERE agent_id='existing'")
            .fetch_one(&pool).await.expect("upgraded row");
        assert_eq!(row.get::<String, _>("name"), "Existing");
        assert_eq!(row.get::<i64, _>("updated_at"), 91);
        let voice = decode_voice_config(row.get("voice_config"));
        assert_eq!(voice, AgentVoiceConfig::default());
        assert!(voice.enabled);
        assert_eq!(voice.mode, crate::vcp_modules::settings_manager::VoiceTtsMode::Builtin);
        assert_eq!(voice.voice, "冰糖");
        assert!(voice.stream);
        let saved = decode_voice_config(r#"{"enabled":false,"voice":"苏打","stream":false}"#);
        assert!(!saved.enabled);
        assert_eq!(saved.voice, "苏打");
        assert!(!saved.stream);
        assert!(sqlx::query("UPDATE agents SET voice_config='invalid-json'")
            .execute(&pool).await.is_err());
    }

    #[tokio::test]
    async fn voice_edit_preserves_a_later_shared_snapshot_and_sync_version() {
        let db = database().await;
        let baseline = seed(&db).await;
        let mut remote = baseline.clone();
        remote.name = "remote rename".into();
        remote.model = "remote model".into();
        remote.system_prompt = "remote private prompt".into();
        let hash = HashAggregator::compute_agent_config_hash(&AgentSyncDTO::from(&remote));
        sqlx::query("UPDATE agents SET name=?, model=?, system_prompt=?, config_hash=?, updated_at=92 WHERE agent_id='a'")
            .bind(&remote.name).bind(&remote.model).bind(&remote.system_prompt).bind(&hash)
            .execute(&db.pool).await.expect("remote edit");
        let mut draft = baseline.clone();
        draft.voice.enabled = true;
        draft.voice.voice = "苏打".into();
        save_agent_edit(&db, &draft, &baseline).await.expect("local edit");
        let row = sqlx::query("SELECT name,model,system_prompt,config_hash,updated_at,voice_config FROM agents WHERE agent_id='a'")
            .fetch_one(&db.pool).await.expect("stored");
        assert_eq!(row.get::<String, _>("name"), remote.name);
        assert_eq!(row.get::<String, _>("model"), remote.model);
        assert_eq!(row.get::<String, _>("system_prompt"), remote.system_prompt);
        assert_eq!(row.get::<String, _>("config_hash"), hash);
        assert_eq!(row.get::<i64, _>("updated_at"), 92);
        assert_eq!(decode_voice_config(row.get("voice_config")), draft.voice);
    }

    #[tokio::test]
    async fn conflicting_edits_are_atomic_and_a_deleted_agent_is_not_recreated() {
        let db = database().await;
        let baseline = seed(&db).await;
        sqlx::query("UPDATE agents SET model='other editor' WHERE agent_id='a'")
            .execute(&db.pool).await.expect("concurrent edit");
        let mut draft = baseline.clone();
        draft.model = "my editor".into();
        draft.voice.enabled = false;
        assert!(save_agent_edit(&db, &draft, &baseline).await.expect_err("conflict").contains("模型"));
        let voice: String = sqlx::query_scalar("SELECT voice_config FROM agents WHERE agent_id='a'")
            .fetch_one(&db.pool).await.expect("voice unchanged");
        assert_eq!(decode_voice_config(&voice), baseline.voice);
        sqlx::query("UPDATE agents SET deleted_at=1 WHERE agent_id='a'")
            .execute(&db.pool).await.expect("tombstone");
        assert!(save_agent_edit(&db, &draft, &baseline).await.expect_err("deleted").contains("不存在"));
        let deleted: i64 = sqlx::query_scalar("SELECT deleted_at FROM agents WHERE agent_id='a'")
            .fetch_one(&db.pool).await.expect("tombstone retained");
        assert_eq!(deleted, 1);
    }

}
