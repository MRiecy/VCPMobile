//! P4.4 app-private knowledge catalog and explicit local_vref grant owner.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use tauri::{AppHandle, Manager, State};
use tokio::sync::{Mutex, MutexGuard};
use uuid::Uuid;

use crate::vcp_modules::persistence::db_manager::DbState;

pub const MAX_SOURCE_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_ACTIVE_SOURCES: u64 = 64;
pub const MAX_CAS_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_PENDING_CANDIDATES: u64 = 16;
pub const MAX_PENDING_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_INDEX_TEXT_BYTES: usize = 1024 * 1024;
pub const MAX_CHUNK_BYTES: usize = 4 * 1024;
pub const CHUNK_OVERLAP_BYTES: usize = 512;
pub const MAX_CHUNKS_PER_SOURCE: usize = 300;
pub const MAX_CATALOG_CHUNKS: u64 = 19_200;
pub const CANDIDATE_TTL_MS: u64 = 24 * 60 * 60 * 1000;

const KNOWLEDGE_SCHEMA_VERSION: u32 = 1;
const STORAGE_HEADROOM_BYTES: u64 = 256 * 1024 * 1024;
const COPY_BUFFER_BYTES: usize = 64 * 1024;
const CANDIDATE_FILE_NAME: &str = "candidate";
const MAX_OPERATION_ID_BYTES: usize = 160;
const MAX_DISPLAY_NAME_BYTES: usize = 240;
const MAX_MIME_BYTES: usize = 128;
const MAX_RETAINED_OPERATIONS: i64 = 1_024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeIndexStatus {
    Indexing,
    Ready,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeSourceDto {
    pub source_id: String,
    pub display_name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub source_sha256: String,
    pub index_status: KnowledgeIndexStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
    pub index_text_truncated: bool,
    pub chunk_count: u32,
    pub granted_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeCatalogSnapshot {
    pub schema_version: u32,
    pub catalog_generation: u64,
    pub used_bytes: u64,
    pub limit_bytes: u64,
    pub pending_used_bytes: u64,
    pub pending_limit_bytes: u64,
    pub active_source_count: u64,
    pub active_source_limit: u64,
    pub pending_candidate_count: u64,
    pub pending_candidate_limit: u64,
    pub sources: Vec<KnowledgeSourceDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeImportCandidate {
    pub token: String,
    pub candidate_sha256: String,
    pub catalog_generation: u64,
    pub display_name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub index_text_truncated: bool,
    pub chunk_count: u32,
    pub used_bytes: u64,
    pub limit_bytes: u64,
    pub pending_used_bytes: u64,
    pub pending_limit_bytes: u64,
    pub warnings: Vec<String>,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeInspectStatus {
    Cancelled,
    Candidate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectKnowledgeImportResponse {
    pub operation_id: String,
    pub status: KnowledgeInspectStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate: Option<KnowledgeImportCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectKnowledgeImportRequest {
    pub operation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitKnowledgeImportRequest {
    pub operation_id: String,
    pub token: String,
    pub candidate_sha256: String,
    pub expected_catalog_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitKnowledgeImportResponse {
    pub operation_id: String,
    pub catalog_generation: u64,
    pub replayed: bool,
    pub source: KnowledgeSourceDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscardKnowledgeImportRequest {
    pub operation_id: String,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscardKnowledgeImportResponse {
    pub operation_id: String,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeKnowledgeGrantRequest {
    pub operation_id: String,
    pub source_id: String,
    pub expected_catalog_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeDeletionState {
    Deleted,
    PendingHolds,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevokeKnowledgeGrantResponse {
    pub operation_id: String,
    pub catalog_generation: u64,
    pub replayed: bool,
    pub source_id: String,
    pub deletion_state: KnowledgeDeletionState,
}

#[derive(Debug, Clone)]
pub(crate) struct ActiveKnowledgeSource {
    pub source_id: String,
    pub display_name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub source_sha256: String,
    pub object_path: PathBuf,
}

#[derive(Default)]
pub struct KnowledgeCatalogOwner {
    mutation_gate: Mutex<()>,
    picker_gate: Mutex<()>,
}

impl KnowledgeCatalogOwner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Serializes the catalog/admission decision against commit/revoke. Runtime holds this guard
    /// from its final generation/grant check through durable Job admission and hold release.
    pub(crate) async fn lock_mutation(&self) -> MutexGuard<'_, ()> {
        self.mutation_gate.lock().await
    }

    pub async fn catalog(
        &self,
        app: &AppHandle,
        pool: &SqlitePool,
    ) -> Result<KnowledgeCatalogSnapshot, String> {
        let _guard = self.mutation_gate.lock().await;
        let now_ms = now_ms()?;
        self.maintenance(app, pool, now_ms).await?;
        catalog_snapshot(pool).await
    }

    pub async fn inspect(
        &self,
        app: &AppHandle,
        pool: &SqlitePool,
        request: InspectKnowledgeImportRequest,
    ) -> Result<InspectKnowledgeImportResponse, String> {
        validate_operation_id(&request.operation_id)?;
        let digest = request_digest("inspect", &request)?;
        {
            let _guard = self.mutation_gate.lock().await;
            if let Some(mut replay) = replay_operation::<InspectKnowledgeImportResponse>(
                pool,
                &request.operation_id,
                "inspect",
                &digest,
            )
            .await?
            {
                if let Some(candidate) = replay.candidate.as_mut() {
                    candidate.replayed = true;
                }
                return Ok(replay);
            }
        }
        // Native picker interaction may remain open indefinitely while a user reviews providers.
        // It has its own single-flight gate and never holds the catalog mutation owner.
        let _picker_guard = self.picker_gate.lock().await;
        {
            let _guard = self.mutation_gate.lock().await;
            if let Some(mut replay) = replay_operation::<InspectKnowledgeImportResponse>(
                pool,
                &request.operation_id,
                "inspect",
                &digest,
            )
            .await?
            {
                if let Some(candidate) = replay.candidate.as_mut() {
                    candidate.replayed = true;
                }
                return Ok(replay);
            }
        }
        let picker_app = app.clone();
        let picked = tauri::async_runtime::spawn_blocking(move || {
            tauri_plugin_vcp_mobile::system::pick_file(
                picker_app,
                Some("knowledge".to_string()),
                Some(MAX_SOURCE_BYTES),
            )
        })
        .await
        .map_err(|error| format!("knowledge_picker_task_failed:{error}"))?;
        let picked = match picked {
            Ok(picked) => picked,
            Err(error) if error == "picker_cancelled" => {
                let _guard = self.mutation_gate.lock().await;
                if let Some(mut replay) = replay_operation::<InspectKnowledgeImportResponse>(
                    pool,
                    &request.operation_id,
                    "inspect",
                    &digest,
                )
                .await?
                {
                    if let Some(candidate) = replay.candidate.as_mut() {
                        candidate.replayed = true;
                    }
                    return Ok(replay);
                }
                let now_ms = now_ms()?;
                let response = InspectKnowledgeImportResponse {
                    operation_id: request.operation_id.clone(),
                    status: KnowledgeInspectStatus::Cancelled,
                    candidate: None,
                };
                record_operation(
                    pool,
                    &request.operation_id,
                    "inspect",
                    &digest,
                    None,
                    &response,
                    now_ms,
                )
                .await?;
                return Ok(response);
            }
            Err(error) => return Err(error),
        };

        let source_path = PathBuf::from(&picked.path);
        {
            let _guard = self.mutation_gate.lock().await;
            if let Some(mut replay) = replay_operation::<InspectKnowledgeImportResponse>(
                pool,
                &request.operation_id,
                "inspect",
                &digest,
            )
            .await?
            {
                remove_picker_source(&source_path);
                if let Some(candidate) = replay.candidate.as_mut() {
                    candidate.replayed = true;
                }
                return Ok(replay);
            }
            self.maintenance(app, pool, now_ms()?).await?;
        }
        let now_ms = now_ms()?;
        let result = self
            .inspect_picked(app, pool, &request.operation_id, &digest, picked, now_ms)
            .await;
        remove_picker_source(&source_path);
        result
    }

    async fn inspect_picked(
        &self,
        app: &AppHandle,
        pool: &SqlitePool,
        operation_id: &str,
        operation_digest: &str,
        picked: tauri_plugin_vcp_mobile::system::PickedFileInfo,
        now_ms: u64,
    ) -> Result<InspectKnowledgeImportResponse, String> {
        validate_picker_metadata(&picked)?;
        let quota = load_quota(pool).await?;
        if quota.pending_count >= MAX_PENDING_CANDIDATES {
            return Err("knowledge_pending_candidate_limit".to_string());
        }
        if quota.pending_bytes.saturating_add(picked.size) > MAX_PENDING_BYTES {
            return Err("knowledge_pending_bytes_limit".to_string());
        }
        let layout = knowledge_layout(app)?;
        let staging_name = Uuid::new_v4().simple().to_string();
        let token = format!("vcp-knowledge-candidate:{}", Uuid::new_v4().simple());
        let staging_dir = layout.staging.join(&staging_name);
        let staging_file = staging_dir.join(CANDIDATE_FILE_NAME);
        let worker_layout = KnowledgeLayout {
            root: layout.root.clone(),
            staging: layout.staging.clone(),
            objects: layout.objects.clone(),
        };
        let worker_staging_dir = staging_dir.clone();
        let worker_staging_file = staging_file.clone();
        let worker_picked = tauri_plugin_vcp_mobile::system::PickedFileInfo {
            path: picked.path.clone(),
            name: picked.name.clone(),
            mime: picked.mime.clone(),
            size: picked.size,
            hash: picked.hash.clone(),
            thumbnail_path: None,
        };
        let audit = tauri::async_runtime::spawn_blocking(move || {
            ensure_layout(&worker_layout)?;
            ensure_free_space(
                &worker_layout.root,
                worker_picked.size.saturating_add(STORAGE_HEADROOM_BYTES),
            )?;
            fs::create_dir(&worker_staging_dir)
                .map_err(|error| format!("knowledge_staging_create_failed:{error}"))?;
            set_mode(&worker_staging_dir, 0o700)?;
            let result = copy_and_audit_candidate(
                Path::new(&worker_picked.path),
                &worker_staging_file,
                &worker_picked,
            );
            let audit = match result {
                Ok(audit) => audit,
                Err(error) => {
                    let _ = fs::remove_dir_all(&worker_staging_dir);
                    return Err(error);
                }
            };
            sync_directory(&worker_staging_dir)?;
            sync_directory(&worker_layout.staging)?;
            Ok::<_, String>(audit)
        })
        .await
        .map_err(|error| format!("knowledge_inspect_task_failed:{error}"))??;

        let _guard = self.mutation_gate.lock().await;
        if let Some(mut replay) = replay_operation::<InspectKnowledgeImportResponse>(
            pool,
            operation_id,
            "inspect",
            operation_digest,
        )
        .await?
        {
            let _ = fs::remove_dir_all(&staging_dir);
            if let Some(candidate) = replay.candidate.as_mut() {
                candidate.replayed = true;
            }
            return Ok(replay);
        }
        let current = load_quota(pool).await?;
        if current.pending_count >= MAX_PENDING_CANDIDATES
            || current.pending_bytes.saturating_add(audit.size_bytes) > MAX_PENDING_BYTES
        {
            let _ = fs::remove_dir_all(&staging_dir);
            return Err("knowledge_pending_quota_changed".to_string());
        }
        let meta = load_meta(pool).await?;
        let duplicate: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM local_knowledge_sources WHERE source_sha256 = ? AND revoked_at_ms IS NULL)",
        )
        .bind(&audit.sha256)
        .fetch_one(pool)
        .await
        .map_err(db_error)?;
        let mut warnings = Vec::new();
        if audit.index_text_truncated {
            warnings.push("index_text_truncated".to_string());
        }
        if duplicate != 0 {
            warnings.push("already_granted".to_string());
        }
        let candidate_digest = candidate_digest(
            &audit,
            &sanitize_display_name(&picked.name)?,
            &normalize_mime(&picked.mime, &picked.name)?,
        );
        let candidate = KnowledgeImportCandidate {
            token: token.clone(),
            candidate_sha256: candidate_digest.clone(),
            catalog_generation: meta.generation,
            display_name: sanitize_display_name(&picked.name)?,
            mime_type: normalize_mime(&picked.mime, &picked.name)?,
            size_bytes: audit.size_bytes,
            index_text_truncated: audit.index_text_truncated,
            chunk_count: audit.chunks.len() as u32,
            used_bytes: meta.used_bytes,
            limit_bytes: MAX_CAS_BYTES,
            pending_used_bytes: current.pending_bytes.saturating_add(audit.size_bytes),
            pending_limit_bytes: MAX_PENDING_BYTES,
            warnings,
            replayed: false,
        };
        let response = InspectKnowledgeImportResponse {
            operation_id: operation_id.to_string(),
            status: KnowledgeInspectStatus::Candidate,
            candidate: Some(candidate.clone()),
        };

        let mut tx = pool.begin().await.map_err(db_error)?;
        sqlx::query(
            "INSERT INTO local_knowledge_import_candidates(
               token, inspect_operation_id, candidate_sha256, source_sha256, staging_name,
               display_name, mime_type, size_bytes, catalog_generation, index_text_truncated,
               chunk_count, created_at_ms, expires_at_ms
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&token)
        .bind(operation_id)
        .bind(&candidate_digest)
        .bind(&audit.sha256)
        .bind(&staging_name)
        .bind(&candidate.display_name)
        .bind(&candidate.mime_type)
        .bind(to_i64(audit.size_bytes, "candidate_size")?)
        .bind(to_i64(meta.generation, "catalog_generation")?)
        .bind(i64::from(audit.index_text_truncated))
        .bind(i64::from(candidate.chunk_count))
        .bind(to_i64(now_ms, "created_at")?)
        .bind(to_i64(
            now_ms.saturating_add(CANDIDATE_TTL_MS),
            "expires_at",
        )?)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        insert_operation_tx(
            &mut tx,
            operation_id,
            "inspect",
            operation_digest,
            None,
            &response,
            now_ms,
        )
        .await?;
        tx.commit().await.map_err(db_error)?;
        Ok(response)
    }

    pub async fn commit(
        &self,
        app: &AppHandle,
        pool: &SqlitePool,
        request: CommitKnowledgeImportRequest,
    ) -> Result<CommitKnowledgeImportResponse, String> {
        validate_operation_id(&request.operation_id)?;
        validate_candidate_token(&request.token)?;
        validate_sha256(&request.candidate_sha256, "candidate_sha256")?;
        let digest = request_digest("commit", &request)?;
        let _guard = self.mutation_gate.lock().await;
        if let Some(mut replay) = replay_operation::<CommitKnowledgeImportResponse>(
            pool,
            &request.operation_id,
            "commit",
            &digest,
        )
        .await?
        {
            replay.replayed = true;
            return Ok(replay);
        }
        let now_ms = now_ms()?;
        self.maintenance(app, pool, now_ms).await?;
        let candidate = load_candidate(pool, &request.token).await?;
        if candidate.candidate_sha256 != request.candidate_sha256 {
            return Err("knowledge_candidate_digest_mismatch".to_string());
        }
        if candidate.catalog_generation != request.expected_catalog_generation {
            return Err("knowledge_catalog_generation_changed".to_string());
        }
        if candidate.expires_at_ms < now_ms {
            return Err("knowledge_candidate_expired".to_string());
        }
        let meta = load_meta(pool).await?;
        if meta.generation != request.expected_catalog_generation {
            return Err("knowledge_catalog_generation_changed".to_string());
        }
        let active_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM local_knowledge_sources WHERE revoked_at_ms IS NULL",
        )
        .fetch_one(pool)
        .await
        .map_err(db_error)?;
        if active_count >= MAX_ACTIVE_SOURCES as i64 {
            return Err("knowledge_active_source_limit".to_string());
        }
        let duplicate: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM local_knowledge_sources WHERE source_sha256 = ?)",
        )
        .bind(&candidate.source_sha256)
        .fetch_one(pool)
        .await
        .map_err(db_error)?;
        if duplicate != 0 {
            return Err("knowledge_source_already_exists".to_string());
        }
        let physical_bytes: i64 =
            sqlx::query_scalar("SELECT COALESCE(SUM(size_bytes), 0) FROM local_knowledge_sources")
                .fetch_one(pool)
                .await
                .map_err(db_error)?;
        let physical_bytes = from_i64(physical_bytes, "physical_bytes")?;
        if physical_bytes.saturating_add(candidate.size_bytes) > MAX_CAS_BYTES {
            return Err("knowledge_cas_limit".to_string());
        }

        let layout = knowledge_layout(app)?;
        ensure_layout(&layout)?;
        let staging_file = layout
            .staging
            .join(&candidate.staging_name)
            .join(CANDIDATE_FILE_NAME);
        let audited = audit_owned_file(&staging_file)?;
        if audited.sha256 != candidate.source_sha256
            || audited.size_bytes != candidate.size_bytes
            || candidate_digest(&audited, &candidate.display_name, &candidate.mime_type)
                != candidate.candidate_sha256
        {
            return Err("knowledge_candidate_changed".to_string());
        }
        let object_path = publish_object(&layout, &staging_file, &audited)?;
        let source_id = source_id_for_sha(&audited.sha256);
        let generation = meta.generation.saturating_add(1);
        let source = KnowledgeSourceDto {
            source_id: source_id.clone(),
            display_name: candidate.display_name.clone(),
            mime_type: candidate.mime_type.clone(),
            size_bytes: audited.size_bytes,
            source_sha256: audited.sha256.clone(),
            index_status: KnowledgeIndexStatus::Ready,
            failure_code: None,
            index_text_truncated: audited.index_text_truncated,
            chunk_count: audited.chunks.len() as u32,
            granted_at_ms: now_ms,
        };
        let response = CommitKnowledgeImportResponse {
            operation_id: request.operation_id.clone(),
            catalog_generation: generation,
            replayed: false,
            source: source.clone(),
        };

        let existing_chunks: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM local_knowledge_chunks")
                .fetch_one(pool)
                .await
                .map_err(db_error)?;
        if from_i64(existing_chunks, "chunk_count")?.saturating_add(audited.chunks.len() as u64)
            > MAX_CATALOG_CHUNKS
        {
            if !object_referenced(pool, &audited.sha256).await? {
                let _ = fs::remove_file(&object_path);
            }
            return Err("knowledge_chunk_limit".to_string());
        }

        let mut tx = pool.begin().await.map_err(db_error)?;
        sqlx::query(
            "INSERT INTO local_knowledge_sources(
               source_id, display_name, mime_type, size_bytes, source_sha256, object_name,
               index_status, failure_code, index_text_truncated, chunk_count, granted_at_ms,
               revoked_at_ms
             ) VALUES (?, ?, ?, ?, ?, ?, 'ready', NULL, ?, ?, ?, NULL)",
        )
        .bind(&source_id)
        .bind(&source.display_name)
        .bind(&source.mime_type)
        .bind(to_i64(source.size_bytes, "source_size")?)
        .bind(&source.source_sha256)
        .bind(&source.source_sha256)
        .bind(i64::from(source.index_text_truncated))
        .bind(i64::from(source.chunk_count))
        .bind(to_i64(now_ms, "granted_at")?)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        for (ordinal, chunk) in audited.chunks.iter().enumerate() {
            sqlx::query(
                "INSERT INTO local_knowledge_chunks(source_id, ordinal, content, content_sha256)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(&source_id)
            .bind(ordinal as i64)
            .bind(chunk)
            .bind(sha256_hex(chunk.as_bytes()))
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        }
        sqlx::query(
            "UPDATE local_knowledge_meta
             SET catalog_generation = ?, used_bytes = used_bytes + ?, updated_at_ms = ?
             WHERE singleton = 1 AND catalog_generation = ?",
        )
        .bind(to_i64(generation, "catalog_generation")?)
        .bind(to_i64(source.size_bytes, "source_size")?)
        .bind(to_i64(now_ms, "updated_at")?)
        .bind(to_i64(
            request.expected_catalog_generation,
            "expected_generation",
        )?)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        sqlx::query("DELETE FROM local_knowledge_import_candidates WHERE token = ?")
            .bind(&request.token)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        insert_operation_tx(
            &mut tx,
            &request.operation_id,
            "commit",
            &digest,
            Some(&source_id),
            &response,
            now_ms,
        )
        .await?;
        tx.commit().await.map_err(db_error)?;
        let candidate_dir = layout.staging.join(&candidate.staging_name);
        let _ = fs::remove_dir_all(candidate_dir);
        sync_directory(&layout.staging)?;
        Ok(response)
    }

    pub async fn discard(
        &self,
        app: &AppHandle,
        pool: &SqlitePool,
        request: DiscardKnowledgeImportRequest,
    ) -> Result<DiscardKnowledgeImportResponse, String> {
        validate_operation_id(&request.operation_id)?;
        validate_candidate_token(&request.token)?;
        let digest = request_digest("discard", &request)?;
        let _guard = self.mutation_gate.lock().await;
        if let Some(mut replay) = replay_operation::<DiscardKnowledgeImportResponse>(
            pool,
            &request.operation_id,
            "discard",
            &digest,
        )
        .await?
        {
            replay.replayed = true;
            return Ok(replay);
        }
        let candidate = load_candidate(pool, &request.token).await?;
        let response = DiscardKnowledgeImportResponse {
            operation_id: request.operation_id.clone(),
            replayed: false,
        };
        let now_ms = now_ms()?;
        let mut tx = pool.begin().await.map_err(db_error)?;
        sqlx::query("DELETE FROM local_knowledge_import_candidates WHERE token = ?")
            .bind(&request.token)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        insert_operation_tx(
            &mut tx,
            &request.operation_id,
            "discard",
            &digest,
            None,
            &response,
            now_ms,
        )
        .await?;
        tx.commit().await.map_err(db_error)?;
        let layout = knowledge_layout(app)?;
        let _ = fs::remove_dir_all(layout.staging.join(candidate.staging_name));
        Ok(response)
    }

    pub async fn revoke(
        &self,
        app: &AppHandle,
        pool: &SqlitePool,
        request: RevokeKnowledgeGrantRequest,
    ) -> Result<RevokeKnowledgeGrantResponse, String> {
        validate_operation_id(&request.operation_id)?;
        validate_source_id(&request.source_id)?;
        let digest = request_digest("revoke", &request)?;
        let _guard = self.mutation_gate.lock().await;
        // Finish any earlier physical deletions before replaying their durable response.
        self.collect_revoked_sources_locked(app, pool, 16).await?;
        if let Some(mut replay) = replay_operation::<RevokeKnowledgeGrantResponse>(
            pool,
            &request.operation_id,
            "revoke",
            &digest,
        )
        .await?
        {
            replay.replayed = true;
            return Ok(replay);
        }
        let now_ms = now_ms()?;
        let mut tx = pool.begin().await.map_err(db_error)?;
        // Acquire SQLite's write owner before reading generation/source/holds. This serializes the
        // revoke decision against bind_tool_vref_projection's conditional hold insert.
        sqlx::query(
            "UPDATE local_knowledge_meta SET updated_at_ms = updated_at_ms WHERE singleton = 1",
        )
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        let meta_row = sqlx::query(
            "SELECT catalog_generation, used_bytes FROM local_knowledge_meta WHERE singleton = 1",
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
        let meta_generation = from_i64(
            meta_row.try_get("catalog_generation").map_err(db_error)?,
            "generation",
        )?;
        if meta_generation != request.expected_catalog_generation {
            return Err("knowledge_catalog_generation_changed".to_string());
        }
        let row = sqlx::query(
            "SELECT size_bytes, source_sha256 FROM local_knowledge_sources
             WHERE source_id = ? AND revoked_at_ms IS NULL",
        )
        .bind(&request.source_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?
        .ok_or_else(|| "knowledge_source_not_active".to_string())?;
        let source_sha256: String = row.try_get("source_sha256").map_err(db_error)?;
        validate_sha256(&source_sha256, "source_sha256")?;
        let holds: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM local_knowledge_attempt_holds WHERE source_id = ?",
        )
        .bind(&request.source_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
        // A source remains physically accounted until the owner has actually unlinked the CAS
        // object and committed the matching row/meta update. The durable response starts pending
        // and is atomically upgraded to Deleted by collect_revoked_sources_locked.
        let deletion_state = KnowledgeDeletionState::PendingHolds;
        let generation = meta_generation.saturating_add(1);
        let response = RevokeKnowledgeGrantResponse {
            operation_id: request.operation_id.clone(),
            catalog_generation: generation,
            replayed: false,
            source_id: request.source_id.clone(),
            deletion_state: deletion_state.clone(),
        };
        let revoked = sqlx::query(
            "UPDATE local_knowledge_sources SET revoked_at_ms = ?
             WHERE source_id = ? AND revoked_at_ms IS NULL",
        )
        .bind(to_i64(now_ms, "revoked_at")?)
        .bind(&request.source_id)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        if revoked.rows_affected() != 1 {
            return Err("knowledge_source_not_active".to_string());
        }
        let meta_updated = sqlx::query(
            "UPDATE local_knowledge_meta
             SET catalog_generation = ?, updated_at_ms = ?
             WHERE singleton = 1 AND catalog_generation = ?",
        )
        .bind(to_i64(generation, "catalog_generation")?)
        .bind(to_i64(now_ms, "updated_at")?)
        .bind(to_i64(
            request.expected_catalog_generation,
            "expected_generation",
        )?)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        if meta_updated.rows_affected() != 1 {
            return Err("knowledge_catalog_generation_changed".to_string());
        }
        insert_operation_tx(
            &mut tx,
            &request.operation_id,
            "revoke",
            &digest,
            Some(&request.source_id),
            &response,
            now_ms,
        )
        .await?;
        tx.commit().await.map_err(db_error)?;
        if holds == 0 {
            self.collect_revoked_sources_locked(app, pool, 1).await?;
        }
        let mut final_response = replay_operation::<RevokeKnowledgeGrantResponse>(
            pool,
            &request.operation_id,
            "revoke",
            &digest,
        )
        .await?
        .ok_or_else(|| "knowledge_revoke_operation_missing_after_commit".to_string())?;
        final_response.replayed = false;
        Ok(final_response)
    }

    pub(crate) async fn active_sources(
        &self,
        app: &AppHandle,
        pool: &SqlitePool,
    ) -> Result<(u64, Vec<ActiveKnowledgeSource>), String> {
        let layout = knowledge_layout(app)?;
        ensure_layout(&layout)?;
        let meta = load_meta(pool).await?;
        let rows = sqlx::query(
            "SELECT source_id, display_name, mime_type, size_bytes, source_sha256, object_name
             FROM local_knowledge_sources
             WHERE revoked_at_ms IS NULL AND index_status = 'ready'
             ORDER BY source_sha256 ASC, source_id ASC",
        )
        .fetch_all(pool)
        .await
        .map_err(db_error)?;
        let mut sources = Vec::with_capacity(rows.len());
        for row in rows {
            let object_name: String = row.try_get("object_name").map_err(db_error)?;
            validate_sha256(&object_name, "object_name")?;
            sources.push(ActiveKnowledgeSource {
                source_id: row.try_get("source_id").map_err(db_error)?,
                display_name: row.try_get("display_name").map_err(db_error)?,
                mime_type: row.try_get("mime_type").map_err(db_error)?,
                size_bytes: from_i64(
                    row.try_get::<i64, _>("size_bytes").map_err(db_error)?,
                    "size_bytes",
                )?,
                source_sha256: row.try_get("source_sha256").map_err(db_error)?,
                object_path: layout.objects.join(object_name),
            });
        }
        Ok((meta.generation, sources))
    }

    pub(crate) async fn projection_sources(
        &self,
        app: &AppHandle,
        pool: &SqlitePool,
        turn_attempt: &str,
        operation_id: &str,
        projection: &super::turn_types::DurableVrefProjection,
    ) -> Result<Vec<ActiveKnowledgeSource>, String> {
        super::knowledge_projection::validate_durable_vref_projection(projection)?;
        let _guard = self.mutation_gate.lock().await;
        let layout = knowledge_layout(app)?;
        ensure_layout(&layout)?;
        let mut output = Vec::with_capacity(projection.sources.len());
        for source in &projection.sources {
            let row = sqlx::query(
                "SELECT s.display_name, s.mime_type, s.size_bytes, s.source_sha256, s.object_name,
                        EXISTS(SELECT 1 FROM local_knowledge_attempt_holds h
                               WHERE h.turn_attempt = ? AND h.operation_id = ?
                                 AND h.source_id = s.source_id
                                 AND h.source_sha256 = s.source_sha256) AS held
                 FROM local_knowledge_sources s WHERE s.source_id = ?",
            )
            .bind(turn_attempt)
            .bind(operation_id)
            .bind(&source.source_id)
            .fetch_optional(pool)
            .await
            .map_err(db_error)?
            .ok_or_else(|| "knowledge_projection_source_missing".to_string())?;
            let held: i64 = row.try_get("held").map_err(db_error)?;
            let object_name: String = row.try_get("object_name").map_err(db_error)?;
            let source_sha256: String = row.try_get("source_sha256").map_err(db_error)?;
            let size_bytes = from_i64(
                row.try_get::<i64, _>("size_bytes").map_err(db_error)?,
                "size_bytes",
            )?;
            if held == 0
                || source_sha256 != source.source_sha256
                || size_bytes != source.size_bytes
                || object_name != source.source_sha256
            {
                return Err("knowledge_projection_hold_mismatch".to_string());
            }
            output.push(ActiveKnowledgeSource {
                source_id: source.source_id.clone(),
                display_name: row.try_get("display_name").map_err(db_error)?,
                mime_type: row.try_get("mime_type").map_err(db_error)?,
                size_bytes,
                source_sha256,
                object_path: layout.objects.join(object_name),
            });
        }
        Ok(output)
    }

    /// Caller must hold `lock_mutation()` until its durable Job admission is committed.
    pub(crate) async fn validate_projection_admission_locked(
        &self,
        pool: &SqlitePool,
        operation_id: &str,
        projection: &super::turn_types::DurableVrefProjection,
    ) -> Result<(), String> {
        super::knowledge_projection::validate_durable_vref_projection(projection)?;
        let generation: i64 = sqlx::query_scalar(
            "SELECT catalog_generation FROM local_knowledge_meta WHERE singleton = 1",
        )
        .fetch_one(pool)
        .await
        .map_err(db_error)?;
        if from_i64(generation, "catalog_generation")? != projection.catalog_generation {
            return Err("knowledge_catalog_generation_changed".to_string());
        }
        for source in &projection.sources {
            let valid: i64 = sqlx::query_scalar(
                "SELECT EXISTS(
                   SELECT 1 FROM local_knowledge_sources s
                   JOIN local_knowledge_attempt_holds h ON h.source_id = s.source_id
                   WHERE h.operation_id = ? AND s.source_id = ? AND s.source_sha256 = ?
                     AND h.source_sha256 = s.source_sha256 AND s.revoked_at_ms IS NULL
                     AND s.index_status = 'ready'
                 )",
            )
            .bind(operation_id)
            .bind(&source.source_id)
            .bind(&source.source_sha256)
            .fetch_one(pool)
            .await
            .map_err(db_error)?;
            if valid == 0 {
                return Err("knowledge_grant_changed_before_admission".to_string());
            }
        }
        Ok(())
    }

    /// Caller must hold `lock_mutation()`. Deletes holds and physically collectable revoked rows
    /// in one transaction, then removes their app-private CAS files.
    pub(crate) async fn release_operation_holds_locked(
        &self,
        app: &AppHandle,
        pool: &SqlitePool,
        operation_id: &str,
    ) -> Result<(), String> {
        let mut tx = pool.begin().await.map_err(db_error)?;
        sqlx::query(
            "UPDATE local_knowledge_meta SET updated_at_ms = updated_at_ms WHERE singleton = 1",
        )
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        sqlx::query("DELETE FROM local_knowledge_attempt_holds WHERE operation_id = ?")
            .bind(operation_id)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        self.collect_revoked_sources_locked(app, pool, 16).await?;
        Ok(())
    }

    async fn collect_revoked_sources_locked(
        &self,
        app: &AppHandle,
        pool: &SqlitePool,
        limit: i64,
    ) -> Result<(), String> {
        let layout = knowledge_layout(app)?;
        let rows = sqlx::query(
            "SELECT source_id, source_sha256, object_name, size_bytes
             FROM local_knowledge_sources s
             WHERE s.revoked_at_ms IS NOT NULL
               AND NOT EXISTS(SELECT 1 FROM local_knowledge_attempt_holds h
                              WHERE h.source_id = s.source_id)
             ORDER BY s.revoked_at_ms ASC, s.source_id ASC LIMIT ?",
        )
        .bind(limit.clamp(1, 32))
        .fetch_all(pool)
        .await
        .map_err(db_error)?;
        for row in rows {
            let source_id: String = row.try_get("source_id").map_err(db_error)?;
            let source_sha256: String = row.try_get("source_sha256").map_err(db_error)?;
            let object_name: String = row.try_get("object_name").map_err(db_error)?;
            let size_bytes = from_i64(
                row.try_get::<i64, _>("size_bytes").map_err(db_error)?,
                "source_size",
            )?;
            validate_source_id(&source_id)?;
            validate_sha256(&source_sha256, "source_sha256")?;
            validate_object_name(&object_name)?;
            if object_name != source_sha256 {
                return Err("knowledge_object_identity_corrupt".to_string());
            }
            let object_path = layout.objects.join(&object_name);
            let delete =
                tauri::async_runtime::spawn_blocking(move || match fs::remove_file(&object_path) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(format!("knowledge_object_delete_failed:{error}")),
                })
                .await
                .map_err(|error| format!("knowledge_object_delete_task_failed:{error}"))?;
            if let Err(error) = delete {
                log::warn!("[VCPMobileCLI] deferred knowledge CAS deletion: {error}");
                continue;
            }

            let mut tx = pool.begin().await.map_err(db_error)?;
            sqlx::query(
                "UPDATE local_knowledge_meta SET updated_at_ms = updated_at_ms WHERE singleton = 1",
            )
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
            let still_collectable: i64 = sqlx::query_scalar(
                "SELECT EXISTS(
                   SELECT 1 FROM local_knowledge_sources s
                   WHERE s.source_id = ? AND s.source_sha256 = ? AND s.object_name = ?
                     AND s.size_bytes = ? AND s.revoked_at_ms IS NOT NULL
                     AND NOT EXISTS(SELECT 1 FROM local_knowledge_attempt_holds h
                                    WHERE h.source_id = s.source_id)
                 )",
            )
            .bind(&source_id)
            .bind(&source_sha256)
            .bind(&object_name)
            .bind(to_i64(size_bytes, "source_size")?)
            .fetch_one(&mut *tx)
            .await
            .map_err(db_error)?;
            if still_collectable == 0 {
                tx.rollback().await.map_err(db_error)?;
                return Err("knowledge_delete_race_detected".to_string());
            }
            sqlx::query("DELETE FROM local_knowledge_sources WHERE source_id = ?")
                .bind(&source_id)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            let updated = sqlx::query(
                "UPDATE local_knowledge_meta
                 SET used_bytes = used_bytes - ?, updated_at_ms = ?
                 WHERE singleton = 1 AND used_bytes >= ?",
            )
            .bind(to_i64(size_bytes, "source_size")?)
            .bind(to_i64(now_ms()?, "updated_at")?)
            .bind(to_i64(size_bytes, "source_size")?)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
            if updated.rows_affected() != 1 {
                return Err("knowledge_used_bytes_accounting_corrupt".to_string());
            }
            update_revoke_operations_deleted(&mut tx, &source_id).await?;
            tx.commit().await.map_err(db_error)?;
        }
        Ok(())
    }

    async fn maintenance(
        &self,
        app: &AppHandle,
        pool: &SqlitePool,
        now_ms: u64,
    ) -> Result<(), String> {
        let layout = knowledge_layout(app)?;
        ensure_layout(&layout)?;
        let expired = sqlx::query(
            "SELECT staging_name, inspect_operation_id FROM local_knowledge_import_candidates
             WHERE expires_at_ms < ? ORDER BY expires_at_ms ASC LIMIT 32",
        )
        .bind(to_i64(now_ms, "now_ms")?)
        .fetch_all(pool)
        .await
        .map_err(db_error)?;
        let mut expired_directories = Vec::with_capacity(expired.len());
        for row in expired {
            let staging_name: String = row.try_get("staging_name").map_err(db_error)?;
            let operation_id: String = row.try_get("inspect_operation_id").map_err(db_error)?;
            validate_staging_name(&staging_name)?;
            validate_operation_id(&operation_id)?;
            sqlx::query("DELETE FROM local_knowledge_import_candidates WHERE staging_name = ?")
                .bind(&staging_name)
                .execute(pool)
                .await
                .map_err(db_error)?;
            sqlx::query("DELETE FROM local_knowledge_operations WHERE operation_id = ?")
                .bind(operation_id)
                .execute(pool)
                .await
                .map_err(db_error)?;
            expired_directories.push(layout.staging.join(staging_name));
        }
        tauri::async_runtime::spawn_blocking(move || {
            for directory in expired_directories {
                remove_candidate_directory(&directory)?;
            }
            Ok::<(), String>(())
        })
        .await
        .map_err(|error| format!("knowledge_expiry_cleanup_task_failed:{error}"))??;
        gc_staging_orphans(pool, &layout).await?;
        self.collect_revoked_sources_locked(app, pool, 16).await?;
        gc_object_orphans(pool, &layout).await?;
        reconcile_used_bytes(pool, now_ms).await?;
        prune_operations(pool).await?;
        Ok(())
    }
}

#[derive(Debug)]
struct KnowledgeLayout {
    root: PathBuf,
    staging: PathBuf,
    objects: PathBuf,
}

#[derive(Debug)]
struct AuditedCandidate {
    size_bytes: u64,
    sha256: String,
    index_text_truncated: bool,
    chunks: Vec<String>,
}

#[derive(Debug)]
struct CandidateRow {
    candidate_sha256: String,
    source_sha256: String,
    staging_name: String,
    display_name: String,
    mime_type: String,
    size_bytes: u64,
    catalog_generation: u64,
    expires_at_ms: u64,
}

#[derive(Debug)]
struct MetaRow {
    generation: u64,
    used_bytes: u64,
}

#[derive(Debug)]
struct QuotaRow {
    pending_count: u64,
    pending_bytes: u64,
}

fn knowledge_layout(app: &AppHandle) -> Result<KnowledgeLayout, String> {
    let root = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("knowledge_root_unavailable:{error}"))?
        .join("vcp-cli")
        .join("knowledge");
    Ok(KnowledgeLayout {
        staging: root.join("staging"),
        objects: root.join("objects"),
        root,
    })
}

fn ensure_layout(layout: &KnowledgeLayout) -> Result<(), String> {
    ensure_private_directory(&layout.root)?;
    ensure_private_directory(&layout.staging)?;
    ensure_private_directory(&layout.objects)?;
    Ok(())
}

fn ensure_private_directory(path: &Path) -> Result<(), String> {
    if path.exists() {
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| format!("knowledge_path_metadata_failed:{error}"))?;
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Err("knowledge_path_not_private_directory".to_string());
        }
    } else {
        fs::create_dir(path).map_err(|error| format!("knowledge_path_create_failed:{error}"))?;
    }
    set_mode(path, 0o700)
}

fn copy_and_audit_candidate(
    source_path: &Path,
    destination: &Path,
    picked: &tauri_plugin_vcp_mobile::system::PickedFileInfo,
) -> Result<AuditedCandidate, String> {
    let mut source = open_regular_nofollow(source_path)?;
    let metadata = source
        .metadata()
        .map_err(|error| format!("knowledge_source_metadata_failed:{error}"))?;
    if metadata.len() != picked.size {
        return Err("knowledge_picker_size_mismatch".to_string());
    }
    let mut destination_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(destination)
        .map_err(|error| format!("knowledge_staging_open_failed:{error}"))?;
    let mut audit = stream_copy_and_audit(&mut source, Some(&mut destination_file))?;
    destination_file
        .sync_all()
        .map_err(|error| format!("knowledge_staging_sync_failed:{error}"))?;
    if audit.size_bytes != picked.size || audit.sha256 != picked.hash.to_ascii_lowercase() {
        let _ = fs::remove_file(destination);
        return Err("knowledge_picker_hash_mismatch".to_string());
    }
    audit.chunks = chunk_index_text(&read_index_text(destination)?)?;
    Ok(audit)
}

fn audit_owned_file(path: &Path) -> Result<AuditedCandidate, String> {
    let mut file = open_regular_nofollow(path)?;
    let mut audit = stream_copy_and_audit(&mut file, None)?;
    audit.chunks = chunk_index_text(&read_index_text(path)?)?;
    Ok(audit)
}

fn stream_copy_and_audit(
    source: &mut File,
    mut destination: Option<&mut File>,
) -> Result<AuditedCandidate, String> {
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = vec![0u8; COPY_BUFFER_BYTES];
    let mut utf8_tail = Vec::with_capacity(4);
    loop {
        let read = source
            .read(&mut buffer)
            .map_err(|error| format!("knowledge_source_read_failed:{error}"))?;
        if read == 0 {
            break;
        }
        size = size
            .checked_add(read as u64)
            .ok_or_else(|| "knowledge_source_size_overflow".to_string())?;
        if size > MAX_SOURCE_BYTES {
            return Err("knowledge_source_too_large".to_string());
        }
        let bytes = &buffer[..read];
        if bytes.contains(&0) {
            return Err("knowledge_source_contains_nul".to_string());
        }
        validate_utf8_chunk(&mut utf8_tail, bytes)?;
        hasher.update(bytes);
        if let Some(file) = destination.as_deref_mut() {
            file.write_all(bytes)
                .map_err(|error| format!("knowledge_staging_write_failed:{error}"))?;
        }
    }
    if !utf8_tail.is_empty() {
        return Err("knowledge_source_not_utf8".to_string());
    }
    Ok(AuditedCandidate {
        size_bytes: size,
        sha256: hex::encode(hasher.finalize()),
        index_text_truncated: size > MAX_INDEX_TEXT_BYTES as u64,
        chunks: Vec::new(),
    })
}

fn validate_utf8_chunk(tail: &mut Vec<u8>, bytes: &[u8]) -> Result<(), String> {
    let mut combined = Vec::with_capacity(tail.len() + bytes.len());
    combined.extend_from_slice(tail);
    combined.extend_from_slice(bytes);
    match std::str::from_utf8(&combined) {
        Ok(_) => tail.clear(),
        Err(error) if error.error_len().is_some() => {
            return Err("knowledge_source_not_utf8".to_string());
        }
        Err(error) => {
            let pending = &combined[error.valid_up_to()..];
            if pending.len() > 3 {
                return Err("knowledge_source_not_utf8".to_string());
            }
            tail.clear();
            tail.extend_from_slice(pending);
        }
    }
    Ok(())
}

fn read_index_text(path: &Path) -> Result<String, String> {
    let mut file = open_regular_nofollow(path)?;
    let mut bytes = Vec::with_capacity(MAX_INDEX_TEXT_BYTES.min(64 * 1024));
    let mut limited = (&mut file).take(MAX_INDEX_TEXT_BYTES as u64 + 3);
    limited
        .read_to_end(&mut bytes)
        .map_err(|error| format!("knowledge_index_read_failed:{error}"))?;
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        bytes.drain(..3);
    }
    if bytes.len() > MAX_INDEX_TEXT_BYTES {
        let mut end = MAX_INDEX_TEXT_BYTES;
        while end > 0 && !is_utf8_boundary(&bytes, end) {
            end -= 1;
        }
        bytes.truncate(end);
    }
    String::from_utf8(bytes).map_err(|_| "knowledge_source_not_utf8".to_string())
}

fn is_utf8_boundary(bytes: &[u8], index: usize) -> bool {
    index == bytes.len()
        || index == 0
        || bytes
            .get(index)
            .is_some_and(|byte| byte & 0b1100_0000 != 0b1000_0000)
}

pub(crate) fn chunk_index_text(text: &str) -> Result<Vec<String>, String> {
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let mut output = Vec::new();
    let mut start = 0usize;
    while start < text.len() && output.len() < MAX_CHUNKS_PER_SOURCE {
        let mut end = (start + MAX_CHUNK_BYTES).min(text.len());
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            return Err("knowledge_chunk_boundary_failed".to_string());
        }
        if end < text.len() {
            let search_start = (start + MAX_CHUNK_BYTES / 2).min(end);
            if let Some(relative) = text[search_start..end].rfind('\n') {
                let candidate = search_start + relative + 1;
                if candidate > start {
                    end = candidate;
                }
            }
        }
        let chunk = &text[start..end];
        if !chunk.trim().is_empty() {
            output.push(chunk.to_string());
        }
        if end == text.len() {
            break;
        }
        let mut next = end.saturating_sub(CHUNK_OVERLAP_BYTES);
        while next < end && !text.is_char_boundary(next) {
            next += 1;
        }
        if next <= start {
            next = end;
        }
        start = next;
    }
    Ok(output)
}

fn publish_object(
    layout: &KnowledgeLayout,
    staging_file: &Path,
    audited: &AuditedCandidate,
) -> Result<PathBuf, String> {
    let final_path = layout.objects.join(&audited.sha256);
    if final_path.exists() {
        let existing = audit_owned_file(&final_path)?;
        if existing.sha256 != audited.sha256 || existing.size_bytes != audited.size_bytes {
            return Err("knowledge_object_collision".to_string());
        }
        return Ok(final_path);
    }
    let temporary = layout.objects.join(format!(
        ".{}.{}.tmp",
        audited.sha256,
        Uuid::new_v4().simple()
    ));
    let result = (|| {
        let mut source = open_regular_nofollow(staging_file)?;
        let mut target = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)
            .map_err(|error| format!("knowledge_object_open_failed:{error}"))?;
        let copied = stream_copy_and_audit(&mut source, Some(&mut target))?;
        if copied.sha256 != audited.sha256 || copied.size_bytes != audited.size_bytes {
            return Err("knowledge_object_copy_changed".to_string());
        }
        target
            .sync_all()
            .map_err(|error| format!("knowledge_object_sync_failed:{error}"))?;
        fs::rename(&temporary, &final_path)
            .map_err(|error| format!("knowledge_object_publish_failed:{error}"))?;
        sync_directory(&layout.objects)?;
        Ok(final_path.clone())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn open_regular_nofollow(path: &Path) -> Result<File, String> {
    reject_unsafe_path(path)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| format!("knowledge_file_open_failed:{error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("knowledge_file_metadata_failed:{error}"))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err("knowledge_file_not_regular".to_string());
    }
    Ok(file)
}

fn reject_unsafe_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("knowledge_path_invalid".to_string());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "knowledge_path_parent_missing".to_string())?;
    let metadata = fs::symlink_metadata(parent)
        .map_err(|error| format!("knowledge_parent_metadata_failed:{error}"))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err("knowledge_parent_not_directory".to_string());
    }
    Ok(())
}

fn validate_picker_metadata(
    picked: &tauri_plugin_vcp_mobile::system::PickedFileInfo,
) -> Result<(), String> {
    if picked.size > MAX_SOURCE_BYTES {
        return Err("knowledge_source_too_large".to_string());
    }
    validate_sha256(&picked.hash.to_ascii_lowercase(), "picker_hash")?;
    let _ = sanitize_display_name(&picked.name)?;
    let _ = normalize_mime(&picked.mime, &picked.name)?;
    if picked.thumbnail_path.is_some() {
        // The thumbnail is deliberately ignored and never crosses the knowledge boundary.
    }
    Ok(())
}

fn sanitize_display_name(name: &str) -> Result<String, String> {
    let basename = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "knowledge_display_name_invalid".to_string())?;
    let cleaned: String = basename
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || cleaned.len() > MAX_DISPLAY_NAME_BYTES {
        return Err("knowledge_display_name_invalid".to_string());
    }
    validate_supported_extension(cleaned)?;
    Ok(cleaned.to_string())
}

fn validate_supported_extension(name: &str) -> Result<(), String> {
    let extension = Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    const ALLOWED: &[&str] = &[
        "txt",
        "md",
        "markdown",
        "json",
        "jsonl",
        "toml",
        "yaml",
        "yml",
        "xml",
        "csv",
        "tsv",
        "ini",
        "conf",
        "cfg",
        "properties",
        "rs",
        "py",
        "js",
        "ts",
        "tsx",
        "jsx",
        "vue",
        "java",
        "kt",
        "kts",
        "c",
        "h",
        "cc",
        "cpp",
        "hpp",
        "cs",
        "go",
        "swift",
        "sh",
        "bash",
        "zsh",
        "fish",
        "ps1",
        "sql",
        "html",
        "css",
        "scss",
        "less",
        "lua",
        "rb",
        "php",
        "r",
        "dart",
        "gradle",
    ];
    if !ALLOWED.contains(&extension.as_str()) {
        return Err("knowledge_file_type_unsupported".to_string());
    }
    Ok(())
}

fn normalize_mime(mime: &str, name: &str) -> Result<String, String> {
    let normalized = mime.trim().to_ascii_lowercase();
    if !normalized.is_empty()
        && normalized.len() <= MAX_MIME_BYTES
        && normalized
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'+' | b'-' | b'.'))
    {
        return Ok(normalized);
    }
    let extension = Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    Ok(match extension.as_str() {
        "md" | "markdown" => "text/markdown",
        "json" | "jsonl" => "application/json",
        "xml" => "application/xml",
        "html" => "text/html",
        "css" => "text/css",
        "csv" => "text/csv",
        _ => "text/plain",
    }
    .to_string())
}

fn validate_operation_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_OPERATION_ID_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
    {
        return Err("knowledge_operation_id_invalid".to_string());
    }
    Ok(())
}

fn validate_candidate_token(value: &str) -> Result<(), String> {
    let suffix = value
        .strip_prefix("vcp-knowledge-candidate:")
        .ok_or_else(|| "knowledge_candidate_token_invalid".to_string())?;
    if suffix.len() != 32 || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("knowledge_candidate_token_invalid".to_string());
    }
    Ok(())
}

fn validate_source_id(value: &str) -> Result<(), String> {
    let suffix = value
        .strip_prefix("vcp-knowledge:")
        .ok_or_else(|| "knowledge_source_id_invalid".to_string())?;
    if suffix.len() != 32 || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("knowledge_source_id_invalid".to_string());
    }
    Ok(())
}

fn validate_staging_name(value: &str) -> Result<(), String> {
    if value.len() != 32
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        || Path::new(value).file_name().and_then(|name| name.to_str()) != Some(value)
    {
        return Err("knowledge_staging_name_invalid".to_string());
    }
    Ok(())
}

fn validate_object_name(value: &str) -> Result<(), String> {
    validate_sha256(value, "object_name")?;
    if Path::new(value).file_name().and_then(|name| name.to_str()) != Some(value) {
        return Err("knowledge_object_name_invalid".to_string());
    }
    Ok(())
}

fn validate_sha256(value: &str, field: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(format!("knowledge_{field}_invalid"));
    }
    Ok(())
}

fn source_id_for_sha(sha256: &str) -> String {
    format!("vcp-knowledge:{}", &sha256[..32])
}

fn candidate_digest(audit: &AuditedCandidate, display_name: &str, mime_type: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"vcp.mobile.knowledge-candidate.v1\0");
    hasher.update(audit.sha256.as_bytes());
    hasher.update([0]);
    hasher.update(audit.size_bytes.to_be_bytes());
    hasher.update(display_name.as_bytes());
    hasher.update([0]);
    hasher.update(mime_type.as_bytes());
    for chunk in &audit.chunks {
        hasher.update([0]);
        hasher.update(sha256_hex(chunk.as_bytes()).as_bytes());
    }
    hex::encode(hasher.finalize())
}

fn request_digest<T: Serialize>(kind: &str, request: &T) -> Result<String, String> {
    let bytes = serde_json::to_vec(request)
        .map_err(|error| format!("knowledge_request_serialize_failed:{error}"))?;
    let mut hasher = Sha256::new();
    hasher.update(b"vcp.mobile.knowledge-operation.v1\0");
    hasher.update(kind.as_bytes());
    hasher.update([0]);
    hasher.update(bytes);
    Ok(hex::encode(hasher.finalize()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

async fn load_meta(pool: &SqlitePool) -> Result<MetaRow, String> {
    let row = sqlx::query(
        "SELECT catalog_generation, used_bytes FROM local_knowledge_meta WHERE singleton = 1",
    )
    .fetch_one(pool)
    .await
    .map_err(db_error)?;
    Ok(MetaRow {
        generation: from_i64(
            row.try_get("catalog_generation").map_err(db_error)?,
            "generation",
        )?,
        used_bytes: from_i64(row.try_get("used_bytes").map_err(db_error)?, "used_bytes")?,
    })
}

async fn load_quota(pool: &SqlitePool) -> Result<QuotaRow, String> {
    let row = sqlx::query(
        "SELECT COUNT(*) AS pending_count, COALESCE(SUM(size_bytes), 0) AS pending_bytes
         FROM local_knowledge_import_candidates",
    )
    .fetch_one(pool)
    .await
    .map_err(db_error)?;
    Ok(QuotaRow {
        pending_count: from_i64(
            row.try_get("pending_count").map_err(db_error)?,
            "pending_count",
        )?,
        pending_bytes: from_i64(
            row.try_get("pending_bytes").map_err(db_error)?,
            "pending_bytes",
        )?,
    })
}

async fn load_candidate(pool: &SqlitePool, token: &str) -> Result<CandidateRow, String> {
    let row = sqlx::query(
        "SELECT candidate_sha256, source_sha256, staging_name, display_name, mime_type,
                size_bytes, catalog_generation, expires_at_ms
         FROM local_knowledge_import_candidates WHERE token = ?",
    )
    .bind(token)
    .fetch_optional(pool)
    .await
    .map_err(db_error)?
    .ok_or_else(|| "knowledge_candidate_not_found".to_string())?;
    let candidate = CandidateRow {
        candidate_sha256: row.try_get("candidate_sha256").map_err(db_error)?,
        source_sha256: row.try_get("source_sha256").map_err(db_error)?,
        staging_name: row.try_get("staging_name").map_err(db_error)?,
        display_name: row.try_get("display_name").map_err(db_error)?,
        mime_type: row.try_get("mime_type").map_err(db_error)?,
        size_bytes: from_i64(row.try_get("size_bytes").map_err(db_error)?, "size_bytes")?,
        catalog_generation: from_i64(
            row.try_get("catalog_generation").map_err(db_error)?,
            "catalog_generation",
        )?,
        expires_at_ms: from_i64(
            row.try_get("expires_at_ms").map_err(db_error)?,
            "expires_at",
        )?,
    };
    validate_sha256(&candidate.candidate_sha256, "candidate_sha256")?;
    validate_sha256(&candidate.source_sha256, "source_sha256")?;
    validate_staging_name(&candidate.staging_name)?;
    Ok(candidate)
}

async fn update_revoke_operations_deleted(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    source_id: &str,
) -> Result<(), String> {
    let rows = sqlx::query(
        "SELECT operation_id, result_json FROM local_knowledge_operations
         WHERE operation_kind = 'revoke' AND source_id = ?",
    )
    .bind(source_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(db_error)?;
    for row in rows {
        let operation_id: String = row.try_get("operation_id").map_err(db_error)?;
        validate_operation_id(&operation_id)?;
        let result_json: String = row.try_get("result_json").map_err(db_error)?;
        let mut response: RevokeKnowledgeGrantResponse = serde_json::from_str(&result_json)
            .map_err(|error| format!("knowledge_operation_result_corrupt:{error}"))?;
        if response.source_id != source_id || response.operation_id != operation_id {
            return Err("knowledge_revoke_operation_identity_corrupt".to_string());
        }
        response.deletion_state = KnowledgeDeletionState::Deleted;
        response.replayed = false;
        let updated_json = serde_json::to_string(&response)
            .map_err(|error| format!("knowledge_operation_result_serialize_failed:{error}"))?;
        sqlx::query("UPDATE local_knowledge_operations SET result_json = ? WHERE operation_id = ?")
            .bind(updated_json)
            .bind(operation_id)
            .execute(&mut **tx)
            .await
            .map_err(db_error)?;
    }
    Ok(())
}

async fn catalog_snapshot(pool: &SqlitePool) -> Result<KnowledgeCatalogSnapshot, String> {
    let meta = load_meta(pool).await?;
    let quota = load_quota(pool).await?;
    let rows = sqlx::query(
        "SELECT source_id, display_name, mime_type, size_bytes, source_sha256, index_status,
                failure_code, index_text_truncated, chunk_count, granted_at_ms
         FROM local_knowledge_sources WHERE revoked_at_ms IS NULL
         ORDER BY granted_at_ms DESC, source_sha256 ASC",
    )
    .fetch_all(pool)
    .await
    .map_err(db_error)?;
    let mut sources = Vec::with_capacity(rows.len());
    for row in rows {
        let index_status: String = row.try_get("index_status").map_err(db_error)?;
        sources.push(KnowledgeSourceDto {
            source_id: row.try_get("source_id").map_err(db_error)?,
            display_name: row.try_get("display_name").map_err(db_error)?,
            mime_type: row.try_get("mime_type").map_err(db_error)?,
            size_bytes: from_i64(row.try_get("size_bytes").map_err(db_error)?, "size_bytes")?,
            source_sha256: row.try_get("source_sha256").map_err(db_error)?,
            index_status: match index_status.as_str() {
                "indexing" => KnowledgeIndexStatus::Indexing,
                "ready" => KnowledgeIndexStatus::Ready,
                "failed" => KnowledgeIndexStatus::Failed,
                _ => return Err("knowledge_index_status_corrupt".to_string()),
            },
            failure_code: row.try_get("failure_code").map_err(db_error)?,
            index_text_truncated: row
                .try_get::<i64, _>("index_text_truncated")
                .map_err(db_error)?
                != 0,
            chunk_count: u32::try_from(row.try_get::<i64, _>("chunk_count").map_err(db_error)?)
                .map_err(|_| "knowledge_chunk_count_invalid".to_string())?,
            granted_at_ms: from_i64(
                row.try_get("granted_at_ms").map_err(db_error)?,
                "granted_at_ms",
            )?,
        });
    }
    Ok(KnowledgeCatalogSnapshot {
        schema_version: KNOWLEDGE_SCHEMA_VERSION,
        catalog_generation: meta.generation,
        used_bytes: meta.used_bytes,
        limit_bytes: MAX_CAS_BYTES,
        pending_used_bytes: quota.pending_bytes,
        pending_limit_bytes: MAX_PENDING_BYTES,
        active_source_count: sources.len() as u64,
        active_source_limit: MAX_ACTIVE_SOURCES,
        pending_candidate_count: quota.pending_count,
        pending_candidate_limit: MAX_PENDING_CANDIDATES,
        sources,
    })
}

async fn replay_operation<T: DeserializeOwned>(
    pool: &SqlitePool,
    operation_id: &str,
    kind: &str,
    digest: &str,
) -> Result<Option<T>, String> {
    let row = sqlx::query(
        "SELECT operation_kind, request_sha256, result_json
         FROM local_knowledge_operations WHERE operation_id = ?",
    )
    .bind(operation_id)
    .fetch_optional(pool)
    .await
    .map_err(db_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let existing_kind: String = row.try_get("operation_kind").map_err(db_error)?;
    let existing_digest: String = row.try_get("request_sha256").map_err(db_error)?;
    if existing_kind != kind || existing_digest != digest {
        return Err("knowledge_operation_conflict".to_string());
    }
    let result_json: String = row.try_get("result_json").map_err(db_error)?;
    serde_json::from_str(&result_json)
        .map(Some)
        .map_err(|error| format!("knowledge_operation_result_corrupt:{error}"))
}

async fn record_operation<T: Serialize>(
    pool: &SqlitePool,
    operation_id: &str,
    kind: &str,
    digest: &str,
    source_id: Option<&str>,
    result: &T,
    now_ms: u64,
) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(db_error)?;
    insert_operation_tx(
        &mut tx,
        operation_id,
        kind,
        digest,
        source_id,
        result,
        now_ms,
    )
    .await?;
    tx.commit().await.map_err(db_error)
}

async fn prune_operations(pool: &SqlitePool) -> Result<(), String> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_knowledge_operations")
        .fetch_one(pool)
        .await
        .map_err(db_error)?;
    let excess = total.saturating_sub(MAX_RETAINED_OPERATIONS);
    if excess == 0 {
        return Ok(());
    }
    // Candidate inspect operations and operations tied to a source row are still required for
    // replay. Only terminal rows whose durable target has already disappeared are eligible.
    sqlx::query(
        "DELETE FROM local_knowledge_operations
         WHERE operation_id IN (
           SELECT o.operation_id
           FROM local_knowledge_operations o
           LEFT JOIN local_knowledge_import_candidates c
             ON c.inspect_operation_id = o.operation_id
           LEFT JOIN local_knowledge_sources s ON s.source_id = o.source_id
           WHERE c.token IS NULL AND (o.source_id IS NULL OR s.source_id IS NULL)
           ORDER BY o.created_at_ms ASC, o.operation_id ASC
           LIMIT ?
         )",
    )
    .bind(excess)
    .execute(pool)
    .await
    .map_err(db_error)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_operation_tx<T: Serialize>(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    operation_id: &str,
    kind: &str,
    digest: &str,
    source_id: Option<&str>,
    result: &T,
    now_ms: u64,
) -> Result<(), String> {
    let result_json = serde_json::to_string(result)
        .map_err(|error| format!("knowledge_operation_result_serialize_failed:{error}"))?;
    if result_json.len() > 256 * 1024 {
        return Err("knowledge_operation_result_too_large".to_string());
    }
    sqlx::query(
        "INSERT INTO local_knowledge_operations(
           operation_id, operation_kind, request_sha256, source_id, result_json, created_at_ms
         ) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(operation_id)
    .bind(kind)
    .bind(digest)
    .bind(source_id)
    .bind(result_json)
    .bind(to_i64(now_ms, "operation_created_at")?)
    .execute(&mut **tx)
    .await
    .map_err(db_error)?;
    Ok(())
}

async fn object_referenced(pool: &SqlitePool, sha256: &str) -> Result<bool, String> {
    let exists: i64 = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM local_knowledge_sources WHERE source_sha256 = ?)",
    )
    .bind(sha256)
    .fetch_one(pool)
    .await
    .map_err(db_error)?;
    Ok(exists != 0)
}

async fn gc_object_orphans(pool: &SqlitePool, layout: &KnowledgeLayout) -> Result<(), String> {
    let referenced: Vec<String> =
        sqlx::query_scalar("SELECT object_name FROM local_knowledge_sources")
            .fetch_all(pool)
            .await
            .map_err(db_error)?;
    for name in &referenced {
        validate_object_name(name)?;
    }
    let referenced: std::collections::BTreeSet<_> = referenced.into_iter().collect();
    let objects = layout.objects.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let entries = fs::read_dir(&objects)
            .map_err(|error| format!("knowledge_objects_scan_failed:{error}"))?;
        for entry in entries.take(128) {
            let entry = entry.map_err(|error| format!("knowledge_object_entry_failed:{error}"))?;
            let name = entry
                .file_name()
                .to_str()
                .ok_or_else(|| "knowledge_object_name_invalid".to_string())?
                .to_string();
            if referenced.contains(&name) {
                continue;
            }
            if !is_object_or_temporary_name(&name) {
                return Err("knowledge_object_name_invalid".to_string());
            }
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| format!("knowledge_object_metadata_failed:{error}"))?;
            if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                return Err("knowledge_object_not_regular".to_string());
            }
            fs::remove_file(entry.path())
                .map_err(|error| format!("knowledge_object_delete_failed:{error}"))?;
        }
        sync_directory(&objects)
    })
    .await
    .map_err(|error| format!("knowledge_object_gc_task_failed:{error}"))?
}

async fn gc_staging_orphans(pool: &SqlitePool, layout: &KnowledgeLayout) -> Result<(), String> {
    let referenced = sqlx::query_scalar::<_, String>(
        "SELECT staging_name FROM local_knowledge_import_candidates",
    )
    .fetch_all(pool)
    .await
    .map_err(db_error)?;
    for name in &referenced {
        validate_staging_name(name)?;
    }
    let referenced: std::collections::BTreeSet<_> = referenced.into_iter().collect();
    let staging = layout.staging.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let entries = fs::read_dir(&staging)
            .map_err(|error| format!("knowledge_staging_scan_failed:{error}"))?;
        for entry in entries.take(64) {
            let entry = entry.map_err(|error| format!("knowledge_staging_entry_failed:{error}"))?;
            let name = entry
                .file_name()
                .to_str()
                .ok_or_else(|| "knowledge_staging_name_invalid".to_string())?
                .to_string();
            validate_staging_name(&name)?;
            if !referenced.contains(&name) {
                remove_candidate_directory(&entry.path())?;
            }
        }
        sync_directory(&staging)
    })
    .await
    .map_err(|error| format!("knowledge_staging_gc_task_failed:{error}"))?
}

async fn reconcile_used_bytes(pool: &SqlitePool, now_ms: u64) -> Result<(), String> {
    let physical: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(size_bytes), 0) FROM local_knowledge_sources")
            .fetch_one(pool)
            .await
            .map_err(db_error)?;
    if physical < 0 {
        return Err("knowledge_used_bytes_accounting_corrupt".to_string());
    }
    sqlx::query(
        "UPDATE local_knowledge_meta SET used_bytes = ?, updated_at_ms = ? WHERE singleton = 1",
    )
    .bind(physical)
    .bind(to_i64(now_ms, "updated_at")?)
    .execute(pool)
    .await
    .map_err(db_error)?;
    Ok(())
}

fn remove_candidate_directory(directory: &Path) -> Result<(), String> {
    match fs::symlink_metadata(directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err("knowledge_staging_not_directory".to_string()),
        Err(error) => return Err(format!("knowledge_staging_metadata_failed:{error}")),
    }
    for entry in fs::read_dir(directory)
        .map_err(|error| format!("knowledge_staging_scan_failed:{error}"))?
        .take(4)
    {
        let entry = entry.map_err(|error| format!("knowledge_staging_entry_failed:{error}"))?;
        let name = entry
            .file_name()
            .to_str()
            .ok_or_else(|| "knowledge_staging_file_invalid".to_string())?
            .to_string();
        if name != CANDIDATE_FILE_NAME {
            return Err("knowledge_staging_file_invalid".to_string());
        }
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| format!("knowledge_staging_metadata_failed:{error}"))?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err("knowledge_staging_file_not_regular".to_string());
        }
        fs::remove_file(entry.path())
            .map_err(|error| format!("knowledge_staging_delete_failed:{error}"))?;
    }
    fs::remove_dir(directory).map_err(|error| format!("knowledge_staging_delete_failed:{error}"))
}

fn is_object_or_temporary_name(value: &str) -> bool {
    if validate_object_name(value).is_ok() {
        return true;
    }
    let Some(body) = value
        .strip_prefix('.')
        .and_then(|name| name.strip_suffix(".tmp"))
    else {
        return false;
    };
    let Some((sha, nonce)) = body.split_once('.') else {
        return false;
    };
    validate_object_name(sha).is_ok()
        && nonce.len() == 32
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn ensure_free_space(path: &Path, required: u64) -> Result<(), String> {
    let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| "knowledge_storage_path_invalid".to_string())?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let result = unsafe { libc::statvfs(c_path.as_ptr(), stats.as_mut_ptr()) };
    if result != 0 {
        return Err(format!(
            "knowledge_storage_stat_failed:{}",
            std::io::Error::last_os_error()
        ));
    }
    let stats = unsafe { stats.assume_init() };
    let free = (stats.f_bavail as u128)
        .saturating_mul(stats.f_frsize as u128)
        .min(u64::MAX as u128) as u64;
    if free < required {
        return Err("knowledge_low_storage".to_string());
    }
    Ok(())
}

fn set_mode(path: &Path, mode: u32) -> Result<(), String> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| format!("knowledge_set_mode_failed:{error}"))
}

fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("knowledge_directory_sync_failed:{error}"))
}

fn remove_picker_source(path: &Path) {
    if path.is_absolute() {
        let _ = fs::remove_file(path);
    }
}

fn now_ms() -> Result<u64, String> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("knowledge_clock_failed:{error}"))?;
    u64::try_from(duration.as_millis()).map_err(|_| "knowledge_clock_overflow".to_string())
}

fn db_error(error: impl std::fmt::Display) -> String {
    format!("knowledge_database_failed:{error}")
}

fn to_i64(value: u64, field: &str) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| format!("knowledge_{field}_overflow"))
}

fn from_i64(value: i64, field: &str) -> Result<u64, String> {
    u64::try_from(value).map_err(|_| format!("knowledge_{field}_invalid"))
}

#[tauri::command]
pub async fn get_vcp_mobile_cli_knowledge_catalog(
    app: AppHandle,
    db: State<'_, DbState>,
    owner: State<'_, KnowledgeCatalogOwner>,
) -> Result<KnowledgeCatalogSnapshot, String> {
    owner.catalog(&app, &db.pool).await
}

#[tauri::command]
pub async fn inspect_vcp_mobile_cli_knowledge_import(
    app: AppHandle,
    db: State<'_, DbState>,
    owner: State<'_, KnowledgeCatalogOwner>,
    request: InspectKnowledgeImportRequest,
) -> Result<InspectKnowledgeImportResponse, String> {
    owner.inspect(&app, &db.pool, request).await
}

#[tauri::command]
pub async fn commit_vcp_mobile_cli_knowledge_import(
    app: AppHandle,
    db: State<'_, DbState>,
    owner: State<'_, KnowledgeCatalogOwner>,
    request: CommitKnowledgeImportRequest,
) -> Result<CommitKnowledgeImportResponse, String> {
    owner.commit(&app, &db.pool, request).await
}

#[tauri::command]
pub async fn discard_vcp_mobile_cli_knowledge_import(
    app: AppHandle,
    db: State<'_, DbState>,
    owner: State<'_, KnowledgeCatalogOwner>,
    request: DiscardKnowledgeImportRequest,
) -> Result<DiscardKnowledgeImportResponse, String> {
    owner.discard(&app, &db.pool, request).await
}

#[tauri::command]
pub async fn revoke_vcp_mobile_cli_knowledge_grant(
    app: AppHandle,
    db: State<'_, DbState>,
    owner: State<'_, KnowledgeCatalogOwner>,
    request: RevokeKnowledgeGrantRequest,
) -> Result<RevokeKnowledgeGrantResponse, String> {
    owner.revoke(&app, &db.pool, request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_are_utf8_bounded_and_overlap() {
        let text = format!("{}\n{}", "中".repeat(900), "后".repeat(900));
        let chunks = chunk_index_text(&text).expect("chunking should succeed");
        assert!(chunks.len() >= 2);
        assert!(chunks.iter().all(|chunk| chunk.len() <= MAX_CHUNK_BYTES));
        assert!(chunks
            .iter()
            .all(|chunk| std::str::from_utf8(chunk.as_bytes()).is_ok()));
        assert!(chunks.windows(2).all(|pair| {
            let suffix = &pair[0][pair[0].len().saturating_sub(CHUNK_OVERLAP_BYTES)..];
            pair[1].starts_with(suffix) || pair[0].ends_with('\n')
        }));
    }

    #[test]
    fn operation_and_tokens_are_closed() {
        assert!(validate_operation_id("knowledge:inspect:1").is_ok());
        assert!(validate_operation_id("bad operation").is_err());
        assert!(validate_candidate_token(
            "vcp-knowledge-candidate:0123456789abcdef0123456789abcdef"
        )
        .is_ok());
        assert!(validate_candidate_token("../../candidate").is_err());
    }

    #[test]
    fn streaming_utf8_carries_split_multibyte_sequences() {
        let mut tail = Vec::new();
        let bytes = "中文".as_bytes();
        validate_utf8_chunk(&mut tail, &bytes[..2]).expect("prefix is incomplete, not invalid");
        assert_eq!(tail, &bytes[..2]);
        validate_utf8_chunk(&mut tail, &bytes[2..]).expect("continuation closes sequence");
        assert!(tail.is_empty());
        assert!(validate_utf8_chunk(&mut tail, &[0xff]).is_err());
    }
}
