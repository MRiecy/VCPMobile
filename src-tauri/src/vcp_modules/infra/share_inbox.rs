//! A single durable share slot. Its file lock is also used by the iOS extension.
//! Copy/render work happens before the short identity + SQLite commit boundary.
use crate::vcp_modules::{
    chat_manager::{Attachment, ChatMessage},
    db_manager::DbState,
    message_repository::{compile_and_serialize_render_async, MessageRepository},
    message_service::{normalize_attachments_from_local_cas, MessageWriteResultDto},
    sync_hash::HashAggregator,
    topic_types::TopicKey,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    path::{Path, PathBuf},
};
#[cfg(any(target_os = "ios", target_os = "android"))]
use tauri::Manager;
use tauri::{AppHandle, State};

const MAX_MANIFEST: u64 = 2 * 1024 * 1024;
const MAX_TEXT: usize = 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Slot {
    generation: u64,
    delivery_id: String,
    state: String,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareItem {
    item_id: String,
    file_name: String,
    #[serde(default)]
    mime_type: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    sha256: String,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    attachment: Option<Attachment>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentShare {
    generation: u64,
    delivery_id: String,
    state: String,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    text: String,
    #[serde(default)]
    items: Vec<ShareItem>,
    #[serde(default)]
    attachments: Vec<Attachment>,
    #[serde(default)]
    manifest_hash: String,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    topic_id: Option<String>,
    #[serde(default)]
    revision: u64,
    #[serde(default)]
    message: Option<ChatMessage>,
}

#[derive(Deserialize)]
struct Manifest {
    text: String,
    items: Vec<ShareItem>,
}

fn leaf(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 100
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("分享文件身份无效".into());
    }
    Ok(())
}

async fn root(app: &AppHandle) -> Result<PathBuf, String> {
    #[cfg(target_os = "ios")]
    let path = {
        #[derive(Deserialize)]
        struct Location {
            path: String,
        }
        let app = app.clone();
        let result: Location = tauri::async_runtime::spawn_blocking(move || {
            app.state::<tauri_plugin_vcp_mobile::VcpMobileState<tauri::Wry>>()
                .mobile_plugin_handle()?
                .run_mobile_plugin("shareInboxLocation", serde_json::json!({}))
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())??;
        PathBuf::from(result.path)
    };
    #[cfg(not(target_os = "ios"))]
    let path = super::file_manager::get_support_root_dir(app)?.join("ShareInbox");
    tokio::fs::create_dir_all(&path)
        .await
        .map_err(|e| e.to_string())?;
    Ok(path)
}

// Rust 1.97's std file locks do not support Android. Use Bionic flock there;
// Apple std locks already use flock, matching the Swift extension. The owning
// File remains the lease: closing it releases the lock on every exit path.
fn lock_file(file: &File) -> std::io::Result<()> {
    #[cfg(target_os = "android")]
    {
        use std::os::fd::AsRawFd;
        loop {
            // SAFETY: file is borrowed throughout the call and owns a valid descriptor.
            // flock only locks that descriptor; it neither closes nor takes ownership of it.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                return Ok(());
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
    #[cfg(not(target_os = "android"))]
    file.lock()
}

fn try_lock_file(file: &File) -> Result<(), std::fs::TryLockError> {
    #[cfg(target_os = "android")]
    {
        use std::os::fd::AsRawFd;
        loop {
            // SAFETY: file owns the descriptor for the whole borrowed call. flock does
            // not retain or close it; File drop still owns release of the acquired lease.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(());
            }
            let error = std::io::Error::last_os_error();
            match error.kind() {
                std::io::ErrorKind::Interrupted => continue,
                std::io::ErrorKind::WouldBlock => return Err(std::fs::TryLockError::WouldBlock),
                _ => return Err(std::fs::TryLockError::Error(error)),
            }
        }
    }
    #[cfg(not(target_os = "android"))]
    file.try_lock()
}

async fn lock(root: &Path) -> Result<File, String> {
    let path = root.join("current.lock");
    tokio::task::spawn_blocking(move || {
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|e| e.to_string())?;
        lock_file(&file).map_err(|e| e.to_string())?;
        Ok(file)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn slot(root: &Path) -> Result<Option<Slot>, String> {
    let path = root.join("current.json");
    if !path.exists() {
        return Ok(None);
    }
    if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 8192 {
        return Err("分享槽位损坏".into());
    }
    let slot: Slot = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    leaf(&slot.delivery_id)?;
    if slot.generation == 0 || !["receiving", "ready", "failed"].contains(&slot.state.as_str()) {
        return Err("分享槽位状态损坏".into());
    }
    Ok(Some(slot))
}

fn write_slot(root: &Path, slot: &Slot) -> Result<(), String> {
    use std::io::Write;
    let temporary = root.join("current.tmp");
    let mut file = File::create(&temporary).map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec(slot).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    std::fs::rename(temporary, root.join("current.json")).map_err(|e| e.to_string())
}

fn assert_current(root: &Path, generation: u64, delivery_id: &str) -> Result<Slot, String> {
    slot(root)?
        .filter(|s| s.generation == generation && s.delivery_id == delivery_id)
        .ok_or_else(|| "分享已被新的分享替换".to_string())
}

async fn load(db: &mut sqlx::SqliteConnection) -> Result<Option<CurrentShare>, String> {
    let value: Option<(String,)> =
        sqlx::query_as("SELECT record FROM current_share WHERE singleton = 1")
            .fetch_optional(&mut *db)
            .await
            .map_err(|e| e.to_string())?;
    value
        .map(|v| serde_json::from_str(&v.0).map_err(|e| e.to_string()))
        .transpose()
}

async fn save(db: &mut sqlx::SqliteConnection, share: &CurrentShare) -> Result<(), String> {
    sqlx::query("INSERT INTO current_share(singleton,record) VALUES(1,?) ON CONFLICT(singleton) DO UPDATE SET record=excluded.record")
        .bind(serde_json::to_string(share).map_err(|e| e.to_string())?)
        .execute(&mut *db).await.map_err(|e| e.to_string())?;
    Ok(())
}

async fn refresh_share_attachment_paths<R: tauri::Runtime>(
    app: &AppHandle<R>,
    share: &mut CurrentShare,
) {
    for attachment in &mut share.attachments {
        super::file_manager::refresh_attachment_paths(app, attachment).await;
    }
    for item in &mut share.items {
        if let Some(attachment) = item.attachment.as_mut() {
            super::file_manager::refresh_attachment_paths(app, attachment).await;
        }
    }
    if let Some(attachments) = share.message.as_mut().and_then(|message| message.attachments.as_mut()) {
        for attachment in attachments {
            super::file_manager::refresh_attachment_paths(app, attachment).await;
        }
    }
}

/// The startup relocation transaction owns this update; share identity,
/// generation, revision and committed message contents remain unchanged.
pub(crate) async fn reconcile_current_share_paths<R: tauri::Runtime>(
    app: &AppHandle<R>,
    db: &mut sqlx::SqliteConnection,
) -> Result<(), String> {
    if let Some(mut share) = load(db).await? {
        refresh_share_attachment_paths(app, &mut share).await;
        save(db, &share).await?;
    }
    Ok(())
}

fn from_slot(slot: &Slot) -> CurrentShare {
    CurrentShare {
        generation: slot.generation,
        delivery_id: slot.delivery_id.clone(),
        state: slot.state.clone(),
        error: slot.error.clone(),
        text: String::new(),
        items: vec![],
        attachments: vec![],
        manifest_hash: String::new(),
        agent_id: None,
        topic_id: None,
        revision: 0,
        message: None,
    }
}

fn matches(share: &CurrentShare, generation: u64, delivery_id: &str) -> bool {
    share.generation == generation && share.delivery_id == delivery_id
}

fn terminal(share: &CurrentShare) -> bool {
    share.state == "sent" || share.state == "discarded"
}

struct AndroidOwnerSnapshot {
    observed_slot: Option<Slot>,
    owner_id: Option<String>,
}

fn acquisition_interrupted(
    root: &Path,
    slot: &Slot,
    android_owner: Option<&AndroidOwnerSnapshot>,
) -> bool {
    if slot.state != "receiving" {
        return false;
    }
    if let Some(snapshot) = android_owner {
        // The native query runs without the DB/slot locks. Its answer can only
        // establish liveness for the identity observed before that query.
        if !snapshot.observed_slot.as_ref().is_some_and(|observed| {
            observed.generation == slot.generation && observed.delivery_id == slot.delivery_id
        }) || snapshot.owner_id.as_deref() == Some(slot.delivery_id.as_str())
        {
            return false;
        }
    }
    match File::options()
        .read(true)
        .write(true)
        .open(root.join(&slot.delivery_id).join("lease.lock"))
    {
        Ok(file) => try_lock_file(&file).is_ok(),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

pub(crate) fn active_attachment_hashes(record: &str) -> Result<Vec<String>, String> {
    let share: CurrentShare = serde_json::from_str(record).map_err(|e| e.to_string())?;
    if terminal(&share) {
        return Ok(vec![]);
    }
    Ok(share
        .attachments
        .iter()
        .chain(share.items.iter().filter_map(|i| i.attachment.as_ref()))
        .filter_map(|a| a.hash.clone())
        .chain(
            share
                .items
                .iter()
                .filter(|item| super::utils::is_valid_cas_hash(&item.sha256))
                .map(|item| item.sha256.clone()),
        )
        .collect())
}

async fn cleanup(root: PathBuf, finished_delivery: Option<String>) {
    let _ = tokio::task::spawn_blocking(move || {
        let Ok(entries) = std::fs::read_dir(&root) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if leaf(&name).is_err() {
                continue;
            }
            let path = entry.path();
            if !entry
                .file_type()
                .is_ok_and(|t| t.is_dir() && !t.is_symlink())
            {
                continue;
            }
            let Ok(slot_guard) = File::options()
                .read(true)
                .write(true)
                .open(root.join("current.lock"))
            else {
                continue;
            };
            if lock_file(&slot_guard).is_err() {
                continue;
            }
            let Ok(latest) = slot(&root) else {
                continue;
            };
            if latest.as_ref().is_some_and(|s| s.delivery_id == name)
                && finished_delivery.as_deref() != Some(&name)
            {
                continue;
            }
            let Ok(lease) = File::options()
                .read(true)
                .write(true)
                .open(path.join("lease.lock"))
            else {
                continue;
            };
            if try_lock_file(&lease).is_err() {
                continue;
            }
            // A live provider/import holds this directory lease. No other generation is touched.
            let _ = std::fs::remove_dir_all(path);
        }
    })
    .await;
}

#[tauri::command]
pub async fn read_current_share(
    app: AppHandle,
    db: State<'_, DbState>,
) -> Result<Option<CurrentShare>, String> {
    let root = root(&app).await?;
    #[cfg(target_os = "android")]
    let android_owner = {
        let observed_slot = {
            let _guard = lock(&root).await?;
            slot(&root)?
        };
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Owner {
            delivery_id: Option<String>,
        }
        let app = app.clone();
        let result: Owner = tokio::task::spawn_blocking(move || {
            app.state::<tauri_plugin_vcp_mobile::VcpMobileState<tauri::Wry>>()
                .mobile_plugin_handle()?
                .run_mobile_plugin("shareInboxOwner", serde_json::json!({}))
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())??;
        Some(AndroidOwnerSnapshot {
            observed_slot,
            owner_id: result.delivery_id,
        })
    };
    #[cfg(not(target_os = "android"))]
    let android_owner: Option<AndroidOwnerSnapshot> = None;
    let mut tx = db.write_transaction("share.reconcile").await?;
    let guard = lock(&root).await?;
    let Some(mut slot) = slot(&root)? else {
        return Ok(None);
    };
    if acquisition_interrupted(&root, &slot, android_owner.as_ref()) {
        slot.state = "failed".into();
        slot.error = Some("分享获取已中断，请从来源应用重新分享".into());
        write_slot(&root, &slot)?;
    }
    let previous = load(&mut tx).await?;
    let replaced = previous
        .as_ref()
        .filter(|p| !matches(p, slot.generation, &slot.delivery_id));
    let old_operations: Vec<String> = replaced
        .map(|p| {
            p.items
                .iter()
                .map(|i| format!("share_{}_{}", p.delivery_id, i.item_id))
                .collect()
        })
        .unwrap_or_default();
    let mut share = previous
        .filter(|p| matches(p, slot.generation, &slot.delivery_id))
        .unwrap_or_else(|| from_slot(&slot));
    if !terminal(&share) && (slot.state == "failed" || slot.state == "receiving") {
        share.state = slot.state.clone();
        share.error = slot.error.clone();
    } else if share.state == "receiving" && slot.state == "ready" {
        share.state = "ready".into();
    }
    refresh_share_attachment_paths(&app, &mut share).await;
    save(&mut tx, &share).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    drop(guard);
    for operation in old_operations {
        super::file_manager::cancel_attachment_operation(&operation).await;
    }
    cleanup(
        root,
        if terminal(&share) {
            Some(share.delivery_id.clone())
        } else {
            None
        },
    )
    .await;
    Ok(Some(share))
}

#[tauri::command]
pub async fn prepare_current_share(
    app: AppHandle,
    db: State<'_, DbState>,
    generation: u64,
    delivery_id: String,
) -> Result<CurrentShare, String> {
    let root = root(&app).await?;
    {
        let mut tx = db.write_transaction("share.prepare-status").await?;
        let guard = lock(&root).await?;
        assert_current(&root, generation, &delivery_id)?;
        let share = load(&mut tx)
            .await?
            .filter(|s| matches(s, generation, &delivery_id))
            .ok_or("分享已被替换")?;
        if terminal(&share) {
            return Ok(share);
        }
        tx.commit().await.map_err(|e| e.to_string())?;
        drop(guard);
    }
    let folder = root.join(&delivery_id);
    leaf(&delivery_id)?;
    let lease_path = folder.join("lease.lock");
    let lease = tokio::task::spawn_blocking(move || {
        let file = File::options()
            .read(true)
            .write(true)
            .open(lease_path)
            .map_err(|e| e.to_string())?;
        lock_file(&file).map_err(|e| e.to_string())?;
        Ok::<_, String>(file)
    })
    .await
    .map_err(|e| e.to_string())??;
    let path = folder.join("manifest.json");
    if tokio::fs::metadata(&path)
        .await
        .map_err(|e| e.to_string())?
        .len()
        > MAX_MANIFEST
    {
        return Err("分享清单超过限制".into());
    }
    let bytes = tokio::fs::read(&path).await.map_err(|e| e.to_string())?;
    let manifest_hash = hex::encode(Sha256::digest(&bytes));
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if manifest.text.len() > MAX_TEXT || manifest.items.len() > 16 {
        return Err("分享内容超过限制".into());
    }
    let total_bytes = manifest
        .items
        .iter()
        .try_fold(0_u64, |total, item| total.checked_add(item.size))
        .ok_or("分享大小溢出")?;
    if total_bytes > 512 * 1024 * 1024 {
        return Err("分享总大小超过 512 MiB".into());
    }
    let mut tx = db.write_transaction("share.prepare").await?;
    let guard = lock(&root).await?;
    let source = assert_current(&root, generation, &delivery_id)?;
    let mut share = load(&mut tx)
        .await?
        .filter(|s| matches(s, generation, &delivery_id))
        .ok_or("分享已被替换")?;
    if terminal(&share) {
        return Ok(share);
    }
    if source.state != "ready" {
        return Err("当前分享尚未准备完成".into());
    }
    if !share.manifest_hash.is_empty() && share.manifest_hash != manifest_hash {
        return Err("同一分享的内容摘要已变化".into());
    }
    if share.manifest_hash.is_empty() {
        share.text = manifest.text;
        share.items = manifest.items;
        share.manifest_hash = manifest_hash;
    }
    save(&mut tx, &share).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    drop(guard);
    for index in 0..share.items.len() {
        if let Some(hash) = share.items[index]
            .attachment
            .as_ref()
            .and_then(|a| a.hash.as_deref())
        {
            if super::file_manager::resolve_attachment_cas_file(&app, &db.pool, hash)
                .await
                .is_ok()
            {
                continue;
            }
            share.items[index].attachment = None;
        }
        let item = &share.items[index];
        if item.sha256.is_empty() {
            continue;
        }
        let result = async {
            leaf(&item.item_id)?;
            let guard = lock(&root).await?;
            assert_current(&root, generation, &delivery_id)?;
            drop(guard);
            let source = tokio::fs::canonicalize(folder.join(&item.item_id))
                .await
                .map_err(|e| e.to_string())?;
            let parent = tokio::fs::canonicalize(&folder)
                .await
                .map_err(|e| e.to_string())?;
            if source.parent() != Some(parent.as_path()) {
                return Err("分享文件超出当前目录".into());
            }
            if tokio::fs::metadata(&source)
                .await
                .map_err(|e| e.to_string())?
                .len()
                != item.size
            {
                return Err("分享文件长度不匹配".into());
            }
            let imported = super::file_manager::import_shared_file(
                &app,
                &db.pool,
                &source,
                item.file_name.clone(),
                Some(item.mime_type.clone()),
                format!("share_{}_{}", delivery_id, item.item_id),
                Some(item.sha256.clone()),
            )
            .await?;
            if imported.hash != item.sha256 {
                return Err("分享文件摘要不匹配".into());
            }
            Ok(Attachment {
                r#type: imported.mime_type,
                src: imported.internal_path.clone(),
                internal_path: imported.internal_path,
                name: imported.name,
                size: imported.size,
                hash: Some(imported.hash),
                status: Some("done".into()),
                thumbnail_path: imported.thumbnail_path,
                extracted_text: imported.extracted_text,
                ..Default::default()
            })
        }
        .await;
        match result {
            Ok(attachment) => {
                share.items[index].attachment = Some(attachment);
                share.items[index].error = None;
            }
            Err(error) => share.items[index].error = Some(error),
        }
        let mut tx = db.write_transaction("share.import-item").await?;
        let guard = lock(&root).await?;
        assert_current(&root, generation, &delivery_id)?;
        let mut stored = load(&mut tx).await?.ok_or("分享记录丢失")?;
        if terminal(&stored) {
            return Ok(stored);
        }
        // Imports update only their item. A concurrent editor owns text, revision and removal.
        let item = share.items[index].clone();
        if stored.state == "draft" {
            if let Some(imported) = &item.attachment {
                for attachment in &mut stored.attachments {
                    if attachment.hash == imported.hash {
                        *attachment = imported.clone();
                    }
                }
            }
        }
        if let Some(target) = stored
            .items
            .iter_mut()
            .find(|target| target.item_id == item.item_id)
        {
            *target = item;
        }
        share = stored;
        save(&mut tx, &share).await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        drop(guard);
    }
    let mut tx = db.write_transaction("share.prepared").await?;
    let guard = lock(&root).await?;
    assert_current(&root, generation, &delivery_id)?;
    let stored = load(&mut tx).await?.ok_or("分享记录丢失")?;
    if terminal(&stored) {
        return Ok(stored);
    }
    share = stored;
    if share.state != "draft" {
        share.state = "prepared".into();
        share.attachments = share
            .items
            .iter()
            .filter_map(|item| item.attachment.clone())
            .collect();
    }
    save(&mut tx, &share).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    drop(guard);
    drop(lease);
    Ok(share)
}

#[tauri::command]
pub async fn read_android_share(app: AppHandle) -> Result<Option<serde_json::Value>, String> {
    #[cfg(target_os = "android")]
    {
        #[derive(Deserialize)]
        struct Pending {
            payload: Option<serde_json::Value>,
        }
        let pending: Pending = tokio::task::spawn_blocking(move || {
            app.state::<tauri_plugin_vcp_mobile::VcpMobileState<tauri::Wry>>()
                .mobile_plugin_handle()?
                .run_mobile_plugin("readShareInboxPayload", serde_json::json!({}))
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())??;
        Ok(pending.payload)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Err("Android 分享快照仅在 Android 中可用".into())
    }
}

async fn acknowledge_android_payload(
    app: &AppHandle,
    delivery_id: &str,
    payload_revision: u64,
    settled: bool,
) {
    #[cfg(target_os = "android")]
    {
        let app = app.clone();
        let delivery_id = delivery_id.to_owned();
        let result = tokio::task::spawn_blocking(move || {
            app.state::<tauri_plugin_vcp_mobile::VcpMobileState<tauri::Wry>>()
                .mobile_plugin_handle()?
                .run_mobile_plugin::<serde_json::Value>("acknowledgeShareInboxPayload", serde_json::json!({
                    "ownerId": delivery_id, "payloadRevision": payload_revision, "settled": settled,
                }))
                .map_err(|error| error.to_string())
        })
        .await;
        // The durable record already owns the result. Retaining a pending native
        // snapshot on ACK failure is safe: the next pull sees the committed phase.
        if !matches!(result, Ok(Ok(_))) {
            log::warn!("Android share acknowledgement failed: {result:?}");
        }
    }
    #[cfg(not(target_os = "android"))]
    let _ = (app, delivery_id, payload_revision, settled);
}

#[tauri::command]
pub async fn accept_android_share(
    app: AppHandle,
    db: State<'_, DbState>,
    delivery_id: String,
    text: String,
    error: Option<String>,
    payload_revision: u64,
    payload_state: String,
) -> Result<CurrentShare, String> {
    leaf(&delivery_id)?;
    if payload_revision == 0 || !["receiving", "ready", "failed"].contains(&payload_state.as_str())
    {
        return Err("分享快照身份或状态无效".into());
    }
    let acknowledge = payload_state != "ready";
    let settled = payload_state == "failed";
    if text.len() > MAX_TEXT {
        return Err("分享文字超过 1 MiB".into());
    }
    let root = root(&app).await?;
    let mut tx = db.write_transaction("share.android.accept").await?;
    let guard = lock(&root).await?;
    let old = slot(&root)?;
    let old_record = load(&mut tx).await?;
    if old.as_ref().is_some_and(|s| s.delivery_id == delivery_id) {
        if let Some(mut current) = load(&mut tx).await? {
            if current.delivery_id == delivery_id {
                if let Some(error) = error {
                    if !terminal(&current) {
                        current.state = "failed".into();
                        current.error = Some(error);
                        let source = Slot {
                            generation: current.generation,
                            delivery_id: current.delivery_id.clone(),
                            state: "failed".into(),
                            error: current.error.clone(),
                        };
                        write_slot(&root, &source)?;
                        save(&mut tx, &current).await?;
                    }
                }
                tx.commit().await.map_err(|e| e.to_string())?;
                drop(guard);
                if acknowledge {
                    acknowledge_android_payload(&app, &delivery_id, payload_revision, settled)
                        .await;
                }
                return Ok(current);
            }
        }
    }
    let slot = Slot {
        generation: old.map_or(1, |s| s.generation + 1),
        delivery_id: delivery_id.clone(),
        state: if error.is_some() {
            "failed"
        } else {
            "receiving"
        }
        .into(),
        error,
    };
    write_slot(&root, &slot)?;
    let mut share = from_slot(&slot);
    share.text = text;
    save(&mut tx, &share).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    drop(guard);
    if let Some(previous) = old_record {
        for item in previous.items {
            super::file_manager::cancel_attachment_operation(&format!(
                "share_{}_{}",
                previous.delivery_id, item.item_id
            ))
            .await;
        }
    }
    if acknowledge {
        acknowledge_android_payload(&app, &delivery_id, payload_revision, settled).await;
    }
    Ok(share)
}

#[tauri::command]
pub async fn finish_android_share(
    app: AppHandle,
    db: State<'_, DbState>,
    generation: u64,
    delivery_id: String,
    text: String,
    files: Vec<tauri_plugin_vcp_mobile::system::SharedFileItem>,
    payload_revision: u64,
) -> Result<CurrentShare, String> {
    let result = finish_android_share_inner(
        app.clone(),
        &db,
        generation,
        delivery_id.clone(),
        text,
        files,
    )
    .await;
    if result
        .as_ref()
        .is_ok_and(|share| share.state != "receiving")
    {
        acknowledge_android_payload(&app, &delivery_id, payload_revision, true).await;
    } else if let Err(error) = &result {
        // Native acquisition may still remember this owner. Persist the actual failed handoff,
        // so a copy/disk/DB error cannot remain a permanently invisible receiving slot.
        if let Ok(root) = root(&app).await {
            if mark_android_share_failed(&root, generation, &delivery_id, error).await {
                acknowledge_android_payload(&app, &delivery_id, payload_revision, true).await;
            }
        }
    }
    result
}

async fn mark_android_share_failed(
    root: &Path,
    generation: u64,
    delivery_id: &str,
    error: &str,
) -> bool {
    let Ok(_guard) = lock(root).await else {
        return false;
    };
    let Ok(mut current) = assert_current(root, generation, delivery_id) else {
        return false;
    };
    // Ready files remain recoverable even if the subsequent SQLite commit failed.
    if current.state == "receiving" {
        current.state = "failed".into();
        current.error = Some(error.to_owned());
        if let Err(failure) = write_slot(root, &current) {
            log::warn!("Cannot persist share acquisition failure: {failure}");
            return false;
        }
    }
    current.state == "failed"
}

async fn finish_android_share_inner(
    app: AppHandle,
    db: &DbState,
    generation: u64,
    delivery_id: String,
    text: String,
    files: Vec<tauri_plugin_vcp_mobile::system::SharedFileItem>,
) -> Result<CurrentShare, String> {
    if files.len() > 16 || text.len() > MAX_TEXT {
        return Err("分享内容超过限制".into());
    }
    let root = root(&app).await?;
    let folder = root.join(&delivery_id);
    leaf(&delivery_id)?;
    let mut tx = db.write_transaction("share.android.acquire").await?;
    let guard = lock(&root).await?;
    let source = assert_current(&root, generation, &delivery_id)?;
    let mut existing = load(&mut tx).await?.ok_or("分享记录丢失")?;
    if source.state != "receiving" && existing.state == "receiving" {
        // Publication succeeded but the previous call could not commit SQLite.
        // Reconcile without claiming the already consumed native file tickets.
        existing.state = source.state;
        existing.error = source.error;
        save(&mut tx, &existing).await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        return Ok(existing);
    }
    if existing.state != "receiving" {
        return Ok(existing);
    }
    tokio::fs::create_dir_all(&folder)
        .await
        .map_err(|e| e.to_string())?;
    let lease = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(folder.join("lease.lock"))
        .map_err(|e| e.to_string())?;
    match try_lock_file(&lease) {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            // A reloaded WebView may request the same snapshot while its old
            // command still owns the copy. Wait without holding the DB/slot locks.
            tx.commit().await.map_err(|error| error.to_string())?;
            drop(guard);
            let _lease = tokio::task::spawn_blocking(move || {
                lock_file(&lease).map_err(|error| error.to_string())?;
                Ok::<_, String>(lease)
            })
            .await
            .map_err(|error| error.to_string())??;
            let mut tx = db.write_transaction("share.android.join").await?;
            let _guard = lock(&root).await?;
            let source = assert_current(&root, generation, &delivery_id)?;
            let mut share = load(&mut tx).await?.ok_or("分享记录丢失")?;
            if share.state == "receiving" {
                share.state = source.state;
                share.error = source.error;
                save(&mut tx, &share).await?;
            }
            tx.commit().await.map_err(|error| error.to_string())?;
            return Ok(share);
        }
        Err(error) => return Err(error.to_string()),
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    drop(guard);
    let mut created = Vec::<PathBuf>::new();
    let mut published = false;
    let result = async {
        let mut items = vec![];
        let mut total_bytes = 0_u64;
        for file in files {
            let name = file.file_name.clone();
            let id = uuid::Uuid::new_v4().to_string();
            let app = app.clone();
            let owner = delivery_id.clone();
            let registered = tokio::task::spawn_blocking(move || {
                tauri_plugin_vcp_mobile::system::register_shared_files(app, owner, vec![file])
            })
            .await
            .map_err(|e| e.to_string())?;
            match registered {
                Ok(mut result) if !result.is_empty() => {
                    let file = result.remove(0);
                    let target = folder.join(&id);
                    created.push(target.clone());
                    copy_android_share_file(
                        Path::new(&file.path),
                        &target,
                        &mut total_bytes,
                        file.size,
                    )
                    .await?;
                    items.push(ShareItem {
                        item_id: id,
                        file_name: file.name,
                        mime_type: file.mime,
                        size: file.size,
                        sha256: file.hash,
                        error: None,
                        attachment: None,
                    });
                }
                result => items.push(ShareItem {
                    item_id: id,
                    file_name: name,
                    mime_type: String::new(),
                    size: 0,
                    sha256: String::new(),
                    error: Some(result.err().unwrap_or_else(|| "提供方未返回文件".into())),
                    attachment: None,
                }),
            }
        }
        let manifest = serde_json::json!({"text": text, "items": items});
        created.push(folder.join("manifest.json"));
        tokio::fs::write(
            folder.join("manifest.json"),
            serde_json::to_vec(&manifest).map_err(|e| e.to_string())?,
        )
        .await
        .map_err(|e| e.to_string())?;
        let mut tx = db.write_transaction("share.android.ready").await?;
        let guard = lock(&root).await?;
        let mut source = assert_current(&root, generation, &delivery_id)?;
        let mut share = load(&mut tx).await?.ok_or("分享记录丢失")?;
        if terminal(&share) {
            return Ok(share);
        }
        source.state = "ready".into();
        write_slot(&root, &source)?;
        published = true;
        share.state = "ready".into();
        save(&mut tx, &share).await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        drop(guard);
        Ok::<_, String>(share)
    }
    .await;
    // A published manifest remains recoverable if SQLite failed afterwards.
    // All other paths belong exclusively to this lease, including partial copies.
    if !published {
        for path in created {
            remove_handoff_file(&path).await;
        }
    }
    if let Err(error) = &result {
        mark_android_share_failed(&root, generation, &delivery_id, error).await;
    }
    drop(lease);
    result
}

async fn copy_android_share_file(
    source: &Path,
    target: &Path,
    total_bytes: &mut u64,
    size: u64,
) -> Result<(), String> {
    // Once native registration returns, Rust owns this upload on every exit,
    // including a size rejection before the destination is opened.
    let result = async {
        *total_bytes = total_bytes.checked_add(size).ok_or("分享文件大小溢出")?;
        if *total_bytes > 512 * 1024 * 1024 {
            return Err("分享超过 512 MiB".into());
        }
        tokio::fs::copy(source, target)
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }
    .await;
    remove_handoff_file(source).await;
    if result.is_err() {
        remove_handoff_file(target).await;
    }
    result
}

async fn remove_handoff_file(path: &Path) {
    if let Err(error) = tokio::fs::remove_file(path).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            log::warn!(
                "Cannot remove share handoff file {}: {error}",
                path.display()
            );
        }
    }
}

#[tauri::command]
pub async fn bind_current_share(
    app: AppHandle,
    db: State<'_, DbState>,
    generation: u64,
    delivery_id: String,
    agent_id: String,
    allow_partial: bool,
) -> Result<CurrentShare, String> {
    let root = root(&app).await?;
    let mut tx = db.write_transaction("share.bind").await?;
    let guard = lock(&root).await?;
    assert_current(&root, generation, &delivery_id)?;
    let mut share = load(&mut tx).await?.ok_or("分享记录丢失")?;
    if !matches(&share, generation, &delivery_id) || terminal(&share) {
        return Err("当前分享已结束或被替换".into());
    }
    if share.state == "draft" {
        let available: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM topics t JOIN agents a ON a.agent_id=t.owner_id AND a.owner_type=t.owner_type WHERE t.owner_type='agent' AND t.owner_id=? AND t.topic_id=? AND t.deleted_at IS NULL AND a.deleted_at IS NULL")
            .bind(&share.agent_id).bind(&share.topic_id).fetch_one(&mut *tx).await.map_err(|e| e.to_string())?;
        if available > 0 {
            return Ok(share);
        }
        share.state = "prepared".into();
        share.agent_id = None;
        share.topic_id = None;
    }
    if share.state != "prepared" {
        return Err("分享内容尚未完成导入".into());
    }
    if !allow_partial && share.items.iter().any(|i| i.error.is_some()) {
        return Err("部分附件失败，请明确选择重试或仅继续成功内容".into());
    }
    if share.text.trim().is_empty() && share.attachments.is_empty() {
        return Err("本次分享没有可继续的文字或附件，请从来源应用重新分享".into());
    }
    let exists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agents WHERE agent_id=? AND deleted_at IS NULL")
            .bind(&agent_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
    if exists == 0 {
        return Err("所选助手已不可用，请重新选择".into());
    }
    let topic = format!("topic_share_{}_{}", delivery_id, uuid::Uuid::new_v4());
    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query("INSERT INTO topics(topic_id, owner_id, owner_type, title, created_at, updated_at, msg_count, locked, unread, unread_count) VALUES(?,?,'agent',?,?,?,0,1,0,0)")
        .bind(&topic).bind(&agent_id).bind(format!("分享 {}", chrono::Local::now().format("%H:%M:%S"))).bind(now).bind(now)
        .execute(&mut *tx).await.map_err(|e| e.to_string())?;
    HashAggregator::bubble_from_topic(&mut tx, &TopicKey::new("agent", &agent_id, &topic)).await?;
    share.agent_id = Some(agent_id);
    share.topic_id = Some(topic);
    share.state = "draft".into();
    share.revision += 1;
    save(&mut tx, &share).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    drop(guard);
    Ok(share)
}

#[tauri::command]
pub async fn update_share_draft(
    app: AppHandle,
    db: State<'_, DbState>,
    generation: u64,
    delivery_id: String,
    revision: u64,
    text: String,
    attachments: Vec<Attachment>,
) -> Result<CurrentShare, String> {
    if text.len() > MAX_TEXT {
        return Err("分享文字超过 1 MiB".into());
    }
    let root = root(&app).await?;
    let mut tx = db.write_transaction("share.draft").await?;
    let guard = lock(&root).await?;
    assert_current(&root, generation, &delivery_id)?;
    let mut share = load(&mut tx).await?.ok_or("分享记录丢失")?;
    if !matches(&share, generation, &delivery_id)
        || share.state != "draft"
        || share.revision != revision
    {
        return Err("分享草稿版本已变化".into());
    }
    share.text = text;
    share.attachments = attachments;
    share.revision += 1;
    save(&mut tx, &share).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    drop(guard);
    Ok(share)
}

#[tauri::command]
pub async fn discard_current_share(
    app: AppHandle,
    db: State<'_, DbState>,
    generation: u64,
    delivery_id: String,
) -> Result<(), String> {
    let root = root(&app).await?;
    let mut tx = db.write_transaction("share.discard").await?;
    let guard = lock(&root).await?;
    assert_current(&root, generation, &delivery_id)?;
    let mut share = load(&mut tx).await?.ok_or("分享记录丢失")?;
    if share.state != "sent" {
        share.state = "discarded".into();
        share.text.clear();
        share.attachments.clear();
        share.items.clear();
        save(&mut tx, &share).await?;
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    drop(guard);
    cleanup(root, Some(delivery_id)).await;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareCommit {
    pub message: ChatMessage,
    pub persisted: MessageWriteResultDto,
    pub already_sent: bool,
}

async fn store_message(
    db: &mut sqlx::SqliteConnection,
    share: &mut CurrentShare,
    message: &mut ChatMessage,
    render: &[u8],
) -> Result<i64, String> {
    let key = TopicKey::new(
        "agent",
        share.agent_id.as_deref().ok_or("分享助手缺失")?,
        share.topic_id.as_deref().ok_or("分享话题缺失")?,
    );
    message.id = format!("msg_share_{}", share.delivery_id);
    message.role = "user".into();
    let activity = MessageRepository::upsert_message(db, message, &key, render, false).await?;
    let activity = match activity {
        Some(value) => value,
        None => HashAggregator::load_topic_activity(db, &key).await?,
    };
    share.state = "sent".into();
    share.message = Some(message.clone());
    share.text.clear();
    share.items.clear();
    share.attachments.clear();
    save(db, share).await?;
    Ok(activity.updated_at)
}

#[tauri::command]
pub async fn commit_share_message(
    app: AppHandle,
    db: State<'_, DbState>,
    generation: u64,
    delivery_id: String,
    revision: u64,
    mut message: ChatMessage,
) -> Result<ShareCommit, String> {
    if message.content.len() > MAX_TEXT {
        return Err("分享文字超过 1 MiB".into());
    }
    let root = root(&app).await?;
    // Read a sent receipt before touching the retried payload or already-cleaned files.
    {
        let mut tx = db.write_transaction("share.send-status").await?;
        let guard = lock(&root).await?;
        assert_current(&root, generation, &delivery_id)?;
        let share = load(&mut tx)
            .await?
            .filter(|s| matches(s, generation, &delivery_id))
            .ok_or("分享已被替换")?;
        if let Some(saved) = share.message {
            let key = TopicKey::new(
                "agent",
                share.agent_id.as_deref().ok_or("分享助手缺失")?,
                share.topic_id.as_deref().ok_or("分享话题缺失")?,
            );
            let activity = HashAggregator::load_topic_activity(&mut tx, &key).await?;
            tx.commit().await.map_err(|e| e.to_string())?;
            drop(guard);
            let (blocks, _) = compile_and_serialize_render_async(saved.content.clone()).await?;
            return Ok(ShareCommit {
                message: saved,
                persisted: MessageWriteResultDto {
                    blocks,
                    topic_updated_at: activity.updated_at,
                },
                already_sent: true,
            });
        }
        tx.commit().await.map_err(|e| e.to_string())?;
        drop(guard);
    }
    normalize_attachments_from_local_cas(&app, &db.pool, &mut message).await?;
    let (blocks, render) = compile_and_serialize_render_async(message.content.clone()).await?;
    let mut tx = db.write_transaction("share.send").await?;
    let guard = lock(&root).await?;
    assert_current(&root, generation, &delivery_id)?;
    let mut share = load(&mut tx).await?.ok_or("分享记录丢失")?;
    if !matches(&share, generation, &delivery_id) {
        return Err("分享已被替换".into());
    }
    let agent = share.agent_id.as_deref().ok_or("分享尚未绑定助手")?;
    let topic = share.topic_id.as_deref().ok_or("分享尚未绑定话题")?;
    let key = TopicKey::new("agent", agent, topic);
    if let Some(saved) = share.message {
        let activity = HashAggregator::load_topic_activity(&mut tx, &key).await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        drop(guard);
        let (blocks, _) = compile_and_serialize_render_async(saved.content.clone()).await?;
        return Ok(ShareCommit {
            message: saved,
            persisted: MessageWriteResultDto {
                blocks,
                topic_updated_at: activity.updated_at,
            },
            already_sent: true,
        });
    }
    if share.state != "draft" || share.revision != revision {
        return Err("分享草稿版本已变化，请重试".into());
    }
    let topic_updated_at = store_message(&mut tx, &mut share, &mut message, &render).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    drop(guard);
    cleanup(root, Some(delivery_id)).await;
    Ok(ShareCommit {
        message,
        persisted: MessageWriteResultDto {
            blocks,
            topic_updated_at,
        },
        already_sent: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Connection;

    #[tokio::test]
    async fn android_copy_consumes_upload_on_success_and_budget_rejection() {
        let temporary = tempfile::tempdir_in("tests").unwrap();
        let source = temporary.path().join("upload");
        let target = temporary.path().join("item");
        tokio::fs::write(&source, b"complete").await.unwrap();
        let mut total = 0;
        copy_android_share_file(&source, &target, &mut total, 8)
            .await
            .unwrap();
        assert_eq!(tokio::fs::read(&target).await.unwrap(), b"complete");
        assert!(!source.exists());
        assert_eq!(total, 8);

        tokio::fs::write(&source, b"over budget").await.unwrap();
        let rejected = temporary.path().join("rejected-item");
        total = 512 * 1024 * 1024;
        assert!(copy_android_share_file(&source, &rejected, &mut total, 1)
            .await
            .is_err());
        assert!(!source.exists());
        assert!(!rejected.exists());
        assert_eq!(tokio::fs::read(&target).await.unwrap(), b"complete");
    }

    #[tokio::test]
    async fn android_copy_failure_removes_upload_and_owned_partial_target() {
        let temporary = tempfile::tempdir_in("tests").unwrap();
        let source = temporary.path().join("upload");
        tokio::fs::write(&source, b"original").await.unwrap();
        let mut total = 0;
        // Opening the destination fails, as it does when storage cannot accept a copy.
        let unavailable = temporary.path().join("missing-parent/item");
        assert!(
            copy_android_share_file(&source, &unavailable, &mut total, 8)
                .await
                .is_err()
        );
        assert!(!source.exists());
        let partial = temporary.path().join("partial-item");
        tokio::fs::write(&partial, b"partial").await.unwrap();
        assert!(copy_android_share_file(&source, &partial, &mut total, 8)
            .await
            .is_err());
        assert!(!partial.exists());
    }

    #[tokio::test]
    async fn late_android_failure_cannot_revoke_published_or_replaced_files() {
        let temporary = tempfile::tempdir_in("tests").unwrap();
        let root = temporary.path();
        write_slot(root, &test_slot(1, "published")).unwrap();
        assert!(!mark_android_share_failed(root, 1, "published", "late database error").await);
        assert_eq!(slot(root).unwrap().unwrap().state, "ready");
        write_slot(root, &test_slot(2, "newer")).unwrap();
        assert!(!mark_android_share_failed(root, 1, "published", "late failure").await);
        assert_eq!(slot(root).unwrap().unwrap().delivery_id, "newer");
    }
    fn test_slot(generation: u64, id: &str) -> Slot {
        Slot {
            generation,
            delivery_id: id.into(),
            state: "ready".into(),
            error: None,
        }
    }
    fn directory(root: &Path, id: &str) -> File {
        let folder = root.join(id);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("payload"), b"owned bytes").unwrap();
        File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(folder.join("lease.lock"))
            .unwrap()
    }

    #[test]
    fn independent_share_lock_handles_exclude_until_the_owner_drops() {
        let temporary = tempfile::tempdir_in("tests").unwrap();
        let owner = directory(temporary.path(), "lease");
        let path = temporary.path().join("lease/lease.lock");
        let contender = File::options().read(true).write(true).open(&path).unwrap();
        lock_file(&owner).unwrap();
        assert!(matches!(
            try_lock_file(&contender),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(owner);
        try_lock_file(&contender).unwrap();
        let next = File::options().read(true).write(true).open(path).unwrap();
        assert!(matches!(
            try_lock_file(&next),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(contender);
        try_lock_file(&next).unwrap();
    }

    #[test]
    fn blocking_share_lock_waits_and_error_exit_releases_the_lease() {
        use std::sync::mpsc;
        use std::time::Duration;

        let temporary = tempfile::tempdir_in("tests").unwrap();
        let owner = directory(temporary.path(), "lease");
        let path = temporary.path().join("lease/lease.lock");
        lock_file(&owner).unwrap();
        let (waiting_tx, waiting_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let waiting_path = path.clone();
        let waiter = std::thread::spawn(move || {
            let result = (|| -> Result<(), &'static str> {
                let lease = File::options()
                    .read(true)
                    .write(true)
                    .open(waiting_path)
                    .unwrap();
                assert!(matches!(
                    try_lock_file(&lease),
                    Err(std::fs::TryLockError::WouldBlock)
                ));
                waiting_tx.send(()).unwrap();
                lock_file(&lease).unwrap();
                acquired_tx.send(()).unwrap();
                Err("import failed after acquiring its lease")
            })();
            assert!(result.is_err());
        });
        waiting_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            acquired_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        drop(owner);
        acquired_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        waiter.join().unwrap();
        let next = File::options().read(true).write(true).open(path).unwrap();
        try_lock_file(&next).unwrap();
    }

    #[tokio::test]
    async fn delayed_cleanup_cannot_delete_a_new_share_or_a_live_reader() {
        let temporary = tempfile::tempdir_in("tests").unwrap();
        let root = temporary.path();
        let slot_guard = lock(root).await.unwrap();
        write_slot(root, &test_slot(1, "first")).unwrap();
        let first = directory(root, "first");
        lock_file(&first).unwrap();
        let second = directory(root, "second");
        drop(second);
        write_slot(root, &test_slot(2, "second")).unwrap();
        drop(slot_guard);
        // A's sent callback arrives after B already replaced the slot.
        cleanup(root.to_owned(), Some("first".into())).await;
        assert!(root.join("first/payload").exists());
        assert!(root.join("second/payload").exists());
        drop(first);
        cleanup(root.to_owned(), Some("first".into())).await;
        assert!(!root.join("first").exists());
        assert!(root.join("second/payload").exists());
        assert!(assert_current(root, 1, "first").is_err());
        assert_eq!(
            assert_current(root, 2, "second").unwrap().delivery_id,
            "second"
        );
    }

    #[tokio::test]
    async fn receiving_is_interrupted_only_after_its_provider_lease_disappears() {
        let temporary = tempfile::tempdir_in("tests").unwrap();
        let root = temporary.path();
        let mut source = test_slot(1, "receiving");
        source.state = "receiving".into();
        let lease = directory(root, "receiving");
        lock_file(&lease).unwrap();
        assert!(!acquisition_interrupted(root, &source, None));
        drop(lease);
        assert!(acquisition_interrupted(root, &source, None));
        // Android provider IO lives in the native process before the app-folder copy begins.
        let mut android = AndroidOwnerSnapshot {
            observed_slot: Some(source.clone()),
            owner_id: Some("receiving".into()),
        };
        assert!(!acquisition_interrupted(root, &source, Some(&android)));
        android.owner_id = None;
        assert!(acquisition_interrupted(root, &source, Some(&android)));
    }

    #[tokio::test]
    async fn stale_android_owner_cannot_fail_a_share_accepted_during_the_native_query() {
        let temporary = tempfile::tempdir_in("tests").unwrap();
        let root = temporary.path();
        let mut first = test_slot(1, "first");
        first.state = "receiving".into();
        let guard = lock(root).await.unwrap();
        write_slot(root, &first).unwrap();
        let observed_slot = slot(root).unwrap();
        drop(guard);

        // The old read is awaiting its native owner response. A new acceptance
        // can acquire the slot and publish B before that response is reconciled.
        let guard = lock(root).await.unwrap();
        let mut second = test_slot(2, "second");
        second.state = "receiving".into();
        write_slot(root, &second).unwrap();
        drop(guard);
        let stale = AndroidOwnerSnapshot {
            observed_slot,
            owner_id: Some("first".into()),
        };

        let guard = lock(root).await.unwrap();
        let current = slot(root).unwrap().unwrap();
        assert!(!root.join("second/lease.lock").exists());
        assert!(!acquisition_interrupted(root, &current, Some(&stale)));
        // The same protection applies when the old read started before any
        // share existed; absence is not owner evidence for a later acceptance.
        let before_first_share = AndroidOwnerSnapshot {
            observed_slot: None,
            owner_id: None,
        };
        assert!(!acquisition_interrupted(root, &current, Some(&before_first_share)));
        // A fresh observation of the same receiving slot can still detect a
        // crashed native provider, without requiring another lease or polling.
        let crashed = AndroidOwnerSnapshot {
            observed_slot: Some(current.clone()),
            owner_id: None,
        };
        assert!(acquisition_interrupted(root, &current, Some(&crashed)));
        drop(guard);

        // B's provider handoff remains able to publish its normal ready state.
        let guard = lock(root).await.unwrap();
        let mut ready = assert_current(root, 2, "second").unwrap();
        ready.state = "ready".into();
        write_slot(root, &ready).unwrap();
        assert_eq!(slot(root).unwrap().unwrap().state, "ready");
        drop(guard);
    }

    async fn database() -> sqlx::SqliteConnection {
        let mut db = sqlx::SqliteConnection::connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../../../migrations/0100_baseline_v2.sql"))
            .execute(&mut db)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../../../migrations/0101_current_share.sql"))
            .execute(&mut db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agents(owner_type,agent_id,name,model,updated_at) VALUES('agent','agent','Agent','model',1)").execute(&mut db).await.unwrap();
        sqlx::query("INSERT INTO topics(owner_type,owner_id,topic_id,title,created_at,updated_at) VALUES('agent','agent','topic','Share',1,1)").execute(&mut db).await.unwrap();
        db
    }

    #[tokio::test]
    async fn draft_survives_reopen_and_message_receipt_rolls_back_with_message() {
        let mut db = database().await;
        let mut share = from_slot(&test_slot(7, "delivery"));
        share.state = "draft".into();
        share.agent_id = Some("agent".into());
        share.topic_id = Some("topic".into());
        share.text = "edited draft".into();
        share.revision = 4;
        save(&mut db, &share).await.unwrap();
        let recovered = load(&mut db).await.unwrap().unwrap();
        assert_eq!(
            (recovered.text.as_str(), recovered.revision),
            ("edited draft", 4)
        );
        // Force the receipt write to fail after MessageRepository has written the user message.
        sqlx::query("CREATE TRIGGER fail_receipt BEFORE UPDATE ON current_share BEGIN SELECT RAISE(ABORT,'disk write failed'); END").execute(&mut db).await.unwrap();
        let mut message = ChatMessage {
            content: "final edit".into(),
            timestamp: 1234,
            ..Default::default()
        };
        let (_, render) = compile_and_serialize_render_async(message.content.clone())
            .await
            .unwrap();
        let mut tx = db.begin().await.unwrap();
        assert!(store_message(&mut tx, &mut share, &mut message, &render)
            .await
            .is_err());
        tx.rollback().await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
            .fetch_one(&mut db)
            .await
            .unwrap();
        assert_eq!(count, 0);
        assert_eq!(load(&mut db).await.unwrap().unwrap().state, "draft");
        sqlx::query("DROP TRIGGER fail_receipt")
            .execute(&mut db)
            .await
            .unwrap();
        let mut share = load(&mut db).await.unwrap().unwrap();
        let mut tx = db.begin().await.unwrap();
        store_message(&mut tx, &mut share, &mut message, &render)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let receipt = load(&mut db).await.unwrap().unwrap();
        assert_eq!(receipt.state, "sent");
        assert_eq!(receipt.message.unwrap().id, "msg_share_delivery");
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE msg_id='msg_share_delivery'")
                .fetch_one(&mut db)
                .await
                .unwrap();
        assert_eq!(count, 1);
    }
}
