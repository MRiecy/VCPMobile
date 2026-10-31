//! Domain configuration storage and the commit/publication boundary.
//!
//! Reads and saves persist the current typed defaults for invalid fields while
//! retaining valid siblings. Field-base conflict checks still protect concurrent
//! edits. Database and cache publication have
//! one durable owner even when the caller disappears while COMMIT is pending.

use super::migration::ensure_migrated;
use super::schema::{
    domain_defaults, domain_fields, normalize_changes, normalize_domain, parse_config_json,
    set_value_at_path, settings_from_domains, value_at_path,
};
use super::types::{
    ConfigBackupImportResult, ConfigDomain, ConfigSaveResult, ConfigSnapshot, ConfigUpdate,
    DomainStatus, FieldIssue, Settings,
};
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group::group_types::GroupLocalSettingsPatch;
use crate::vcp_modules::infra::vcp_ws_endpoint::{
    compile_distributed_connection, compile_log_connection, resolve_distributed_connection,
    BaseKind, EndpointField,
};
use serde_json::Value;
use sqlx::{Row, SqliteConnection};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};

#[derive(Clone)]
pub struct SettingsState {
    inner: Arc<SettingsInner>,
}

struct SettingsInner {
    cache: RwLock<Option<Arc<LoadedConfig>>>,
    // Orders the single configuration publisher. Physical SQLite admission is
    // still exclusively owned by DbState's existing write coordinator.
    writer: Arc<Mutex<()>>,
    runtime_generation: AtomicU64,
    runtime_pending: AtomicU8,
    runtime_reconcile: Arc<Mutex<()>>,
}

struct LoadedConfig {
    snapshot: Arc<ConfigSnapshot>,
    settings: Arc<Settings>,
}

#[derive(Clone)]
struct DomainRecord {
    schema_version: u32,
    revision: i64,
    value: Value,
}

#[derive(Default, Clone, Debug)]
pub struct BackupImportPayload {
    pub group_preferences: BTreeMap<String, GroupLocalSettingsPatch>,
    pub agent_preferences: BTreeMap<String, crate::vcp_modules::agent::agent_types::AgentLocalSettingsPatch>,
    pub model_favorites: Vec<String>,
    pub tarven_rules: Vec<crate::vcp_modules::chat::context_injection::TarvenRule>,
}

impl BackupImportPayload {
    pub fn is_empty(&self) -> bool {
        self.group_preferences.is_empty()
            && self.agent_preferences.is_empty()
            && self.model_favorites.is_empty()
            && self.tarven_rules.is_empty()
    }
}

impl From<BTreeMap<String, GroupLocalSettingsPatch>> for BackupImportPayload {
    fn from(group_preferences: BTreeMap<String, GroupLocalSettingsPatch>) -> Self {
        Self {
            group_preferences,
            agent_preferences: BTreeMap::new(),
            model_favorites: Vec::new(),
            tarven_rules: Vec::new(),
        }
    }
}

enum ConfigMutation {
    Update(ConfigUpdate),
    Import(ConfigUpdate, BackupImportPayload),
}

impl Default for SettingsState {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsState {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(SettingsInner {
                cache: RwLock::new(None),
                writer: Arc::new(Mutex::new(())),
                runtime_generation: AtomicU64::new(0),
                runtime_pending: AtomicU8::new(0),
                runtime_reconcile: Arc::new(Mutex::new(())),
            }),
        }
    }

    pub(crate) async fn lock_runtime_reconcile(&self) -> OwnedMutexGuard<()> {
        self.inner.runtime_reconcile.clone().lock_owned().await
    }

    pub(crate) fn reserve_runtime_reconcile(&self, flags: u8) -> u64 {
        self.inner.runtime_pending.fetch_or(flags, Ordering::SeqCst);
        self.inner.runtime_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub(crate) fn take_runtime_changes(&self) -> u8 {
        self.inner.runtime_pending.swap(0, Ordering::SeqCst)
    }

    pub(crate) fn is_runtime_generation_current(&self, generation: u64) -> bool {
        self.inner.runtime_generation.load(Ordering::SeqCst) == generation
    }

    pub(crate) async fn read_snapshot(&self, db: &DbState) -> Result<ConfigSnapshot, String> {
        Ok(self.load(db).await?.snapshot.as_ref().clone())
    }

    pub(crate) async fn read_config(&self, db: &DbState) -> Result<Arc<Settings>, String> {
        Ok(self.load(db).await?.settings.clone())
    }

    async fn load(&self, db: &DbState) -> Result<Arc<LoadedConfig>, String> {
        if let Some(current) = self.inner.cache.read().await.as_ref() {
            return Ok(current.clone());
        }
        let guard = self.inner.writer.clone().lock_owned().await;
        if let Some(current) = self.inner.cache.read().await.as_ref() {
            return Ok(current.clone());
        }
        let state = self.clone();
        let db = db.clone();
        // Migration can commit too, so initial publication uses the same durable
        // ownership as ordinary saves. Only waiting for admission is cancellable.
        tokio::spawn(async move {
            let _guard = guard;
            ensure_migrated(&db).await?;
            let mut transaction = db.write_transaction("settings.normalize").await?;
            let mut records = read_records(&mut transaction).await?;
            normalize_records(&mut transaction, &mut records).await?;
            let loaded = Arc::new(project_records(&records)?);
            transaction.commit().await?;
            *state.inner.cache.write().await = Some(loaded.clone());
            Ok(loaded)
        })
        .await
        .map_err(|error| format!("配置加载任务失败: {error}"))?
    }

    pub(crate) async fn apply<F>(
        &self,
        db: &DbState,
        request: ConfigUpdate,
        on_committed: F,
    ) -> Result<ConfigSaveResult, String>
    where
        F: FnOnce(Arc<Settings>, Arc<ConfigSnapshot>) + Send + 'static,
    {
        self.mutate(db, ConfigMutation::Update(request), on_committed)
            .await
            .map(|response| response.result)
    }

    pub(crate) async fn import_backup<F, P>(
        &self,
        db: &DbState,
        request: ConfigUpdate,
        payload: P,
        on_committed: F,
    ) -> Result<ConfigBackupImportResult, String>
    where
        P: Into<BackupImportPayload> + Send + 'static,
        F: FnOnce(Arc<Settings>, Arc<ConfigSnapshot>) + Send + 'static,
    {
        self.mutate(db, ConfigMutation::Import(request, payload.into()), on_committed)
            .await
    }

    async fn mutate<F>(
        &self,
        db: &DbState,
        request: ConfigMutation,
        on_committed: F,
    ) -> Result<ConfigBackupImportResult, String>
    where
        F: FnOnce(Arc<Settings>, Arc<ConfigSnapshot>) + Send + 'static,
    {
        let guard = self.inner.writer.clone().lock_owned().await;
        let state = self.clone();
        let db = db.clone();
        tokio::spawn(async move { state.apply_owned(&db, request, guard, on_committed).await })
            .await
            .map_err(|error| format!("配置保存任务失败: {error}"))?
    }

    async fn apply_owned<F>(
        &self,
        db: &DbState,
        request: ConfigMutation,
        guard: OwnedMutexGuard<()>,
        on_committed: F,
    ) -> Result<ConfigBackupImportResult, String>
    where
        F: FnOnce(Arc<Settings>, Arc<ConfigSnapshot>),
    {
        ensure_migrated(db).await?;
        let mut transaction = db.write_transaction("settings.save-domains").await?;
        // Read under the same physical transaction as writes. Revision conflict
        // detection cannot rely only on an earlier in-memory snapshot.
        let previous_settings = self.inner.cache.read().await.as_ref().map(|loaded| loaded.settings.clone());
        let mut records = read_records(&mut transaction).await?;
        let normalized = normalize_records(&mut transaction, &mut records).await?;
        let current = Arc::new(project_records(&records)?);
        let (prepared, import_payload) = match request {
            ConfigMutation::Update(request) => (
                prepare_update(&records, &current.snapshot, request)?,
                BackupImportPayload::default(),
            ),
            ConfigMutation::Import(request, payload) => (
                prepare_update(&records, &current.snapshot, request)?,
                payload,
            ),
        };
        let writes = match prepared {
            PreparedUpdate::Rejected(issues) => {
                transaction.commit().await?;
                *self.inner.cache.write().await = Some(current.clone());
                drop(guard);
                if normalized {
                    on_committed(previous_settings.unwrap_or_else(|| current.settings.clone()), current.snapshot.clone());
                }
                return Ok(ConfigBackupImportResult {
                    result: ConfigSaveResult::Rejected { issues },
                    missing_groups: false,
                    missing_agents: false,
                });
            }
            PreparedUpdate::Conflict(conflicts) => {
                transaction.commit().await?;
                *self.inner.cache.write().await = Some(current.clone());
                drop(guard);
                if normalized {
                    on_committed(previous_settings.unwrap_or_else(|| current.settings.clone()), current.snapshot.clone());
                }
                return Ok(ConfigBackupImportResult {
                    result: ConfigSaveResult::Conflict {
                        snapshot: current.snapshot.as_ref().clone(),
                        conflicts,
                    },
                    missing_groups: false,
                    missing_agents: false,
                });
            }
            PreparedUpdate::Ready(writes) => writes,
        };
        if writes.is_empty() && import_payload.is_empty() {
            transaction.commit().await?;
            *self.inner.cache.write().await = Some(current.clone());
            drop(guard);
            if normalized {
                on_committed(previous_settings.unwrap_or_else(|| current.settings.clone()), current.snapshot.clone());
            }
            return Ok(ConfigBackupImportResult {
                result: ConfigSaveResult::Saved {
                    snapshot: current.snapshot.as_ref().clone(),
                },
                missing_groups: false,
                missing_agents: false,
            });
        }

        // 1. Domains: write config_domains
        let mut next_records = records;
        for write in writes {
            persist_record(&mut transaction, write.domain, &write.record).await?;
            next_records.insert(write.domain, write.record);
        }

        // 2. Group local settings
        let missing_groups = if !import_payload.group_preferences.is_empty() {
            crate::vcp_modules::group::group_service::import_group_local_settings(
                &mut transaction,
                &import_payload.group_preferences,
            )
            .await?
        } else {
            false
        };

        // 3. Agent local settings
        let missing_agents = if !import_payload.agent_preferences.is_empty() {
            crate::vcp_modules::agent::agent_service::import_agent_local_settings(
                &mut transaction,
                &import_payload.agent_preferences,
            )
            .await?
        } else {
            false
        };

        // 4. Model favorites
        if !import_payload.model_favorites.is_empty() {
            crate::vcp_modules::infra::model_manager::import_model_favorites(
                &mut transaction,
                &import_payload.model_favorites,
            )
            .await?;
        }

        // 5. Tarven rules
        if !import_payload.tarven_rules.is_empty() {
            crate::vcp_modules::chat::context_injection::import_tarven_rules(
                &mut transaction,
                &import_payload.tarven_rules,
            )
            .await?;
        }

        // Projection is fallible; finish it before committing so an internal
        // schema error cannot leave committed data without a publishable state.
        let next = Arc::new(project_records(&next_records)?);
        transaction.commit().await?;
        *self.inner.cache.write().await = Some(next.clone());
        drop(guard);
        // This synchronous hook only schedules domain side effects. It is inside
        // the durable owner and must run even if the IPC JoinHandle is dropped.
        on_committed(previous_settings.unwrap_or_else(|| current.settings.clone()), next.snapshot.clone());
        Ok(ConfigBackupImportResult {
            result: ConfigSaveResult::Saved {
                snapshot: next.snapshot.as_ref().clone(),
            },
            missing_groups,
            missing_agents,
        })
    }
}

fn storage_error(error: sqlx::Error) -> String {
    format!("配置数据库操作失败: {error}")
}

async fn persist_record(
    connection: &mut SqliteConnection,
    domain: ConfigDomain,
    record: &DomainRecord,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO config_domains (domain, schema_version, revision, value, updated_at)
         VALUES (?, ?, ?, ?, ?) ON CONFLICT(domain) DO UPDATE SET
         schema_version = excluded.schema_version, revision = excluded.revision,
         value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(domain.as_str())
    .bind(i64::from(record.schema_version))
    .bind(record.revision)
    .bind(record.value.to_string())
    .bind(crate::vcp_modules::infra::utils::now_millis())
    .execute(connection)
    .await
    .map_err(storage_error)?;
    Ok(())
}

async fn normalize_records(
    connection: &mut SqliteConnection,
    records: &mut BTreeMap<ConfigDomain, DomainRecord>,
) -> Result<bool, String> {
    let mut changed = false;
    for domain in ConfigDomain::ALL {
        let existing = records.get(&domain);
        let value = normalize_domain(
            domain,
            existing.map(|record| &record.value).unwrap_or(&Value::Null),
        );
        if existing.is_some_and(|record| {
            record.schema_version == domain.schema_version() && record.value == value
        }) {
            continue;
        }
        let revision = existing
            .map_or(0, |record| record.revision)
            .checked_add(1)
            .ok_or_else(|| "配置版本号已达到上限".to_string())?;
        let record = DomainRecord {
            schema_version: domain.schema_version(),
            revision,
            value,
        };
        persist_record(connection, domain, &record).await?;
        records.insert(domain, record);
        changed = true;
    }
    Ok(changed)
}

async fn read_records(
    connection: &mut SqliteConnection,
) -> Result<BTreeMap<ConfigDomain, DomainRecord>, String> {
    let rows = sqlx::query(
        "SELECT domain, schema_version, revision, value,
                typeof(schema_version) AS schema_type, typeof(revision) AS revision_type,
                typeof(value) AS value_type FROM config_domains",
    )
        .fetch_all(&mut *connection)
        .await
        .map_err(storage_error)?;
    let mut records = BTreeMap::new();
    for row in rows {
        let name: String = row.try_get("domain").map_err(storage_error)?;
        let Some(domain) = ConfigDomain::ALL
            .into_iter()
            .find(|domain| domain.as_str() == name)
        else {
            sqlx::query("DELETE FROM config_domains WHERE domain = ?")
                .bind(name)
                .execute(&mut *connection)
                .await
                .map_err(storage_error)?;
            continue;
        };
        // SQLite affinity is not a type constraint. A bad schema marker can be
        // repaired without discarding otherwise valid configuration values.
        let schema_version = if row.try_get::<String, _>("schema_type").map_err(storage_error)? == "integer" {
            let version: i64 = row.try_get("schema_version").map_err(storage_error)?;
            u32::try_from(version).unwrap_or(0)
        } else {
            0
        };
        // Revision is an optimistic-concurrency token, not a user preference.
        // Resetting it to a default could admit an old request with the same token.
        if row.try_get::<String, _>("revision_type").map_err(storage_error)? != "integer" {
            return Err(format!("配置领域 {domain} 的版本号类型无效，已保留原记录"));
        }
        let revision: i64 = row.try_get("revision").map_err(storage_error)?;
        if revision < 1 || revision == i64::MAX {
            return Err(format!("配置领域 {domain} 的版本号无效或已达到上限，已保留原记录"));
        }
        let value = if row.try_get::<String, _>("value_type").map_err(storage_error)? == "text" {
            match row.try_get::<String, _>("value") {
                Ok(raw) => parse_config_json(&raw, &domain_defaults(domain)),
                // Invalid UTF-8 is corrupted stored content, not an IO failure.
                Err(sqlx::Error::ColumnDecode { .. }) => Value::Null,
                Err(error) => return Err(storage_error(error)),
            }
        } else {
            Value::Null
        };
        records.insert(
            domain,
            DomainRecord {
                schema_version,
                revision,
                value,
            },
        );
    }
    Ok(records)
}

fn project_records(records: &BTreeMap<ConfigDomain, DomainRecord>) -> Result<LoadedConfig, String> {
    let mut values = BTreeMap::new();
    let mut domains = BTreeMap::new();
    for domain in ConfigDomain::ALL {
        let record = records.get(&domain);
        values.insert(
            domain,
            record
                .map(|record| record.value.clone())
                .unwrap_or_else(|| domain_defaults(domain)),
        );
        domains.insert(
            domain,
            DomainStatus {
                revision: record.map_or(0, |record| record.revision).to_string(),
                schema_version: domain.schema_version(),
            },
        );
    }
    let settings = settings_from_domains(&values)?;
    Ok(LoadedConfig {
        snapshot: Arc::new(ConfigSnapshot {
            data: settings.clone(),
            domains,
        }),
        settings: Arc::new(settings),
    })
}

struct DomainWrite {
    domain: ConfigDomain,
    record: DomainRecord,
}

enum PreparedUpdate {
    Ready(Vec<DomainWrite>),
    Rejected(Vec<FieldIssue>),
    Conflict(Vec<FieldIssue>),
}

#[derive(Default)]
struct ConnectionEdits {
    log: bool,
    distributed: bool,
    distributed_enabled: bool,
}

fn connection_issues(settings: &Settings, edits: &ConnectionEdits) -> Vec<FieldIssue> {
    let mut issues = Vec::new();
    if edits.log {
        if let Err(error) = compile_log_connection(&settings.log.url, &settings.log.key) {
            let field = match error.field {
                EndpointField::Url => "url",
                EndpointField::Key => "key",
            };
            issues.push(issue(ConfigDomain::Log, field, error.code, error.message));
        }
    }
    if edits.distributed {
        if let Err(error) = compile_distributed_connection(
            &settings.distributed.ws_url,
            &settings.distributed.vcp_key,
            BaseKind::Distributed,
        ) {
            let field = match error.field {
                EndpointField::Url => "wsUrl",
                EndpointField::Key => "vcpKey",
            };
            issues.push(issue(ConfigDomain::Distributed, field, error.code, error.message));
        }
    }
    if settings.distributed.enabled
        && (edits.log || edits.distributed || edits.distributed_enabled)
    {
        let input = resolve_distributed_connection(settings);
        if let Err(error) = compile_distributed_connection(input.url, input.key, input.base_kind()) {
            let (domain, field) = match error.field {
                EndpointField::Url if input.url_from_log => (ConfigDomain::Log, "url"),
                EndpointField::Url => (ConfigDomain::Distributed, "wsUrl"),
                EndpointField::Key if input.key_from_log => (ConfigDomain::Log, "key"),
                EndpointField::Key => (ConfigDomain::Distributed, "vcpKey"),
            };
            let issue = issue(domain, field, error.code, error.message);
            if !issues.contains(&issue) {
                issues.push(issue);
            }
        }
    }
    issues
}

fn prepare_update(
    records: &BTreeMap<ConfigDomain, DomainRecord>,
    snapshot: &ConfigSnapshot,
    request: ConfigUpdate,
) -> Result<PreparedUpdate, String> {
    let mut seen = BTreeSet::new();
    let mut issues = Vec::new();
    let mut conflicts = Vec::new();
    let mut candidates = BTreeMap::new();
    let mut connection_edits = ConnectionEdits::default();
    for mut patch in request.patches {
        let domain = patch.domain;
        if !seen.insert(domain) {
            issues.push(issue(
                domain,
                "",
                "duplicateDomain",
                "一次保存只能包含该领域的一份修改。",
            ));
            continue;
        }
        let descriptors = domain_fields(domain);
        let unknown: Vec<_> = patch
            .changes
            .keys()
            .filter(|path| !descriptors.iter().any(|field| field.path == **path))
            .collect();
        if !unknown.is_empty() {
            issues.extend(
                unknown
                    .into_iter()
                    .map(|path| issue(domain, path, "unknownField", "该配置字段不存在。")),
            );
            continue;
        }
        // Presence expresses the submitted group, even if normalization or an
        // idempotent replay later makes its values equal to the stored values.
        match domain {
            ConfigDomain::Log => {
                connection_edits.log = patch.changes.contains_key("url")
                    || patch.changes.contains_key("key");
            }
            ConfigDomain::Distributed => {
                connection_edits.distributed = patch.changes.contains_key("wsUrl")
                    || patch.changes.contains_key("vcpKey");
                connection_edits.distributed_enabled = patch.changes.contains_key("enabled");
            }
            _ => {}
        }
        patch.changes = normalize_changes(domain, &patch.changes);
        let Some(status) = snapshot.domains.get(&domain) else {
            return Err("配置领域状态缺失".into());
        };
        let existing = records.get(&domain);
        let current_value = existing
            .map(|record| record.value.clone())
            .unwrap_or_else(|| domain_defaults(domain));
        let revision_changed = patch.expected_revision != status.revision;
        if revision_changed {
            for (path, value) in &patch.changes {
                let current = value_at_path(&current_value, path);
                // A repeated request that already reached the desired value is
                // idempotent. Otherwise only the edited field's base matters.
                if current != Some(value) && current != patch.base_values.get(path) {
                    conflicts.push(issue(
                        domain,
                        path,
                        "fieldConflict",
                        "该选项已被其他操作修改，请确认后重新保存。",
                    ));
                }
            }
            if !conflicts.is_empty() {
                continue;
            }
        }
        let mut next = current_value.clone();
        for (path, value) in &patch.changes {
            set_value_at_path(&mut next, path, value.clone())?;
        }
        let next = normalize_domain(domain, &next);
        candidates.insert(domain, (current_value, next));
    }
    if !issues.is_empty() {
        return Ok(PreparedUpdate::Rejected(issues));
    }
    if !conflicts.is_empty() {
        return Ok(PreparedUpdate::Conflict(conflicts));
    }

    // Resolve cross-domain consumers only after every patch has contributed to
    // the candidate. Import order must not select a stale URL or credential.
    if connection_edits.log
        || connection_edits.distributed
        || connection_edits.distributed_enabled
    {
        let mut values: BTreeMap<_, _> = records
            .iter()
            .map(|(domain, record)| (*domain, record.value.clone()))
            .collect();
        for (domain, (_, next)) in &candidates {
            values.insert(*domain, next.clone());
        }
        let settings = settings_from_domains(&values)?;
        let issues = connection_issues(&settings, &connection_edits);
        if !issues.is_empty() {
            return Ok(PreparedUpdate::Rejected(issues));
        }
    }

    let mut writes = Vec::new();
    for (domain, (current_value, next)) in candidates {
        if current_value == next {
            continue;
        }
        let existing = records.get(&domain);
        let revision = existing
            .map_or(0, |record| record.revision)
            .checked_add(1)
            .ok_or_else(|| "配置版本号已达到上限".to_string())?;
        writes.push(DomainWrite {
            domain,
            record: DomainRecord {
                schema_version: domain.schema_version(),
                revision,
                value: next,
            },
        });
    }
    Ok(PreparedUpdate::Ready(writes))
}

fn issue(domain: ConfigDomain, field: &str, code: &str, message: &str) -> FieldIssue {
    FieldIssue {
        domain,
        field: field.into(),
        code: code.into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::DomainPatch;
    use super::*;
    use serde_json::{json, Map};
    use std::time::Duration;

    async fn database(legacy: Option<&str>) -> DbState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at BIGINT NOT NULL)")
            .execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!(
            "../../../../migrations/0103_config_domains.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        if let Some(legacy) = legacy {
            sqlx::query("INSERT INTO settings (key,value,updated_at) VALUES ('global',?,1)")
                .bind(legacy)
                .execute(&pool)
                .await
                .unwrap();
        }
        DbState::new(pool, std::path::PathBuf::new())
    }

    fn patch(snapshot: &ConfigSnapshot, domain: ConfigDomain, changes: Value) -> DomainPatch {
        let values = super::super::schema::settings_to_domains(&snapshot.data).unwrap();
        let changes = changes.as_object().unwrap().clone();
        let base_values = changes
            .keys()
            .map(|path| {
                (
                    path.clone(),
                    value_at_path(&values[&domain], path).unwrap().clone(),
                )
            })
            .collect();
        DomainPatch {
            domain,
            expected_revision: snapshot.domains[&domain].revision.clone(),
            changes,
            base_values,
        }
    }

    async fn save(
        state: &SettingsState,
        db: &DbState,
        patches: Vec<DomainPatch>,
    ) -> ConfigSaveResult {
        state
            .apply(db, ConfigUpdate { patches }, |_, _| {})
            .await
            .unwrap()
    }

    fn saved(result: ConfigSaveResult) -> ConfigSnapshot {
        match result {
            ConfigSaveResult::Saved { snapshot } => snapshot,
            other => panic!("expected committed save, got {other:?}"),
        }
    }

    async fn raw_domain(db: &DbState, domain: ConfigDomain) -> String {
        sqlx::query_scalar("SELECT value FROM config_domains WHERE domain = ?")
            .bind(domain.as_str())
            .fetch_one(&db.pool)
            .await
            .unwrap()
    }

    fn rejected_fields(result: ConfigSaveResult) -> Vec<(ConfigDomain, String)> {
        match result {
            ConfigSaveResult::Rejected { issues } => issues
                .into_iter()
                .map(|issue| (issue.domain, issue.field))
                .collect(),
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn websocket_rejection_preserves_all_domains_revisions_and_publication() {
        let db = database(None).await;
        let state = SettingsState::new();
        let initial = state.read_snapshot(&db).await.unwrap();
        // An unreachable host is valid configuration; saving does not probe DNS.
        let base = saved(save(&state, &db, vec![
            patch(&initial, ConfigDomain::Log, json!({
                "url":"wss://example.invalid:5876", "key":"retained-secret"
            })),
            patch(&initial, ConfigDomain::Distributed, json!({"enabled":true})),
        ]).await);
        let raw_before = raw_domain(&db, ConfigDomain::Log).await;
        let result = state.apply(&db, ConfigUpdate { patches: vec![
            patch(&base, ConfigDomain::Identity, json!({"userName":"must-not-commit"})),
            patch(&base, ConfigDomain::Log, json!({"url":"example.invalid"})),
        ] }, |_, _| panic!("rejected connection changes must not publish")).await.unwrap();
        assert!(!serde_json::to_string(&result).unwrap().contains("retained-secret"));
        // Log and the dependent distributed connection report the same field once.
        assert_eq!(rejected_fields(result), vec![(ConfigDomain::Log, "url".into())]);
        assert_eq!(state.read_snapshot(&db).await.unwrap(), base);
        assert_eq!(raw_domain(&db, ConfigDomain::Log).await, raw_before);
        assert_eq!(SettingsState::new().read_snapshot(&db).await.unwrap(), base);
    }

    #[tokio::test]
    async fn websocket_key_edits_validate_against_current_url_and_can_clear_configuration() {
        let db = database(Some(r#"{"vcpLogUrl":"example.invalid","vcpLogKey":"old-key"}"#)).await;
        let state = SettingsState::new();
        let base = state.read_snapshot(&db).await.unwrap();
        assert_eq!(rejected_fields(save(&state, &db, vec![patch(
            &base, ConfigDomain::Log, json!({"key":"new-key"}),
        )]).await), vec![(ConfigDomain::Log, "url".into())]);
        // Explicitly resubmitting an unchanged invalid value is still validated.
        assert_eq!(rejected_fields(save(&state, &db, vec![patch(
            &base, ConfigDomain::Log, json!({"url":"example.invalid"}),
        )]).await), vec![(ConfigDomain::Log, "url".into())]);
        let cleared = saved(save(&state, &db, vec![patch(
            &base, ConfigDomain::Log, json!({"key":""}),
        )]).await);
        assert_eq!(cleared.data.log.url, "example.invalid");
        assert!(cleared.data.log.key.is_empty());

        let valid = saved(save(&state, &db, vec![patch(
            &cleared, ConfigDomain::Log, json!({"url":"ws://example.invalid", "key":"valid-key"}),
        )]).await);
        assert_eq!(rejected_fields(save(&state, &db, vec![patch(
            &valid, ConfigDomain::Log, json!({"key":"invalid#secret"}),
        )]).await), vec![(ConfigDomain::Log, "key".into())]);
        assert_eq!(state.read_snapshot(&db).await.unwrap(), valid);
        let cleared_url = saved(save(&state, &db, vec![patch(
            &valid, ConfigDomain::Log, json!({"url":""}),
        )]).await);
        assert!(cleared_url.data.log.url.is_empty());
        assert_eq!(cleared_url.data.log.key, "valid-key");
    }

    #[tokio::test]
    async fn unrelated_edits_and_disabling_do_not_gate_on_legacy_websocket_values() {
        let db = database(Some(r#"{"vcpLogUrl":"example.invalid","vcpLogKey":"retained-key","distributedEnabled":true,"distributedWsUrl":"also-invalid","distributedVcpKey":"retained-node-key"}"#)).await;
        let state = SettingsState::new();
        let base = state.read_snapshot(&db).await.unwrap();
        let renamed = saved(save(&state, &db, vec![
            patch(&base, ConfigDomain::Identity, json!({"userName":"updated"})),
            patch(&base, ConfigDomain::Distributed, json!({"deviceName":"renamed"})),
        ]).await);
        assert_eq!(renamed.data.log, base.data.log);
        assert_eq!(renamed.data.distributed.ws_url, base.data.distributed.ws_url);
        assert_eq!(renamed.data.distributed.vcp_key, base.data.distributed.vcp_key);
        assert!(renamed.data.distributed.enabled);
        let disabled = saved(save(&state, &db, vec![patch(
            &renamed, ConfigDomain::Distributed, json!({"enabled":false}),
        )]).await);
        assert!(!disabled.data.distributed.enabled);
        assert_eq!(disabled.data.log, base.data.log);
        assert_eq!(rejected_fields(save(&state, &db, vec![patch(
            &disabled, ConfigDomain::Distributed, json!({"enabled":true}),
        )]).await), vec![(ConfigDomain::Log, "url".into())]);
    }

    #[tokio::test]
    async fn websocket_candidates_merge_across_domains_before_validation_in_either_order() {
        for reverse in [false, true] {
            let db = database(Some(r#"{"vcpLogKey":"old#invalid","distributedEnabled":true,"distributedWsUrl":"old-invalid"}"#)).await;
            let state = SettingsState::new();
            let base = state.read_snapshot(&db).await.unwrap();
            let mut patches = vec![
                patch(&base, ConfigDomain::Log, json!({"key":""})),
                patch(&base, ConfigDomain::Distributed, json!({
                    "wsUrl":"wss://example.invalid/proxy", "vcpKey":"new-key"
                })),
            ];
            if reverse { patches.reverse(); }
            let result = saved(save(&state, &db, patches).await);
            assert!(result.data.log.key.is_empty());
            assert_eq!(result.data.distributed.ws_url, "wss://example.invalid/proxy");
            assert_eq!(result.data.distributed.vcp_key, "new-key");
        }
    }

    #[tokio::test]
    async fn effective_distributed_issues_follow_each_selected_field_source() {
        for (log_url, log_key, node_url, node_key, domain, field) in [
            ("ws://example.invalid", "", "", "bad#key", ConfigDomain::Distributed, "vcpKey"),
            ("", "bad#key", "ws://example.invalid", "node-key", ConfigDomain::Log, "key"),
            ("invalid-log", "", "ws://example.invalid", "node-key", ConfigDomain::Log, "url"),
            ("", "log-key", "invalid-node", "", ConfigDomain::Distributed, "wsUrl"),
        ] {
            let legacy = json!({
                "vcpLogUrl":log_url, "vcpLogKey":log_key,
                "distributedWsUrl":node_url, "distributedVcpKey":node_key,
            }).to_string();
            let db = database(Some(&legacy)).await;
            let state = SettingsState::new();
            let base = state.read_snapshot(&db).await.unwrap();
            assert_eq!(rejected_fields(save(&state, &db, vec![patch(
                &base, ConfigDomain::Distributed, json!({"enabled":true}),
            )]).await), vec![(domain, field.into())]);
            assert_eq!(state.read_snapshot(&db).await.unwrap(), base);
        }
    }

    #[tokio::test]
    async fn clearing_log_fields_validates_the_newly_selected_distributed_source() {
        for (changes, node_url, node_key, field) in [
            (json!({"url":""}), "invalid-node", "node-key", "wsUrl"),
            (json!({"key":""}), "ws://node.invalid", "bad#key", "vcpKey"),
        ] {
            let legacy = json!({
                "vcpLogUrl":"ws://log.invalid", "vcpLogKey":"log-key",
                "distributedEnabled":true, "distributedWsUrl":node_url,
                "distributedVcpKey":node_key,
            }).to_string();
            let db = database(Some(&legacy)).await;
            let state = SettingsState::new();
            let base = state.read_snapshot(&db).await.unwrap();
            assert_eq!(rejected_fields(save(&state, &db, vec![patch(
                &base, ConfigDomain::Log, changes,
            )]).await), vec![(ConfigDomain::Distributed, field.into())]);
            assert_eq!(state.read_snapshot(&db).await.unwrap(), base);
        }
    }

    #[tokio::test]
    async fn explicit_distributed_edits_validate_even_when_disabled_or_overridden() {
        for enabled in [false, true] {
            let db = database(None).await;
            let state = SettingsState::new();
            let initial = state.read_snapshot(&db).await.unwrap();
            let base = saved(save(&state, &db, vec![
                patch(&initial, ConfigDomain::Log, json!({"url":"ws://example.invalid", "key":"log-key"})),
                patch(&initial, ConfigDomain::Distributed, json!({"enabled":enabled})),
            ]).await);
            assert_eq!(rejected_fields(save(&state, &db, vec![patch(
                &base, ConfigDomain::Distributed, json!({"wsUrl":"invalid-node", "vcpKey":"node-key"}),
            )]).await), vec![(ConfigDomain::Distributed, "wsUrl".into())]);
            assert_eq!(state.read_snapshot(&db).await.unwrap(), base);
        }
    }

    #[tokio::test]
    async fn websocket_conflicts_precede_semantic_validation() {
        let db = database(None).await;
        let state = SettingsState::new();
        let initial = state.read_snapshot(&db).await.unwrap();
        let base = saved(save(&state, &db, vec![patch(
            &initial, ConfigDomain::Log, json!({"url":"ws://first.invalid", "key":"valid-key"}),
        )]).await);
        let latest = saved(save(&state, &db, vec![patch(
            &base, ConfigDomain::Log, json!({"url":"ws://second.invalid"}),
        )]).await);
        let result = save(&state, &db, vec![
            patch(&base, ConfigDomain::Log, json!({"url":"invalid-url"})),
            patch(&base, ConfigDomain::Distributed, json!({"wsUrl":"invalid-node", "vcpKey":"node-key"})),
        ]).await;
        match result {
            ConfigSaveResult::Conflict { snapshot, conflicts } => {
                assert_eq!(snapshot, latest);
                assert_eq!(conflicts.len(), 1);
                assert_eq!(conflicts[0].domain, ConfigDomain::Log);
                assert_eq!(conflicts[0].field, "url");
            }
            other => panic!("expected conflict before URL validation, got {other:?}"),
        }
        assert_eq!(state.read_snapshot(&db).await.unwrap(), latest);
    }

    #[tokio::test]
    async fn invalid_websocket_backup_rejects_configuration_and_group_preferences_together() {
        use crate::vcp_modules::group::group_service;
        let db = database(None).await;
        sqlx::query("CREATE TABLE groups (owner_type TEXT NOT NULL, group_id TEXT NOT NULL, deleted_at BIGINT, PRIMARY KEY(owner_type,group_id))")
            .execute(&db.pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../../../migrations/0104_group_local_settings.sql"))
            .execute(&db.pool).await.unwrap();
        let state = SettingsState::new();
        let base = state.read_snapshot(&db).await.unwrap();
        let preferences = group_service::parse_group_local_settings_backup(&json!({
            "pending-group": { "chatMode": "jev" }
        })).unwrap();
        let result = state.import_backup(&db, ConfigUpdate { patches: vec![
            patch(&base, ConfigDomain::Log, json!({"url":"invalid-url", "key":"valid-key"})),
            patch(&base, ConfigDomain::Identity, json!({"userName":"must-not-import"})),
        ] }, preferences, |_, _| panic!("rejected import must not publish")).await.unwrap();
        assert!(!result.missing_groups);
        assert_eq!(rejected_fields(result.result), vec![(ConfigDomain::Log, "url".into())]);
        assert_eq!(state.read_snapshot(&db).await.unwrap(), base);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM group_local_settings")
            .fetch_one(&db.pool).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn migration_preserves_values_and_other_settings_then_never_reads_global_again() {
        let original = r#"{"userName":"旧用户","vcpApiKey":"fixture-secret","chatEndpointMode":"raw","voiceAsrStream":false,"voiceVadEndSilenceMs":777,"agentOrder":["b","a"],"lastAutomaticMaintenance":1234,"unknownFutureValue":17}"#;
        let db = database(Some(original)).await;
        sqlx::query("INSERT INTO settings(key,value,updated_at) VALUES ('cached_models','[1]',1)")
            .execute(&db.pool)
            .await
            .unwrap();
        let snapshot = SettingsState::new().read_snapshot(&db).await.unwrap();
        assert_eq!(snapshot.data.identity.user_name, "旧用户");
        assert_eq!(snapshot.data.server.api_key, "fixture-secret");
        assert_eq!(
            snapshot.data.server.endpoint_mode,
            super::super::types::ChatEndpointMode::Raw
        );
        assert!(!snapshot.data.voice.asr.stream);
        assert_eq!(snapshot.data.voice.vad.end_silence_ms, 777);
        assert_eq!(snapshot.data.navigation.agent_order, ["b", "a"]);
        let archived: String =
            sqlx::query_scalar("SELECT value FROM config_recovery WHERE domain='legacy.global'")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(archived, original);
        let maintenance: String =
            sqlx::query_scalar("SELECT value FROM settings WHERE key='last_automatic_maintenance'")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(maintenance, "1234");
        let cache: String =
            sqlx::query_scalar("SELECT value FROM settings WHERE key='cached_models'")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(cache, "[1]");
        let global_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM settings WHERE key='global'")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(global_count, 0);

        // Simulate an obsolete writer after the marker: there is no fallback or
        // dual read, including after a fresh SettingsState (process restart).
        sqlx::query("INSERT INTO settings(key,value,updated_at) VALUES ('global','{\"userName\":\"obsolete\"}',2)")
            .execute(&db.pool).await.unwrap();
        let restarted = SettingsState::new().read_snapshot(&db).await.unwrap();
        assert_eq!(restarted.data.identity.user_name, "旧用户");
        let archive_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM config_recovery WHERE domain='legacy.global'")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(archive_count, 1);
    }

    #[tokio::test]
    async fn interrupted_migration_rolls_back_every_part_and_restarts_once() {
        let original = r#"{"userName":"保留","lastAutomaticMaintenance":88}"#;
        let db = database(Some(original)).await;
        sqlx::raw_sql("CREATE TRIGGER fail_voice_migration BEFORE INSERT ON config_domains WHEN NEW.domain='voice' BEGIN SELECT RAISE(ABORT,'fixture failure'); END;")
            .execute(&db.pool).await.unwrap();
        assert!(SettingsState::new().read_snapshot(&db).await.is_err());
        for table in ["config_domains", "config_recovery", "config_metadata"] {
            let count: i64 =
                sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
                    .fetch_one(&db.pool)
                    .await
                    .unwrap();
            assert_eq!(count, 0, "failed migration must not retain {table}");
        }
        let retained: String = sqlx::query_scalar("SELECT value FROM settings WHERE key='global'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(retained, original);
        let moved_state_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM settings WHERE key='last_automatic_maintenance'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(moved_state_count, 0);
        sqlx::query("DROP TRIGGER fail_voice_migration")
            .execute(&db.pool)
            .await
            .unwrap();
        assert_eq!(
            SettingsState::new()
                .read_snapshot(&db)
                .await
                .unwrap()
                .data
                .identity
                .user_name,
            "保留"
        );
        assert_eq!(
            SettingsState::new()
                .read_snapshot(&db)
                .await
                .unwrap()
                .data
                .identity
                .user_name,
            "保留"
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM config_recovery")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn malformed_initial_configuration_is_persisted_as_current_defaults_and_editable() {
        let db = database(Some("{broken-original")).await;
        let state = SettingsState::new();
        let before = state.read_snapshot(&db).await.unwrap();
        assert_eq!(before.data, Settings::default());
        for domain in ConfigDomain::ALL {
            let stored: Value = serde_json::from_str(&raw_domain(&db, domain).await).unwrap();
            assert_eq!(stored, domain_defaults(domain));
            assert_eq!(before.domains[&domain].schema_version, domain.schema_version());
        }
        let after = saved(save(&state, &db, vec![patch(
            &before, ConfigDomain::Identity, json!({"userName":"新名字"}),
        )]).await);
        assert_eq!(after.data.identity.user_name, "新名字");
        assert_eq!(after.data.server, before.data.server);
    }

    #[tokio::test]
    async fn non_text_stored_domain_defaults_without_decoding_or_losing_other_domains() {
        let db = database(None).await;
        let initial = SettingsState::new().read_snapshot(&db).await.unwrap();
        let server_before = raw_domain(&db, ConfigDomain::Server).await;
        // Even valid UTF-8 JSON in a BLOB is the wrong persisted type.
        sqlx::query("UPDATE config_domains SET value=?,revision=7 WHERE domain='voice'")
            .bind(br#"{"tts":{"voice":"Mia"}}"#.as_slice())
            .execute(&db.pool).await.unwrap();
        let state = SettingsState::new();
        let repaired = state.read_snapshot(&db).await.unwrap();
        assert_eq!(repaired.data.voice, Settings::default().voice);
        assert_eq!(repaired.data.server, initial.data.server);
        assert_eq!(raw_domain(&db, ConfigDomain::Server).await, server_before);
        assert_eq!(repaired.domains[&ConfigDomain::Voice].revision, "8");
        let stored_type: String = sqlx::query_scalar("SELECT typeof(value) FROM config_domains WHERE domain='voice'")
            .fetch_one(&db.pool).await.unwrap();
        assert_eq!(stored_type, "text");
        assert_eq!(SettingsState::new().read_snapshot(&db).await.unwrap(), repaired);
    }

    #[tokio::test]
    async fn unreadable_utf8_content_defaults_only_its_domain() {
        let db = database(None).await;
        let initial = SettingsState::new().read_snapshot(&db).await.unwrap();
        sqlx::query("UPDATE config_domains SET value=CAST(x'FF' AS TEXT),revision=7 WHERE domain='voice'")
            .execute(&db.pool).await.unwrap();
        let repaired = SettingsState::new().read_snapshot(&db).await.unwrap();
        assert_eq!(repaired.data.voice, Settings::default().voice);
        assert_eq!(repaired.data.server, initial.data.server);
        assert_eq!(repaired.domains[&ConfigDomain::Voice].revision, "8");
    }

    #[tokio::test]
    async fn first_load_archives_binary_or_invalid_utf8_global_without_coercion() {
        for value in ["x'7B22757365724E616D65223A22426C6F62227D'", "CAST(x'FF' AS TEXT)"] {
            let db = database(Some("{}")).await;
            sqlx::query(sqlx::AssertSqlSafe(format!("UPDATE settings SET value={value} WHERE key='global'")))
                .execute(&db.pool).await.unwrap();
            let original: (String, String) = sqlx::query_as("SELECT typeof(value),hex(value) FROM settings WHERE key='global'")
                .fetch_one(&db.pool).await.unwrap();
            let first = SettingsState::new().read_snapshot(&db).await.unwrap();
            assert_eq!(first.data, Settings::default());
            let archived: (String, String) = sqlx::query_as("SELECT typeof(value),hex(value) FROM config_recovery WHERE domain='legacy.global'")
                .fetch_one(&db.pool).await.unwrap();
            assert_eq!(archived, original);
            assert_eq!(SettingsState::new().read_snapshot(&db).await.unwrap(), first);
            let archives: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM config_recovery WHERE domain='legacy.global'")
                .fetch_one(&db.pool).await.unwrap();
            assert_eq!(archives, 1);
        }
    }

    #[tokio::test]
    async fn first_load_overflow_only_defaults_its_legacy_leaf_and_archives_original_text() {
        let raw = r#"{"userName":"keep","voiceVadEndSilenceMs":1e400,"voiceTtsStream":false,"voiceTtsVoice":"Mia"}"#;
        let db = database(Some(raw)).await;
        let first = SettingsState::new().read_snapshot(&db).await.unwrap();
        assert_eq!(first.data.identity.user_name, "keep");
        assert_eq!(first.data.voice.vad.end_silence_ms, Settings::default().voice.vad.end_silence_ms);
        assert!(!first.data.voice.tts.stream);
        assert_eq!(first.data.voice.tts.voice, "Mia");
        let archived: String = sqlx::query_scalar("SELECT value FROM config_recovery WHERE domain='legacy.global'")
            .fetch_one(&db.pool).await.unwrap();
        assert_eq!(archived, raw);
        assert_eq!(SettingsState::new().read_snapshot(&db).await.unwrap(), first);
    }

    #[tokio::test]
    async fn overflowing_json_number_preserves_and_persists_valid_siblings() {
        let db = database(None).await;
        SettingsState::new().read_snapshot(&db).await.unwrap();
        sqlx::query("UPDATE config_domains SET value=?,revision=7 WHERE domain='jev'")
            .bind(r#"{"timeoutMs":1e400,"maxRetries":3,"openrouter":{"apiUrl":"https://provider.example/v1","apiKey":"keep-key","model":"keep-model"}}"#)
            .execute(&db.pool).await.unwrap();
        let repaired = SettingsState::new().read_snapshot(&db).await.unwrap();
        assert_eq!(repaired.data.jev.timeout_ms, Settings::default().jev.timeout_ms);
        assert_eq!(repaired.data.jev.max_retries, 3);
        assert_eq!(repaired.data.jev.openrouter.api_key, "keep-key");
        assert_eq!(repaired.data.jev.openrouter.model, "keep-model");
        assert_eq!(repaired.domains[&ConfigDomain::Jev].revision, "8");
        let stored: Value = serde_json::from_str(&raw_domain(&db, ConfigDomain::Jev).await).unwrap();
        assert_eq!(stored["timeoutMs"], Settings::default().jev.timeout_ms);
        assert_eq!(stored["openrouter"]["apiKey"], "keep-key");
        assert_eq!(SettingsState::new().read_snapshot(&db).await.unwrap(), repaired);
    }

    #[tokio::test]
    async fn malformed_schema_markers_repair_without_discarding_valid_values() {
        for marker in ["1.5", "'bad'", "4294967296"] {
            let db = database(None).await;
            SettingsState::new().read_snapshot(&db).await.unwrap();
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE config_domains SET schema_version={marker},value='{{\"userName\":\"Keep\"}}',revision=7 WHERE domain='identity'",
            ))).execute(&db.pool).await.unwrap();
            let repaired = SettingsState::new().read_snapshot(&db).await.unwrap();
            assert_eq!(repaired.data.identity.user_name, "Keep");
            assert_eq!(repaired.domains[&ConfigDomain::Identity].schema_version, ConfigDomain::Identity.schema_version());
            assert_eq!(repaired.domains[&ConfigDomain::Identity].revision, "8");
            assert_eq!(SettingsState::new().read_snapshot(&db).await.unwrap(), repaired);
        }
    }

    #[tokio::test]
    async fn malformed_or_exhausted_revisions_are_not_reset_into_reusable_tokens() {
        for revision in ["1.5", "'bad'", "9223372036854775807"] {
            let db = database(None).await;
            SettingsState::new().read_snapshot(&db).await.unwrap();
            let before = raw_domain(&db, ConfigDomain::Identity).await;
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE config_domains SET revision={revision} WHERE domain='identity'",
            ))).execute(&db.pool).await.unwrap();
            let original: String = sqlx::query_scalar("SELECT quote(revision) FROM config_domains WHERE domain='identity'")
                .fetch_one(&db.pool).await.unwrap();
            let error = SettingsState::new().read_snapshot(&db).await.err().unwrap();
            assert!(error.contains("版本号"));
            let after: String = sqlx::query_scalar("SELECT quote(revision) FROM config_domains WHERE domain='identity'")
                .fetch_one(&db.pool).await.unwrap();
            assert_eq!(after, original);
            assert_eq!(raw_domain(&db, ConfigDomain::Identity).await, before);
        }
    }

    #[tokio::test]
    async fn invalid_leaf_is_persisted_as_default_without_losing_valid_siblings() {
        let db = database(None).await;
        let initial = SettingsState::new().read_snapshot(&db).await.unwrap();
        let mut voice = json!(initial.data.voice);
        voice["tts"]["voice"] = json!("broken-voice");
        voice["guidance"]["rules"] = json!("保留");
        voice["connection"]["apiKey"] = json!("keep-key");
        voice["unknown"] = json!("discard");
        sqlx::query("UPDATE config_domains SET value=?,revision=7 WHERE domain='voice'")
            .bind(voice.to_string()).execute(&db.pool).await.unwrap();
        let state = SettingsState::new();
        let before = state.read_snapshot(&db).await.unwrap();
        assert_eq!(before.data.voice.tts.voice, Settings::default().voice.tts.voice);
        assert_eq!(before.data.voice.guidance.rules, "保留");
        assert_eq!(before.data.voice.connection.api_key, "keep-key");
        assert_eq!(before.domains[&ConfigDomain::Voice].revision, "8");
        let stored: Value = serde_json::from_str(&raw_domain(&db, ConfigDomain::Voice).await).unwrap();
        assert_eq!(stored["tts"]["voice"], Settings::default().voice.tts.voice);
        assert!(stored.get("unknown").is_none());
        let after = saved(save(&state, &db, vec![patch(
            &before, ConfigDomain::Voice, json!({"asr.language":"en"}),
        )]).await);
        assert_eq!(after.data.voice.guidance.rules, "保留");
        assert_eq!(after.data.voice.connection.api_key, "keep-key");
        assert_eq!(after.data.voice.asr.language, "en");
        assert_eq!(SettingsState::new().read_snapshot(&db).await.unwrap(), after);
    }

    #[tokio::test]
    async fn cold_load_normalizes_once_and_keeps_other_valid_saved_values() {
        for mode in [Some(json!("unavailable-mode")), Some(Value::Null), None] {
            let db = database(None).await;
            let initial = SettingsState::new().read_snapshot(&db).await.unwrap();
            let server_before = raw_domain(&db, ConfigDomain::Server).await;
            let mut voice = json!(initial.data.voice);
            voice["connection"]["apiKey"] = json!("retained-key");
            voice["tts"]["voice"] = json!("Mia");
            voice["tts"]["stream"] = json!(false);
            voice["guidance"]["rules"] = json!("保留风格规则");
            voice["tts"]["cloneSampleId"] = json!("retained-sample");
            voice["futureField"] = json!({"keep":true});
            match mode {
                Some(mode) => voice["tts"]["mode"] = mode,
                None => { voice["tts"].as_object_mut().unwrap().remove("mode"); }
            }
            sqlx::query("UPDATE config_domains SET value=?,revision=7 WHERE domain='voice'")
                .bind(voice.to_string()).execute(&db.pool).await.unwrap();
            let loaded = SettingsState::new().read_snapshot(&db).await.unwrap();
            assert_eq!(loaded.data.voice.tts.mode, super::super::VoiceTtsMode::Builtin);
            assert_eq!(loaded.domains[&ConfigDomain::Voice].revision, "8");
            voice["tts"]["mode"] = json!("builtin");
            voice.as_object_mut().unwrap().remove("futureField");
            let stored: Value = serde_json::from_str(&raw_domain(&db, ConfigDomain::Voice).await).unwrap();
            assert_eq!(stored, voice, "normalization preserves valid siblings and removes unknown fields");
            assert_eq!(raw_domain(&db, ConfigDomain::Server).await, server_before);
            let reopened = SettingsState::new().read_snapshot(&db).await.unwrap();
            assert_eq!(reopened.domains[&ConfigDomain::Voice].revision, "8");
        }
    }

    #[tokio::test]
    async fn saving_an_invalid_voice_mode_uses_default_without_resetting_voice_settings() {
        let db = database(None).await;
        let state = SettingsState::new();
        let initial = state.read_snapshot(&db).await.unwrap();
        let selected = saved(save(&state, &db, vec![patch(&initial, ConfigDomain::Voice,
            json!({"tts.mode":"clone", "guidance.rules":"保留", "tts.cloneSampleId":"sample", "connection.apiKey":"voice-key"}))]).await);
        assert_eq!(selected.data.voice.tts.mode, super::super::VoiceTtsMode::Clone);
        let restored = saved(save(&state, &db, vec![patch(&selected, ConfigDomain::Voice,
            json!({"tts.mode":false}))]).await);
        assert_eq!(restored.data.voice.tts.mode, super::super::VoiceTtsMode::Builtin);
        assert_eq!(restored.data.voice.guidance.rules, "保留");
        assert_eq!(restored.data.voice.tts.clone_sample_id, "sample");
        assert_eq!(restored.data.voice.connection.api_key, "voice-key");
        let raw: Value = serde_json::from_str(&raw_domain(&db, ConfigDomain::Voice).await).unwrap();
        assert_eq!(raw["tts"]["mode"], "builtin");
    }

    #[tokio::test]
    async fn different_fields_merge_but_same_field_conflicts_without_partial_save() {
        let db = database(None).await;
        let state = SettingsState::new();
        let base = state.read_snapshot(&db).await.unwrap();
        let first = saved(
            save(
                &state,
                &db,
                vec![patch(
                    &base,
                    ConfigDomain::Server,
                    json!({"url":"https://one.test"}),
                )],
            )
            .await,
        );
        let merged = saved(
            save(
                &state,
                &db,
                vec![patch(
                    &base,
                    ConfigDomain::Server,
                    json!({"apiKey":"new-token"}),
                )],
            )
            .await,
        );
        assert_eq!(merged.data.server.url, "https://one.test");
        assert_eq!(merged.data.server.api_key, "new-token");
        assert_ne!(
            first.domains[&ConfigDomain::Server].revision,
            merged.domains[&ConfigDomain::Server].revision
        );
        let result = save(
            &state,
            &db,
            vec![
                patch(
                    &base,
                    ConfigDomain::Identity,
                    json!({"userName":"must-not-commit"}),
                ),
                patch(
                    &base,
                    ConfigDomain::Server,
                    json!({"url":"https://two.test"}),
                ),
            ],
        )
        .await;
        match result {
            ConfigSaveResult::Conflict {
                conflicts,
                snapshot,
            } => {
                assert_eq!(conflicts.len(), 1);
                assert_eq!(conflicts[0].field, "url");
                assert_eq!(snapshot.data.server.url, "https://one.test");
                assert_eq!(
                    snapshot.data.identity.user_name,
                    base.data.identity.user_name
                );
            }
            other => panic!("expected field conflict, got {other:?}"),
        }
        assert_eq!(state.read_snapshot(&db).await.unwrap(), merged);
        // Replaying a stale update whose intended value already exists is safe.
        let replay = saved(
            save(
                &state,
                &db,
                vec![patch(
                    &base,
                    ConfigDomain::Server,
                    json!({"url":"https://one.test"}),
                )],
            )
            .await,
        );
        assert_eq!(replay, merged);
    }

    #[tokio::test]
    async fn group_backup_and_jev_domains_commit_or_rollback_together() {
        use crate::vcp_modules::group::{group_service, group_types::GroupLocalSettings};
        let db = database(None).await;
        sqlx::query("CREATE TABLE groups (owner_type TEXT NOT NULL, group_id TEXT NOT NULL, deleted_at BIGINT, PRIMARY KEY(owner_type,group_id))")
            .execute(&db.pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../../../migrations/0104_group_local_settings.sql"))
            .execute(&db.pool).await.unwrap();
        sqlx::query("INSERT INTO groups(owner_type,group_id) VALUES('group','synced-group')")
            .execute(&db.pool).await.unwrap();
        let state = SettingsState::new();
        let base = state.read_snapshot(&db).await.unwrap();
        let local = GroupLocalSettings { chat_mode: crate::vcp_modules::group_types::GroupChatMode::Jev, ..Default::default() };
        let preferences = group_service::parse_group_local_settings_backup(&json!({
            "synced-group": { "chatMode": "jev" }
        })).unwrap();
        let update = ConfigUpdate { patches: vec![patch(&base, ConfigDomain::Jev, json!({"enabled":true,"openrouter.apiKey":"backup-key"}))] };
        sqlx::raw_sql("CREATE TRIGGER fail_jev_restore BEFORE UPDATE ON config_domains WHEN NEW.domain='jev' BEGIN SELECT RAISE(ABORT,'fixture failure'); END;")
            .execute(&db.pool).await.unwrap();
        assert!(state.import_backup(&db, update.clone(), preferences.clone(), |_, _| panic!("failed import must not publish")).await.is_err());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM group_local_settings")
            .fetch_one(&db.pool).await.unwrap();
        assert_eq!(count, 0, "a later domain failure must roll back earlier group writes");
        assert_eq!(state.read_snapshot(&db).await.unwrap(), base);
        sqlx::query("DROP TRIGGER fail_jev_restore").execute(&db.pool).await.unwrap();

        let imported = state.import_backup(&db, update, preferences, |_, _| {}).await.unwrap();
        assert!(!imported.missing_groups);
        let result = saved(imported.result);
        assert!(result.data.jev.enabled);
        assert_eq!(result.data.jev.openrouter.api_key, "backup-key");
        let restored = crate::vcp_modules::group::group_service::export_group_local_settings(&db).await.unwrap();
        assert_eq!(restored["synced-group"], local);
        assert_eq!(state.read_snapshot(&db).await.unwrap(), result);
    }

    #[tokio::test]
    async fn group_backup_restores_present_entities_and_reports_unsynced_groups() {
        use crate::vcp_modules::group::{group_service, group_types::GroupLocalSettings};
        let db = database(None).await;
        sqlx::query("CREATE TABLE groups (owner_type TEXT NOT NULL, group_id TEXT NOT NULL, deleted_at BIGINT, PRIMARY KEY(owner_type,group_id))")
            .execute(&db.pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../../../migrations/0104_group_local_settings.sql"))
            .execute(&db.pool).await.unwrap();
        sqlx::query("INSERT INTO groups(owner_type,group_id,deleted_at) VALUES ('group','existing',NULL), ('group','new-device',NULL), ('group','deleted',1)")
            .execute(&db.pool).await.unwrap();
        let state = SettingsState::new();
        let mut original = GroupLocalSettings {
            chat_mode: crate::vcp_modules::group_types::GroupChatMode::Jev,
            enable_context_message_window: true,
            context_message_window_size: 48,
            ..Default::default()
        };
        original.mode_settings.jev.navigator_prompt = "保留当前群导航".into();
        let seed = group_service::parse_group_local_settings_backup(&json!({"existing": original})).unwrap();
        saved(state.import_backup(&db, ConfigUpdate::default(), seed, |_, _| {}).await.unwrap().result);

        let patches = group_service::parse_group_local_settings_backup(&json!({
            "existing": {"modeSettings":{"jev":{"speakerThreshold":0.25}}},
            "new-device": {"modeSettings":{"jev":{"speakerThreshold":0.25}}},
            "missing": {"chatMode":"jev"},
            "deleted": {"chatMode":"jev"}
        })).unwrap();
        let base = state.read_snapshot(&db).await.unwrap();
        let update = ConfigUpdate { patches: vec![patch(&base, ConfigDomain::Identity, json!({"userName":"restored"}))] };
        let imported = state.import_backup(&db, update, patches, |_, _| {}).await.unwrap();
        assert!(imported.missing_groups);
        assert_eq!(saved(imported.result).data.identity.user_name, "restored", "missing groups do not block unrelated configuration");
        let restored = group_service::export_group_local_settings(&db).await.unwrap();
        original.mode_settings.jev.speaker_threshold = 0.25;
        assert_eq!(restored["existing"], original, "omitted local fields must survive backup import");
        let mut fresh = GroupLocalSettings::default();
        fresh.mode_settings.jev.speaker_threshold = 0.25;
        assert_eq!(restored["new-device"], fresh, "a synced group without local preferences starts from current defaults");
        assert!(!restored.contains_key("missing"));
        assert!(!restored.contains_key("deleted"));
        let absent: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM group_local_settings WHERE group_id IN ('missing','deleted')")
            .fetch_one(&db.pool).await.unwrap();
        assert_eq!(absent, 0, "neither unsynced nor tombstoned groups acquire local preferences");
    }

    #[tokio::test]
    async fn cross_domain_sql_failure_does_not_publish_or_leave_partial_writes() {
        let db = database(None).await;
        let state = SettingsState::new();
        let base = state.read_snapshot(&db).await.unwrap();
        let server_before = raw_domain(&db, ConfigDomain::Server).await;
        sqlx::raw_sql("CREATE TRIGGER fail_voice_write BEFORE UPDATE ON config_domains WHEN NEW.domain='voice' BEGIN SELECT RAISE(ABORT,'fixture failure'); END;")
            .execute(&db.pool).await.unwrap();
        let result = state
            .apply(
                &db,
                ConfigUpdate {
                    patches: vec![
                        patch(
                            &base,
                            ConfigDomain::Server,
                            json!({"apiKey":"not-persisted"}),
                        ),
                        patch(&base, ConfigDomain::Voice, json!({"asr.language":"en"})),
                    ],
                },
                |_, _| panic!("failed save must not publish effects"),
            )
            .await;
        assert!(result.is_err());
        assert_eq!(raw_domain(&db, ConfigDomain::Server).await, server_before);
        assert_eq!(state.read_snapshot(&db).await.unwrap(), base);

    }

    #[tokio::test]
    async fn cancelled_ipc_after_sql_commit_still_publishes_cache_and_effects() {
        let db = database(None).await;
        let state = SettingsState::new();
        let base = state.read_snapshot(&db).await.unwrap();
        let mut metrics = db.subscribe_write_metrics();
        // Allow the save to capture the previous settings, while blocking only
        // its post-commit cache publication.
        let cache_guard = state.inner.cache.read().await;
        let request = ConfigUpdate {
            patches: vec![patch(
                &base,
                ConfigDomain::Identity,
                json!({"userName":"durable"}),
            )],
        };
        let (published, observed) = tokio::sync::oneshot::channel();
        let caller_state = state.clone();
        let caller_db = db.clone();
        let caller = tokio::spawn(async move {
            caller_state
                .apply(&caller_db, request, move |old, new| {
                    let _ = published.send((
                        old.identity.user_name.clone(),
                        new.data.identity.user_name.clone(),
                    ));
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let metric = metrics.recv().await.unwrap();
                if metric.operation == "settings.save-domains" && metric.outcome == "committed" {
                    break;
                }
            }
        })
        .await
        .unwrap();
        // SQL has committed, while publication is deliberately blocked. This is
        // the dangerous window the repository's durable task must span.
        assert_eq!(
            cache_guard.as_ref().unwrap().settings.identity.user_name,
            base.data.identity.user_name
        );
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        drop(cache_guard);
        let (old, new) = tokio::time::timeout(Duration::from_secs(3), observed)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(old, base.data.identity.user_name);
        assert_eq!(new, "durable");
        assert_eq!(
            state.read_config(&db).await.unwrap().identity.user_name,
            "durable"
        );
        assert_eq!(
            SettingsState::new()
                .read_snapshot(&db)
                .await
                .unwrap()
                .data
                .identity
                .user_name,
            "durable"
        );
    }

    #[tokio::test]
    async fn configuration_save_does_not_wait_for_runtime_reconnection() {
        let db = database(None).await;
        let state = SettingsState::new();
        let base = state.read_snapshot(&db).await.unwrap();
        let _runtime = state.lock_runtime_reconcile().await;
        let first = state.reserve_runtime_reconcile(1);
        let current = state.reserve_runtime_reconcile(2);
        assert!(!state.is_runtime_generation_current(first));
        assert!(state.is_runtime_generation_current(current));
        assert_eq!(state.take_runtime_changes(), 3);
        assert_eq!(state.take_runtime_changes(), 0);
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            save(
                &state,
                &db,
                vec![patch(
                    &base,
                    ConfigDomain::Voice,
                    json!({"guidance.rules":"低声"}),
                )],
            ),
        )
        .await
        .unwrap();
        assert_eq!(saved(result).data.voice.guidance.rules, "低声");
    }

    #[tokio::test]
    async fn invalid_groups_and_schema_differences_normalize_known_values_without_gates() {
        let db = database(None).await;
        SettingsState::new().read_snapshot(&db).await.unwrap();
        sqlx::query("UPDATE config_domains SET value=?,revision=9 WHERE domain='voice'")
            .bind(r#"{"connection":{"apiBaseUrl":"https://voice.example/v1","apiKey":"retained-key"},"tts":"invalid-group","asr":{"language":"en","stream":false}}"#)
            .execute(&db.pool).await.unwrap();
        sqlx::query("UPDATE config_domains SET schema_version=99,value=?,revision=4 WHERE domain='server'")
            .bind(r#"{"url":"https://future.example/v1","apiKey":"keep","unknownFuture":17}"#)
            .execute(&db.pool).await.unwrap();
        let state = SettingsState::new();
        let base = state.read_snapshot(&db).await.unwrap();
        assert_eq!(base.data.voice.asr.language, "en");
        assert!(!base.data.voice.asr.stream);
        assert_eq!(base.data.voice.connection.api_key, "retained-key");
        assert_eq!(base.data.voice.tts, Settings::default().voice.tts);
        assert_eq!(base.data.server.url, "https://future.example/v1");
        assert_eq!(base.data.server.api_key, "keep");
        assert_eq!(base.domains[&ConfigDomain::Server].schema_version, ConfigDomain::Server.schema_version());
        assert_eq!(base.domains[&ConfigDomain::Server].revision, "5");
        let stored: Value = serde_json::from_str(&raw_domain(&db, ConfigDomain::Server).await).unwrap();
        assert!(stored.get("unknownFuture").is_none());
        let after = saved(save(&state, &db, vec![patch(
            &base, ConfigDomain::Voice, json!({"tts.voice":"白桦"}),
        )]).await);
        assert_eq!(after.data.voice.tts.voice, "白桦");
        assert_eq!(after.data.voice.connection.api_key, "retained-key");
    }

    #[tokio::test]
    async fn invalid_known_values_default_but_unknown_paths_reject_without_partial_edits() {
        let db = database(None).await;
        let state = SettingsState::new();
        let base = state.read_snapshot(&db).await.unwrap();
        let after = saved(save(&state, &db, vec![patch(
            &base, ConfigDomain::Voice,
            json!({"asr.stream":"bad-type","guidance.rules":"valid sibling"}),
        )]).await);
        assert_eq!(after.data.voice.asr.stream, Settings::default().voice.asr.stream);
        assert_eq!(after.data.voice.guidance.rules, "valid sibling");
        let invalid = DomainPatch {
            domain: ConfigDomain::Server,
            expected_revision: after.domains[&ConfigDomain::Server].revision.clone(),
            changes: json!({"arbitrary":"private-input"}).as_object().unwrap().clone(),
            base_values: Map::new(),
        };
        let result = save(&state, &db, vec![
            invalid,
            patch(&after, ConfigDomain::Identity, json!({"userName":"must-not-commit"})),
        ]).await;
        assert!(matches!(result, ConfigSaveResult::Rejected { .. }));
        assert!(!serde_json::to_string(&result).unwrap().contains("private-input"));
        assert_eq!(state.read_snapshot(&db).await.unwrap(), after);
    }

    #[tokio::test]
    async fn normalized_field_uses_visible_baseline_after_unrelated_revision_change() {
        let db = database(Some(r#"{"voiceVadEndSilenceMs":1}"#)).await;
        let state = SettingsState::new();
        let base = state.read_snapshot(&db).await.unwrap();
        let default_wait = base.data.voice.vad.end_silence_ms;
        assert_ne!(default_wait, 1);
        let reset = patch(
            &base,
            ConfigDomain::Voice,
            json!({"vad.endSilenceMs": default_wait}),
        );
        let newer = saved(
            save(
                &state,
                &db,
                vec![patch(
                    &base,
                    ConfigDomain::Voice,
                    json!({"guidance.rules":"后来保存的规则"}),
                )],
            )
            .await,
        );
        assert_ne!(
            newer.domains[&ConfigDomain::Voice].revision,
            reset.expected_revision
        );

        let restored = saved(save(&state, &db, vec![reset]).await);
        assert_eq!(restored.data.voice.vad.end_silence_ms, default_wait);
        assert_eq!(restored.data.voice.guidance.rules, "后来保存的规则");
        let stored: Value =
            serde_json::from_str(&raw_domain(&db, ConfigDomain::Voice).await).unwrap();
        assert_eq!(stored["vad"]["endSilenceMs"], default_wait);
        let archive_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM config_recovery WHERE domain='voice'")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(archive_count, 0);
    }

    #[tokio::test]
    async fn rejected_patch_still_commits_and_publishes_normalized_current_storage() {
        let db = database(None).await;
        let state = SettingsState::new();
        let base = state.read_snapshot(&db).await.unwrap();
        sqlx::query("UPDATE config_domains SET value='{broken-original', revision=revision+1 WHERE domain='voice'")
            .execute(&db.pool).await.unwrap();
        let invalid = DomainPatch {
            domain: ConfigDomain::Voice,
            expected_revision: base.domains[&ConfigDomain::Voice].revision.clone(),
            changes: json!({"unknown":"private-input"}).as_object().unwrap().clone(),
            base_values: Map::new(),
        };
        let published = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observer = published.clone();
        let result = state.apply(&db, ConfigUpdate {
            patches: vec![patch(&base, ConfigDomain::Identity, json!({"userName":"must-not-commit"})), invalid],
        }, move |_, _| { observer.fetch_add(1, Ordering::SeqCst); }).await.unwrap();
        assert!(matches!(result, ConfigSaveResult::Rejected { .. }));
        assert_eq!(published.load(Ordering::SeqCst), 1);
        let refreshed = state.read_snapshot(&db).await.unwrap();
        assert_eq!(refreshed.domains[&ConfigDomain::Voice].revision, "3");
        assert_eq!(refreshed.data.identity, base.data.identity);
        let stored: Value = serde_json::from_str(&raw_domain(&db, ConfigDomain::Voice).await).unwrap();
        assert_eq!(stored, domain_defaults(ConfigDomain::Voice));
        assert_eq!(SettingsState::new().read_snapshot(&db).await.unwrap(), refreshed);
    }

    #[tokio::test]
    async fn normalization_database_failure_does_not_publish_defaults() {
        let db = database(None).await;
        let state = SettingsState::new();
        let base = state.read_snapshot(&db).await.unwrap();
        sqlx::query("UPDATE config_domains SET value='{broken-original' WHERE domain='voice'")
            .execute(&db.pool).await.unwrap();
        sqlx::raw_sql("CREATE TRIGGER fail_normalization BEFORE UPDATE ON config_domains WHEN NEW.domain='voice' BEGIN SELECT RAISE(ABORT,'fixture storage failure'); END;")
            .execute(&db.pool).await.unwrap();
        let result = state.apply(&db, ConfigUpdate { patches: vec![] },
            |_, _| panic!("a failed database commit cannot publish")).await;
        assert!(result.is_err());
        assert_eq!(state.read_snapshot(&db).await.unwrap(), base);
        assert_eq!(raw_domain(&db, ConfigDomain::Voice).await, "{broken-original");
        assert!(SettingsState::new().read_snapshot(&db).await.is_err());
    }

    #[tokio::test]
    async fn conflicting_patch_does_not_rollback_normalization_of_other_current_values() {
        let db = database(None).await;
        let state = SettingsState::new();
        let base = state.read_snapshot(&db).await.unwrap();
        let request = patch(&base, ConfigDomain::Identity, json!({"userName":"stale draft"}));
        sqlx::query("UPDATE config_domains SET value=?,revision=revision+1 WHERE domain='identity'")
            .bind(json!({"userName":"newer writer"}).to_string()).execute(&db.pool).await.unwrap();
        sqlx::query("UPDATE config_domains SET value='{broken-original',revision=revision+1 WHERE domain='voice'")
            .execute(&db.pool).await.unwrap();
        let result = state.apply(&db, ConfigUpdate { patches: vec![request] }, |_, _| {}).await.unwrap();
        let current = match result {
            ConfigSaveResult::Conflict { snapshot, conflicts } => {
                assert_eq!(conflicts[0].field, "userName");
                snapshot
            }
            other => panic!("expected conflict, got {other:?}"),
        };
        assert_eq!(current.data.identity.user_name, "newer writer");
        assert_eq!(current.domains[&ConfigDomain::Voice].revision, "3");
        let stored: Value = serde_json::from_str(&raw_domain(&db, ConfigDomain::Voice).await).unwrap();
        assert_eq!(stored, domain_defaults(ConfigDomain::Voice));
        assert_eq!(state.read_snapshot(&db).await.unwrap(), current);
    }

    #[tokio::test]
    async fn normalized_configuration_still_protects_newer_edits_from_stale_writers() {
        let db = database(None).await;
        SettingsState::new().read_snapshot(&db).await.unwrap();
        sqlx::query("UPDATE config_domains SET value=? WHERE domain='voice'")
            .bind(r#"{"vad":"broken-group","guidance":{"rules":"原来的规则"}}"#)
            .execute(&db.pool).await.unwrap();
        let state = SettingsState::new();
        let base = state.read_snapshot(&db).await.unwrap();
        let request = patch(&base, ConfigDomain::Voice, json!({"guidance.rules":"stale draft"}));
        let other_writer = SettingsState::new();
        let newer = saved(save(&other_writer, &db, vec![patch(
            &base, ConfigDomain::Voice, json!({"guidance.rules":"另一个操作保存的规则"}),
        )]).await);
        assert_eq!(state.read_snapshot(&db).await.unwrap(), base);
        let before = raw_domain(&db, ConfigDomain::Voice).await;
        match save(&state, &db, vec![request]).await {
            ConfigSaveResult::Conflict { snapshot, conflicts } => {
                assert_eq!(conflicts.len(), 1);
                assert_eq!(conflicts[0].field, "guidance.rules");
                assert_eq!(conflicts[0].code, "fieldConflict");
                assert_eq!(snapshot, newer);
            }
            other => panic!("expected field conflict, got {other:?}"),
        }
        assert_eq!(raw_domain(&db, ConfigDomain::Voice).await, before);
        assert_eq!(state.read_snapshot(&db).await.unwrap(), newer);
    }
}
