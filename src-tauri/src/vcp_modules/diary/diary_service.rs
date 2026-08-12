use super::diary_types::*;
use crate::vcp_modules::settings_manager::{read_settings, Settings, SettingsState};
use futures_util::StreamExt;
use percent_encoding::percent_decode_str;
use reqwest::{redirect::Policy, Client, Method, RequestBuilder, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
const SEARCH_TOTAL_TIMEOUT: Duration = Duration::from_secs(40);
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_DOCUMENT_BYTES: usize = 2 * 1024 * 1024;
// A valid 2 MiB UTF-8 body can be considerably larger while JSON-escaped.
const MAX_DOCUMENT_JSON_BYTES: usize = MAX_DOCUMENT_BYTES * 6 + 64 * 1024;
const MAX_LIST_JSON_BYTES: usize = 8 * 1024 * 1024;
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;
const MAX_TOOL_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_FOLDER_COUNT: usize = 10_000;
const MAX_NOTE_COUNT: usize = 10_000;
const MAX_BATCH_ITEMS: usize = 1_000;
const MAX_PREVIEW_CHARS: usize = 4_096;
const MAX_SEARCH_TERM_CHARS: usize = 100;
const MAX_SEMANTIC_QUERY_CHARS: usize = 4_000;
const INDEX_CATCH_UP_HINT_MS: u64 = 60_000;
const TOOL_REQUEST_START: &str = "<<<[TOOL_REQUEST]>>>";
const TOOL_REQUEST_END: &str = "<<<[END_TOOL_REQUEST]>>>";
const TOOL_REQUEST_START_ESCAPE: &str = "<<<[TOOL_REQUEST_ESCAPE]>>>";
const TOOL_REQUEST_END_ESCAPE: &str = "<<<[END_TOOL_REQUEST_ESCAPE]>>>";
const FIELD_ESCAPE_START: &str = "「始ESCAPE」";
const FIELD_ESCAPE_END: &str = "「末ESCAPE」";

#[derive(Debug)]
struct SearchOwner {
    generation: u64,
    request_id: String,
    token: CancellationToken,
    finished: CancellationToken,
}

#[derive(Debug, Default)]
struct SearchOwnerSlot {
    generation: u64,
    active: Option<SearchOwner>,
}

#[derive(Debug, Clone)]
struct SearchLease {
    generation: u64,
    request_id: String,
    token: CancellationToken,
    finished: CancellationToken,
}

pub struct DiaryServiceState {
    client: Client,
    mutation_gate: Mutex<()>,
    text_search_lifecycle: Mutex<()>,
    text_search_owner: Mutex<SearchOwnerSlot>,
    semantic_search_lifecycle: Mutex<()>,
    semantic_search_owner: Mutex<SearchOwnerSlot>,
    last_mutation_ms: AtomicU64,
}

impl DiaryServiceState {
    pub fn new() -> Result<Self, DiaryError> {
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(Policy::none())
            .build()
            .map_err(|_| {
                DiaryError::new(
                    DiaryErrorCode::ServiceUnavailable,
                    "无法初始化日记网络客户端",
                )
            })?;

        Ok(Self {
            client,
            mutation_gate: Mutex::new(()),
            text_search_lifecycle: Mutex::new(()),
            text_search_owner: Mutex::new(SearchOwnerSlot::default()),
            semantic_search_lifecycle: Mutex::new(()),
            semantic_search_owner: Mutex::new(SearchOwnerSlot::default()),
            last_mutation_ms: AtomicU64::new(0),
        })
    }

    fn mark_mutation(&self) {
        self.last_mutation_ms.store(now_millis(), Ordering::SeqCst);
    }

    fn index_may_be_catching_up(&self) -> bool {
        let last = self.last_mutation_ms.load(Ordering::SeqCst);
        last != 0 && now_millis().saturating_sub(last) <= INDEX_CATCH_UP_HINT_MS
    }

    async fn begin_search(
        lifecycle: &Mutex<()>,
        slot: &Mutex<SearchOwnerSlot>,
        request_id: &str,
    ) -> Result<SearchLease, DiaryError> {
        validate_request_id(request_id)?;
        let _lifecycle_guard = lifecycle.lock().await;
        let previous = slot.lock().await.active.take();
        if let Some(previous) = previous {
            previous.token.cancel();
            let _ =
                tokio::time::timeout(Duration::from_secs(3), previous.finished.cancelled()).await;
        }
        let mut owner_slot = slot.lock().await;
        owner_slot.generation = owner_slot.generation.wrapping_add(1).max(1);
        let lease = SearchLease {
            generation: owner_slot.generation,
            request_id: request_id.to_string(),
            token: CancellationToken::new(),
            finished: CancellationToken::new(),
        };
        owner_slot.active = Some(SearchOwner {
            generation: lease.generation,
            request_id: lease.request_id.clone(),
            token: lease.token.clone(),
            finished: lease.finished.clone(),
        });
        Ok(lease)
    }

    async fn complete_search(slot: &Mutex<SearchOwnerSlot>, lease: &SearchLease) -> bool {
        let mut owner_slot = slot.lock().await;
        let is_current = owner_slot.active.as_ref().is_some_and(|owner| {
            owner.generation == lease.generation && owner.request_id == lease.request_id
        });
        if is_current {
            owner_slot.active = None;
        }
        lease.finished.cancel();
        is_current
    }

    async fn cancel_search_owner(
        lifecycle: &Mutex<()>,
        slot: &Mutex<SearchOwnerSlot>,
        request_id: Option<&str>,
    ) {
        let _lifecycle_guard = lifecycle.lock().await;
        let mut owner_slot = slot.lock().await;
        let should_cancel = owner_slot
            .active
            .as_ref()
            .is_some_and(|owner| request_id.is_none_or(|expected| expected == owner.request_id));
        if should_cancel {
            let owner = owner_slot.active.take();
            if let Some(owner) = owner.as_ref() {
                owner.token.cancel();
            }
            owner_slot.generation = owner_slot.generation.wrapping_add(1).max(1);
            drop(owner_slot);
            if let Some(owner) = owner {
                let _ =
                    tokio::time::timeout(Duration::from_secs(3), owner.finished.cancelled()).await;
            }
        }
    }

    async fn list_folders(&self, settings: &Settings) -> Result<DiaryFolderList, DiaryError> {
        let request = self.admin_request(settings, Method::GET, &["folders"])?;
        let response: FolderEnvelope = self
            .request_json(request, MAX_LIST_JSON_BYTES, DEFAULT_TOTAL_TIMEOUT, None)
            .await?;
        if response.folders.len() > MAX_FOLDER_COUNT {
            return Err(DiaryError::new(
                DiaryErrorCode::ResponseTooLarge,
                "文件夹数量超过移动端上限",
            ));
        }
        for folder in &response.folders {
            validate_path_segment(folder, "文件夹")?;
        }
        Ok(DiaryFolderList {
            folders: response.folders,
        })
    }

    async fn list_notes(
        &self,
        settings: &Settings,
        folder: &str,
    ) -> Result<Vec<DiaryNoteSummary>, DiaryError> {
        validate_path_segment(folder, "文件夹")?;
        let request = self.admin_request(settings, Method::GET, &["folder", folder])?;
        let response: NotesEnvelope = self
            .request_json(request, MAX_LIST_JSON_BYTES, DEFAULT_TOTAL_TIMEOUT, None)
            .await?;
        normalize_note_summaries(response.notes, Some(folder), false)
    }

    async fn get_note(
        &self,
        settings: &Settings,
        key: &DiaryNoteKey,
    ) -> Result<DiaryDocument, DiaryError> {
        validate_note_key(key)?;
        let request =
            self.admin_request(settings, Method::GET, &["note", &key.folder, &key.file])?;
        let response: NoteEnvelope = self
            .request_json(
                request,
                MAX_DOCUMENT_JSON_BYTES,
                DEFAULT_TOTAL_TIMEOUT,
                None,
            )
            .await?;
        validate_document_content(&response.content)?;
        Ok(DiaryDocument {
            key: key.clone(),
            content_hash: content_hash(&response.content),
            content: response.content,
        })
    }

    async fn search(
        &self,
        settings: &Settings,
        request: &DiarySearchRequest,
    ) -> Result<DiarySearchResponse, DiaryError> {
        ensure_admin_config(settings)?;
        validate_search_term(&request.term)?;
        if let Some(folder) = request.folder.as_deref() {
            validate_path_segment(folder, "文件夹")?;
        }

        let lease = Self::begin_search(
            &self.text_search_lifecycle,
            &self.text_search_owner,
            &request.request_id,
        )
        .await?;
        let result = async {
            let mut url = self.admin_url(settings, &["search"])?;
            {
                let mut query = url.query_pairs_mut();
                query.append_pair("term", request.term.trim());
                if let Some(folder) = request.folder.as_deref() {
                    query.append_pair("folder", folder);
                }
            }
            let request_builder = self
                .client
                .get(url)
                .basic_auth(&settings.admin_username, Some(&settings.admin_password))
                .header(reqwest::header::ACCEPT, "application/json");
            let response: SearchEnvelope = self
                .request_json(
                    request_builder,
                    MAX_LIST_JSON_BYTES,
                    SEARCH_TOTAL_TIMEOUT,
                    Some(&lease.token),
                )
                .await?;
            let notes = normalize_note_summaries(
                response.notes,
                request.folder.as_deref(),
                request.folder.is_none(),
            )?;
            Ok(DiarySearchResponse {
                total: response.total.max(notes.len()),
                limited: response.limited,
                notes,
            })
        }
        .await;

        let is_current = Self::complete_search(&self.text_search_owner, &lease).await;
        if !is_current {
            return Err(cancelled_error());
        }
        result
    }

    async fn semantic_search(
        &self,
        settings: &Settings,
        request: &DiarySemanticSearchRequest,
    ) -> Result<DiarySemanticResponse, DiaryError> {
        validate_semantic_request(request)?;
        let lease = Self::begin_search(
            &self.semantic_search_lifecycle,
            &self.semantic_search_owner,
            &request.request_id,
        )
        .await?;

        let result = async {
            let mut fields = vec![
                ("tool_name", "LightMemo".to_string()),
                ("maid", "Memo".to_string()),
                ("query", request.query.trim().to_string()),
                ("k", request.k.clamp(1, 50).to_string()),
                ("search_all_knowledge_bases", request.search_all.to_string()),
            ];
            if !request.search_all {
                if let Some(folder) = request.folder.as_ref() {
                    fields.push(("folder", folder.clone()));
                }
            }
            let body = serialize_tool_request(&fields)?;
            let payload = self
                .request_human_tool(settings, body, Some(&lease.token))
                .await?;
            if let Some(message) = find_tool_error(&payload, 0) {
                return Err(DiaryError::new(DiaryErrorCode::ToolError, message));
            }
            let text = extract_tool_text(&payload, 0).ok_or_else(|| {
                DiaryError::new(
                    DiaryErrorCode::InvalidResponse,
                    "LightMemo 未返回可解析的文本结果",
                )
            })?;
            let mut hits = parse_semantic_hits(&text);
            if !request.search_all {
                if let Some(folder) = request.folder.as_deref() {
                    hits.retain(|hit| hit.key.folder == folder);
                }
            }
            hits.truncate(request.k as usize);
            Ok(DiarySemanticResponse {
                hits,
                index_may_be_catching_up: self.index_may_be_catching_up(),
            })
        }
        .await;

        let is_current = Self::complete_search(&self.semantic_search_owner, &lease).await;
        if !is_current {
            return Err(cancelled_error());
        }
        result
    }

    async fn save_note(
        &self,
        settings: &Settings,
        request: &DiarySaveRequest,
    ) -> Result<DiarySaveOutcome, DiaryError> {
        validate_note_key(&request.key)?;
        validate_document_content(&request.content)?;
        validate_hash(&request.baseline_hash)?;
        let _mutation_guard = self.mutation_gate.lock().await;

        let remote = self.get_note(settings, &request.key).await?;
        if !request.force && remote.content_hash != request.baseline_hash {
            return Err(DiaryError::new(
                DiaryErrorCode::Conflict,
                "远端内容已发生变化，草稿未覆盖",
            ));
        }

        let candidate_hash = content_hash(&request.content);
        let outcome = self
            .write_and_verify(
                settings,
                &request.key,
                &request.content,
                &candidate_hash,
                Some(&remote.content_hash),
            )
            .await?;
        self.mark_mutation();
        Ok(outcome)
    }

    async fn rename_note(
        &self,
        settings: &Settings,
        request: &DiaryRenameRequest,
    ) -> Result<DiaryRenameOutcome, DiaryError> {
        validate_note_key(&request.source)?;
        validate_hash(&request.baseline_hash)?;
        let target_file = normalize_rename_target(&request.source.file, &request.target_file)?;
        let target = DiaryNoteKey {
            folder: request.source.folder.clone(),
            file: target_file,
        };
        if target == request.source {
            return Err(DiaryError::new(
                DiaryErrorCode::InvalidRequest,
                "新文件名必须与原文件名不同",
            ));
        }

        let _mutation_guard = self.mutation_gate.lock().await;
        let source = self.get_note(settings, &request.source).await?;
        if source.content_hash != request.baseline_hash {
            return Err(DiaryError::new(
                DiaryErrorCode::Conflict,
                "远端源文件已变化，未执行重命名",
            ));
        }

        match self.get_note(settings, &target).await {
            Ok(_) => {
                return Err(DiaryError::new(
                    DiaryErrorCode::Conflict,
                    "目标文件已存在，未覆盖",
                ));
            }
            Err(error) if error.code == DiaryErrorCode::NotFound => {}
            Err(error) => return Err(error),
        }

        let candidate_hash = source.content_hash.clone();
        self.write_and_verify(settings, &target, &source.content, &candidate_hash, None)
            .await?;
        self.mark_mutation();

        let delete_succeeded = self
            .delete_notes_inner(settings, std::slice::from_ref(&request.source))
            .await
            .is_ok_and(|outcome| outcome.succeeded.contains(&request.source));
        // Once the target copy is verified, never hide a source whose deletion
        // cannot be proven. A failed/ambiguous delete is surfaced as the
        // conservative source-retained partial outcome and a later list refresh
        // reconciles the actual server state.
        let source_deleted = delete_succeeded
            || matches!(
                self.get_note(settings, &request.source).await,
                Err(error) if error.code == DiaryErrorCode::NotFound
            );
        if source_deleted {
            self.mark_mutation();
            Ok(DiaryRenameOutcome {
                key: target,
                content_hash: candidate_hash,
                status: DiaryRenameStatus::Renamed,
            })
        } else {
            Ok(DiaryRenameOutcome {
                key: target,
                content_hash: candidate_hash,
                status: DiaryRenameStatus::CopiedSourceRetained,
            })
        }
    }

    async fn create_note(
        &self,
        settings: &Settings,
        request: &DiaryCreateRequest,
    ) -> Result<DiaryCreateOutcome, DiaryError> {
        validate_create_request(request)?;
        let _mutation_guard = self.mutation_gate.lock().await;

        let mut fields = vec![
            ("maid", request.maid.trim().to_string()),
            ("tool_name", "DailyNote".to_string()),
            ("command", "create".to_string()),
            ("Date", request.date.trim().to_string()),
        ];
        if let Some(folder) = request.folder.as_ref().filter(|value| !value.is_empty()) {
            fields.push(("folder", folder.clone()));
        }
        if let Some(file_name) = request
            .file_name_suffix
            .as_ref()
            .filter(|value| !value.is_empty())
        {
            fields.push(("fileName", file_name.clone()));
        }
        if let Some(tag) = request.tag.as_ref().filter(|value| !value.is_empty()) {
            fields.push(("Tag", tag.clone()));
        }
        fields.push(("Content", request.content.clone()));

        let body = serialize_tool_request(&fields)?;
        let payload = match self.request_human_tool(settings, body, None).await {
            Ok(payload) => payload,
            Err(error) if mutation_outcome_may_be_ambiguous(error.code) => {
                self.mark_mutation();
                return Err(create_uncertain_error());
            }
            Err(error) => return Err(error),
        };
        if let Some(message) = find_tool_error(&payload, 0) {
            return Err(DiaryError::new(DiaryErrorCode::ToolError, message));
        }
        let Some((folder, file)) = find_create_outcome(&payload, 0) else {
            self.mark_mutation();
            return Err(create_uncertain_error());
        };
        let key = DiaryNoteKey { folder, file };
        if validate_note_key(&key).is_err() {
            self.mark_mutation();
            return Err(create_uncertain_error());
        }
        self.mark_mutation();
        Ok(DiaryCreateOutcome {
            key,
            index_status: DiaryIndexStatus::Queued,
        })
    }

    async fn move_notes(
        &self,
        settings: &Settings,
        request: &DiaryMoveRequest,
    ) -> Result<DiaryBatchOutcome, DiaryError> {
        validate_batch(&request.sources)?;
        validate_path_segment(&request.target_folder, "目标文件夹")?;
        let _mutation_guard = self.mutation_gate.lock().await;

        let request_builder = self
            .admin_request(settings, Method::POST, &["move"])?
            .json(&json!({
                "sourceNotes": request.sources,
                "targetFolder": request.target_folder,
            }));
        let response: MoveEnvelope = self
            .request_json(
                request_builder,
                MAX_LIST_JSON_BYTES,
                DEFAULT_TOTAL_TIMEOUT,
                None,
            )
            .await?;
        let outcome = normalize_move_outcome(&request.sources, response);
        if !outcome.succeeded.is_empty() {
            self.mark_mutation();
        }
        Ok(outcome)
    }

    async fn delete_notes(
        &self,
        settings: &Settings,
        request: &DiaryDeleteRequest,
    ) -> Result<DiaryBatchOutcome, DiaryError> {
        validate_batch(&request.sources)?;
        let _mutation_guard = self.mutation_gate.lock().await;
        let outcome = self.delete_notes_inner(settings, &request.sources).await?;
        if !outcome.succeeded.is_empty() {
            self.mark_mutation();
        }
        Ok(outcome)
    }

    async fn delete_notes_inner(
        &self,
        settings: &Settings,
        sources: &[DiaryNoteKey],
    ) -> Result<DiaryBatchOutcome, DiaryError> {
        let request_builder = self
            .admin_request(settings, Method::POST, &["delete-batch"])?
            .json(&json!({ "notesToDelete": sources }));
        let response: DeleteEnvelope = self
            .request_json(
                request_builder,
                MAX_LIST_JSON_BYTES,
                DEFAULT_TOTAL_TIMEOUT,
                None,
            )
            .await?;
        Ok(normalize_delete_outcome(sources, response))
    }

    async fn delete_empty_folder(
        &self,
        settings: &Settings,
        folder: &str,
    ) -> Result<(), DiaryError> {
        validate_path_segment(folder, "文件夹")?;
        let _mutation_guard = self.mutation_gate.lock().await;
        let request_builder = self
            .admin_request(settings, Method::POST, &["folder", "delete"])?
            .json(&json!({ "folderName": folder }));
        let _: Value = self
            .request_json(
                request_builder,
                MAX_ERROR_BODY_BYTES,
                DEFAULT_TOTAL_TIMEOUT,
                None,
            )
            .await?;
        self.mark_mutation();
        Ok(())
    }

    async fn write_and_verify(
        &self,
        settings: &Settings,
        key: &DiaryNoteKey,
        content: &str,
        candidate_hash: &str,
        baseline_hash: Option<&str>,
    ) -> Result<DiarySaveOutcome, DiaryError> {
        let request_builder = self
            .admin_request(settings, Method::POST, &["note", &key.folder, &key.file])?
            .json(&json!({ "content": content }));
        let post_result: Result<Value, DiaryError> = self
            .request_json(
                request_builder,
                MAX_ERROR_BODY_BYTES,
                DEFAULT_TOTAL_TIMEOUT,
                None,
            )
            .await;

        if let Err(error) = &post_result {
            if !mutation_outcome_may_be_ambiguous(error.code) {
                return Err(error.clone());
            }
        }

        let read_back = self.get_note(settings, key).await.map_err(|_| {
            DiaryError::new(
                DiaryErrorCode::SaveUncertain,
                "写入结果无法读回确认，草稿仍保留",
            )
        })?;
        if read_back.content_hash == candidate_hash {
            return Ok(DiarySaveOutcome {
                content_hash: candidate_hash.to_string(),
                verified: true,
            });
        }
        if baseline_hash.is_some_and(|baseline| baseline == read_back.content_hash) {
            return Err(DiaryError::new(
                DiaryErrorCode::SaveUncertain,
                "远端仍是保存前版本，未自动重试写入",
            ));
        }
        Err(DiaryError::new(
            DiaryErrorCode::Conflict,
            "写入后远端内容与草稿不一致，未宣称保存成功",
        ))
    }

    fn admin_request(
        &self,
        settings: &Settings,
        method: Method,
        suffix: &[&str],
    ) -> Result<RequestBuilder, DiaryError> {
        ensure_admin_config(settings)?;
        let url = self.admin_url(settings, suffix)?;
        Ok(self
            .client
            .request(method, url)
            .basic_auth(&settings.admin_username, Some(&settings.admin_password))
            .header(reqwest::header::ACCEPT, "application/json"))
    }

    fn admin_url(&self, settings: &Settings, suffix: &[&str]) -> Result<Url, DiaryError> {
        let mut url = normalize_server_base(&settings.vcp_server_url)?;
        append_url_segments(&mut url, &["admin_api", "dailynotes"])?;
        append_url_segments(&mut url, suffix)?;
        Ok(url)
    }

    fn human_tool_url(&self, settings: &Settings) -> Result<Url, DiaryError> {
        let mut url = normalize_server_base(&settings.vcp_server_url)?;
        append_url_segments(&mut url, &["v1", "human", "tool"])?;
        Ok(url)
    }

    async fn request_human_tool(
        &self,
        settings: &Settings,
        body: String,
        cancel: Option<&CancellationToken>,
    ) -> Result<Value, DiaryError> {
        ensure_bearer_config(settings)?;
        let url = self.human_tool_url(settings)?;
        let request = self
            .client
            .post(url)
            .bearer_auth(&settings.vcp_api_key)
            .header(reqwest::header::CONTENT_TYPE, "text/plain;charset=UTF-8")
            .header(reqwest::header::ACCEPT, "application/json")
            .body(body);
        let bytes = self
            .request_bytes(
                request,
                MAX_TOOL_RESPONSE_BYTES,
                SEARCH_TOTAL_TIMEOUT,
                cancel,
            )
            .await?;
        let text = std::str::from_utf8(&bytes).map_err(|_| {
            DiaryError::new(
                DiaryErrorCode::InvalidResponse,
                "Human Tool 返回了非 UTF-8 响应",
            )
        })?;
        Ok(serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string())))
    }

    async fn request_json<T: DeserializeOwned>(
        &self,
        request: RequestBuilder,
        success_budget: usize,
        total_timeout: Duration,
        cancel: Option<&CancellationToken>,
    ) -> Result<T, DiaryError> {
        let bytes = self
            .request_bytes(request, success_budget, total_timeout, cancel)
            .await?;
        serde_json::from_slice(&bytes).map_err(|_| {
            DiaryError::new(
                DiaryErrorCode::InvalidResponse,
                "服务器返回了不符合契约的 JSON",
            )
        })
    }

    async fn request_bytes(
        &self,
        request: RequestBuilder,
        success_budget: usize,
        total_timeout: Duration,
        cancel: Option<&CancellationToken>,
    ) -> Result<Vec<u8>, DiaryError> {
        let operation = async {
            let response = request.send().await.map_err(map_transport_error)?;
            let status = response.status();
            let budget = if status.is_success() {
                success_budget
            } else {
                MAX_ERROR_BODY_BYTES
            };

            if response
                .content_length()
                .is_some_and(|length| length > budget as u64)
            {
                if status.is_success() {
                    return Err(response_too_large_error());
                }
                return Err(map_http_status(status, None));
            }

            let body = match read_bounded_body(response, budget).await {
                Ok(body) => body,
                Err(_error) if !status.is_success() => {
                    return Err(map_http_status(status, None));
                }
                Err(error) => return Err(error),
            };
            if !status.is_success() {
                return Err(map_http_status(status, Some(&body)));
            }
            Ok(body)
        };

        match await_controlled(operation, total_timeout, cancel).await {
            Ok(result) => result,
            Err(ControlledWaitError::Cancelled) => Err(cancelled_error()),
            Err(ControlledWaitError::Timeout) => Err(DiaryError::new(
                DiaryErrorCode::Timeout,
                "请求超时，请稍后重试",
            )),
        }
    }
}

#[derive(Debug)]
enum ControlledWaitError {
    Cancelled,
    Timeout,
}

async fn await_controlled<F, T>(
    future: F,
    timeout: Duration,
    cancel: Option<&CancellationToken>,
) -> Result<T, ControlledWaitError>
where
    F: Future<Output = T>,
{
    if let Some(cancel) = cancel {
        tokio::select! {
            _ = cancel.cancelled() => Err(ControlledWaitError::Cancelled),
            result = tokio::time::timeout(timeout, future) => {
                result.map_err(|_| ControlledWaitError::Timeout)
            }
        }
    } else {
        tokio::time::timeout(timeout, future)
            .await
            .map_err(|_| ControlledWaitError::Timeout)
    }
}

async fn read_bounded_body(
    response: reqwest::Response,
    budget: usize,
) -> Result<Vec<u8>, DiaryError> {
    let mut body = Vec::with_capacity(budget.min(64 * 1024));
    let mut stream = response.bytes_stream();
    while let Some(chunk) = tokio::time::timeout(STREAM_IDLE_TIMEOUT, stream.next())
        .await
        .map_err(|_| DiaryError::new(DiaryErrorCode::Timeout, "响应流长时间无数据"))?
    {
        let chunk = chunk.map_err(map_transport_error)?;
        let next_size = checked_body_size(body.len(), chunk.len(), budget)?;
        body.reserve(next_size.saturating_sub(body.len()));
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn checked_body_size(current: usize, chunk: usize, budget: usize) -> Result<usize, DiaryError> {
    let next = current
        .checked_add(chunk)
        .ok_or_else(response_too_large_error)?;
    if next > budget {
        return Err(response_too_large_error());
    }
    Ok(next)
}

fn response_too_large_error() -> DiaryError {
    DiaryError::new(
        DiaryErrorCode::ResponseTooLarge,
        "响应超过移动端安全上限，未截断加载",
    )
}

fn map_transport_error(error: reqwest::Error) -> DiaryError {
    if error.is_timeout() {
        DiaryError::new(DiaryErrorCode::Timeout, "网络请求超时")
    } else {
        DiaryError::new(DiaryErrorCode::Transport, "无法连接 VCP 服务")
    }
}

fn map_http_status(status: StatusCode, body: Option<&[u8]>) -> DiaryError {
    let summary = body
        .and_then(safe_error_summary)
        .unwrap_or_else(|| default_status_message(status).to_string());
    let code = match status.as_u16() {
        400 | 422 => DiaryErrorCode::InvalidRequest,
        401 => DiaryErrorCode::AuthRequired,
        403 => DiaryErrorCode::Forbidden,
        404 => DiaryErrorCode::NotFound,
        408 | 504 => DiaryErrorCode::Timeout,
        409 => DiaryErrorCode::Conflict,
        413 => DiaryErrorCode::ResponseTooLarge,
        429 => DiaryErrorCode::RateLimited,
        499 => DiaryErrorCode::Cancelled,
        503 => DiaryErrorCode::ServiceUnavailable,
        300..=399 => DiaryErrorCode::Forbidden,
        500..=599 => DiaryErrorCode::ServerError,
        _ => DiaryErrorCode::InvalidResponse,
    };
    DiaryError::new(code, summary)
}

fn default_status_message(status: StatusCode) -> &'static str {
    match status.as_u16() {
        400 | 422 => "请求参数不符合服务端契约",
        401 => "日记服务凭据无效或缺失",
        403 => "服务端拒绝访问该路径",
        404 => "目标文件夹或文件不存在",
        408 | 504 => "服务端请求超时",
        409 => "服务端检测到文件冲突",
        413 => "请求或响应超过服务端上限",
        429 => "请求过于频繁，请稍后再试",
        499 => "搜索已取消",
        503 => "日记服务暂不可用",
        300..=399 => "服务器返回重定向，已拒绝携带凭据跟随",
        500..=599 => "日记服务内部错误",
        _ => "服务器返回未知状态",
    }
}

fn safe_error_summary(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let object = value.as_object()?;
    for key in ["error", "message"] {
        if let Some(message) = object.get(key).and_then(Value::as_str) {
            let sanitized = sanitize_remote_message(message);
            if !sanitized.is_empty() {
                return Some(sanitized);
            }
        }
    }
    None
}

fn sanitize_remote_message(message: &str) -> String {
    let sanitized = message
        .chars()
        .filter(|character| !character.is_control())
        .take(240)
        .collect::<String>()
        .trim()
        .to_string();
    if remote_message_may_contain_sensitive_data(&sanitized) {
        String::new()
    } else {
        sanitized
    }
}

fn remote_message_may_contain_sensitive_data(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    if lower.contains("authorization")
        || lower.contains("bearer ")
        || lower.contains("basic ")
        || lower.contains("file://")
        || lower.contains("://")
        || message.contains("\\\\")
    {
        return true;
    }

    let bytes = message.as_bytes();
    if bytes.windows(3).any(|window| {
        window[0].is_ascii_alphabetic() && window[1] == b':' && matches!(window[2], b'/' | b'\\')
    }) {
        return true;
    }

    message.split_whitespace().any(|token| {
        let candidate = token.trim_matches(|character| {
            matches!(
                character,
                '\'' | '"' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'
            )
        });
        candidate.len() > 1 && (candidate.starts_with('/') || candidate.starts_with("~/"))
    })
}

fn sanitize_batch_error(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    if lower.contains("already exists") {
        "目标文件已存在".to_string()
    } else if lower.contains("not found") || lower.contains("enoent") {
        "源文件不存在".to_string()
    } else if lower.contains("invalid") || lower.contains("forbidden") {
        "服务端拒绝了该文件路径".to_string()
    } else {
        "服务器未完成该文件操作".to_string()
    }
}

fn normalize_server_base(raw: &str) -> Result<Url, DiaryError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(DiaryError::new(
            DiaryErrorCode::ConfigMissing,
            "尚未配置 VCP Server URL",
        ));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(DiaryError::new(
            DiaryErrorCode::InvalidRequest,
            "VCP Server URL 含控制字符",
        ));
    }

    let mut url = Url::parse(trimmed)
        .map_err(|_| DiaryError::new(DiaryErrorCode::InvalidRequest, "VCP Server URL 格式无效"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(DiaryError::new(
            DiaryErrorCode::InvalidRequest,
            "VCP Server URL 必须是无内嵌凭据的 HTTP(S) 地址",
        ));
    }
    url.set_query(None);
    url.set_fragment(None);

    let path = url.path().trim_end_matches('/');
    let base_path = path.strip_suffix("/v1/chat/completions").unwrap_or(path);
    let normalized_path = if base_path.is_empty() {
        "/".to_string()
    } else {
        format!("{}/", base_path.trim_end_matches('/'))
    };
    url.set_path(&normalized_path);
    Ok(url)
}

fn append_url_segments(url: &mut Url, segments: &[&str]) -> Result<(), DiaryError> {
    let mut path = url.path_segments_mut().map_err(|_| {
        DiaryError::new(
            DiaryErrorCode::InvalidRequest,
            "VCP Server URL 不能作为分层 HTTP 地址",
        )
    })?;
    path.pop_if_empty();
    for segment in segments {
        path.push(segment);
    }
    Ok(())
}

fn ensure_admin_config(settings: &Settings) -> Result<(), DiaryError> {
    normalize_server_base(&settings.vcp_server_url)?;
    if settings.admin_username.trim().is_empty() || settings.admin_password.is_empty() {
        return Err(DiaryError::new(
            DiaryErrorCode::ConfigMissing,
            "日记管理需要管理员用户名与密码",
        ));
    }
    Ok(())
}

fn ensure_bearer_config(settings: &Settings) -> Result<(), DiaryError> {
    normalize_server_base(&settings.vcp_server_url)?;
    if settings.vcp_api_key.trim().is_empty() {
        return Err(DiaryError::new(
            DiaryErrorCode::ConfigMissing,
            "DailyNote 与 LightMemo 需要 VCP API Key",
        ));
    }
    Ok(())
}

fn validate_request_id(request_id: &str) -> Result<(), DiaryError> {
    if request_id.is_empty() || request_id.len() > 160 || request_id.chars().any(char::is_control) {
        return Err(DiaryError::new(
            DiaryErrorCode::InvalidRequest,
            "搜索 requestId 无效",
        ));
    }
    Ok(())
}

fn validate_path_segment(value: &str, label: &str) -> Result<(), DiaryError> {
    let is_windows_absolute = value.len() >= 2
        && value.as_bytes()[0].is_ascii_alphabetic()
        && value.as_bytes()[1] == b':';
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.len() > 1_024
        || value.contains('/')
        || value.contains('\\')
        || value.chars().any(char::is_control)
        || is_windows_absolute
    {
        return Err(DiaryError::new(
            DiaryErrorCode::InvalidRequest,
            format!("{label}不是安全的单一路径段"),
        ));
    }
    Ok(())
}

fn validate_note_key(key: &DiaryNoteKey) -> Result<(), DiaryError> {
    validate_path_segment(&key.folder, "文件夹")?;
    validate_path_segment(&key.file, "文件名")
}

fn validate_hash(hash: &str) -> Result<(), DiaryError> {
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(DiaryError::new(
            DiaryErrorCode::InvalidRequest,
            "baselineHash 必须是 SHA-256",
        ));
    }
    Ok(())
}

fn validate_document_content(content: &str) -> Result<(), DiaryError> {
    if content.len() > MAX_DOCUMENT_BYTES {
        return Err(DiaryError::new(
            DiaryErrorCode::ResponseTooLarge,
            "正文超过 2 MiB 移动端上限",
        ));
    }
    Ok(())
}

fn validate_search_term(term: &str) -> Result<(), DiaryError> {
    let trimmed = term.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_SEARCH_TERM_CHARS {
        return Err(DiaryError::new(
            DiaryErrorCode::InvalidRequest,
            "普通搜索词必须为 1 至 100 个字符",
        ));
    }
    Ok(())
}

fn validate_semantic_request(request: &DiarySemanticSearchRequest) -> Result<(), DiaryError> {
    let query = request.query.trim();
    if query.is_empty() || query.chars().count() > MAX_SEMANTIC_QUERY_CHARS {
        return Err(DiaryError::new(
            DiaryErrorCode::InvalidRequest,
            "语义查询必须为 1 至 4000 个字符",
        ));
    }
    if !(1..=50).contains(&request.k) {
        return Err(DiaryError::new(
            DiaryErrorCode::InvalidRequest,
            "LightMemo k 必须在 1 至 50 之间",
        ));
    }
    if request.search_all {
        if request.folder.is_some() {
            return Err(DiaryError::new(
                DiaryErrorCode::InvalidRequest,
                "全局语义搜索不能同时指定 folder",
            ));
        }
    } else {
        let folder = request.folder.as_deref().ok_or_else(|| {
            DiaryError::new(
                DiaryErrorCode::InvalidRequest,
                "当前文件夹语义搜索必须指定 folder",
            )
        })?;
        validate_path_segment(folder, "文件夹")?;
    }
    Ok(())
}

fn validate_create_request(request: &DiaryCreateRequest) -> Result<(), DiaryError> {
    if request.maid.trim().is_empty() || request.maid.chars().count() > 200 {
        return Err(DiaryError::new(
            DiaryErrorCode::InvalidRequest,
            "署名不能为空且不能超过 200 字符",
        ));
    }
    if request.date.trim().is_empty() || request.date.chars().count() > 64 {
        return Err(DiaryError::new(
            DiaryErrorCode::InvalidRequest,
            "日期不能为空且不能超过 64 字符",
        ));
    }
    validate_document_content(&request.content)?;
    if request.content.trim().is_empty() {
        return Err(DiaryError::new(
            DiaryErrorCode::InvalidRequest,
            "日记正文不能为空",
        ));
    }
    if let Some(folder) = request.folder.as_ref().filter(|value| !value.is_empty()) {
        validate_path_segment(folder, "文件夹")?;
    }
    if let Some(file_name) = request
        .file_name_suffix
        .as_ref()
        .filter(|value| !value.is_empty())
    {
        validate_path_segment(file_name, "文件名后缀")?;
    }
    if request
        .tag
        .as_ref()
        .is_some_and(|tag| tag.chars().count() > 2_000)
    {
        return Err(DiaryError::new(
            DiaryErrorCode::InvalidRequest,
            "Tag 不能超过 2000 字符",
        ));
    }
    Ok(())
}

fn validate_batch(sources: &[DiaryNoteKey]) -> Result<(), DiaryError> {
    if sources.is_empty() || sources.len() > MAX_BATCH_ITEMS {
        return Err(DiaryError::new(
            DiaryErrorCode::InvalidRequest,
            "批量操作数量必须在 1 至 1000 之间",
        ));
    }
    let mut seen = HashSet::with_capacity(sources.len());
    for key in sources {
        validate_note_key(key)?;
        if !seen.insert(key) {
            return Err(DiaryError::new(
                DiaryErrorCode::InvalidRequest,
                "批量操作包含重复文件",
            ));
        }
    }
    Ok(())
}

fn normalize_rename_target(source: &str, requested: &str) -> Result<String, DiaryError> {
    let trimmed = requested.trim();
    validate_path_segment(trimmed, "目标文件名")?;
    let source_extension = source.rsplit_once('.').map(|(_, extension)| extension);
    let requested_extension = trimmed.rsplit_once('.').map(|(_, extension)| extension);
    let target = match (source_extension, requested_extension) {
        (Some(source_extension), None) => format!("{trimmed}.{source_extension}"),
        (Some(source_extension), Some(target_extension))
            if source_extension.eq_ignore_ascii_case(target_extension) =>
        {
            trimmed.to_string()
        }
        (Some(_), Some(_)) => {
            return Err(DiaryError::new(
                DiaryErrorCode::InvalidRequest,
                "重命名必须保留原扩展名",
            ));
        }
        (None, _) => trimmed.to_string(),
    };
    validate_path_segment(&target, "目标文件名")?;
    Ok(target)
}

fn content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn cancelled_error() -> DiaryError {
    DiaryError::new(DiaryErrorCode::Cancelled, "搜索已取消")
}

fn mutation_outcome_may_be_ambiguous(code: DiaryErrorCode) -> bool {
    matches!(
        code,
        DiaryErrorCode::Timeout
            | DiaryErrorCode::Transport
            | DiaryErrorCode::ResponseTooLarge
            | DiaryErrorCode::InvalidResponse
            | DiaryErrorCode::ServiceUnavailable
            | DiaryErrorCode::ServerError
    )
}

fn create_uncertain_error() -> DiaryError {
    DiaryError::new(
        DiaryErrorCode::CreateUncertain,
        "创建结果无法确认；请返回列表刷新目标文件夹，核对后再决定是否重建",
    )
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn escape_tool_value(value: &str) -> Result<String, DiaryError> {
    if contains_reserved_field_escape_marker(value)
        || value.contains(TOOL_REQUEST_START_ESCAPE)
        || value.contains(TOOL_REQUEST_END_ESCAPE)
    {
        return Err(DiaryError::new(
            DiaryErrorCode::InvalidRequest,
            "输入包含 Human Tool ESCAPE 协议保留标记",
        ));
    }
    Ok(value
        .replace(TOOL_REQUEST_START, TOOL_REQUEST_START_ESCAPE)
        .replace(TOOL_REQUEST_END, TOOL_REQUEST_END_ESCAPE))
}

fn contains_reserved_field_escape_marker(value: &str) -> bool {
    // VCP's authoritative parser accepts ESCAPE case-insensitively and allows
    // either Chinese quotes or braces on each side. VCPChat also recognizes the
    // historical EXP spelling, so reject that bounded compatibility surface too.
    const RESERVED_MARKERS: [&str; 16] = [
        "「始escape」",
        "「始escape}",
        "{始escape」",
        "{始escape}",
        "「末escape」",
        "「末escape}",
        "{末escape」",
        "{末escape}",
        "「始exp」",
        "「始exp}",
        "{始exp」",
        "{始exp}",
        "「末exp」",
        "「末exp}",
        "{末exp」",
        "{末exp}",
    ];
    let normalized = value.to_ascii_lowercase();
    RESERVED_MARKERS
        .iter()
        .any(|marker| normalized.contains(marker))
}

fn serialize_tool_request(fields: &[(&str, String)]) -> Result<String, DiaryError> {
    let mut output = String::from(TOOL_REQUEST_START);
    output.push('\n');
    for (index, (key, value)) in fields.iter().enumerate() {
        if key.is_empty()
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(DiaryError::new(
                DiaryErrorCode::InvalidRequest,
                "Human Tool 字段名无效",
            ));
        }
        let escaped = escape_tool_value(value)?;
        output.push_str(key);
        output.push(':');
        output.push_str(FIELD_ESCAPE_START);
        output.push_str(&escaped);
        output.push_str(FIELD_ESCAPE_END);
        if index + 1 < fields.len() {
            output.push(',');
        }
        output.push('\n');
    }
    output.push_str(TOOL_REQUEST_END);
    Ok(output)
}

fn parse_nested_json(value: &str) -> Option<Value> {
    let trimmed = value.trim();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

fn find_tool_error(value: &Value, depth: usize) -> Option<String> {
    if depth > 10 {
        return None;
    }
    match value {
        Value::String(text) => parse_nested_json(text)
            .as_ref()
            .and_then(|nested| find_tool_error(nested, depth + 1)),
        Value::Array(items) => items
            .iter()
            .find_map(|item| find_tool_error(item, depth + 1)),
        Value::Object(object) => {
            for key in ["plugin_error", "error"] {
                if let Some(message) = object.get(key).and_then(Value::as_str) {
                    if !message.trim().is_empty() {
                        return Some("Human Tool 执行失败，远端细节已隐藏".to_string());
                    }
                }
            }
            if object
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| status.eq_ignore_ascii_case("error"))
            {
                return Some("Human Tool 返回错误状态".to_string());
            }
            ["original_plugin_output", "result", "content", "data"]
                .iter()
                .filter_map(|key| object.get(*key))
                .find_map(|nested| find_tool_error(nested, depth + 1))
        }
        _ => None,
    }
}

fn find_create_outcome(value: &Value, depth: usize) -> Option<(String, String)> {
    if depth > 10 {
        return None;
    }
    match value {
        Value::String(text) => parse_nested_json(text)
            .as_ref()
            .and_then(|nested| find_create_outcome(nested, depth + 1)),
        Value::Array(items) => items
            .iter()
            .find_map(|item| find_create_outcome(item, depth + 1)),
        Value::Object(object) => {
            let folder = object.get("folder").and_then(Value::as_str);
            let file = object
                .get("fileName")
                .or_else(|| object.get("file"))
                .and_then(Value::as_str);
            if let (Some(folder), Some(file)) = (folder, file) {
                return Some((folder.to_string(), file.to_string()));
            }
            ["original_plugin_output", "result", "content", "data"]
                .iter()
                .filter_map(|key| object.get(*key))
                .find_map(|nested| find_create_outcome(nested, depth + 1))
        }
        _ => None,
    }
}

fn extract_tool_text(value: &Value, depth: usize) -> Option<String> {
    if depth > 10 {
        return None;
    }
    match value {
        Value::String(text) => {
            if let Some(nested) = parse_nested_json(text) {
                extract_tool_text(&nested, depth + 1).or_else(|| Some(text.clone()))
            } else if text.trim().is_empty() {
                None
            } else {
                Some(text.clone())
            }
        }
        Value::Array(items) => {
            let joined = items
                .iter()
                .filter_map(|item| extract_tool_text(item, depth + 1))
                .collect::<Vec<_>>()
                .join("\n");
            (!joined.is_empty()).then_some(joined)
        }
        Value::Object(object) => {
            for key in [
                "original_plugin_output",
                "result",
                "content",
                "text",
                "data",
            ] {
                if let Some(nested) = object.get(key) {
                    if let Some(text) = extract_tool_text(nested, depth + 1) {
                        return Some(text);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn parse_semantic_hits(output: &str) -> Vec<DiarySemanticHit> {
    let normalized = output.replace("\r\n", "\n").replace('\r', "\n");
    let mut sections: Vec<Vec<&str>> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in normalized.lines() {
        if is_semantic_source_line(line) {
            if !current.is_empty() {
                sections.push(current);
            }
            current = vec![line];
        } else if !current.is_empty() {
            current.push(line);
        }
    }
    if !current.is_empty() {
        sections.push(current);
    }

    let mut hits: Vec<DiarySemanticHit> = Vec::new();
    let mut indices: HashMap<DiaryNoteKey, usize> = HashMap::new();
    for section in sections {
        let header = section.first().copied().unwrap_or_default();
        let source_folder = parse_semantic_source_folder(header);
        let path = section
            .iter()
            .find_map(|line| parse_semantic_path_line(line));
        let Some((parent_folder, file)) = path else {
            continue;
        };
        let folder = source_folder
            .filter(|value| !value.is_empty())
            .unwrap_or(parent_folder);
        let key = DiaryNoteKey { folder, file };
        if validate_note_key(&key).is_err() {
            continue;
        }

        let preview = section
            .iter()
            .skip(1)
            .map(|line| line.trim())
            .filter(|line| !line.is_empty() && !is_semantic_metadata_line(line))
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(300)
            .collect::<String>();
        let hit = DiarySemanticHit {
            key: key.clone(),
            preview: if preview.is_empty() {
                "语义匹配片段".to_string()
            } else {
                preview
            },
            score: parse_semantic_score(header),
        };
        if let Some(index) = indices.get(&key).copied() {
            let existing = &mut hits[index];
            if hit.preview.chars().count() > existing.preview.chars().count() {
                existing.preview = hit.preview;
            }
            existing.score = match (existing.score, hit.score) {
                (Some(left), Some(right)) => Some(left.max(right)),
                (score @ Some(_), None) | (None, score @ Some(_)) => score,
                (None, None) => None,
            };
        } else {
            indices.insert(key, hits.len());
            hits.push(hit);
        }
    }
    hits
}

fn is_semantic_source_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("---") && (trimmed.contains("来源:") || trimmed.contains("来源："))
}

fn parse_semantic_source_folder(header: &str) -> Option<String> {
    let marker_index = header.find("来源:").or_else(|| header.find("来源："))?;
    let marker_len = if header[marker_index..].starts_with("来源：") {
        "来源：".len()
    } else {
        "来源:".len()
    };
    let rest = header[marker_index + marker_len..].trim();
    let end = [", 相关", "，相关", ",相关", "， 相关", ")"]
        .iter()
        .filter_map(|marker| rest.find(marker))
        .min()
        .unwrap_or(rest.len());
    let folder = rest[..end].trim();
    (!folder.is_empty()).then(|| folder.to_string())
}

fn parse_semantic_path_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if !trimmed.starts_with("[路径") || !trimmed.ends_with(']') {
        return None;
    }
    let colon = trimmed.find(':').or_else(|| trimmed.find('：'))?;
    let mut path = trimmed[colon + 1..trimmed.len() - 1].trim().to_string();
    for prefix in ["file:///", "file://", "file:"] {
        if path.to_ascii_lowercase().starts_with(prefix) {
            path = path[prefix.len()..].to_string();
            break;
        }
    }
    if let Some(index) = path.find(['?', '#']) {
        path.truncate(index);
    }
    let decoded = percent_decode_str(&path).decode_utf8_lossy();
    let normalized = decoded.replace('\\', "/");
    let mut parts = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let file = parts.pop()?.to_string();
    let parent = parts.pop()?.to_string();
    Some((parent, file))
}

fn is_semantic_metadata_line(line: &str) -> bool {
    line.starts_with("[路径")
        || line.starts_with("[TagMemo")
        || line.starts_with("[RiverMemo")
        || line.starts_with("Tag:")
        || line.starts_with("Tag：")
        || line.starts_with("---")
        || line.starts_with("[---")
        || line.starts_with("[查询内容")
        || line.starts_with("[搜索范围")
        || line.starts_with("[找到")
}

fn parse_semantic_score(header: &str) -> Option<f64> {
    let marker = header.find("相关性:").or_else(|| header.find("相关性："))?;
    let rest = &header[marker..];
    let percent_index = rest.find('%')?;
    let numeric = rest[..percent_index]
        .chars()
        .rev()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    numeric.parse::<f64>().ok().map(|value| value / 100.0)
}

#[derive(Debug, Deserialize)]
struct FolderEnvelope {
    folders: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct NotesEnvelope {
    #[serde(alias = "memos")]
    notes: Vec<RawNoteSummary>,
}

#[derive(Debug, Deserialize)]
struct NoteEnvelope {
    content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawNoteSummary {
    name: String,
    #[serde(default)]
    folder_name: Option<String>,
    #[serde(default)]
    last_modified: String,
    #[serde(default)]
    preview: String,
}

#[derive(Debug, Deserialize)]
struct SearchEnvelope {
    #[serde(default)]
    notes: Vec<RawNoteSummary>,
    #[serde(default)]
    total: usize,
    #[serde(default)]
    limited: bool,
}

#[derive(Debug, Deserialize)]
struct RawBatchError {
    note: String,
    #[serde(default)]
    error: String,
}

#[derive(Debug, Deserialize)]
struct MoveEnvelope {
    #[serde(default)]
    moved: Vec<String>,
    #[serde(default)]
    errors: Vec<RawBatchError>,
}

#[derive(Debug, Deserialize)]
struct DeleteEnvelope {
    #[serde(default)]
    deleted: Vec<String>,
    #[serde(default)]
    errors: Vec<RawBatchError>,
}

fn normalize_note_summaries(
    raw: Vec<RawNoteSummary>,
    fallback_folder: Option<&str>,
    require_folder: bool,
) -> Result<Vec<DiaryNoteSummary>, DiaryError> {
    if raw.len() > MAX_NOTE_COUNT {
        return Err(DiaryError::new(
            DiaryErrorCode::ResponseTooLarge,
            "文件摘要数量超过移动端上限",
        ));
    }
    raw.into_iter()
        .map(|note| {
            let folder = note
                .folder_name
                .as_deref()
                .filter(|value| !value.is_empty())
                .or(fallback_folder)
                .ok_or_else(|| {
                    DiaryError::new(
                        DiaryErrorCode::InvalidResponse,
                        "跨文件夹结果缺少 folderName",
                    )
                })?;
            if require_folder && note.folder_name.as_deref().unwrap_or_default().is_empty() {
                return Err(DiaryError::new(
                    DiaryErrorCode::InvalidResponse,
                    "全局搜索结果缺少 folderName",
                ));
            }
            let key = DiaryNoteKey {
                folder: folder.to_string(),
                file: note.name,
            };
            validate_note_key(&key)?;
            if note.preview.chars().count() > MAX_PREVIEW_CHARS
                || note.last_modified.chars().count() > 256
            {
                return Err(DiaryError::new(
                    DiaryErrorCode::InvalidResponse,
                    "文件摘要字段超过契约上限",
                ));
            }
            Ok(DiaryNoteSummary {
                folder: key.folder,
                file: key.file,
                last_modified: note.last_modified,
                preview: note.preview,
            })
        })
        .collect()
}

fn key_wire_value(key: &DiaryNoteKey) -> String {
    format!("{}/{}", key.folder, key.file)
}

fn error_map(
    requested: &[DiaryNoteKey],
    errors: Vec<RawBatchError>,
) -> HashMap<DiaryNoteKey, String> {
    let by_wire = requested
        .iter()
        .map(|key| (key_wire_value(key), key.clone()))
        .collect::<HashMap<_, _>>();
    errors
        .into_iter()
        .filter_map(|error| {
            by_wire
                .get(&error.note)
                .cloned()
                .map(|key| (key, sanitize_batch_error(&error.error)))
        })
        .collect()
}

fn normalize_move_outcome(requested: &[DiaryNoteKey], response: MoveEnvelope) -> DiaryBatchOutcome {
    let errors = error_map(requested, response.errors);
    let succeeded = requested
        .iter()
        .filter(|key| {
            let prefix = format!("{} to ", key_wire_value(key));
            response.moved.iter().any(|item| item.starts_with(&prefix))
                && !errors.contains_key(*key)
        })
        .cloned()
        .collect::<Vec<_>>();
    let succeeded_set = succeeded.iter().cloned().collect::<HashSet<_>>();
    let error_items = requested
        .iter()
        .filter(|key| !succeeded_set.contains(*key))
        .map(|key| DiaryBatchError {
            key: key.clone(),
            message: errors
                .get(key)
                .cloned()
                .unwrap_or_else(|| "服务器未返回该文件的移动结果".to_string()),
        })
        .collect();
    DiaryBatchOutcome {
        succeeded,
        errors: error_items,
    }
}

fn normalize_delete_outcome(
    requested: &[DiaryNoteKey],
    response: DeleteEnvelope,
) -> DiaryBatchOutcome {
    let errors = error_map(requested, response.errors);
    let deleted = response.deleted.into_iter().collect::<HashSet<_>>();
    let succeeded = requested
        .iter()
        .filter(|key| deleted.contains(&key_wire_value(key)) && !errors.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();
    let succeeded_set = succeeded.iter().cloned().collect::<HashSet<_>>();
    let error_items = requested
        .iter()
        .filter(|key| !succeeded_set.contains(*key))
        .map(|key| DiaryBatchError {
            key: key.clone(),
            message: errors
                .get(key)
                .cloned()
                .unwrap_or_else(|| "服务器未返回该文件的删除结果".to_string()),
        })
        .collect();
    DiaryBatchOutcome {
        succeeded,
        errors: error_items,
    }
}

async fn settings_snapshot<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
) -> Result<Settings, String> {
    if app_handle
        .try_state::<crate::vcp_modules::db_manager::DbState>()
        .is_none()
    {
        return Err(
            DiaryError::new(DiaryErrorCode::ServiceUnavailable, "应用尚未完成初始化")
                .command_string(),
        );
    }
    read_settings(app_handle, settings_state)
        .await
        .map_err(|_| {
            DiaryError::new(DiaryErrorCode::ServiceUnavailable, "无法读取应用设置").command_string()
        })
}

fn command_error(error: DiaryError) -> String {
    error.command_string()
}

#[tauri::command]
pub async fn diary_list_folders<R: Runtime>(
    app_handle: AppHandle<R>,
    service: State<'_, DiaryServiceState>,
    settings_state: State<'_, SettingsState>,
) -> Result<DiaryFolderList, String> {
    let settings = settings_snapshot(app_handle, settings_state).await?;
    service.list_folders(&settings).await.map_err(command_error)
}

#[tauri::command]
pub async fn diary_list_notes<R: Runtime>(
    app_handle: AppHandle<R>,
    service: State<'_, DiaryServiceState>,
    settings_state: State<'_, SettingsState>,
    folder: String,
) -> Result<Vec<DiaryNoteSummary>, String> {
    let settings = settings_snapshot(app_handle, settings_state).await?;
    service
        .list_notes(&settings, &folder)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn diary_get_note<R: Runtime>(
    app_handle: AppHandle<R>,
    service: State<'_, DiaryServiceState>,
    settings_state: State<'_, SettingsState>,
    key: DiaryNoteKey,
) -> Result<DiaryDocument, String> {
    let settings = settings_snapshot(app_handle, settings_state).await?;
    service
        .get_note(&settings, &key)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn diary_search<R: Runtime>(
    app_handle: AppHandle<R>,
    service: State<'_, DiaryServiceState>,
    settings_state: State<'_, SettingsState>,
    request: DiarySearchRequest,
) -> Result<DiarySearchResponse, String> {
    let settings = settings_snapshot(app_handle, settings_state).await?;
    service
        .search(&settings, &request)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn diary_cancel_search(
    service: State<'_, DiaryServiceState>,
    request: DiaryCancelRequest,
) -> Result<(), String> {
    DiaryServiceState::cancel_search_owner(
        &service.text_search_lifecycle,
        &service.text_search_owner,
        request.request_id.as_deref(),
    )
    .await;
    Ok(())
}

#[tauri::command]
pub async fn diary_semantic_search<R: Runtime>(
    app_handle: AppHandle<R>,
    service: State<'_, DiaryServiceState>,
    settings_state: State<'_, SettingsState>,
    request: DiarySemanticSearchRequest,
) -> Result<DiarySemanticResponse, String> {
    let settings = settings_snapshot(app_handle, settings_state).await?;
    service
        .semantic_search(&settings, &request)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn diary_cancel_semantic_search(
    service: State<'_, DiaryServiceState>,
    request: DiaryCancelRequest,
) -> Result<(), String> {
    DiaryServiceState::cancel_search_owner(
        &service.semantic_search_lifecycle,
        &service.semantic_search_owner,
        request.request_id.as_deref(),
    )
    .await;
    Ok(())
}

#[tauri::command]
pub async fn diary_save_note<R: Runtime>(
    app_handle: AppHandle<R>,
    service: State<'_, DiaryServiceState>,
    settings_state: State<'_, SettingsState>,
    request: DiarySaveRequest,
) -> Result<DiarySaveOutcome, String> {
    let settings = settings_snapshot(app_handle, settings_state).await?;
    service
        .save_note(&settings, &request)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn diary_rename_note<R: Runtime>(
    app_handle: AppHandle<R>,
    service: State<'_, DiaryServiceState>,
    settings_state: State<'_, SettingsState>,
    request: DiaryRenameRequest,
) -> Result<DiaryRenameOutcome, String> {
    let settings = settings_snapshot(app_handle, settings_state).await?;
    service
        .rename_note(&settings, &request)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn diary_create_note<R: Runtime>(
    app_handle: AppHandle<R>,
    service: State<'_, DiaryServiceState>,
    settings_state: State<'_, SettingsState>,
    request: DiaryCreateRequest,
) -> Result<DiaryCreateOutcome, String> {
    let settings = settings_snapshot(app_handle, settings_state).await?;
    service
        .create_note(&settings, &request)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn diary_move_notes<R: Runtime>(
    app_handle: AppHandle<R>,
    service: State<'_, DiaryServiceState>,
    settings_state: State<'_, SettingsState>,
    request: DiaryMoveRequest,
) -> Result<DiaryBatchOutcome, String> {
    let settings = settings_snapshot(app_handle, settings_state).await?;
    service
        .move_notes(&settings, &request)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn diary_delete_notes<R: Runtime>(
    app_handle: AppHandle<R>,
    service: State<'_, DiaryServiceState>,
    settings_state: State<'_, SettingsState>,
    request: DiaryDeleteRequest,
) -> Result<DiaryBatchOutcome, String> {
    let settings = settings_snapshot(app_handle, settings_state).await?;
    service
        .delete_notes(&settings, &request)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn diary_delete_empty_folder<R: Runtime>(
    app_handle: AppHandle<R>,
    service: State<'_, DiaryServiceState>,
    settings_state: State<'_, SettingsState>,
    request: DiaryDeleteFolderRequest,
) -> Result<(), String> {
    let settings = settings_snapshot(app_handle, settings_state).await?;
    service
        .delete_empty_folder(&settings, &request.folder)
        .await
        .map_err(command_error)
}

#[cfg(test)]
#[path = "../../../tests/unit/diary_service_tests.rs"]
mod tests;
