//! The registry publishes an audio asset only after its complete file is durable.
//! Unknown files are never promoted or deleted merely because they are unselected.

use super::{bounded_read, sample_path, VoiceCloneModelDto, MAX_STORED_SAMPLE_BYTES};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    future::Future,
    io::ErrorKind,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use tokio::io::AsyncWriteExt;

const MAX_REGISTRY_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Deserialize, Serialize)]
pub(super) struct Registry {
    version: u32,
    pub samples: Vec<VoiceCloneModelDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pending_deletions: Vec<VoiceCloneModelDto>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            version: 1,
            samples: Vec::new(),
            pending_deletions: Vec::new(),
        }
    }
}

pub(super) fn audio_path(root: &Path, sample: &VoiceCloneModelDto) -> Result<PathBuf, String> {
    Ok(sample_path(&root.join("samples"), &sample.id)?.with_extension(sample.format.extension()))
}

pub(super) async fn load(root: &Path) -> Result<Registry, String> {
    let path = root.join("registry.json");
    match tokio::fs::metadata(&path).await {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Registry::default()),
        Err(error) => return Err(error.to_string()),
        Ok(_) => {}
    }
    let bytes = bounded_read(&path, MAX_REGISTRY_BYTES).await?;
    let registry: Registry = serde_json::from_slice(&bytes)
        .map_err(|_| "声音库索引损坏，已保留原文件，请先修复索引。".to_string())?;
    if registry.version != 1 {
        return Err("声音库版本不受支持，已保留原文件。".into());
    }
    let mut ids = HashSet::new();
    for sample in registry.samples.iter().chain(&registry.pending_deletions) {
        sample_path(root, &sample.id)?;
        if !ids.insert(&sample.id)
            || sample.file_bytes == 0
            || sample.file_bytes > MAX_STORED_SAMPLE_BYTES
            || sample.duration_ms == 0
            || sample.name.trim().is_empty()
            || sample.name.chars().count() > 80
            || sample.file_hash.len() != 64
            || !sample.file_hash.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("声音库索引包含无效记录，已保留原文件。".into());
        }
    }
    Ok(registry)
}

pub(super) async fn write_durable(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await
        .map_err(|e| e.to_string())?;
    file.write_all(bytes).await.map_err(|e| e.to_string())?;
    file.sync_all().await.map_err(|e| e.to_string())
}

/// Namespace persistence is separate from syncing an individual file's data.
/// Only the mobile targets claim power-loss durability; host builds exercise
/// publication/recovery logic without emulating a desktop product filesystem.
async fn sync_directory(directory: &Path) -> Result<(), String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let directory = directory.to_owned();
        tokio::task::spawn_blocking(move || std::fs::File::open(&directory)?.sync_all())
            .await
            .map_err(|_| "声音库目录同步任务异常结束。".to_string())?
            .map_err(|error| format!("声音库目录同步失败：{error}"))
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        if !tokio::fs::metadata(directory)
            .await
            .map_err(|e| e.to_string())?
            .is_dir()
        {
            return Err("声音库目录不存在。".into());
        }
        Ok(())
    }
}

/// Apple's fsync may leave data in a device cache. After the parent directory
/// has been synchronized, FULLFSYNC on the published regular file establishes
/// the device flush barrier; unsupported/error results are not downgraded.
async fn flush_published_file(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "ios")]
    {
        use std::os::fd::AsRawFd;
        let path = path.to_owned();
        tokio::task::spawn_blocking(move || {
            let file = std::fs::File::open(path)?;
            // SAFETY: the live File owns this valid descriptor for the call.
            if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) } == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        })
        .await
        .map_err(|_| "声音库设备同步任务异常结束。".to_string())?
        .map_err(|error: std::io::Error| format!("声音库设备同步失败：{error}"))
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = path;
        Ok(())
    }
}

async fn ensure_library_directory(root: &Path) -> Result<(), String> {
    tokio::fs::create_dir_all(root)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(parent) = root.parent() {
        sync_directory(parent).await?;
    }
    Ok(())
}

#[derive(Debug)]
enum CommitError {
    Unpublished(String),
    Published(String),
}

impl From<CommitError> for String {
    fn from(error: CommitError) -> Self {
        match error {
            CommitError::Unpublished(message) | CommitError::Published(message) => message,
        }
    }
}

async fn rename_registry(source: PathBuf, destination: PathBuf) -> Result<(), String> {
    tokio::fs::rename(source, destination)
        .await
        .map_err(|e| e.to_string())
}

async fn persist_registry(root: PathBuf, destination: PathBuf) -> Result<(), String> {
    sync_directory(&root).await?;
    flush_published_file(&destination).await
}

pub(super) async fn commit(root: &Path, registry: &Registry) -> Result<(), String> {
    commit_with_io(root, registry, rename_registry, persist_registry)
        .await
        .map_err(String::from)
}

// Keep the actual rename as the publication boundary. Per-call operations also
// let failure tests exercise the real file/registry sequence without global hooks.
async fn commit_with_io<R, RF, B, BF>(
    root: &Path,
    registry: &Registry,
    rename: R,
    barrier: B,
) -> Result<(), CommitError>
where
    R: FnOnce(PathBuf, PathBuf) -> RF,
    RF: Future<Output = Result<(), String>>,
    B: FnOnce(PathBuf, PathBuf) -> BF,
    BF: Future<Output = Result<(), String>>,
{
    let bytes =
        serde_json::to_vec(registry).map_err(|e| CommitError::Unpublished(e.to_string()))?;
    if bytes.len() as u64 > MAX_REGISTRY_BYTES {
        return Err(CommitError::Unpublished(
            "声音库索引已达到容量上限。".into(),
        ));
    }
    ensure_library_directory(root)
        .await
        .map_err(CommitError::Unpublished)?;
    let pending = root.join(format!(
        ".pending-registry-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let destination = root.join("registry.json");
    let result = async {
        write_durable(&pending, &bytes).await?;
        rename(pending.clone(), destination.clone()).await
    }
    .await;
    if let Err(error) = result {
        let _ = tokio::fs::remove_file(&pending).await;
        return Err(CommitError::Unpublished(error));
    }
    barrier(root.to_owned(), destination)
        .await
        .map_err(CommitError::Published)
}

pub(super) async fn publish(
    root: &Path,
    sample: VoiceCloneModelDto,
    bytes: &[u8],
) -> Result<VoiceCloneModelDto, String> {
    publish_with_io(root, sample, bytes, rename_registry, persist_registry).await
}

async fn publish_with_io<R, RF, B, BF>(
    root: &Path,
    sample: VoiceCloneModelDto,
    bytes: &[u8],
    rename: R,
    barrier: B,
) -> Result<VoiceCloneModelDto, String>
where
    R: FnOnce(PathBuf, PathBuf) -> RF,
    RF: Future<Output = Result<(), String>>,
    B: FnOnce(PathBuf, PathBuf) -> BF,
    BF: Future<Output = Result<(), String>>,
{
    let mut registry = load(root).await?;
    ensure_library_directory(root).await?;
    let directory = root.join("samples");
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|e| e.to_string())?;
    sync_directory(root).await?;
    let destination = audio_path(root, &sample)?;
    let pending = directory.join(format!(".pending-{}", sample.id));
    let audio_ready = async {
        write_durable(&pending, bytes).await?;
        tokio::fs::rename(&pending, &destination)
            .await
            .map_err(|e| e.to_string())?;
        sync_directory(&directory).await?;
        flush_published_file(&destination).await
    }
    .await;
    if let Err(error) = audio_ready {
        // These paths belong only to this uncommitted import. Registered assets
        // and files from other operations are never touched by rollback.
        let _ = tokio::fs::remove_file(&pending).await;
        let _ = tokio::fs::remove_file(&destination).await;
        let _ = sync_directory(&directory).await;
        return Err(error);
    }
    registry.samples.push(sample.clone());
    if let Err(error) = commit_with_io(root, &registry, rename, barrier).await {
        match error {
            CommitError::Unpublished(error) => {
                // The old registry cannot reference this import. Roll back only
                // the audio we own; never scan unknown or registered assets.
                let rollback = async {
                    match tokio::fs::remove_file(&destination).await {
                        Ok(()) => {}
                        Err(error) if error.kind() == ErrorKind::NotFound => {}
                        Err(error) => return Err(error.to_string()),
                    }
                    sync_directory(&directory).await
                }
                .await;
                return Err(match rollback {
                    Ok(()) => error,
                    Err(cleanup) => format!("{error}；本次导入音频回收失败：{cleanup}"),
                });
            }
            // Rename succeeded: a failed durability barrier must not remove
            // the complete audio already referenced by the visible registry.
            CommitError::Published(error) => return Err(error),
        }
    }
    Ok(sample)
}

pub(super) async fn finish_deletions(root: &Path) -> Result<(), String> {
    let mut registry = load(root).await?;
    if registry.pending_deletions.is_empty() {
        return Ok(());
    }
    // A preceding commit may have returned an error after rename but before
    // its barrier. Re-establish the tombstone's namespace durability before
    // removing any audio, including when resuming in the same process.
    sync_directory(root).await?;
    flush_published_file(&root.join("registry.json")).await?;
    for sample in &registry.pending_deletions {
        match tokio::fs::remove_file(audio_path(root, sample)?).await {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(format!("声音模型已从库中移除，但音频清理失败：{error}")),
        }
    }
    sync_directory(&root.join("samples")).await?;
    registry.pending_deletions.clear();
    commit(root, &registry).await
}

/// Publish the user's deletion before touching its audio. A durable tombstone
/// allows interrupted file cleanup to resume without guessing file ownership.
pub(super) async fn delete(root: &Path, id: &str) -> Result<bool, String> {
    let mut registry = load(root).await?;
    let Some(index) = registry.samples.iter().position(|sample| sample.id == id) else {
        finish_deletions(root).await?;
        return Ok(false);
    };
    let sample = registry.samples.remove(index);
    registry.pending_deletions.push(sample);
    commit(root, &registry).await?;
    finish_deletions(root).await?;
    Ok(true)
}

pub(super) async fn cleanup_pending(root: &Path) -> Result<(), String> {
    // A corrupt or unknown registry never authorizes maintenance.
    let registry = load(root).await?;
    let registered: HashSet<_> = registry
        .samples
        .iter()
        .map(|sample| sample.id.as_str())
        .collect();
    for (directory, prefix) in [
        (root.to_owned(), ".pending-registry-"),
        (root.join("samples"), ".pending-"),
    ] {
        let mut entries = match tokio::fs::read_dir(&directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        let mut removed = false;
        for _ in 0..256 {
            let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? else {
                break;
            };
            if !entry
                .file_type()
                .await
                .map_err(|e| e.to_string())?
                .is_file()
            {
                continue;
            }
            let name = entry.file_name();
            let Some(id) = name.to_str().and_then(|name| name.strip_prefix(prefix)) else {
                continue;
            };
            if registered.contains(id) || sample_path(root, id).is_err() {
                continue;
            }
            let modified = entry
                .metadata()
                .await
                .map_err(|e| e.to_string())?
                .modified()
                .map_err(|e| e.to_string())?;
            if SystemTime::now()
                .duration_since(modified)
                .is_ok_and(|age| age > Duration::from_secs(3600))
            {
                tokio::fs::remove_file(entry.path())
                    .await
                    .map_err(|e| e.to_string())?;
                removed = true;
            }
        }
        if removed {
            sync_directory(&directory).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::CloneSampleFormat;
    use super::*;

    fn sample() -> VoiceCloneModelDto {
        VoiceCloneModelDto {
            id: uuid::Uuid::new_v4().simple().to_string(),
            name: "测试声音".into(),
            file_name: "test.wav".into(),
            format: CloneSampleFormat::Wav,
            file_bytes: 4,
            duration_ms: 1,
            file_hash: "a".repeat(64),
            created_at: 1,
            updated_at: 1,
        }
    }

    async fn assert_no_pending(root: &Path) {
        for directory in [root.to_owned(), root.join("samples")] {
            let mut entries = tokio::fs::read_dir(directory).await.unwrap();
            while let Some(entry) = entries.next_entry().await.unwrap() {
                assert!(!entry.file_name().to_string_lossy().starts_with(".pending-"));
            }
        }
    }

    #[tokio::test]
    async fn index_capacity_failure_rolls_back_only_the_new_audio() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let root = temp.path().join("library");
        let kept = publish(&root, sample(), b"keep").await.unwrap();
        let mut registry = load(&root).await.unwrap();
        // Fixed-size valid metadata fills the index up to less than one entry
        // below the real limit; no lowered test-only capacity is involved.
        let entry_bytes = serde_json::to_vec(&sample()).unwrap().len() + 1;
        let available = MAX_REGISTRY_BYTES as usize - serde_json::to_vec(&registry).unwrap().len();
        registry
            .samples
            .extend((0..available / entry_bytes).map(|_| sample()));
        commit(&root, &registry).await.unwrap();
        let before = tokio::fs::read(root.join("registry.json")).await.unwrap();
        let unknown = root.join("samples/unknown");
        tokio::fs::write(&unknown, b"unrelated").await.unwrap();
        let imported = sample();
        let destination = audio_path(&root, &imported).unwrap();

        let error = publish(&root, imported, b"data").await.unwrap_err();

        assert!(error.contains("容量上限"), "{error}");
        assert!(!destination.exists());
        assert_eq!(
            tokio::fs::read(root.join("registry.json")).await.unwrap(),
            before
        );
        assert_eq!(
            tokio::fs::read(audio_path(&root, &kept).unwrap())
                .await
                .unwrap(),
            b"keep"
        );
        assert_eq!(tokio::fs::read(unknown).await.unwrap(), b"unrelated");
        assert_no_pending(&root).await;
    }

    #[tokio::test]
    async fn index_rename_failure_rolls_back_audio_and_staged_index() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let root = temp.path().join("library");
        let kept = publish(&root, sample(), b"keep").await.unwrap();
        let before = tokio::fs::read(root.join("registry.json")).await.unwrap();
        let imported = sample();
        let destination = audio_path(&root, &imported).unwrap();
        let observed_audio = destination.clone();

        let result = publish_with_io(
            &root,
            imported,
            b"data",
            move |source, destination| async move {
                assert_eq!(tokio::fs::read(observed_audio).await.unwrap(), b"data");
                assert!(source.is_file());
                // An unusable parent path produces a real filesystem rename failure.
                rename_registry(source, destination.join("missing/registry.json")).await
            },
            persist_registry,
        )
        .await;

        assert!(result.is_err());
        assert!(!destination.exists());
        assert_eq!(
            tokio::fs::read(root.join("registry.json")).await.unwrap(),
            before
        );
        assert_eq!(
            tokio::fs::read(audio_path(&root, &kept).unwrap())
                .await
                .unwrap(),
            b"keep"
        );
        assert_no_pending(&root).await;
    }

    #[tokio::test]
    async fn failed_barrier_after_index_rename_preserves_published_audio() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let root = temp.path().join("library");
        let kept = publish(&root, sample(), b"keep").await.unwrap();
        let imported = sample();
        let imported_id = imported.id.clone();
        let destination = audio_path(&root, &imported).unwrap();

        let result = publish_with_io(
            &root,
            imported,
            b"data",
            rename_registry,
            |root, destination| async move {
                // The failure occurs only after the real rename made both entries
                // visible. It must not trigger the unpublished-import rollback.
                assert_eq!(load(&root).await.unwrap().samples.len(), 2);
                assert!(destination.is_file());
                Err("injected registry durability failure".into())
            },
        )
        .await;

        assert_eq!(result.unwrap_err(), "injected registry durability failure");
        let reopened = load(&root).await.unwrap();
        assert!(reopened.samples.iter().any(|item| item.id == imported_id));
        assert_eq!(tokio::fs::read(destination).await.unwrap(), b"data");
        assert_eq!(
            tokio::fs::read(audio_path(&root, &kept).unwrap())
                .await
                .unwrap(),
            b"keep"
        );
        assert_no_pending(&root).await;
    }

    #[tokio::test]
    async fn interrupted_deletion_removes_only_the_explicitly_tombstoned_audio() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let root = temp.path().join("library");
        let deleted = publish(&root, sample(), b"data").await.unwrap();
        let kept = publish(&root, sample(), b"keep").await.unwrap();
        let mut registry = load(&root).await.unwrap();
        registry.samples.retain(|item| item.id != deleted.id);
        registry.pending_deletions.push(deleted.clone());
        commit(&root, &registry).await.unwrap();
        finish_deletions(&root).await.unwrap();
        assert!(!audio_path(&root, &deleted).unwrap().exists());
        assert_eq!(
            tokio::fs::read(audio_path(&root, &kept).unwrap())
                .await
                .unwrap(),
            b"keep"
        );
        assert_eq!(load(&root).await.unwrap().samples.len(), 1);
    }

    #[tokio::test]
    async fn pending_cleanup_respects_age_and_registered_assets() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let root = temp.path().join("library");
        let kept = publish(&root, sample(), b"keep").await.unwrap();
        let expired = root
            .join("samples")
            .join(format!(".pending-{}", uuid::Uuid::new_v4().simple()));
        let fresh = root
            .join("samples")
            .join(format!(".pending-{}", uuid::Uuid::new_v4().simple()));
        let unknown = root.join("samples/unknown");
        for path in [&expired, &fresh, &unknown] {
            tokio::fs::write(path, b"pending").await.unwrap();
        }
        let file = std::fs::File::options().write(true).open(&expired).unwrap();
        file.set_times(
            std::fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(7200)),
        )
        .unwrap();
        cleanup_pending(&root).await.unwrap();
        assert!(!expired.exists());
        assert!(fresh.exists());
        assert!(unknown.exists());
        assert!(audio_path(&root, &kept).unwrap().exists());
    }
}
