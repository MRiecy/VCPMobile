//! Frozen, offline Model2Vec owner for `river=semantic:N`.
//!
//! The model/tokenizer are APK assets staged by the existing Android CLI installer. Runtime
//! validation is fail-closed, inference is mmap-only, and SQLite stores only disposable vectors.

use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fancy_regex::Regex;
use half::f16;
use memmap2::{Mmap, MmapOptions};
use safetensors::{Dtype, SafeTensors};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::{Pool, QueryBuilder, Row, Sqlite};
use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;

const EMBEDDED_SEMANTIC_PROFILE_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/runtime-assets/vcp-cli/semantic-profile.json"
));
pub const FROZEN_SEMANTIC_MODEL_ID: &str =
    "Nourh7/granite-embedding-97m-multilingual-r2@b77044bfd84eef0b552c5346eeacc851264592b3-mobile64-v1";
const PACK_MAGIC: &[u8; 8] = b"VCPBPE1\0";
const PACK_HEADER_BYTES: usize = 24;
const PACK_VOCAB_ROW_BYTES: usize = 12;
const PACK_MERGE_ROW_BYTES: usize = 16;
const HASH_BUFFER_BYTES: usize = 64 * 1024;
const VECTOR_BYTES_PER_DIMENSION: usize = 4;
const CACHE_LOOKUP_CHUNK: usize = 128;
const MAX_SEMANTIC_QUERY_BYTES: usize = 2_000;
const MAX_SEMANTIC_SOURCE_BYTES: usize = 16 * 1024;
const MAX_SEMANTIC_SELECTION_MS: u64 = 60_000;
pub(crate) const MAX_SEMANTIC_CANDIDATES: usize = 512;
pub(crate) const MAX_SEMANTIC_CANDIDATE_BYTES: usize = 4 * 1024 * 1024;
const ADDED_TOKEN: &str = "<|endoftext|>";
const ADDED_TOKEN_ID: u32 = 179_934;
const PRETOKEN_PATTERN: &str = r"[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]*[\p{Ll}\p{Lm}\p{Lo}\p{M}]+(?i:'s|'t|'re|'ve|'m|'ll|'d)?|[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]+[\p{Ll}\p{Lm}\p{Lo}\p{M}]*(?i:'s|'t|'re|'ve|'m|'ll|'d)?|\p{N}{1,3}| ?[^\s\p{L}\p{N}]+[\r\n/]*|\s*[\r\n]+|\s+(?!\S)|\s+";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SemanticProfile {
    pub schema_version: u32,
    pub model_id: String,
    pub upstream_repository: String,
    pub upstream_revision: String,
    pub license: String,
    pub dimension: usize,
    pub max_tokens: usize,
    pub median_token_length: usize,
    pub upstream_config: SemanticSourceProfile,
    pub model: SemanticAssetProfile,
    pub tokenizer_pack: SemanticTokenizerProfile,
    pub cache: SemanticCacheProfile,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SemanticAssetProfile {
    pub asset: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SemanticSourceProfile {
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SemanticTokenizerProfile {
    pub asset: String,
    pub bytes: u64,
    pub sha256: String,
    pub vocab_size: usize,
    pub merge_count: usize,
    pub string_bytes: usize,
    pub source_bytes: u64,
    pub source_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SemanticCacheProfile {
    pub schema_version: u32,
    pub max_rows: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticCandidate {
    pub source_index: usize,
    pub content_sha256: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticSelection {
    pub source_indices: Vec<usize>,
    pub model_id: String,
}

pub struct SemanticSelectRequest<'a> {
    pub model_path: &'a Path,
    pub tokenizer_path: &'a Path,
    pub query: &'a str,
    pub candidates: &'a [SemanticCandidate],
    pub limit: usize,
    pub now_ms: i64,
    pub cancellation_token: CancellationToken,
    pub deadline_at_ms: u64,
}

pub struct LocalEmbeddingOwner {
    loaded: Mutex<Option<LoadedSemanticModel>>,
    selection_permit: Semaphore,
}

#[derive(Clone)]
struct SemanticWorkBudget {
    cancellation_token: CancellationToken,
    deadline: Instant,
}

struct LoadedSemanticModel {
    model_path: PathBuf,
    tokenizer_path: PathBuf,
    engine: Arc<SemanticEngine>,
}

struct SemanticEngine {
    profile: SemanticProfile,
    tokenizer: CompactBpe,
    model_mmap: Mmap,
    embedding_offset: usize,
    embedding_bytes: usize,
    weight_offset: usize,
    weight_bytes: usize,
    rows: usize,
}

struct CompactBpe {
    mmap: Mmap,
    vocab_count: usize,
    merge_count: usize,
    merge_offset: usize,
    string_offset: usize,
    regex: Regex,
    word_regex: Regex,
    byte_chars: [char; 256],
}

#[derive(Clone, Copy)]
struct BpeNode {
    id: u32,
    previous: Option<usize>,
    next: Option<usize>,
    alive: bool,
}

type MergeCandidate = Reverse<(u32, usize, u32, u32, u32)>;

impl SemanticWorkBudget {
    fn new(
        cancellation_token: CancellationToken,
        turn_deadline_at_ms: u64,
    ) -> Result<Self, String> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| format!("system clock is before the Unix epoch: {error}"))?
            .as_millis();
        let now_ms = u64::try_from(now_ms)
            .map_err(|_| "semantic clock exceeds the supported range".to_string())?;
        if cancellation_token.is_cancelled() {
            return Err("semantic selection cancelled".to_string());
        }
        if now_ms >= turn_deadline_at_ms {
            return Err("semantic selection reached the turn deadline".to_string());
        }
        let remaining_ms = turn_deadline_at_ms
            .saturating_sub(now_ms)
            .min(MAX_SEMANTIC_SELECTION_MS);
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(remaining_ms))
            .ok_or_else(|| "semantic work deadline overflowed".to_string())?;
        Ok(Self {
            cancellation_token,
            deadline,
        })
    }

    #[cfg(test)]
    fn unbounded() -> Self {
        Self {
            cancellation_token: CancellationToken::new(),
            deadline: Instant::now() + Duration::from_secs(300),
        }
    }

    fn check(&self) -> Result<(), String> {
        if self.cancellation_token.is_cancelled() {
            return Err("semantic selection cancelled".to_string());
        }
        if Instant::now() >= self.deadline {
            return Err("semantic selection exceeded its 60 second work budget".to_string());
        }
        Ok(())
    }
}

impl LocalEmbeddingOwner {
    pub fn new() -> Self {
        Self {
            loaded: Mutex::new(None),
            selection_permit: Semaphore::new(1),
        }
    }

    pub async fn select(
        &self,
        pool: &Pool<Sqlite>,
        request: SemanticSelectRequest<'_>,
    ) -> Result<SemanticSelection, String> {
        let query = bounded_query(request.query)?;
        if request.candidates.is_empty() || request.limit == 0 {
            return Err("semantic candidates are empty".to_string());
        }
        let candidate_bytes = request
            .candidates
            .iter()
            .try_fold(0_usize, |total, candidate| {
                total.checked_add(candidate.text.len())
            })
            .ok_or_else(|| "semantic candidate byte count overflowed".to_string())?;
        if request.candidates.len() > MAX_SEMANTIC_CANDIDATES
            || candidate_bytes > MAX_SEMANTIC_CANDIDATE_BYTES
        {
            return Err("semantic candidate set exceeds its bounded local budget".to_string());
        }
        let budget =
            SemanticWorkBudget::new(request.cancellation_token.clone(), request.deadline_at_ms)?;
        let permit = tokio::select! {
            _ = request.cancellation_token.cancelled() => {
                return Err("semantic selection cancelled".to_string());
            }
            result = tokio::time::timeout_at(
                tokio::time::Instant::from_std(budget.deadline),
                self.selection_permit.acquire(),
            ) => result
                .map_err(|_| "semantic selection exceeded its 60 second work budget".to_string())?
                .map_err(|_| "semantic selection owner is closed".to_string())?,
        };
        budget.check()?;
        let engine = self
            .engine(request.model_path, request.tokenizer_path, &budget)
            .await?;
        let query = query.to_string();
        let query_engine = Arc::clone(&engine);
        let query_budget = budget.clone();
        let query_vector = tauri::async_runtime::spawn_blocking(move || {
            query_engine.embed_with_budget(&query, &query_budget)
        })
        .await
        .map_err(|error| format!("semantic query task failed: {error}"))??;
        budget.check()?;

        let mut vectors = HashMap::new();
        let expected_bytes = engine
            .profile
            .dimension
            .checked_mul(VECTOR_BYTES_PER_DIMENSION)
            .ok_or_else(|| "semantic vector byte length overflowed".to_string())?;
        let candidate_hashes = request
            .candidates
            .iter()
            .map(|candidate| candidate.content_sha256.as_str())
            .collect::<HashSet<_>>();
        for hashes in candidate_hashes
            .iter()
            .copied()
            .collect::<Vec<_>>()
            .chunks(CACHE_LOOKUP_CHUNK)
        {
            budget.check()?;
            let mut query = QueryBuilder::<Sqlite>::new(
                "SELECT content_sha256, vector FROM local_semantic_embedding_cache WHERE model_id = ",
            );
            query
                .push_bind(&engine.profile.model_id)
                .push(" AND dimension = ")
                .push_bind(engine.profile.dimension as i64)
                .push(" AND length(vector) = ")
                .push_bind(expected_bytes as i64)
                .push(" AND content_sha256 IN (");
            let mut separated = query.separated(", ");
            for hash in hashes.iter().copied() {
                separated.push_bind(hash);
            }
            separated.push_unseparated(")");
            for row in query
                .build()
                .fetch_all(pool)
                .await
                .map_err(|error| format!("cannot read semantic cache: {error}"))?
            {
                budget.check()?;
                let hash: String = row
                    .try_get("content_sha256")
                    .map_err(|error| format!("invalid semantic cache hash: {error}"))?;
                let bytes: Vec<u8> = row
                    .try_get("vector")
                    .map_err(|error| format!("invalid semantic cache vector: {error}"))?;
                // A damaged derived row is a cache miss. It must never abort the command or make
                // us deserialize unrelated model rows into memory.
                if let Ok(vector) = decode_vector(&bytes) {
                    vectors.insert(hash, vector);
                }
            }
        }

        let mut missing = Vec::new();
        let mut missing_hashes = HashSet::new();
        for candidate in request.candidates {
            budget.check()?;
            if !vectors.contains_key(&candidate.content_sha256)
                && missing_hashes.insert(candidate.content_sha256.clone())
            {
                missing.push(candidate.clone());
            }
        }
        if !missing.is_empty() {
            let missing_engine = Arc::clone(&engine);
            let missing_budget = budget.clone();
            let computed = tauri::async_runtime::spawn_blocking(move || {
                missing
                    .into_iter()
                    .map(|candidate| {
                        missing_budget.check()?;
                        let vector =
                            missing_engine.embed_with_budget(&candidate.text, &missing_budget)?;
                        Ok::<_, String>((candidate.content_sha256, vector))
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .await
            .map_err(|error| format!("semantic candidate task failed: {error}"))??;
            budget.check()?;
            let mut transaction = pool
                .begin()
                .await
                .map_err(|error| format!("cannot begin semantic cache transaction: {error}"))?;
            for (hash, vector) in computed {
                budget.check()?;
                let bytes = encode_vector(&vector);
                sqlx::query(
                    "INSERT INTO local_semantic_embedding_cache \
                     (model_id, content_sha256, dimension, vector, created_at_ms, last_used_at_ms) \
                     VALUES (?, ?, ?, ?, ?, ?) \
                     ON CONFLICT(model_id, content_sha256) DO UPDATE SET \
                     dimension = excluded.dimension, vector = excluded.vector, \
                     last_used_at_ms = excluded.last_used_at_ms",
                )
                .bind(&engine.profile.model_id)
                .bind(&hash)
                .bind(engine.profile.dimension as i64)
                .bind(bytes)
                .bind(request.now_ms)
                .bind(request.now_ms)
                .execute(&mut *transaction)
                .await
                .map_err(|error| format!("cannot persist semantic vector: {error}"))?;
                vectors.insert(hash, vector);
            }
            prune_cache(
                &mut transaction,
                &engine.profile.model_id,
                engine.profile.cache.max_rows,
            )
            .await?;
            budget.check()?;
            transaction
                .commit()
                .await
                .map_err(|error| format!("cannot commit semantic cache: {error}"))?;
        }

        budget.check()?;
        let mut touch = pool
            .begin()
            .await
            .map_err(|error| format!("cannot begin semantic cache touch: {error}"))?;
        invalidate_old_model_cache(&mut touch, &engine.profile.model_id).await?;
        for candidate in request.candidates {
            budget.check()?;
            sqlx::query(
                "UPDATE local_semantic_embedding_cache SET last_used_at_ms = ? \
                 WHERE model_id = ? AND content_sha256 = ?",
            )
            .bind(request.now_ms)
            .bind(&engine.profile.model_id)
            .bind(&candidate.content_sha256)
            .execute(&mut *touch)
            .await
            .map_err(|error| format!("cannot touch semantic cache row: {error}"))?;
        }
        prune_cache(
            &mut touch,
            &engine.profile.model_id,
            engine.profile.cache.max_rows,
        )
        .await?;
        budget.check()?;
        touch
            .commit()
            .await
            .map_err(|error| format!("cannot commit semantic cache touch: {error}"))?;

        let mut scored = request
            .candidates
            .iter()
            .filter_map(|candidate| {
                vectors.get(&candidate.content_sha256).map(|vector| {
                    (
                        cosine(&query_vector, vector),
                        candidate.source_index,
                        candidate.content_sha256.as_str(),
                    )
                })
            })
            .collect::<Vec<_>>();
        budget.check()?;
        if scored.len() != request.candidates.len() {
            return Err("semantic cache did not cover every eligible candidate".to_string());
        }
        scored.sort_by(|left, right| {
            right
                .0
                .partial_cmp(&left.0)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.cmp(right.2))
        });
        let mut source_indices = scored
            .into_iter()
            .take(request.limit.min(request.candidates.len()))
            .map(|(_, index, _)| index)
            .collect::<Vec<_>>();
        source_indices.sort_unstable();
        drop(permit);

        Ok(SemanticSelection {
            source_indices,
            model_id: engine.profile.model_id.clone(),
        })
    }

    async fn engine(
        &self,
        model_path: &Path,
        tokenizer_path: &Path,
        budget: &SemanticWorkBudget,
    ) -> Result<Arc<SemanticEngine>, String> {
        budget.check()?;
        let mut loaded = self.loaded.lock().await;
        budget.check()?;
        if let Some(current) = loaded.as_ref() {
            if current.model_path == model_path && current.tokenizer_path == tokenizer_path {
                return Ok(Arc::clone(&current.engine));
            }
        }
        let model = model_path.to_path_buf();
        let tokenizer = tokenizer_path.to_path_buf();
        let open_model = model.clone();
        let open_tokenizer = tokenizer.clone();
        let open_budget = budget.clone();
        let engine = tauri::async_runtime::spawn_blocking(move || {
            SemanticEngine::open_with_budget(&open_model, &open_tokenizer, &open_budget)
                .map(Arc::new)
        })
        .await
        .map_err(|error| format!("semantic model load task failed: {error}"))??;
        budget.check()?;
        *loaded = Some(LoadedSemanticModel {
            model_path: model,
            tokenizer_path: tokenizer,
            engine: Arc::clone(&engine),
        });
        Ok(engine)
    }
}

impl SemanticEngine {
    #[cfg(test)]
    fn open(model_path: &Path, tokenizer_path: &Path) -> Result<Self, String> {
        Self::open_with_budget(model_path, tokenizer_path, &SemanticWorkBudget::unbounded())
    }

    fn open_with_budget(
        model_path: &Path,
        tokenizer_path: &Path,
        budget: &SemanticWorkBudget,
    ) -> Result<Self, String> {
        budget.check()?;
        let profile = embedded_semantic_profile()?;
        verify_regular_asset_with_budget(
            model_path,
            profile.model.bytes,
            &profile.model.sha256,
            "semantic model",
            budget,
        )?;
        verify_regular_asset_with_budget(
            tokenizer_path,
            profile.tokenizer_pack.bytes,
            &profile.tokenizer_pack.sha256,
            "semantic tokenizer",
            budget,
        )?;
        let tokenizer = CompactBpe::open(tokenizer_path, &profile, budget)?;
        budget.check()?;
        let model_file = File::open(model_path)
            .map_err(|error| format!("cannot open semantic model: {error}"))?;
        let model_mmap = unsafe { MmapOptions::new().map(&model_file) }
            .map_err(|error| format!("cannot mmap semantic model: {error}"))?;
        let tensors = SafeTensors::deserialize(&model_mmap)
            .map_err(|error| format!("invalid semantic safetensors: {error}"))?;
        budget.check()?;
        let embeddings = tensors
            .tensor("embeddings")
            .map_err(|error| format!("missing embeddings tensor: {error}"))?;
        let weights = tensors
            .tensor("weights")
            .map_err(|error| format!("missing weights tensor: {error}"))?;
        if embeddings.dtype() != Dtype::F16 || weights.dtype() != Dtype::F64 {
            return Err(
                "semantic model tensor dtypes do not match the frozen contract".to_string(),
            );
        }
        if embeddings.shape().len() != 2
            || embeddings.shape()[1] != profile.dimension
            || weights.shape() != [embeddings.shape()[0]]
        {
            return Err(
                "semantic model tensor shapes do not match the frozen contract".to_string(),
            );
        }
        let base = model_mmap.as_ptr() as usize;
        let embedding_offset = embeddings.data().as_ptr() as usize - base;
        let weight_offset = weights.data().as_ptr() as usize - base;
        Ok(Self {
            rows: embeddings.shape()[0],
            embedding_offset,
            embedding_bytes: embeddings.data().len(),
            weight_offset,
            weight_bytes: weights.data().len(),
            profile,
            tokenizer,
            model_mmap,
        })
    }

    #[cfg(test)]
    fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        self.embed_with_budget(text, &SemanticWorkBudget::unbounded())
    }

    fn embed_with_budget(
        &self,
        text: &str,
        budget: &SemanticWorkBudget,
    ) -> Result<Vec<f32>, String> {
        budget.check()?;
        let ids = self.token_ids_with_budget(text, budget)?;
        if ids.is_empty() {
            return Err("semantic input produced no tokens".to_string());
        }
        let embeddings = self
            .model_mmap
            .get(self.embedding_offset..self.embedding_offset + self.embedding_bytes)
            .ok_or_else(|| "semantic embeddings tensor escaped mmap".to_string())?;
        let weights = self
            .model_mmap
            .get(self.weight_offset..self.weight_offset + self.weight_bytes)
            .ok_or_else(|| "semantic weights tensor escaped mmap".to_string())?;
        let mut vector = vec![0.0_f64; self.profile.dimension];
        for id in &ids {
            budget.check()?;
            let row = usize::try_from(*id)
                .map_err(|_| "semantic token ID cannot fit usize".to_string())?;
            if row >= self.rows {
                return Err("semantic token ID exceeds model rows".to_string());
            }
            let weight_offset = row
                .checked_mul(8)
                .ok_or_else(|| "semantic weight offset overflowed".to_string())?;
            let weight = f64::from_le_bytes(
                weights
                    .get(weight_offset..weight_offset + 8)
                    .ok_or_else(|| "semantic weight tensor is truncated".to_string())?
                    .try_into()
                    .map_err(|_| "semantic weight bytes are invalid".to_string())?,
            );
            let row_offset = row
                .checked_mul(self.profile.dimension)
                .and_then(|value| value.checked_mul(2))
                .ok_or_else(|| "semantic embedding offset overflowed".to_string())?;
            for (dimension, value) in vector.iter_mut().enumerate() {
                let offset = row_offset + dimension * 2;
                let raw: [u8; 2] = embeddings
                    .get(offset..offset + 2)
                    .ok_or_else(|| "semantic embedding tensor is truncated".to_string())?
                    .try_into()
                    .map_err(|_| "semantic embedding bytes are invalid".to_string())?;
                *value += f64::from(f16::from_le_bytes(raw).to_f32()) * weight;
            }
        }
        normalize_f64(&mut vector)?;
        budget.check()?;
        Ok(vector.into_iter().map(|value| value as f32).collect())
    }

    #[cfg(test)]
    fn token_ids(&self, text: &str) -> Result<Vec<u32>, String> {
        self.token_ids_with_budget(text, &SemanticWorkBudget::unbounded())
    }

    fn token_ids_with_budget(
        &self,
        text: &str,
        budget: &SemanticWorkBudget,
    ) -> Result<Vec<u32>, String> {
        budget.check()?;
        // This mirrors Model2Vec's public `tokenize(max_length=...)` contract: bound Unicode
        // codepoints before invoking BPE, then enforce the exact token ceiling afterwards.
        let max_chars = self
            .profile
            .max_tokens
            .checked_mul(self.profile.median_token_length)
            .ok_or_else(|| "semantic character pre-limit overflowed".to_string())?;
        let bounded = truncate_chars(text, max_chars);
        let mut ids = self.tokenizer.encode_with_budget(bounded, budget)?;
        ids.truncate(self.profile.max_tokens);
        budget.check()?;
        Ok(ids)
    }
}

impl CompactBpe {
    fn open(
        path: &Path,
        profile: &SemanticProfile,
        budget: &SemanticWorkBudget,
    ) -> Result<Self, String> {
        budget.check()?;
        let file = File::open(path)
            .map_err(|error| format!("cannot open semantic tokenizer pack: {error}"))?;
        let mmap = unsafe { MmapOptions::new().map(&file) }
            .map_err(|error| format!("cannot mmap semantic tokenizer pack: {error}"))?;
        if mmap.get(0..8) != Some(PACK_MAGIC.as_slice()) {
            return Err("semantic tokenizer pack has invalid magic".to_string());
        }
        let vocab_count = usize::try_from(read_u32(&mmap, 8)?)
            .map_err(|_| "semantic vocab count overflowed".to_string())?;
        let merge_count = usize::try_from(read_u32(&mmap, 12)?)
            .map_err(|_| "semantic merge count overflowed".to_string())?;
        let string_bytes = usize::try_from(read_u32(&mmap, 16)?)
            .map_err(|_| "semantic string bytes overflowed".to_string())?;
        if read_u32(&mmap, 20)? != 0
            || vocab_count != profile.tokenizer_pack.vocab_size
            || merge_count != profile.tokenizer_pack.merge_count
            || string_bytes != profile.tokenizer_pack.string_bytes
        {
            return Err("semantic tokenizer pack header does not match profile".to_string());
        }
        let merge_offset = PACK_HEADER_BYTES
            .checked_add(
                vocab_count
                    .checked_mul(PACK_VOCAB_ROW_BYTES)
                    .ok_or_else(|| "semantic tokenizer pack overflowed".to_string())?,
            )
            .ok_or_else(|| "semantic tokenizer pack overflowed".to_string())?;
        let string_offset = merge_offset
            .checked_add(
                merge_count
                    .checked_mul(PACK_MERGE_ROW_BYTES)
                    .ok_or_else(|| "semantic tokenizer pack overflowed".to_string())?,
            )
            .ok_or_else(|| "semantic tokenizer pack overflowed".to_string())?;
        if string_offset
            .checked_add(string_bytes)
            .ok_or_else(|| "semantic tokenizer pack overflowed".to_string())?
            != mmap.len()
        {
            return Err("semantic tokenizer pack length does not match header".to_string());
        }
        Ok(Self {
            mmap,
            vocab_count,
            merge_count,
            merge_offset,
            string_offset,
            regex: Regex::new(PRETOKEN_PATTERN)
                .map_err(|error| format!("semantic pre-tokenizer regex is invalid: {error}"))?,
            word_regex: Regex::new(r"\w")
                .map_err(|error| format!("semantic added-token regex is invalid: {error}"))?,
            byte_chars: byte_chars(),
        })
    }

    #[cfg(test)]
    fn encode(&self, text: &str) -> Result<Vec<u32>, String> {
        self.encode_with_budget(text, &SemanticWorkBudget::unbounded())
    }

    fn encode_with_budget(
        &self,
        text: &str,
        budget: &SemanticWorkBudget,
    ) -> Result<Vec<u32>, String> {
        let mut output = Vec::new();
        let mut segment_start = 0;
        let mut search_start = 0;
        while let Some(relative) = text[search_start..].find(ADDED_TOKEN) {
            budget.check()?;
            let start = search_start + relative;
            let end = start + ADDED_TOKEN.len();
            if !self.added_token_has_word_boundaries(text, start, end)? {
                search_start = start + 1;
                continue;
            }
            let token_start = text[..start]
                .char_indices()
                .rev()
                .take_while(|(_, character)| character.is_whitespace())
                .map(|(index, _)| index)
                .last()
                .unwrap_or(start)
                .max(segment_start);
            self.encode_base(&text[segment_start..token_start], &mut output, budget)?;
            output.push(ADDED_TOKEN_ID);
            let trailing = text[end..]
                .char_indices()
                .take_while(|(_, character)| character.is_whitespace())
                .map(|(index, character)| index + character.len_utf8())
                .last()
                .unwrap_or(0);
            segment_start = end + trailing;
            search_start = segment_start;
        }
        self.encode_base(&text[segment_start..], &mut output, budget)?;
        Ok(output)
    }

    fn encode_base(
        &self,
        text: &str,
        output: &mut Vec<u32>,
        budget: &SemanticWorkBudget,
    ) -> Result<(), String> {
        let mut cursor = 0;
        for found in self.regex.find_iter(text) {
            budget.check()?;
            let matched =
                found.map_err(|error| format!("semantic pre-tokenizer failed: {error}"))?;
            if matched.start() > cursor {
                self.encode_piece(&text[cursor..matched.start()], output, budget)?;
            }
            if matched.end() > matched.start() {
                self.encode_piece(matched.as_str(), output, budget)?;
            }
            cursor = matched.end();
        }
        if cursor < text.len() {
            self.encode_piece(&text[cursor..], output, budget)?;
        }
        Ok(())
    }

    fn added_token_has_word_boundaries(
        &self,
        text: &str,
        start: usize,
        end: usize,
    ) -> Result<bool, String> {
        let previous_is_word = text[..start]
            .chars()
            .next_back()
            .map(|character| self.is_added_token_word_character(character))
            .transpose()?
            .unwrap_or(false);
        let next_is_word = text[end..]
            .chars()
            .next()
            .map(|character| self.is_added_token_word_character(character))
            .transpose()?
            .unwrap_or(false);
        Ok(!previous_is_word && !next_is_word)
    }

    fn is_added_token_word_character(&self, character: char) -> Result<bool, String> {
        self.word_regex
            .is_match(&character.to_string())
            .map_err(|error| format!("semantic added-token boundary failed: {error}"))
    }

    fn encode_piece(
        &self,
        piece: &str,
        output: &mut Vec<u32>,
        budget: &SemanticWorkBudget,
    ) -> Result<(), String> {
        budget.check()?;
        let mut prepared = String::new();
        if !piece.starts_with(' ') {
            prepared.push(self.byte_chars[usize::from(b' ')]);
        }
        for byte in piece.as_bytes() {
            prepared.push(self.byte_chars[usize::from(*byte)]);
        }
        if let Some(id) = self.vocab_id(prepared.as_bytes())? {
            output.push(id);
            return Ok(());
        }
        let symbols = prepared
            .chars()
            .map(|character| {
                let mut bytes = [0; 4];
                self.vocab_id(character.encode_utf8(&mut bytes).as_bytes())?
                    .ok_or_else(|| {
                        "semantic tokenizer is missing an initial byte token".to_string()
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if symbols.len() < 2 {
            output.extend(symbols);
            return Ok(());
        }

        let symbol_count = symbols.len();
        let mut nodes = symbols
            .into_iter()
            .enumerate()
            .map(|(index, id)| BpeNode {
                id,
                previous: index.checked_sub(1),
                next: (index + 1 < symbol_count).then_some(index + 1),
                alive: true,
            })
            .collect::<Vec<_>>();
        let mut merges = BinaryHeap::new();
        for left in 0..nodes.len().saturating_sub(1) {
            budget.check()?;
            self.enqueue_merge(&nodes, left, &mut merges)?;
        }
        while let Some(Reverse((rank, left, left_id, right_id, new_id))) = merges.pop() {
            budget.check()?;
            let Some(right) = nodes
                .get(left)
                .filter(|node| node.alive)
                .and_then(|node| node.next)
            else {
                continue;
            };
            if !nodes[right].alive
                || nodes[left].id != left_id
                || nodes[right].id != right_id
                || self.merge(left_id, right_id)? != Some((rank, new_id))
            {
                continue;
            }
            let previous = nodes[left].previous;
            let next = nodes[right].next;
            nodes[left].id = new_id;
            nodes[left].next = next;
            nodes[right].alive = false;
            nodes[right].previous = None;
            nodes[right].next = None;
            if let Some(next) = next {
                nodes[next].previous = Some(left);
            }
            if let Some(previous) = previous {
                self.enqueue_merge(&nodes, previous, &mut merges)?;
            }
            self.enqueue_merge(&nodes, left, &mut merges)?;
        }
        let mut current = Some(0_usize);
        while let Some(index) = current {
            budget.check()?;
            if nodes[index].alive {
                output.push(nodes[index].id);
            }
            current = nodes[index].next;
        }
        Ok(())
    }

    fn enqueue_merge(
        &self,
        nodes: &[BpeNode],
        left: usize,
        heap: &mut BinaryHeap<MergeCandidate>,
    ) -> Result<(), String> {
        let Some(left_node) = nodes.get(left).filter(|node| node.alive) else {
            return Ok(());
        };
        let Some(right) = left_node.next else {
            return Ok(());
        };
        let right_node = nodes
            .get(right)
            .filter(|node| node.alive)
            .ok_or_else(|| "semantic BPE linked list is inconsistent".to_string())?;
        if let Some((rank, new_id)) = self.merge(left_node.id, right_node.id)? {
            heap.push(Reverse((rank, left, left_node.id, right_node.id, new_id)));
        }
        Ok(())
    }

    fn vocab_id(&self, token: &[u8]) -> Result<Option<u32>, String> {
        let mut low = 0;
        let mut high = self.vocab_count;
        while low < high {
            let mid = low + (high - low) / 2;
            let (candidate, id) = self.vocab_row(mid)?;
            match candidate.cmp(token) {
                Ordering::Less => low = mid + 1,
                Ordering::Greater => high = mid,
                Ordering::Equal => return Ok(Some(id)),
            }
        }
        Ok(None)
    }

    fn vocab_row(&self, index: usize) -> Result<(&[u8], u32), String> {
        let row = PACK_HEADER_BYTES + index * PACK_VOCAB_ROW_BYTES;
        let offset = usize::try_from(read_u32(&self.mmap, row)?)
            .map_err(|_| "semantic vocab offset overflowed".to_string())?;
        let length = usize::try_from(read_u32(&self.mmap, row + 4)?)
            .map_err(|_| "semantic vocab length overflowed".to_string())?;
        let id = read_u32(&self.mmap, row + 8)?;
        let start = self
            .string_offset
            .checked_add(offset)
            .ok_or_else(|| "semantic vocab row overflowed".to_string())?;
        Ok((
            self.mmap
                .get(start..start + length)
                .ok_or_else(|| "semantic vocab row is truncated".to_string())?,
            id,
        ))
    }

    fn merge(&self, left: u32, right: u32) -> Result<Option<(u32, u32)>, String> {
        let target = (u64::from(left) << 32) | u64::from(right);
        let mut low = 0;
        let mut high = self.merge_count;
        while low < high {
            let mid = low + (high - low) / 2;
            let row = self.merge_offset + mid * PACK_MERGE_ROW_BYTES;
            let key = read_u64(&self.mmap, row)?;
            match key.cmp(&target) {
                Ordering::Less => low = mid + 1,
                Ordering::Greater => high = mid,
                Ordering::Equal => {
                    return Ok(Some((
                        read_u32(&self.mmap, row + 8)?,
                        read_u32(&self.mmap, row + 12)?,
                    )))
                }
            }
        }
        Ok(None)
    }
}

pub fn embedded_semantic_profile() -> Result<SemanticProfile, String> {
    static PROFILE: OnceLock<Result<SemanticProfile, String>> = OnceLock::new();
    PROFILE
        .get_or_init(|| {
            let profile: SemanticProfile = serde_json::from_str(EMBEDDED_SEMANTIC_PROFILE_JSON)
                .map_err(|error| format!("cannot parse semantic-profile.json: {error}"))?;
            validate_profile(&profile)?;
            Ok(profile)
        })
        .clone()
}

fn validate_profile(profile: &SemanticProfile) -> Result<(), String> {
    if profile.schema_version != 1
        || profile.cache.schema_version != 1
        || profile.dimension != 64
        || profile.max_tokens != 512
        || profile.median_token_length != 6
        || profile.cache.max_rows == 0
        || profile.cache.max_rows > 100_000
        || profile.model_id != FROZEN_SEMANTIC_MODEL_ID
        || profile.upstream_repository != "Nourh7/granite-embedding-97m-multilingual-r2"
        || profile.upstream_revision != "b77044bfd84eef0b552c5346eeacc851264592b3"
        || profile.license != "MIT"
    {
        return Err("semantic profile identity or budget is invalid".to_string());
    }
    for (label, asset) in [
        ("model", &profile.model),
        (
            "tokenizerPack",
            &SemanticAssetProfile {
                asset: profile.tokenizer_pack.asset.clone(),
                bytes: profile.tokenizer_pack.bytes,
                sha256: profile.tokenizer_pack.sha256.clone(),
            },
        ),
    ] {
        if asset.bytes == 0
            || !is_sha256(&asset.sha256)
            || Path::new(&asset.asset).is_absolute()
            || asset
                .asset
                .split('/')
                .any(|component| component.is_empty() || matches!(component, "." | ".."))
        {
            return Err(format!("semantic profile {label} asset is invalid"));
        }
    }
    if !is_sha256(&profile.tokenizer_pack.source_sha256)
        || !is_sha256(&profile.upstream_config.sha256)
        || profile.upstream_config.bytes == 0
        || profile.tokenizer_pack.source_bytes == 0
        || profile.tokenizer_pack.vocab_size == 0
        || profile.tokenizer_pack.merge_count == 0
        || profile.tokenizer_pack.string_bytes == 0
    {
        return Err("semantic tokenizer source contract is invalid".to_string());
    }
    Ok(())
}

fn bounded_query(query: &str) -> Result<&str, String> {
    let trimmed = query.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_SEMANTIC_QUERY_BYTES {
        return Err("semantic query is empty or exceeds 2000 UTF-8 bytes".to_string());
    }
    Ok(trimmed)
}

pub fn semantic_content(text: &str) -> Option<(String, String)> {
    let trimmed = text.trim();
    if trimmed.chars().count() <= 10 {
        return None;
    }
    let bounded = truncate_utf8(trimmed, MAX_SEMANTIC_SOURCE_BYTES);
    let hash = format!("{:x}", Sha256::digest(bounded.as_bytes()));
    Some((hash, bounded))
}

pub fn semantic_query_from_args(fields: &[(String, String)]) -> Result<String, String> {
    let mut query = String::new();
    for (key, value) in fields {
        if matches!(
            key.as_str(),
            "tool_name" | "action" | "ink" | "river" | "vref" | "archery"
        ) {
            continue;
        }
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if !query.is_empty() {
            query.push('\n');
        }
        let remaining = MAX_SEMANTIC_QUERY_BYTES.saturating_sub(query.len());
        if remaining == 0 {
            break;
        }
        query.push_str(&truncate_utf8(value, remaining));
    }
    bounded_query(&query).map(str::to_string)
}

async fn prune_cache(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    model_id: &str,
    max_rows: usize,
) -> Result<(), String> {
    sqlx::query(
        "DELETE FROM local_semantic_embedding_cache WHERE model_id = ? AND content_sha256 IN (\
         SELECT content_sha256 FROM local_semantic_embedding_cache WHERE model_id = ? \
         ORDER BY last_used_at_ms DESC, content_sha256 ASC LIMIT -1 OFFSET ?)",
    )
    .bind(model_id)
    .bind(model_id)
    .bind(max_rows as i64)
    .execute(&mut **transaction)
    .await
    .map_err(|error| format!("cannot prune semantic cache: {error}"))?;
    Ok(())
}

async fn invalidate_old_model_cache(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    model_id: &str,
) -> Result<(), String> {
    sqlx::query("DELETE FROM local_semantic_embedding_cache WHERE model_id != ?")
        .bind(model_id)
        .execute(&mut **transaction)
        .await
        .map(|_| ())
        .map_err(|error| format!("cannot invalidate old semantic model cache: {error}"))
}

fn verify_regular_asset_with_budget(
    path: &Path,
    expected_bytes: u64,
    expected_sha256: &str,
    label: &str,
    budget: &SemanticWorkBudget,
) -> Result<(), String> {
    budget.check()?;
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {label}: {error}"))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!("{label} must be a direct regular file"));
    }
    if metadata.len() != expected_bytes {
        return Err(format!("{label} byte size does not match semantic profile"));
    }
    let mut file = File::open(path).map_err(|error| format!("cannot open {label}: {error}"))?;
    let mut digest = Sha256::new();
    let mut buffer = [0; HASH_BUFFER_BYTES];
    loop {
        budget.check()?;
        let read = std::io::Read::read(&mut file, &mut buffer)
            .map_err(|error| format!("cannot hash {label}: {error}"))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    budget.check()?;
    if format!("{:x}", digest.finalize()) != expected_sha256 {
        return Err(format!("{label} SHA-256 does not match semantic profile"));
    }
    Ok(())
}

fn normalize_f64(vector: &mut [f64]) -> Result<(), String> {
    let norm = vector.iter().map(|value| value * value).sum::<f64>().sqrt();
    if !norm.is_finite() || norm <= f64::EPSILON {
        return Err("semantic vector has invalid norm".to_string());
    }
    for value in vector {
        *value /= norm;
    }
    Ok(())
}

fn cosine(left: &[f32], right: &[f32]) -> f32 {
    left.iter().zip(right).map(|(a, b)| a * b).sum()
}

fn encode_vector(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn decode_vector(bytes: &[u8]) -> Result<Vec<f32>, String> {
    if bytes.is_empty() || !bytes.len().is_multiple_of(VECTOR_BYTES_PER_DIMENSION) {
        return Err("semantic cache vector byte length is invalid".to_string());
    }
    let vector = bytes
        .chunks_exact(4)
        .map(|chunk| {
            let raw: [u8; 4] = chunk
                .try_into()
                .map_err(|_| "semantic vector chunk is invalid".to_string())?;
            let value = f32::from_le_bytes(raw);
            value
                .is_finite()
                .then_some(value)
                .ok_or_else(|| "semantic vector contains a non-finite value".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if !norm.is_finite() || norm <= f32::EPSILON || (norm - 1.0).abs() > 0.001 {
        return Err("semantic cache vector is not unit-normalized".to_string());
    }
    Ok(vector)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let raw: [u8; 4] = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| "semantic tokenizer pack is truncated".to_string())?
        .try_into()
        .map_err(|_| "semantic tokenizer u32 is invalid".to_string())?;
    Ok(u32::from_le_bytes(raw))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, String> {
    let raw: [u8; 8] = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| "semantic tokenizer pack is truncated".to_string())?
        .try_into()
        .map_err(|_| "semantic tokenizer u64 is invalid".to_string())?;
    Ok(u64::from_le_bytes(raw))
}

fn byte_chars() -> [char; 256] {
    let mut bytes = Vec::new();
    bytes.extend(b'!'..=b'~');
    bytes.extend(b'\xA1'..=b'\xAC');
    bytes.extend(b'\xAE'..=b'\xFF');
    let mut codepoints = bytes
        .iter()
        .map(|byte| u32::from(*byte))
        .collect::<Vec<_>>();
    let mut extra = 0;
    for byte in 0..=u8::MAX {
        if !bytes.contains(&byte) {
            bytes.push(byte);
            codepoints.push(256 + extra);
            extra += 1;
        }
    }
    let mut result = ['\0'; 256];
    for (byte, codepoint) in bytes.into_iter().zip(codepoints) {
        result[usize::from(byte)] =
            char::from_u32(codepoint).expect("byte alphabet codepoint is valid");
    }
    result
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn truncate_chars(value: &str, max_chars: usize) -> &str {
    value
        .char_indices()
        .nth(max_chars)
        .map_or(value, |(end, _)| &value[..end])
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository_asset(profile_path: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("runtime-assets/vcp-cli")
            .join(profile_path)
    }

    #[test]
    fn repository_assets_match_profile_and_token_fixture() {
        let profile = embedded_semantic_profile().expect("semantic profile");
        assert_eq!(profile.upstream_config.bytes, 341);
        assert_eq!(
            profile.upstream_config.sha256,
            "f30b0d724a845bf677901851617e816f36686d8719260f2a238fc7ef0e9ae420"
        );
        let engine = SemanticEngine::open(
            &repository_asset(&profile.model.asset),
            &repository_asset(&profile.tokenizer_pack.asset),
        )
        .expect("load frozen semantic engine");
        assert_eq!(
            engine
                .tokenizer
                .encode("取消后台运行的命令并终止整个进程树")
                .expect("tokenize fixture"),
            vec![
                180, 52016, 133976, 95285, 1566, 28537, 45399, 20547, 40368, 26532, 164355, 10754,
                13456, 50698,
            ]
        );
        let added_token_fixtures = [
            ("<|endoftext|>", vec![179_934]),
            (" <|endoftext|> ", vec![179_934]),
            ("a <|endoftext|> b", vec![221, 179_934, 247]),
            (".<|endoftext|>.", vec![843, 179_934, 843]),
            (
                "a<|endoftext|>",
                vec![221, 423, 91, 1221, 1391, 875, 146_542],
            ),
            (
                "<|endoftext|>a",
                vec![423, 91, 1221, 1391, 875, 146_542, 221],
            ),
            ("x-<|endoftext|>-y", vec![1168, 492, 179_934, 492, 88]),
            ("\n\t<|endoftext|>\r\n", vec![179_934]),
            ("<|endoftext|><|endoftext|>", vec![179_934, 179_934]),
        ];
        for (text, expected) in added_token_fixtures {
            assert_eq!(
                engine.tokenizer.encode(text).expect("added-token fixture"),
                expected,
                "added-token semantics diverged for {text:?}"
            );
        }
        let related = engine
            .embed("停止正在执行的任务，确保所有子进程退出")
            .expect("related vector");
        let query = engine
            .embed("取消后台运行的命令并终止整个进程树")
            .expect("query vector");
        let upstream_prefix = [
            -0.641_593_9,
            0.105_635_26,
            -0.066_551_31,
            -0.012_563_977,
            -0.134_617_02,
            0.109_239_98,
            -0.262_336_67,
            0.053_612_76,
        ];
        for (actual, expected) in query.iter().zip(upstream_prefix) {
            assert!(
                (actual - expected).abs() < 0.000_02,
                "Rust vector diverged from the frozen upstream Model2Vec fixture"
            );
        }
        let unrelated = engine
            .embed("今天北京天气晴朗，适合散步")
            .expect("unrelated vector");
        assert!((cosine(&query, &related) - 0.836_126_74).abs() < 0.000_02);
        assert!((cosine(&query, &unrelated) - (-0.106_977_9)).abs() < 0.000_02);
    }

    #[test]
    fn bounded_tokenizer_matches_upstream_long_input_fixtures() {
        let profile = embedded_semantic_profile().expect("semantic profile");
        let engine = SemanticEngine::open(
            &repository_asset(&profile.model.asset),
            &repository_asset(&profile.tokenizer_pack.asset),
        )
        .expect("load frozen semantic engine");
        let fixtures = [
            (
                "a".repeat(4_000),
                386,
                "1ec004213ac89970923618f40406ac90267689a8213f4b73fb2779fdaf7650ad",
            ),
            (
                "取消后台任务 🚀 café naïve Ελληνικά العربية हिन्दी 日本語 한국어 ".repeat(200),
                512,
                "159778f56b1811c530670cf7f70f5ab902f515565735346f1abba7b1891c90bb",
            ),
            (
                " \t\n\r".repeat(2_000),
                512,
                "97c479c82aeb6db23c8d0c4414a11cf5fcfcb559f5d37d7577f21e96a4f81a7a",
            ),
        ];
        for (text, expected_tokens, expected_digest) in fixtures {
            let ids = engine.token_ids(&text).expect("bounded token IDs");
            let digest = ids
                .iter()
                .fold(Sha256::new(), |mut digest, id| {
                    digest.update(id.to_le_bytes());
                    digest
                })
                .finalize();
            assert_eq!(ids.len(), expected_tokens);
            assert_eq!(format!("{digest:x}"), expected_digest);
        }
    }

    #[test]
    fn query_contract_uses_only_bounded_non_meta_values() {
        let query = semantic_query_from_args(&[
            ("tool_name".into(), "VCPMobileCLI".into()),
            ("river".into(), "semantic:3".into()),
            ("command".into(), "  rg cancellation src  ".into()),
            ("description".into(), "find process cleanup".into()),
        ])
        .expect("query");
        assert_eq!(query, "rg cancellation src\nfind process cleanup");
        assert!(!query.contains("semantic:3"));
    }

    #[test]
    fn short_candidates_are_not_indexed() {
        assert!(semantic_content("short text").is_none());
        let (hash, text) = semantic_content("this candidate has enough visible characters")
            .expect("eligible candidate");
        assert_eq!(hash.len(), 64);
        assert_eq!(text, "this candidate has enough visible characters");
    }

    #[test]
    fn vector_codec_is_exact_and_rejects_invalid_norms() {
        let vector = vec![0.6, 0.8];
        assert_eq!(
            decode_vector(&encode_vector(&vector)).expect("decode"),
            vector
        );
        assert!(decode_vector(&f32::NAN.to_le_bytes()).is_err());
        assert!(decode_vector(&encode_vector(&[0.0, 0.0])).is_err());
        assert!(decode_vector(&encode_vector(&[0.25, -0.5, 1.0])).is_err());
    }

    #[tokio::test]
    async fn offline_selection_caches_all_candidates_and_restores_source_order() {
        let profile = embedded_semantic_profile().expect("semantic profile");
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("semantic cache database");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("semantic cache migration");
        let candidates = [
            (4, "今天北京天气晴朗，适合散步"),
            (8, "停止正在执行的任务，确保所有子进程退出"),
            (12, "安装本地技能包但不要自动执行脚本"),
        ]
        .into_iter()
        .map(|(source_index, text)| {
            let (content_sha256, text) = semantic_content(text).expect("eligible candidate");
            SemanticCandidate {
                source_index,
                content_sha256,
                text,
            }
        })
        .collect::<Vec<_>>();
        let owner = LocalEmbeddingOwner::new();
        let selection = owner
            .select(
                &pool,
                SemanticSelectRequest {
                    model_path: &repository_asset(&profile.model.asset),
                    tokenizer_path: &repository_asset(&profile.tokenizer_pack.asset),
                    query: "取消后台命令并终止子进程",
                    candidates: &candidates,
                    limit: 2,
                    now_ms: 1_700_000_000_000,
                    cancellation_token: CancellationToken::new(),
                    deadline_at_ms: u64::MAX,
                },
            )
            .await
            .expect("offline selection");
        assert_eq!(selection.source_indices, vec![8, 12]);
        let cache_rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM local_semantic_embedding_cache WHERE model_id = ?",
        )
        .bind(&profile.model_id)
        .fetch_one(&pool)
        .await
        .expect("cache row count");
        assert_eq!(cache_rows, 3);

        let replay = owner
            .select(
                &pool,
                SemanticSelectRequest {
                    model_path: &repository_asset(&profile.model.asset),
                    tokenizer_path: &repository_asset(&profile.tokenizer_pack.asset),
                    query: "取消后台命令并终止子进程",
                    candidates: &candidates,
                    limit: 2,
                    now_ms: 1_700_000_000_001,
                    cancellation_token: CancellationToken::new(),
                    deadline_at_ms: u64::MAX,
                },
            )
            .await
            .expect("cached selection");
        assert_eq!(replay, selection);
    }

    #[tokio::test]
    async fn candidate_only_cache_lookup_recomputes_invalid_vector_rows() {
        let profile = embedded_semantic_profile().expect("semantic profile");
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("semantic cache database");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("semantic cache migration");
        let (content_sha256, text) =
            semantic_content("停止当前后台任务并等待所有子进程退出").expect("candidate");
        let candidate = SemanticCandidate {
            source_index: 3,
            content_sha256: content_sha256.clone(),
            text,
        };
        let zero_norm = vec![0_u8; profile.dimension * VECTOR_BYTES_PER_DIMENSION];
        let mut non_finite = zero_norm.clone();
        non_finite[..4].copy_from_slice(&f32::NAN.to_le_bytes());
        for (hash, last_used, damaged) in [
            (&content_sha256, 10_i64, &zero_norm),
            (&"f".repeat(64), 11_i64, &non_finite),
        ] {
            sqlx::query(
                "INSERT INTO local_semantic_embedding_cache \
                 (model_id, content_sha256, dimension, vector, created_at_ms, last_used_at_ms) \
                 VALUES (?, ?, ?, ?, 1, ?)",
            )
            .bind(&profile.model_id)
            .bind(hash)
            .bind(profile.dimension as i64)
            .bind(damaged)
            .bind(last_used)
            .execute(&pool)
            .await
            .expect("insert damaged derived row");
        }

        let owner = LocalEmbeddingOwner::new();
        owner
            .select(
                &pool,
                SemanticSelectRequest {
                    model_path: &repository_asset(&profile.model.asset),
                    tokenizer_path: &repository_asset(&profile.tokenizer_pack.asset),
                    query: "取消后台命令",
                    candidates: &[candidate],
                    limit: 1,
                    now_ms: 20,
                    cancellation_token: CancellationToken::new(),
                    deadline_at_ms: u64::MAX,
                },
            )
            .await
            .expect("damaged candidate is recomputed");
        let repaired: Vec<u8> = sqlx::query_scalar(
            "SELECT vector FROM local_semantic_embedding_cache \
             WHERE model_id = ? AND content_sha256 = ?",
        )
        .bind(&profile.model_id)
        .bind(&content_sha256)
        .fetch_one(&pool)
        .await
        .expect("repaired vector");
        assert!(decode_vector(&repaired).is_ok());
        let unrelated_last_used: i64 = sqlx::query_scalar(
            "SELECT last_used_at_ms FROM local_semantic_embedding_cache \
             WHERE model_id = ? AND content_sha256 = ?",
        )
        .bind(&profile.model_id)
        .bind("f".repeat(64))
        .fetch_one(&pool)
        .await
        .expect("unrelated row remains");
        assert_eq!(unrelated_last_used, 11);
    }

    #[tokio::test]
    async fn queued_selection_is_single_owner_and_cancellable() {
        let owner = Arc::new(LocalEmbeddingOwner::new());
        let held = owner
            .selection_permit
            .acquire()
            .await
            .expect("hold semantic selection permit");
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("semantic cache database");
        let cancellation_token = CancellationToken::new();
        let task_token = cancellation_token.clone();
        let task_owner = Arc::clone(&owner);
        let task = tokio::spawn(async move {
            let candidate = SemanticCandidate {
                source_index: 1,
                content_sha256: "a".repeat(64),
                text: "queued semantic candidate with enough visible characters".to_string(),
            };
            task_owner
                .select(
                    &pool,
                    SemanticSelectRequest {
                        model_path: Path::new("missing-model"),
                        tokenizer_path: Path::new("missing-tokenizer"),
                        query: "queued selection",
                        candidates: &[candidate],
                        limit: 1,
                        now_ms: 1,
                        cancellation_token: task_token,
                        deadline_at_ms: u64::MAX,
                    },
                )
                .await
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(owner.selection_permit.available_permits(), 0);
        cancellation_token.cancel();
        let error = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("queued selection cancellation timeout")
            .expect("queued selection task")
            .expect_err("queued selection must cancel");
        assert_eq!(error, "semantic selection cancelled");
        drop(held);
    }

    #[tokio::test]
    async fn selection_owner_rejects_candidate_fanout_before_loading_assets() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("semantic cache database");
        let candidates = (0..=MAX_SEMANTIC_CANDIDATES)
            .map(|source_index| SemanticCandidate {
                source_index,
                content_sha256: format!("{source_index:064x}"),
                text: "bounded candidate".to_string(),
            })
            .collect::<Vec<_>>();
        let error = LocalEmbeddingOwner::new()
            .select(
                &pool,
                SemanticSelectRequest {
                    model_path: Path::new("missing-model"),
                    tokenizer_path: Path::new("missing-tokenizer"),
                    query: "bounded query",
                    candidates: &candidates,
                    limit: 1,
                    now_ms: 1,
                    cancellation_token: CancellationToken::new(),
                    deadline_at_ms: u64::MAX,
                },
            )
            .await
            .expect_err("candidate fanout must fail before asset loading");
        assert!(error.contains("bounded local budget"));
    }

    #[tokio::test]
    async fn cache_prune_and_model_upgrade_cleanup_are_deterministic() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("semantic cache database");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("semantic cache migration");
        let profile = embedded_semantic_profile().expect("semantic profile");
        let vector = encode_vector(&[1.0, 0.0]);
        for (model_id, hash, last_used) in [
            (profile.model_id.as_str(), "a".repeat(64), 1_i64),
            (profile.model_id.as_str(), "b".repeat(64), 2_i64),
            (profile.model_id.as_str(), "c".repeat(64), 3_i64),
            ("obsolete-model", "d".repeat(64), 4_i64),
        ] {
            sqlx::query(
                "INSERT INTO local_semantic_embedding_cache \
                 (model_id, content_sha256, dimension, vector, created_at_ms, last_used_at_ms) \
                 VALUES (?, ?, 2, ?, 1, ?)",
            )
            .bind(model_id)
            .bind(hash)
            .bind(&vector)
            .bind(last_used)
            .execute(&pool)
            .await
            .expect("insert cache row");
        }
        let mut transaction = pool.begin().await.expect("cache maintenance transaction");
        invalidate_old_model_cache(&mut transaction, &profile.model_id)
            .await
            .expect("invalidate old model");
        prune_cache(&mut transaction, &profile.model_id, 2)
            .await
            .expect("prune current model");
        transaction
            .commit()
            .await
            .expect("commit cache maintenance");
        let rows = sqlx::query(
            "SELECT model_id, content_sha256 FROM local_semantic_embedding_cache \
             ORDER BY content_sha256",
        )
        .fetch_all(&pool)
        .await
        .expect("read bounded cache");
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| {
            row.get::<String, _>("model_id") == profile.model_id
                && matches!(
                    row.get::<String, _>("content_sha256").as_bytes()[0],
                    b'b' | b'c'
                )
        }));
    }
}
