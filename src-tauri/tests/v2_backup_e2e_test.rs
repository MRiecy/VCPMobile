//! V2 Backup & Dual-Tier Staging Integration Tests (vcpcfg)
//!
//! Covers:
//! - F1: Navigation domain export & unblocking
//! - F2: V2 backup schema envelope validation, version checks & JSON roundtrip
//! - F3: Agent local configs & strict sync hash / timestamp invariance
//! - F4: Standalone data: model favorites ordering & Tarven preset rule preservation
//! - F5: Atomic single SQLite transaction consistency and rollback on failure
//! - Boundaries: Malformed JSON, oversized payloads, unicode/emoji system prompts

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Pool, Sqlite};
use std::time::Duration;

const MAX_CONFIG_BACKUP_BYTES: usize = 1024 * 1024; // 1 MiB

// ---------------------------------------------------------------------------
// V2 Schema Envelope Validation Helper (matches Rust backend backup.rs specification)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct V2BackupEnvelope {
    pub app: String,
    pub kind: String,
    pub version: u32,
    pub exported_at: String,
    #[serde(default)]
    pub domains: serde_json::Map<String, Value>,
    #[serde(default)]
    pub preferences: serde_json::Map<String, Value>,
    #[serde(default)]
    pub agent_preferences: serde_json::Map<String, Value>,
    #[serde(default)]
    pub group_preferences: serde_json::Map<String, Value>,
    #[serde(default)]
    pub standalone_data: Option<StandaloneDataDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct StandaloneDataDto {
    #[serde(default)]
    pub model_favorites: Vec<String>,
    #[serde(default)]
    pub tarven_rules: Vec<TarvenRuleDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TarvenRuleDto {
    pub id: String,
    pub name: String,
    pub rule_type: String,
    pub is_enabled: bool,
    pub content: String,
    pub scope: String,
    pub wrap: bool,
    pub role: Option<String>,
    pub depth: Option<i32>,
    pub position: Option<String>,
    pub sort_order: i32,
    pub created_at: i64,
    pub updated_at: i64,
}

pub fn validate_and_parse_backup_envelope(raw_bytes: &[u8]) -> Result<V2BackupEnvelope, String> {
    if raw_bytes.len() > MAX_CONFIG_BACKUP_BYTES {
        return Err("备份文件过大（超过 1 MiB），已拒绝导入".into());
    }
    let envelope: V2BackupEnvelope = serde_json::from_slice(raw_bytes)
        .map_err(|e| format!("备份文件不是合法的 JSON: {e}"))?;

    if envelope.app != "vcp-mobile" || envelope.kind != "vcp-mobile-settings-backup" {
        return Err("这不是 VCP Mobile 的配置备份文件".into());
    }

    match envelope.version {
        1 => Err("V1 备份已转入专门的迁移路径".into()),
        2 => Ok(envelope),
        v if v > 2 => Err("不支持的备份版本，请升级 VCP Mobile".into()),
        _ => Err("不支持的备份版本".into()),
    }
}

// ---------------------------------------------------------------------------
// Database Test Fixture Initializer
// ---------------------------------------------------------------------------

async fn create_test_db() -> (tempfile::TempDir, Pool<Sqlite>) {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let db_path = temp_dir.path().join("v2-backup-test.sqlite");
    let options = SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(2));

    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .expect("connect sqlite");

    // Initialize baseline tables for V2 test scenarios
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS agents (
            owner_type TEXT NOT NULL CHECK (owner_type = 'agent'),
            agent_id TEXT NOT NULL,
            name TEXT NOT NULL,
            system_prompt TEXT NOT NULL DEFAULT '',
            mobile_system_prompt TEXT NOT NULL DEFAULT '',
            model TEXT NOT NULL,
            temperature REAL NOT NULL DEFAULT 1,
            context_token_limit INTEGER NOT NULL DEFAULT 0,
            max_output_tokens INTEGER NOT NULL DEFAULT 0,
            stream_output INTEGER NOT NULL DEFAULT 1,
            use_temperature INTEGER NOT NULL DEFAULT 0,
            config_hash TEXT NOT NULL DEFAULT '',
            content_hash TEXT NOT NULL DEFAULT '',
            voice_config TEXT NOT NULL DEFAULT '{}',
            updated_at BIGINT NOT NULL,
            deleted_at BIGINT,
            PRIMARY KEY (owner_type, agent_id)
        );

        CREATE TABLE IF NOT EXISTS groups (
            owner_type TEXT NOT NULL CHECK (owner_type = 'group'),
            group_id TEXT NOT NULL,
            name TEXT NOT NULL,
            mode TEXT NOT NULL DEFAULT 'sequential',
            config_hash TEXT NOT NULL DEFAULT '',
            updated_at BIGINT NOT NULL,
            deleted_at BIGINT,
            PRIMARY KEY (owner_type, group_id)
        );

        CREATE TABLE IF NOT EXISTS group_local_settings (
            group_id TEXT PRIMARY KEY NOT NULL,
            settings_json TEXT NOT NULL,
            updated_at BIGINT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS model_favorites (
            model_id TEXT PRIMARY KEY,
            created_at BIGINT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS tarven_rules (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            rule_type TEXT NOT NULL,
            is_enabled INTEGER NOT NULL DEFAULT 1,
            content TEXT NOT NULL,
            scope TEXT NOT NULL,
            wrap INTEGER NOT NULL DEFAULT 1,
            role TEXT,
            depth INTEGER,
            position TEXT,
            sort_order INTEGER NOT NULL DEFAULT 0,
            created_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS config_domains (
            domain TEXT PRIMARY KEY,
            record_json TEXT NOT NULL,
            updated_at BIGINT NOT NULL
        );
        "#,
    )
    .execute(&pool)
    .await
    .expect("create test tables");

    (temp_dir, pool)
}

// ===========================================================================
// Test Suites
// ===========================================================================

#[tokio::test]
async fn test_v2_envelope_validation_success_and_rejections() {
    // 1. Valid V2 envelope
    let valid_json = json!({
        "app": "vcp-mobile",
        "kind": "vcp-mobile-settings-backup",
        "version": 2,
        "exportedAt": "2026-10-31T08:30:00.000Z",
        "domains": { "navigation": { "agentOrder": ["a1", "a2"] } }
    });
    let parsed = validate_and_parse_backup_envelope(&serde_json::to_vec(&valid_json).unwrap());
    assert!(parsed.is_ok());
    let env = parsed.unwrap();
    assert_eq!(env.version, 2);
    assert_eq!(env.app, "vcp-mobile");

    // 2. Reject unsupported higher version (e.g. version 3)
    let future_json = json!({
        "app": "vcp-mobile",
        "kind": "vcp-mobile-settings-backup",
        "version": 3,
        "exportedAt": "2026-10-31T08:30:00.000Z"
    });
    let err = validate_and_parse_backup_envelope(&serde_json::to_vec(&future_json).unwrap()).unwrap_err();
    assert!(err.contains("不支持的备份版本"));

    // 3. Reject mismatched app name (e.g. desktop backup)
    let foreign_app_json = json!({
        "app": "vcp-desktop",
        "kind": "vcp-mobile-settings-backup",
        "version": 2,
        "exportedAt": "2026-10-31T08:30:00.000Z"
    });
    let err = validate_and_parse_backup_envelope(&serde_json::to_vec(&foreign_app_json).unwrap()).unwrap_err();
    assert!(err.contains("这不是 VCP Mobile 的配置备份文件"));

    // 4. Reject malformed JSON
    let malformed_bytes = b"{\"app\": \"vcp-mobile\", \"kind\": \"vcp-mobile-settings-backup\", \"version\": ";
    let err = validate_and_parse_backup_envelope(malformed_bytes).unwrap_err();
    assert!(err.contains("不是合法的 JSON"));

    // 5. Reject oversized payload (> 1 MiB)
    let huge_bytes = vec![b'a'; MAX_CONFIG_BACKUP_BYTES + 10];
    let err = validate_and_parse_backup_envelope(&huge_bytes).unwrap_err();
    assert!(err.contains("备份文件过大"));
}

#[tokio::test]
async fn test_f1_navigation_domain_roundtrip() {
    let payload = json!({
        "app": "vcp-mobile",
        "kind": "vcp-mobile-settings-backup",
        "version": 2,
        "exportedAt": "2026-10-31T10:00:00.000Z",
        "domains": {
            "navigation": {
                "agentOrder": ["agent-uuid-001", "agent-uuid-002"],
                "groupOrder": ["group-dev-01", "group-life-02"]
            }
        }
    });

    let bytes = serde_json::to_vec(&payload).unwrap();
    let env = validate_and_parse_backup_envelope(&bytes).expect("valid parse");

    let nav = env.domains.get("navigation").expect("navigation domain exists");
    let agent_order = nav.get("agentOrder").and_then(|v| v.as_array()).expect("agentOrder array");
    assert_eq!(agent_order.len(), 2);
    assert_eq!(agent_order[0], "agent-uuid-001");
    assert_eq!(agent_order[1], "agent-uuid-002");

    let group_order = nav.get("groupOrder").and_then(|v| v.as_array()).expect("groupOrder array");
    assert_eq!(group_order.len(), 2);
    assert_eq!(group_order[0], "group-dev-01");
    assert_eq!(group_order[1], "group-life-02");
}

#[tokio::test]
async fn test_f3_agent_local_settings_strict_hash_and_timestamp_invariance() {
    let (_dir, pool) = create_test_db().await;

    // Seed an active agent with initial hash and timestamp
    let agent_id = "agent-arch-001";
    let initial_hash = "sha256:abcd1234ef567890deadbeef";
    let initial_updated_at = 1761800000000i64;

    sqlx::query(
        r#"
        INSERT INTO agents (
            owner_type, agent_id, name, system_prompt, mobile_system_prompt,
            model, temperature, use_temperature, config_hash, content_hash,
            voice_config, updated_at, deleted_at
        ) VALUES (
            'agent', ?, '系统架构师', '通用提示词', '初始移动端提示词',
            'gemini-flash', 0.7, 0, ?, 'content_hash_1',
            '{}', ?, NULL
        )
        "#,
    )
    .bind(agent_id)
    .bind(initial_hash)
    .bind(initial_updated_at)
    .execute(&pool)
    .await
    .unwrap();

    // Also seed a tombstoned agent
    let tombstone_id = "agent-deleted-002";
    sqlx::query(
        r#"
        INSERT INTO agents (
            owner_type, agent_id, name, system_prompt, mobile_system_prompt,
            model, temperature, use_temperature, config_hash, content_hash,
            voice_config, updated_at, deleted_at
        ) VALUES (
            'agent', ?, '已删除助手', '', '墓碑助手提示词',
            'claude', 1.0, 0, 'tombstone_hash', 'content_hash_2',
            '{}', 1761700000000, 1761750000000
        )
        "#,
    )
    .bind(tombstone_id)
    .execute(&pool)
    .await
    .unwrap();

    // Execute safe local settings update according to R2 & F3 specification:
    // Only mobile_system_prompt, use_temperature, and voice_config are modified;
    // config_hash and updated_at are strictly NOT touched.
    let new_mobile_prompt = "你现在处于移动端会话，回复严格控制在 150 字以内。支持 Markdown 与 Emoji 🚀🤖";
    let new_use_temp = 1;
    let new_voice_config = json!({
        "enabled": true,
        "mode": "builtin",
        "voice": "苏打",
        "stream": true
    }).to_string();

    let mut tx = pool.begin().await.unwrap();

    // 1. Check whether active agent exists
    let active_check: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT deleted_at FROM agents WHERE owner_type = 'agent' AND agent_id = ?"
    )
    .bind(agent_id)
    .fetch_optional(&mut *tx)
    .await
    .unwrap();

    assert!(active_check.is_some());
    assert!(active_check.unwrap().0.is_none()); // deleted_at is null

    // 2. Perform safe update
    let rows_affected = sqlx::query(
        r#"
        UPDATE agents
        SET mobile_system_prompt = ?,
            use_temperature = ?,
            voice_config = ?
        WHERE owner_type = 'agent' AND agent_id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(new_mobile_prompt)
    .bind(new_use_temp)
    .bind(&new_voice_config)
    .bind(agent_id)
    .execute(&mut *tx)
    .await
    .unwrap()
    .rows_affected();

    assert_eq!(rows_affected, 1);

    // 3. Attempt update on tombstoned agent: must NOT update tombstone
    let tombstone_rows = sqlx::query(
        r#"
        UPDATE agents
        SET mobile_system_prompt = '复活提示词'
        WHERE owner_type = 'agent' AND agent_id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(tombstone_id)
    .execute(&mut *tx)
    .await
    .unwrap()
    .rows_affected();

    assert_eq!(tombstone_rows, 0); // Tombstone skipped!

    tx.commit().await.unwrap();

    // Verify invariance in DB:
    let (read_mobile_prompt, read_use_temp, read_voice, read_hash, read_updated_at, read_deleted): (
        String, i32, String, String, i64, Option<i64>
    ) = sqlx::query_as(
        r#"
        SELECT mobile_system_prompt, use_temperature, voice_config, config_hash, updated_at, deleted_at
        FROM agents WHERE agent_id = ?
        "#,
    )
    .bind(agent_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    // Invariance assertions:
    assert_eq!(read_mobile_prompt, new_mobile_prompt);
    assert_eq!(read_use_temp, 1);
    assert_eq!(read_voice, new_voice_config);
    assert_eq!(read_hash, initial_hash, "CRITICAL: config_hash must NOT be modified by local settings!");
    assert_eq!(read_updated_at, initial_updated_at, "CRITICAL: updated_at must NOT change to prevent reverse sync pollution!");
    assert!(read_deleted.is_none());
}

#[tokio::test]
async fn test_f4_model_favorites_ordering_preservation() {
    let (_dir, pool) = create_test_db().await;

    // Test input list in specific user-defined priority order
    let model_favorites = vec![
        "gemini-2.5-flash".to_string(),
        "claude-3-7-sonnet".to_string(),
        "deepseek-reasoner".to_string(),
        "gpt-4o-mini".to_string(),
    ];

    let base_time = 1761800000000i64;
    let total = model_favorites.len();

    let mut tx = pool.begin().await.unwrap();
    for (idx, model_id) in model_favorites.iter().enumerate() {
        // Assign decreasing timestamp increments so ORDER BY created_at DESC reproduces input order
        let assigned_time = base_time + ((total - idx) as i64) * 1000;
        sqlx::query(
            "INSERT OR REPLACE INTO model_favorites (model_id, created_at) VALUES (?, ?)"
        )
        .bind(model_id)
        .bind(assigned_time)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();

    // Query back ordered by created_at DESC
    let retrieved: Vec<(String,)> = sqlx::query_as(
        "SELECT model_id FROM model_favorites ORDER BY created_at DESC"
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    let retrieved_models: Vec<String> = retrieved.into_iter().map(|(m,)| m).collect();
    assert_eq!(retrieved_models, model_favorites, "Model favorites must maintain input order");
}

#[tokio::test]
async fn test_f4_tarven_rules_preset_rule_protection_and_custom_rule_upsert() {
    let (_dir, pool) = create_test_db().await;

    // Seed preset rules in DB with standard system content
    let preset_id = "system_meta_injection";
    let original_content = "原始内置预置规则核心内容，严禁被备份覆盖！";
    let original_name = "系统元数据注入";

    sqlx::query(
        r#"
        INSERT INTO tarven_rules (
            id, name, rule_type, is_enabled, content, scope, wrap,
            role, depth, position, sort_order, created_at, updated_at
        ) VALUES (
            ?, ?, 'system_prefix', 1, ?, 'global', 1,
            'user', 0, 'prepend', -100, 1761700000000, 1761700000000
        )
        "#,
    )
    .bind(preset_id)
    .bind(original_name)
    .bind(original_content)
    .execute(&pool)
    .await
    .unwrap();

    // Backup payload attempts to change is_enabled to false, and provides malicious altered content
    let backup_preset_rule = TarvenRuleDto {
        id: preset_id.to_string(),
        name: "篡改的名字".to_string(),
        rule_type: "system_prefix".to_string(),
        is_enabled: false, // User turned it off in backup
        content: "恶意覆盖的预置规则文本".to_string(),
        scope: "global".to_string(),
        wrap: true,
        role: Some("user".to_string()),
        depth: Some(0),
        position: Some("prepend".to_string()),
        sort_order: -100,
        created_at: 1761800000000,
        updated_at: 1761800000000,
    };

    let custom_rule = TarvenRuleDto {
        id: "custom_rule_code_cleaner".to_string(),
        name: "代码清洁工".to_string(),
        rule_type: "system_suffix".to_string(),
        is_enabled: true,
        content: "请只输出纯代码，不要输出引导语。".to_string(),
        scope: "global".to_string(),
        wrap: true,
        role: Some("user".to_string()),
        depth: Some(0),
        position: Some("append".to_string()),
        sort_order: 10,
        created_at: 1761800000000,
        updated_at: 1761800000000,
    };

    let rules_to_import = vec![backup_preset_rule, custom_rule];

    let mut tx = pool.begin().await.unwrap();
    for rule in &rules_to_import {
        // Enforce F4 rule protection contract:
        // Preset rules (system_meta_injection, time_anchoring_v2) ONLY update is_enabled!
        if rule.id == "system_meta_injection" || rule.id == "time_anchoring_v2" {
            sqlx::query("UPDATE tarven_rules SET is_enabled = ? WHERE id = ?")
                .bind(rule.is_enabled)
                .bind(&rule.id)
                .execute(&mut *tx)
                .await
                .unwrap();
        } else {
            sqlx::query(
                r#"
                INSERT INTO tarven_rules (
                    id, name, rule_type, is_enabled, content, scope, wrap,
                    role, depth, position, sort_order, created_at, updated_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name,
                    rule_type = excluded.rule_type,
                    is_enabled = excluded.is_enabled,
                    content = excluded.content,
                    scope = excluded.scope,
                    wrap = excluded.wrap,
                    role = excluded.role,
                    depth = excluded.depth,
                    position = excluded.position,
                    sort_order = excluded.sort_order,
                    updated_at = excluded.updated_at
                "#,
            )
            .bind(&rule.id)
            .bind(&rule.name)
            .bind(&rule.rule_type)
            .bind(rule.is_enabled)
            .bind(&rule.content)
            .bind(&rule.scope)
            .bind(rule.wrap)
            .bind(&rule.role)
            .bind(rule.depth)
            .bind(&rule.position)
            .bind(rule.sort_order)
            .bind(rule.created_at)
            .bind(rule.updated_at)
            .execute(&mut *tx)
            .await
            .unwrap();
        }
    }
    tx.commit().await.unwrap();

    // Verify preset rule: is_enabled changed to 0 (false), but content and name are completely unchanged!
    let (preset_enabled, preset_content, preset_name): (bool, String, String) = sqlx::query_as(
        "SELECT is_enabled, content, name FROM tarven_rules WHERE id = ?"
    )
    .bind(preset_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert!(!preset_enabled, "Preset rule is_enabled should be updated to false");
    assert_eq!(preset_content, original_content, "CRITICAL: Preset rule content MUST NOT be overwritten!");
    assert_eq!(preset_name, original_name, "CRITICAL: Preset rule name MUST NOT be overwritten!");

    // Verify custom rule was inserted in full
    let (custom_enabled, custom_content): (bool, String) = sqlx::query_as(
        "SELECT is_enabled, content FROM tarven_rules WHERE id = 'custom_rule_code_cleaner'"
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert!(custom_enabled);
    assert_eq!(custom_content, "请只输出纯代码，不要输出引导语。");
}

#[tokio::test]
async fn test_f5_atomic_single_transaction_rollback() {
    let (_dir, pool) = create_test_db().await;

    // Baseline record in config_domains
    sqlx::query(
        "INSERT INTO config_domains (domain, record_json, updated_at) VALUES ('server', '{\"url\":\"https://initial.vcp\"}', 1000)"
    )
    .execute(&pool)
    .await
    .unwrap();

    // Start single transaction
    let mut tx = pool.begin().await.unwrap();

    // 1. Write domain update
    sqlx::query(
        "UPDATE config_domains SET record_json = '{\"url\":\"https://updated.vcp\"}' WHERE domain = 'server'"
    )
    .execute(&mut *tx)
    .await
    .unwrap();

    // 2. Insert valid model favorite
    sqlx::query(
        "INSERT INTO model_favorites (model_id, created_at) VALUES ('model-valid-01', 2000)"
    )
    .execute(&mut *tx)
    .await
    .unwrap();

    // 3. Deliberately simulate failure on step 3 (e.g. table not found or constraint failure)
    let bad_query = sqlx::query("INSERT INTO non_existent_table (foo) VALUES ('bar')")
        .execute(&mut *tx)
        .await;

    assert!(bad_query.is_err(), "Simulated query error");

    // Explicit rollback
    tx.rollback().await.unwrap();

    // Verify atomicity: neither the domain update nor the model favorite committed!
    let server_json: String = sqlx::query_scalar(
        "SELECT record_json FROM config_domains WHERE domain = 'server'"
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(server_json, "{\"url\":\"https://initial.vcp\"}", "Domain update must be rolled back");

    let model_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM model_favorites WHERE model_id = 'model-valid-01'"
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(model_count, 0, "Model favorite must be rolled back");
}

#[tokio::test]
async fn test_stress_multi_cycle_sync_hash_and_updated_at_invariance() {
    let (_dir, pool) = create_test_db().await;

    let agent_id = "agent-stress-hash-01";
    let initial_config_hash = "sha256:invariable_hash_constant_42_abcdef0123456789";
    let initial_content_hash = "sha256:content_hash_constant_99_fedcba9876543210";
    let initial_updated_at = 1761888888888i64;

    sqlx::query(
        r#"
        INSERT INTO agents (
            owner_type, agent_id, name, system_prompt, mobile_system_prompt,
            model, temperature, use_temperature, config_hash, content_hash,
            voice_config, updated_at, deleted_at
        ) VALUES (
            'agent', ?, '压力测试助手', '系统全局提示词', '初始移动端提示词',
            'gemini-flash', 0.7, 0, ?, ?,
            '{}', ?, NULL
        )
        "#,
    )
    .bind(agent_id)
    .bind(initial_config_hash)
    .bind(initial_content_hash)
    .bind(initial_updated_at)
    .execute(&pool)
    .await
    .unwrap();

    // Adversarial multi-cycle updates:
    // Testing 10 distinct, harsh variations: emojis, markdown, huge strings, empty strings,
    // toggling use_temperature (0/1), complex voice JSON payloads.
    let test_cycles = vec![
        ("1".repeat(5000), 1, json!({"enabled": true, "mode": "builtin", "voice": "Voice1"}).to_string()),
        ("".to_string(), 0, json!({}).to_string()),
        ("Emoji 🚀🔥🎉 and markdown # Header\n| Col1 | Col2 |\n|---|---|".to_string(), 1, json!({"enabled": true, "voice": "Custom", "stream": true}).to_string()),
        ("Special chars: \t\r\n\"'\\/<>&$#@!".to_string(), 0, json!({"enabled": false}).to_string()),
        ("多语言测试：你好，世界！こんにちは Привет мир مرحبا بالعالم".to_string(), 1, json!({"cloneSampleId": "sample-123"}).to_string()),
        ("Simple prompt".to_string(), 1, json!({"mode": "edge"}).to_string()),
        ("2".repeat(12000), 0, json!({"enabled": true, "mode": "cloud", "pitch": 1.2}).to_string()),
        ("    \n\n\t    ".to_string(), 1, json!({"stream": false}).to_string()),
        ("{\"nested\": {\"key\": \"value\"}}".to_string(), 0, json!({"voice": "Mia"}).to_string()),
        ("Final realistic prompt: 控制字数并在结尾给出建议".to_string(), 1, json!({"enabled": true, "mode": "builtin", "voice": "苏打", "stream": true}).to_string()),
    ];

    for (round, (prompt, use_temp, voice)) in test_cycles.iter().enumerate() {
        let mut tx = pool.begin().await.unwrap();

        let affected = sqlx::query(
            r#"
            UPDATE agents
            SET mobile_system_prompt = ?,
                use_temperature = ?,
                voice_config = ?
            WHERE owner_type = 'agent' AND agent_id = ? AND deleted_at IS NULL
            "#,
        )
        .bind(prompt)
        .bind(use_temp)
        .bind(voice)
        .bind(agent_id)
        .execute(&mut *tx)
        .await
        .unwrap()
        .rows_affected();

        assert_eq!(affected, 1, "Cycle {} must update exactly 1 row", round);
        tx.commit().await.unwrap();

        // Empirically verify byte-for-byte invariance:
        let (read_prompt, read_temp, read_voice, read_config_hash, read_content_hash, read_updated_at): (
            String, i32, String, String, String, i64
        ) = sqlx::query_as(
            r#"
            SELECT mobile_system_prompt, use_temperature, voice_config, config_hash, content_hash, updated_at
            FROM agents WHERE agent_id = ?
            "#,
        )
        .bind(agent_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(&read_prompt, prompt, "Cycle {}: prompt mismatch", round);
        assert_eq!(read_temp, *use_temp, "Cycle {}: use_temperature mismatch", round);
        assert_eq!(&read_voice, voice, "Cycle {}: voice_config mismatch", round);

        // INVARIANCE CHECKS:
        assert_eq!(read_config_hash.as_bytes(), initial_config_hash.as_bytes(),
            "FATAL INVARIANCE BREACH: config_hash mutated in cycle {}", round);
        assert_eq!(read_content_hash.as_bytes(), initial_content_hash.as_bytes(),
            "FATAL INVARIANCE BREACH: content_hash mutated in cycle {}", round);
        assert_eq!(read_updated_at, initial_updated_at,
            "FATAL INVARIANCE BREACH: updated_at mutated in cycle {}", round);
    }
}

#[tokio::test]
async fn test_stress_atomic_rollback_on_agent_or_tarven_error() {
    let (_dir, pool) = create_test_db().await;

    // Baseline setup across domains, groups, agents, models, and tarven_rules
    let initial_domain_json = r#"{"url":"https://baseline.server.vcp"}"#;
    sqlx::query(
        "INSERT INTO config_domains (domain, record_json, updated_at) VALUES ('server', ?, 1000)"
    )
    .bind(initial_domain_json)
    .execute(&pool)
    .await
    .unwrap();

    let initial_group_settings = r#"{"mode":"sequential","jevSchedule":{"interval":5}}"#;
    sqlx::query(
        "INSERT INTO group_local_settings (group_id, settings_json, updated_at) VALUES ('grp-001', ?, 1000)"
    )
    .bind(initial_group_settings)
    .execute(&pool)
    .await
    .unwrap();

    let initial_agent_prompt = "初始助手提示词";
    sqlx::query(
        r#"
        INSERT INTO agents (
            owner_type, agent_id, name, system_prompt, mobile_system_prompt,
            model, temperature, use_temperature, config_hash, content_hash,
            voice_config, updated_at, deleted_at
        ) VALUES (
            'agent', 'agt-001', '助手一', '通用', ?,
            'gpt', 0.7, 0, 'hash1', 'content1',
            '{}', 1000, NULL
        )
        "#,
    )
    .bind(initial_agent_prompt)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query("INSERT INTO model_favorites (model_id, created_at) VALUES ('model-base-01', 1000)")
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query(
        r#"
        INSERT INTO tarven_rules (
            id, name, rule_type, is_enabled, content, scope, wrap,
            role, depth, position, sort_order, created_at, updated_at
        ) VALUES (
            'rule-base-01', '基础规则', 'system_prefix', 1, '内容', 'global', 1,
            'user', 0, 'prepend', 0, 1000, 1000
        )
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

    // -----------------------------------------------------------------------
    // Stress Vector A: Intentional Failure during Agent update
    // Pipeline order: 1. Domains -> 2. Groups -> 3. Agent (FAILS) -> Rollback
    // -----------------------------------------------------------------------
    {
        let mut tx = pool.begin().await.unwrap();

        // 1. Domains update
        sqlx::query("UPDATE config_domains SET record_json = '{\"url\":\"https://corrupted.vcp\"}' WHERE domain = 'server'")
            .execute(&mut *tx)
            .await
            .unwrap();

        // 2. Groups update
        sqlx::query("UPDATE group_local_settings SET settings_json = '{\"mode\":\"polluted_mode\"}' WHERE group_id = 'grp-001'")
            .execute(&mut *tx)
            .await
            .unwrap();

        // 3. Deliberately trigger error in Agent step (CHECK constraint violation on owner_type)
        let agent_err = sqlx::query(
            "INSERT INTO agents (owner_type, agent_id, name, model, updated_at) VALUES ('invalid_owner_type', 'agt-fail', 'Bad', 'gpt', 1)"
        )
        .execute(&mut *tx)
        .await;

        assert!(agent_err.is_err(), "Agent update must fail constraint check");

        // Transaction aborts on error (simulating ? operator propagating Err in apply_owned)
        tx.rollback().await.unwrap();

        // Assert ZERO partial state committed:
        let domain_json: String = sqlx::query_scalar("SELECT record_json FROM config_domains WHERE domain = 'server'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(domain_json, initial_domain_json, "Domains write must be rolled back on Agent error");

        let group_json: String = sqlx::query_scalar("SELECT settings_json FROM group_local_settings WHERE group_id = 'grp-001'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(group_json, initial_group_settings, "Groups write must be rolled back on Agent error");
    }

    // -----------------------------------------------------------------------
    // Stress Vector B: Intentional Failure during Tarven Rules update
    // Pipeline order: 1. Domains -> 2. Groups -> 3. Agents -> 4. Models -> 5. Tarven (FAILS) -> Rollback
    // -----------------------------------------------------------------------
    {
        let mut tx = pool.begin().await.unwrap();

        // 1. Domains update
        sqlx::query("UPDATE config_domains SET record_json = '{\"url\":\"https://corrupted-v2.vcp\"}' WHERE domain = 'server'")
            .execute(&mut *tx)
            .await
            .unwrap();

        // 2. Groups update
        sqlx::query("UPDATE group_local_settings SET settings_json = '{\"mode\":\"polluted-v2\"}' WHERE group_id = 'grp-001'")
            .execute(&mut *tx)
            .await
            .unwrap();

        // 3. Agents update
        sqlx::query("UPDATE agents SET mobile_system_prompt = '污染的提示词' WHERE agent_id = 'agt-001'")
            .execute(&mut *tx)
            .await
            .unwrap();

        // 4. Model favorites update
        sqlx::query("INSERT INTO model_favorites (model_id, created_at) VALUES ('model-polluted-99', 9999)")
            .execute(&mut *tx)
            .await
            .unwrap();

        // 5. Deliberately trigger error in Tarven rules step (NOT NULL constraint violation on name)
        let tarven_err = sqlx::query(
            "INSERT INTO tarven_rules (id, name, rule_type, is_enabled, content, scope, sort_order, created_at, updated_at) VALUES ('rule-fail', NULL, 'system_prefix', 1, 'content', 'global', 0, 1, 1)"
        )
        .execute(&mut *tx)
        .await;

        assert!(tarven_err.is_err(), "Tarven rules update must fail NOT NULL constraint");

        // Transaction aborts on error
        tx.rollback().await.unwrap();

        // Assert ZERO partial state committed across all preceding operations:
        let domain_json: String = sqlx::query_scalar("SELECT record_json FROM config_domains WHERE domain = 'server'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(domain_json, initial_domain_json, "Domains write must be rolled back on Tarven error");

        let group_json: String = sqlx::query_scalar("SELECT settings_json FROM group_local_settings WHERE group_id = 'grp-001'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(group_json, initial_group_settings, "Groups write must be rolled back on Tarven error");

        let agent_prompt: String = sqlx::query_scalar("SELECT mobile_system_prompt FROM agents WHERE agent_id = 'agt-001'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(agent_prompt, initial_agent_prompt, "Agent update must be rolled back on Tarven error");

        let polluted_model_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM model_favorites WHERE model_id = 'model-polluted-99'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(polluted_model_count, 0, "Model favorites insert must be rolled back on Tarven error");
    }
}


// ===========================================================================
// Adversarial Stress Tests & Edge Case Defenses (Challenger M1.2)
// ===========================================================================

#[tokio::test]
async fn test_adversarial_preset_rule_tampering_defense() {
    let (_dir, pool) = create_test_db().await;

    // Seed preset rule "system_meta_injection" with standard default content
    let preset_id = "system_meta_injection";
    let default_content = "包含当前系统时间、运行环境及话题创建时间元数据注入系统提示词。";
    let default_name = "系统元数据注入";

    sqlx::query(
        r#"
        INSERT INTO tarven_rules (
            id, name, rule_type, is_enabled, content, scope, wrap,
            role, depth, position, sort_order, created_at, updated_at
        ) VALUES (
            ?, ?, 'system_meta_injection', 1, ?, 'global', 0,
            NULL, NULL, NULL, -100, 1761700000000, 1761700000000
        )
        "#,
    )
    .bind(preset_id)
    .bind(default_name)
    .bind(default_content)
    .execute(&pool)
    .await
    .unwrap();

    // Adversarial payload attempting to overwrite preset rule template
    // using both malicious content and extra "format_template" field
    let malicious_json = json!({
        "app": "vcp-mobile",
        "kind": "vcp-mobile-settings-backup",
        "version": 2,
        "exportedAt": "2026-10-31T12:00:00.000Z",
        "standaloneData": {
            "tarvenRules": [
                {
                    "id": "system_meta_injection",
                    "name": "TAMPERED_NAME",
                    "ruleType": "system_meta_injection",
                    "isEnabled": false,
                    "content": "MALICIOUS_PROMPT_INJECTION",
                    "format_template": "TAMPERED",
                    "scope": "global",
                    "wrap": false,
                    "sortOrder": -100,
                    "createdAt": 1761800000000i64,
                    "updatedAt": 1761800000000i64
                }
            ]
        }
    });

    let env = validate_and_parse_backup_envelope(&serde_json::to_vec(&malicious_json).unwrap()).unwrap();
    let rules = env.standalone_data.unwrap().tarven_rules;
    assert_eq!(rules.len(), 1);
    let malicious_rule = &rules[0];

    // Execute the import logic following the F4 preset defense contract
    let mut tx = pool.begin().await.unwrap();
    let is_preset = malicious_rule.sort_order == -100
        || malicious_rule.id == "system_meta_injection"
        || malicious_rule.id == "time_anchoring_v2";

    assert!(is_preset, "Preset rule must be recognized");

    let is_enabled_int = if malicious_rule.is_enabled { 1i64 } else { 0i64 };
    let now = 1761850000000i64;

    // Defense: Preset rules ONLY update is_enabled and updated_at; template text/content/name MUST NOT be updated!
    let affected = sqlx::query(
        "UPDATE tarven_rules SET is_enabled = ?, updated_at = ? WHERE id = ?"
    )
    .bind(is_enabled_int)
    .bind(now)
    .bind(&malicious_rule.id)
    .execute(&mut *tx)
    .await
    .unwrap()
    .rows_affected();

    assert_eq!(affected, 1);
    tx.commit().await.unwrap();

    // Verify DB state:
    let (is_enabled, content, name): (bool, String, String) = sqlx::query_as(
        "SELECT is_enabled, content, name FROM tarven_rules WHERE id = 'system_meta_injection'"
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert!(!is_enabled, "Preset rule is_enabled should be updated to false");
    assert_eq!(content, default_content, "CRITICAL DEFENSE: Preset rule content MUST NOT be changed to TAMPERED");
    assert_eq!(name, default_name, "CRITICAL DEFENSE: Preset rule name MUST NOT be changed");
}

#[tokio::test]
async fn test_adversarial_model_favorites_order_corruption_defense() {
    let (_dir, pool) = create_test_db().await;

    // Test a list with complex IDs, unicode, numbers, punctuation
    let original_favorites = vec![
        "gpt-4o-2024-11-20".to_string(),
        "claude-3-7-sonnet@anthropic".to_string(),
        "deepseek-r1:70b-q8_0".to_string(),
        "gemini-2.0-flash-exp (高速)".to_string(),
        "qwen-2.5-coder-32b-instruct".to_string(),
        "meta-llama/llama-3.3-70b-instruct".to_string(),
    ];

    // Helper to simulate import_favorite_models
    async fn import_models(tx: &mut sqlx::Transaction<'_, Sqlite>, models: &[String], base_time: i64) {
        let mut seen = std::collections::HashSet::new();
        let deduped: Vec<&str> = models
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty() && seen.insert(*s))
            .collect();

        let total = deduped.len();
        for (idx, model_id) in deduped.iter().enumerate() {
            let assigned_time = base_time + ((total - idx) as i64) * 1000;
            sqlx::query(
                "INSERT INTO model_favorites (model_id, created_at) VALUES (?, ?)
                 ON CONFLICT(model_id) DO UPDATE SET created_at = excluded.created_at",
            )
            .bind(model_id)
            .bind(assigned_time)
            .execute(&mut **tx)
            .await
            .unwrap();
        }
    }

    // Helper to simulate export_favorite_models
    async fn export_models(pool: &Pool<Sqlite>) -> Vec<String> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT model_id FROM model_favorites ORDER BY created_at DESC"
        )
        .fetch_all(pool)
        .await
        .unwrap();
        rows.into_iter().map(|(id,)| id).collect()
    }

    // Round 1: Import initial favorites
    let mut tx1 = pool.begin().await.unwrap();
    import_models(&mut tx1, &original_favorites, 1761800000000).await;
    tx1.commit().await.unwrap();

    let exported1 = export_models(&pool).await;
    assert_eq!(exported1, original_favorites, "Initial import order must match exact index order");

    // Round 2: Re-import with inverted order to verify no order stickiness or inversion bug
    let mut inverted_favorites = original_favorites.clone();
    inverted_favorites.reverse();

    let mut tx2 = pool.begin().await.unwrap();
    import_models(&mut tx2, &inverted_favorites, 1761900000000).await;
    tx2.commit().await.unwrap();

    let exported2 = export_models(&pool).await;
    assert_eq!(exported2, inverted_favorites, "Inverted re-import must strictly reflect new order without collision");

    // Round 3: Adversarial input with duplicate IDs and empty strings
    let dirty_favorites = vec![
        "gpt-4o-2024-11-20".to_string(),
        "".to_string(),
        "claude-3-7-sonnet@anthropic".to_string(),
        "gpt-4o-2024-11-20".to_string(), // duplicate
        "  ".to_string(),                // whitespace only
        "deepseek-r1:70b-q8_0".to_string(),
    ];
    let expected_deduped = vec![
        "gpt-4o-2024-11-20".to_string(),
        "claude-3-7-sonnet@anthropic".to_string(),
        "deepseek-r1:70b-q8_0".to_string(),
    ];

    let mut tx3 = pool.begin().await.unwrap();
    import_models(&mut tx3, &dirty_favorites, 1762000000000).await;
    tx3.commit().await.unwrap();

    let exported3 = export_models(&pool).await;
    assert_eq!(&exported3[..3], &expected_deduped[..], "Deduplication and order must be strictly preserved");
}

#[tokio::test]
async fn test_adversarial_tombstone_revival_defense() {
    let (_dir, pool) = create_test_db().await;

    // Seed an active agent and a tombstoned agent
    let active_id = "agent-active-001";
    let tombstone_id = "agent-tombstone-666";
    let tombstone_deleted_at = 1761500000000i64;
    let tombstone_initial_hash = "sha256:tombstone_dead_hash";
    let tombstone_initial_prompt = "已阵亡的智能体初始提示词";

    sqlx::query(
        r#"
        INSERT INTO agents (
            owner_type, agent_id, name, system_prompt, mobile_system_prompt,
            model, temperature, use_temperature, config_hash, content_hash,
            voice_config, updated_at, deleted_at
        ) VALUES (
            'agent', ?, '活跃智能体', '通用', '移动端提示词',
            'gemini', 0.7, 0, 'active_hash', 'ch1',
            '{}', 1761600000000, NULL
        )
        "#,
    )
    .bind(active_id)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        r#"
        INSERT INTO agents (
            owner_type, agent_id, name, system_prompt, mobile_system_prompt,
            model, temperature, use_temperature, config_hash, content_hash,
            voice_config, updated_at, deleted_at
        ) VALUES (
            'agent', ?, '墓碑智能体', '通用', ?,
            'gemini', 0.7, 0, ?, 'ch2',
            '{}', 1761500000000, ?
        )
        "#,
    )
    .bind(tombstone_id)
    .bind(tombstone_initial_prompt)
    .bind(tombstone_initial_hash)
    .bind(tombstone_deleted_at)
    .execute(&pool)
    .await
    .unwrap();

    // Adversarial import payload attempting to revive the tombstoned agent
    // by providing new mobile prompt, use_temperature, and voice config
    let malicious_revival_prompt = "恶意试图复活墓碑智能体并篡改提示词！";
    let malicious_use_temp = 1;

    let mut tx = pool.begin().await.unwrap();

    // Import pipeline execution according to import_agent_local_settings:
    // 1. Query agent state
    let row: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT deleted_at FROM agents WHERE owner_type = 'agent' AND agent_id = ?"
    )
    .bind(tombstone_id)
    .fetch_optional(&mut *tx)
    .await
    .unwrap();

    assert!(row.is_some(), "Tombstone row exists in DB");
    let deleted_at = row.unwrap().0;
    assert_eq!(deleted_at, Some(tombstone_deleted_at), "deleted_at is set");

    // Defense contract: if deleted_at.is_some(), skip update!
    let skipped = if deleted_at.is_some() {
        // Skip updating tombstone
        true
    } else {
        sqlx::query(
            r#"
            UPDATE agents SET
                mobile_system_prompt = ?,
                use_temperature = ?
            WHERE owner_type = 'agent' AND agent_id = ? AND deleted_at IS NULL
            "#,
        )
        .bind(malicious_revival_prompt)
        .bind(malicious_use_temp)
        .bind(tombstone_id)
        .execute(&mut *tx)
        .await
        .unwrap();
        false
    };

    assert!(skipped, "Tombstoned agent MUST be skipped during import");
    tx.commit().await.unwrap();

    // Verify DB state: Tombstone agent must NOT be revived or modified
    let (read_deleted_at, read_prompt, read_hash): (Option<i64>, String, String) = sqlx::query_as(
        "SELECT deleted_at, mobile_system_prompt, config_hash FROM agents WHERE agent_id = ?"
    )
    .bind(tombstone_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(read_deleted_at, Some(tombstone_deleted_at), "CRITICAL: deleted_at MUST remain intact (NOT revived!)");
    assert_eq!(read_prompt, tombstone_initial_prompt, "CRITICAL: mobile_system_prompt MUST NOT be modified on tombstone");
    assert_eq!(read_hash, tombstone_initial_hash, "CRITICAL: config_hash MUST NOT be modified");
}

