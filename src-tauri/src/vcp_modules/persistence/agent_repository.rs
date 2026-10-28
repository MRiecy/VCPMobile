//! Agent edits compare their actual baseline inside the shared SQLite write
//! transaction. A local voice edit cannot write back stale synchronized fields.

use super::db_manager::DbState;
use crate::vcp_modules::{
    agent_types::{AgentConfig, AgentVoiceConfig},
    settings_manager::VOICE_TTS_VOICES,
    sync_dto::AgentSyncDTO,
    sync_hash::HashAggregator,
};
use sqlx::Row;

pub(crate) fn decode_voice_config(value: &str) -> Result<AgentVoiceConfig, String> {
    serde_json::from_str(value).map_err(|error| format!("助手音色配置读取失败：{error}"))
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
    let mut current = AgentConfig {
        id: draft.id.clone(),
        name: row.get("name"),
        system_prompt: row.get("system_prompt"),
        mobile_system_prompt: row.get("mobile_system_prompt"),
        model: row.get("model"),
        temperature: row.get("temperature"),
        context_token_limit: row.get("context_token_limit"),
        max_output_tokens: row.get("max_output_tokens"),
        stream_output: row.get::<i32, _>("stream_output") != 0,
        use_temperature: row.get::<i32, _>("use_temperature") != 0,
        voice: decode_voice_config(row.get("voice_config"))?,
        avatar_calculated_color: None,
        topics: Vec::new(),
    };
    merge_agent_edit(&mut current, draft, baseline)?;
    let config_hash = HashAggregator::compute_agent_config_hash(&AgentSyncDTO::from(&current));
    let voice = serde_json::to_string(&current.voice).map_err(|error| error.to_string())?;
    // system_prompt and topics are never editable through this mobile form.
    // Shared updated_at advances only when the sync hash actually changes.
    sqlx::query(
        "UPDATE agents SET name = ?, mobile_system_prompt = ?, model = ?, temperature = ?,
            context_token_limit = ?, max_output_tokens = ?, stream_output = ?,
            use_temperature = ?, voice_config = ?,
            updated_at = CASE WHEN config_hash IS NOT ? THEN ? ELSE updated_at END,
            config_hash = ?
         WHERE owner_type = 'agent' AND agent_id = ? AND deleted_at IS NULL",
    )
    .bind(&current.name)
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
    async fn voice_migration_preserves_existing_agents_with_inherited_defaults() {
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
        let voice = decode_voice_config(row.get("voice_config")).expect("default config");
        assert_eq!(voice, AgentVoiceConfig::default());
        assert!(!voice.enabled);
        assert_eq!(voice.mode, crate::vcp_modules::settings_manager::VoiceTtsMode::None);
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
        assert_eq!(decode_voice_config(row.get("voice_config")).expect("voice"), draft.voice);
    }

    #[tokio::test]
    async fn conflicting_edits_are_atomic_and_a_deleted_agent_is_not_recreated() {
        let db = database().await;
        let baseline = seed(&db).await;
        sqlx::query("UPDATE agents SET model='other editor' WHERE agent_id='a'")
            .execute(&db.pool).await.expect("concurrent edit");
        let mut draft = baseline.clone();
        draft.model = "my editor".into();
        draft.voice.enabled = true;
        assert!(save_agent_edit(&db, &draft, &baseline).await.expect_err("conflict").contains("模型"));
        let voice: String = sqlx::query_scalar("SELECT voice_config FROM agents WHERE agent_id='a'")
            .fetch_one(&db.pool).await.expect("voice unchanged");
        assert!(!decode_voice_config(&voice).expect("voice").enabled);
        sqlx::query("UPDATE agents SET deleted_at=1 WHERE agent_id='a'")
            .execute(&db.pool).await.expect("tombstone");
        assert!(save_agent_edit(&db, &draft, &baseline).await.expect_err("deleted").contains("不存在"));
        let deleted: i64 = sqlx::query_scalar("SELECT deleted_at FROM agents WHERE agent_id='a'")
            .fetch_one(&db.pool).await.expect("tombstone retained");
        assert_eq!(deleted, 1);
    }

}
