//! Complete TTS segments survive narration cancellation and process restarts.
//! The caller owns these futures through their filesystem/decode cleanup; cancel
//! through the token, rather than dropping a commit future or aborting its task.

use super::{
    audio_codec::{self, ContainerFormat, DecodeError, DecodedAudio, MAX_AUDIO_FILE_BYTES},
    network::CANCELLED,
};
use crate::vcp_modules::settings_manager::VoiceTtsMode;
use bytes::Bytes;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    future::Future,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Weak},
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    sync::{Mutex, MutexGuard},
};
use tokio_util::sync::CancellationToken;

const KEY_VERSION: &[u8] = b"vcp-tts-cache-v1";
const RENAME_RETRY_MS: [u64; 3] = [50, 100, 200];

/// Values come from the prepared request, including its normalized endpoint.
/// Transport mode/container are deliberately absent: both modes share audio.
#[derive(Clone, Debug)]
pub(super) struct TtsCacheIdentity {
    pub endpoint: String,
    pub mode: VoiceTtsMode,
    pub model: Option<String>,
    pub voice: Option<String>,
    pub clone_sample_id: Option<String>,
    pub guidance: Option<String>,
    pub text: String,
}

pub(super) fn segment_key(identity: &TtsCacheIdentity) -> String {
    fn field(hash: &mut Sha256, tag: u8, value: Option<&str>) {
        hash.update([tag]);
        if let Some(value) = value {
            hash.update([1]);
            hash.update((value.len() as u64).to_le_bytes());
            hash.update(value.as_bytes());
        } else {
            hash.update([0]);
        }
    }
    let mut hash = Sha256::new();
    hash.update(KEY_VERSION);
    field(&mut hash, 1, Some(&identity.endpoint));
    field(
        &mut hash,
        2,
        Some(match identity.mode {
            VoiceTtsMode::Builtin => "builtin",
            VoiceTtsMode::Clone => "clone",
        }),
    );
    field(&mut hash, 3, identity.model.as_deref());
    field(&mut hash, 4, identity.voice.as_deref());
    field(&mut hash, 5, identity.clone_sample_id.as_deref());
    field(
        &mut hash,
        6,
        identity
            .guidance
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty()),
    );
    // Whitespace in spoken text is part of the actual request, not metadata.
    field(&mut hash, 7, Some(&identity.text));
    hex::encode(hash.finalize())
}

#[derive(Default)]
struct RootState {
    recovered: Mutex<bool>,
    publications: Mutex<HashMap<String, Weak<Mutex<PublicationState>>>>,
}

#[derive(Default)]
struct PublicationState {
    // A lookup retains its token through decoding, so a later publication
    // cannot be mistaken for the old file even when sizes/timestamps match.
    generation: Arc<()>,
}

// Keep the once-per-process recovery flag even when no narration owns a cache.
// Only open waits for recovery; independent keys never share an I/O lock.
static ROOTS: LazyLock<Mutex<HashMap<PathBuf, Arc<RootState>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone)]
pub(super) struct TtsCache {
    root: PathBuf,
    state: Arc<RootState>,
}

impl TtsCache {
    pub(super) async fn open(root: PathBuf) -> Result<Self, String> {
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|error| format!("创建语音缓存目录失败：{error}"))?;
        let root = tokio::fs::canonicalize(root)
            .await
            .map_err(|error| format!("读取语音缓存目录失败：{error}"))?;
        let state = ROOTS
            .lock()
            .await
            .entry(root.clone())
            .or_insert_with(|| Arc::new(RootState::default()))
            .clone();
        {
            let mut recovered = state.recovered.lock().await;
            if !*recovered {
                recover_pending(&root).await?;
                *recovered = true;
            }
        }
        Ok(Self { root, state })
    }

    pub(super) async fn lookup(
        &self,
        key: &str,
        cancel: &CancellationToken,
    ) -> Result<Option<Vec<u8>>, String> {
        self.lookup_with_decode(key, cancel, |bytes, cancel| async move {
            audio_codec::decode(bytes, &cancel).await
        })
        .await
    }

    async fn publication(
        &self,
        key: &str,
        cancel: &CancellationToken,
    ) -> Result<Arc<Mutex<PublicationState>>, String> {
        let mut publications = lock_cancelled(&self.state.publications, cancel).await?;
        // Cache history stays on disk; only concurrent operations retain keys.
        publications.retain(|_, state| state.strong_count() != 0);
        let entry = publications.entry(key.to_owned()).or_default();
        if let Some(state) = entry.upgrade() {
            return Ok(state);
        }
        let state = Arc::new(Mutex::new(PublicationState::default()));
        *entry = Arc::downgrade(&state);
        Ok(state)
    }

    async fn lookup_with_decode<F, Fut>(
        &self,
        key: &str,
        cancel: &CancellationToken,
        mut decode: F,
    ) -> Result<Option<Vec<u8>>, String>
    where
        F: FnMut(Bytes, CancellationToken) -> Fut,
        Fut: Future<Output = Result<DecodedAudio, DecodeError>>,
    {
        validate_key(key)?;
        let publication = self.publication(key, cancel).await?;
        for format in [ContainerFormat::Wav, ContainerFormat::Mp3] {
            check_cancelled(cancel)?;
            let path = self.root.join(format!("{key}.{}", format.extension()));
            let (file, generation) = {
                let state = lock_cancelled(&publication, cancel).await?;
                let file = match tokio::fs::File::open(&path).await {
                    Ok(file) => file,
                    Err(error) if error.kind() == ErrorKind::NotFound => continue,
                    Err(error) => return Err(format!("读取语音缓存失败：{error}")),
                };
                (file, state.generation.clone())
            };
            let metadata = file
                .metadata()
                .await
                .map_err(|error| format!("读取语音缓存信息失败：{error}"))?;
            if !metadata.is_file() {
                return Err("语音缓存路径不是文件。".into());
            }
            if metadata.len() > MAX_AUDIO_FILE_BYTES as u64 {
                return Err("语音缓存音频超过大小限制。".into());
            }
            let mut bytes = Vec::new();
            if metadata.len() > 0 {
                // Bound the read too, in case an external writer changes size.
                file.take(MAX_AUDIO_FILE_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|error| format!("读取语音缓存失败：{error}"))?;
            } else {
                drop(file);
            }
            check_cancelled(cancel)?;
            if bytes.len() > MAX_AUDIO_FILE_BYTES {
                return Err("语音缓存音频超过大小限制。".into());
            }
            if !bytes.is_empty() {
                match decode(bytes.into(), cancel.clone()).await {
                    Ok(decoded) if decoded.format.extension() == format.extension() => {
                        check_cancelled(cancel)?;
                        return Ok(Some(decoded.pcm_24k_mono));
                    }
                    Ok(_) => {} // A supported container under the wrong suffix is invalid.
                    Err(error) if error.is_corrupt() => {}
                    Err(error) => return Err(error.to_string()),
                }
            }
            check_cancelled(cancel)?;
            let state = lock_cancelled(&publication, cancel).await?;
            // Decode is outside the lock. Delete only the generation that was
            // actually checked; a newer complete publication remains reusable.
            if Arc::ptr_eq(&generation, &state.generation) {
                check_cancelled(cancel)?;
                remove_file(&path).await?;
            }
        }
        check_cancelled(cancel)?;
        Ok(None)
    }

    pub(super) async fn commit(
        &self,
        key: &str,
        format: ContainerFormat,
        bytes: &[u8],
        cancel: &CancellationToken,
    ) -> Result<(), String> {
        self.commit_with_rename(key, format, bytes, cancel, |from, to| async move {
            tokio::fs::rename(from, to).await
        })
        .await
    }

    async fn commit_with_rename<F, Fut>(
        &self,
        key: &str,
        format: ContainerFormat,
        bytes: &[u8],
        cancel: &CancellationToken,
        mut rename: F,
    ) -> Result<(), String>
    where
        F: FnMut(PathBuf, PathBuf) -> Fut,
        Fut: Future<Output = Result<(), std::io::Error>>,
    {
        validate_key(key)?;
        check_cancelled(cancel)?;
        if bytes.is_empty() || bytes.len() > MAX_AUDIO_FILE_BYTES {
            return Err("语音缓存音频大小无效。".into());
        }
        let publication = self.publication(key, cancel).await?;
        let pending = self
            .root
            .join(format!(".pending-{}.tmp", uuid::Uuid::new_v4()));
        let target = self.root.join(format!("{key}.{}", format.extension()));

        // Await the real filesystem work. A cancelled future does not stop a
        // tokio filesystem worker; deleting its path before it finishes races.
        let written = tokio::fs::write(&pending, bytes).await;
        let result = async {
            check_cancelled(cancel)?;
            written.map_err(|error| format!("写入语音缓存失败：{error}"))?;
            let mut retry_delays = RENAME_RETRY_MS.into_iter();
            loop {
                check_cancelled(cancel)?;
                let renamed = {
                    let mut state = lock_cancelled(&publication, cancel).await?;
                    check_cancelled(cancel)?;
                    let renamed = rename(pending.clone(), target.clone()).await;
                    if renamed.is_ok() {
                        state.generation = Arc::new(());
                    }
                    renamed
                };
                match renamed {
                    // Successful rename is the publication point. Cancellation
                    // after it must preserve this complete, reusable audio file.
                    Ok(()) => return check_cancelled(cancel),
                    Err(error) => {
                        let Some(delay_ms) = retry_delays.next() else {
                            return Err(format!("发布语音缓存失败：{error}"));
                        };
                        tokio::select! {
                            biased;
                            _ = cancel.cancelled() => return Err(CANCELLED.into()),
                            _ = tokio::time::sleep(Duration::from_millis(delay_ms)) => {}
                        }
                    }
                }
            }
        }
        .await;
        // Only this invocation's unique staging file is removed. After a
        // successful rename it is absent; never delete the published target.
        if let Err(cleanup_error) = remove_file(&pending).await {
            return Err(match result {
                Err(error) => format!("{error}；{cleanup_error}"),
                Ok(()) => cleanup_error,
            });
        }
        result
    }
}

fn validate_key(key: &str) -> Result<(), String> {
    if key.len() == 64
        && key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err("语音缓存标识无效。".into())
    }
}

fn check_cancelled(cancel: &CancellationToken) -> Result<(), String> {
    if cancel.is_cancelled() {
        Err(CANCELLED.into())
    } else {
        Ok(())
    }
}

async fn lock_cancelled<'a, T>(
    state: &'a Mutex<T>,
    cancel: &CancellationToken,
) -> Result<MutexGuard<'a, T>, String> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(CANCELLED.into()),
        guard = state.lock() => Ok(guard),
    }
}

async fn remove_file(path: &Path) -> Result<(), String> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("清理语音缓存文件失败：{error}")),
    }
}

async fn recover_pending(root: &Path) -> Result<(), String> {
    let mut entries = tokio::fs::read_dir(root)
        .await
        .map_err(|error| format!("扫描语音缓存目录失败：{error}"))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| error.to_string())?
    {
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|name| name.strip_prefix(".pending-"))
            .and_then(|name| name.strip_suffix(".tmp"))
        else {
            continue;
        };
        if uuid::Uuid::parse_str(id).is_ok()
            && entry
                .file_type()
                .await
                .map_err(|error| error.to_string())?
                .is_file()
        {
            remove_file(&entry.path()).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn identity() -> TtsCacheIdentity {
        TtsCacheIdentity {
            endpoint: "https://voice.example/v1/chat/completions".into(),
            mode: VoiceTtsMode::Builtin,
            model: Some("tts-model".into()),
            voice: Some("voice-a".into()),
            clone_sample_id: None,
            guidance: None,
            text: "这是缓存的一句话。".into(),
        }
    }

    fn wav() -> Vec<u8> {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&40_u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&24_000_u32.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&4_u32.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes
    }

    async fn file_names(root: &Path) -> Vec<String> {
        let mut entries = tokio::fs::read_dir(root).await.unwrap();
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            names.push(entry.file_name().to_str().unwrap().to_owned());
        }
        names.sort();
        names
    }

    #[test]
    fn identity_preserves_request_text_and_distinguishes_each_synthesis_parameter() {
        let base = identity();
        let key = segment_key(&base);
        assert_eq!(key.len(), 64);
        assert!(validate_key(&key).is_ok());
        for guidance in ["", " \n\t "] {
            let mut changed = base.clone();
            changed.guidance = Some(guidance.into());
            assert_eq!(segment_key(&changed), key);
        }
        let variants = [
            TtsCacheIdentity {
                endpoint: "https://other.example/tts".into(),
                ..base.clone()
            },
            TtsCacheIdentity {
                mode: VoiceTtsMode::Clone,
                ..base.clone()
            },
            TtsCacheIdentity {
                model: None,
                ..base.clone()
            },
            TtsCacheIdentity {
                voice: None,
                ..base.clone()
            },
            TtsCacheIdentity {
                clone_sample_id: Some("immutable-reference-id".into()),
                ..base.clone()
            },
            TtsCacheIdentity {
                guidance: Some("轻声".into()),
                ..base.clone()
            },
            TtsCacheIdentity {
                text: format!(" {} ", base.text),
                ..base.clone()
            },
        ];
        for variant in variants {
            assert_ne!(segment_key(&variant), key);
        }
        let mut first = base.clone();
        first.model = Some("a|b".into());
        first.voice = Some("c".into());
        let mut second = base;
        second.model = Some("a".into());
        second.voice = Some("b|c".into());
        assert_ne!(segment_key(&first), segment_key(&second));
    }

    #[tokio::test]
    async fn commits_complete_audio_and_recovers_only_previous_process_pending_files() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let pending = temp
            .path()
            .join(format!(".pending-{}.tmp", uuid::Uuid::new_v4()));
        let key = segment_key(&identity());
        let formal = temp.path().join(format!("{key}.wav"));
        tokio::fs::write(&pending, b"interrupted write")
            .await
            .unwrap();
        tokio::fs::write(&formal, wav()).await.unwrap();
        tokio::fs::write(temp.path().join(".pending-not-a-uuid.tmp"), b"foreign")
            .await
            .unwrap();
        let cache = TtsCache::open(temp.path().to_owned()).await.unwrap();
        assert!(!pending.exists());
        assert!(formal.exists());
        // A later open must never sweep staging files owned by this process.
        tokio::fs::write(&pending, b"current writer").await.unwrap();
        let reopened = TtsCache::open(temp.path().join(".")).await.unwrap();
        assert!(Arc::ptr_eq(&cache.state, &reopened.state));
        assert!(pending.exists());
        assert!(temp.path().join(".pending-not-a-uuid.tmp").exists());
        assert_eq!(
            cache.lookup(&key, &CancellationToken::new()).await.unwrap(),
            Some(vec![0; 4])
        );

        let mut other = identity();
        other.text.push('！');
        let other_key = segment_key(&other);
        cache
            .commit(
                &other_key,
                ContainerFormat::Wav,
                &wav(),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            tokio::fs::read(temp.path().join(format!("{other_key}.wav")))
                .await
                .unwrap(),
            wav()
        );
    }

    #[tokio::test]
    async fn corrupt_wav_is_removed_and_valid_mp3_with_same_key_is_used() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let cache = TtsCache::open(temp.path().to_owned()).await.unwrap();
        let key = segment_key(&identity());
        let mp3 = tokio::task::spawn_blocking(|| {
            vcp_voice_mp3::encode_pcm16_mono(&vec![0; 16_000], || false).unwrap()
        })
        .await
        .unwrap();
        cache
            .commit(&key, ContainerFormat::Mp3, &mp3, &CancellationToken::new())
            .await
            .unwrap();
        let bad_wav = temp.path().join(format!("{key}.wav"));
        // The RIFF signature alone is insufficient: incomplete chunks fail decode.
        let mut truncated = wav();
        truncated.pop();
        tokio::fs::write(&bad_wav, truncated).await.unwrap();
        let pcm = cache
            .lookup(&key, &CancellationToken::new())
            .await
            .unwrap()
            .unwrap();
        assert!(!pcm.is_empty());
        assert_eq!(pcm.len() % 2, 0);
        assert!(!bad_wav.exists());
        assert_eq!(file_names(temp.path()).await, [format!("{key}.mp3")]);
    }

    #[tokio::test]
    async fn empty_files_are_evicted_but_oversized_files_are_preserved_without_loading_them() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let cache = TtsCache::open(temp.path().to_owned()).await.unwrap();
        let key = segment_key(&identity());
        tokio::fs::write(temp.path().join(format!("{key}.wav")), [])
            .await
            .unwrap();
        let oversized = tokio::fs::File::create(temp.path().join(format!("{key}.mp3")))
            .await
            .unwrap();
        oversized
            .set_len(MAX_AUDIO_FILE_BYTES as u64 + 1)
            .await
            .unwrap();
        drop(oversized);
        assert!(cache
            .lookup(&key, &CancellationToken::new())
            .await
            .unwrap_err()
            .contains("超过大小限制"));
        assert_eq!(file_names(temp.path()).await, [format!("{key}.mp3")]);
        assert_eq!(
            tokio::fs::metadata(temp.path().join(format!("{key}.mp3")))
                .await
                .unwrap()
                .len(),
            MAX_AUDIO_FILE_BYTES as u64 + 1
        );
    }

    #[tokio::test(start_paused = true)]
    async fn rename_retries_the_same_complete_pending_file_with_bounded_delays() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let cache = TtsCache::open(temp.path().to_owned()).await.unwrap();
        let key = segment_key(&identity());
        let calls = Arc::new(AtomicUsize::new(0));
        let times = Arc::new(std::sync::Mutex::new(Vec::new()));
        let paths = Arc::new(std::sync::Mutex::new(Vec::new()));
        cache
            .commit_with_rename(
                &key,
                ContainerFormat::Wav,
                &wav(),
                &CancellationToken::new(),
                {
                    let calls = calls.clone();
                    let times = times.clone();
                    let paths = paths.clone();
                    move |from, to| {
                        let attempt = calls.fetch_add(1, Ordering::SeqCst);
                        times.lock().unwrap().push(tokio::time::Instant::now());
                        paths.lock().unwrap().push(from.clone());
                        async move {
                            assert_eq!(tokio::fs::read(&from).await.unwrap(), wav());
                            if attempt < 3 {
                                Err(std::io::Error::new(ErrorKind::PermissionDenied, "busy"))
                            } else {
                                tokio::fs::rename(from, to).await
                            }
                        }
                    }
                },
            )
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        let observed: Vec<_> = times
            .lock()
            .unwrap()
            .windows(2)
            .map(|pair| pair[1].duration_since(pair[0]).as_millis())
            .collect();
        assert_eq!(observed, [50, 100, 200]);
        {
            let paths = paths.lock().unwrap();
            assert!(paths.iter().all(|path| path == &paths[0]));
        }
        assert_eq!(file_names(temp.path()).await, [format!("{key}.wav")]);
    }

    #[tokio::test(start_paused = true)]
    async fn rename_exhaustion_cleans_staging_without_touching_an_existing_cache() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let cache = TtsCache::open(temp.path().to_owned()).await.unwrap();
        let key = segment_key(&identity());
        let target = temp.path().join(format!("{key}.wav"));
        tokio::fs::write(&target, wav()).await.unwrap();
        let calls = AtomicUsize::new(0);
        let error = cache
            .commit_with_rename(
                &key,
                ContainerFormat::Wav,
                &wav(),
                &CancellationToken::new(),
                |_, _| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    std::future::ready(Err(std::io::Error::new(
                        ErrorKind::PermissionDenied,
                        "busy",
                    )))
                },
            )
            .await
            .unwrap_err();
        assert!(error.contains("发布语音缓存失败"));
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        assert_eq!(tokio::fs::read(target).await.unwrap(), wav());
        assert_eq!(file_names(temp.path()).await, [format!("{key}.wav")]);
    }

    #[tokio::test]
    async fn cancellation_during_retry_cleans_pending_and_never_retries_again() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let cache = TtsCache::open(temp.path().to_owned()).await.unwrap();
        let cancel = CancellationToken::new();
        let calls = AtomicUsize::new(0);
        let error = cache
            .commit_with_rename(
                &segment_key(&identity()),
                ContainerFormat::Wav,
                &wav(),
                &cancel,
                |_, _| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    cancel.cancel();
                    std::future::ready(Err(std::io::Error::new(
                        ErrorKind::PermissionDenied,
                        "busy",
                    )))
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error, CANCELLED);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(file_names(temp.path()).await.is_empty());
    }

    #[tokio::test]
    async fn cancellation_after_successful_publication_preserves_the_complete_file() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let cache = TtsCache::open(temp.path().to_owned()).await.unwrap();
        let key = segment_key(&identity());
        let cancel = CancellationToken::new();
        let error = cache
            .commit_with_rename(&key, ContainerFormat::Wav, &wav(), &cancel, |from, to| {
                let cancel = cancel.clone();
                async move {
                    tokio::fs::rename(from, to).await?;
                    cancel.cancel();
                    Ok(())
                }
            })
            .await
            .unwrap_err();
        assert_eq!(error, CANCELLED);
        assert_eq!(file_names(temp.path()).await, [format!("{key}.wav")]);
        assert_eq!(
            cache.lookup(&key, &CancellationToken::new()).await.unwrap(),
            Some(vec![0; 4])
        );
    }

    #[tokio::test]
    async fn cancellation_awaits_rename_without_blocking_another_key() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let cache = TtsCache::open(temp.path().to_owned()).await.unwrap();
        let cancel = CancellationToken::new();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let task = tokio::spawn({
            let cache = cache.clone();
            let cancel = cancel.clone();
            let entered = entered.clone();
            let release = release.clone();
            async move {
                cache
                    .commit_with_rename(
                        &segment_key(&identity()),
                        ContainerFormat::Wav,
                        &wav(),
                        &cancel,
                        move |from, to| {
                            let entered = entered.clone();
                            let release = release.clone();
                            async move {
                                entered.notify_one();
                                release.notified().await;
                                tokio::fs::rename(from, to).await
                            }
                        },
                    )
                    .await
            }
        });
        entered.notified().await;
        cancel.cancel();
        assert!(cache.state.recovered.try_lock().is_ok());
        assert!(!task.is_finished());
        let current_pending = file_names(temp.path()).await;
        assert_eq!(current_pending.len(), 1);
        assert!(current_pending[0].starts_with(".pending-"));
        let mut other = identity();
        other.text.push('！');
        let other_key = segment_key(&other);
        tokio::time::timeout(
            Duration::from_secs(1),
            cache.commit(
                &other_key,
                ContainerFormat::Wav,
                &wav(),
                &CancellationToken::new(),
            ),
        )
        .await
        .expect("another key must not wait for the cancelled rename")
        .unwrap();
        assert_eq!(
            cache
                .lookup(&other_key, &CancellationToken::new())
                .await
                .unwrap(),
            Some(vec![0; 4])
        );
        release.notify_one();
        assert_eq!(task.await.unwrap().unwrap_err(), CANCELLED);
        assert!(file_names(temp.path())
            .await
            .iter()
            .all(|name| !name.starts_with(".pending-")));
        let reopened = TtsCache::open(temp.path().to_owned()).await.unwrap();
        assert_eq!(
            reopened
                .lookup(&segment_key(&identity()), &CancellationToken::new())
                .await
                .unwrap(),
            Some(vec![0; 4])
        );
    }

    #[tokio::test]
    async fn cancellation_before_io_and_while_waiting_for_publication_preserves_files() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let cache = TtsCache::open(temp.path().to_owned()).await.unwrap();
        let key = segment_key(&identity());
        cache
            .commit(
                &key,
                ContainerFormat::Wav,
                &wav(),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let publication = cache
            .publication(&key, &CancellationToken::new())
            .await
            .unwrap();
        let guard = publication.lock().await;
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(cache.lookup(&key, &cancel).await.unwrap_err(), CANCELLED);
        assert_eq!(
            cache
                .commit(&key, ContainerFormat::Wav, &wav(), &cancel)
                .await
                .unwrap_err(),
            CANCELLED
        );
        assert_eq!(file_names(temp.path()).await, [format!("{key}.wav")]);
        let waiting_cancel = CancellationToken::new();
        let task = tokio::spawn({
            let cache = cache.clone();
            let key = key.clone();
            let waiting_cancel = waiting_cancel.clone();
            async move { cache.lookup(&key, &waiting_cancel).await }
        });
        tokio::task::yield_now().await;
        assert!(!task.is_finished());
        waiting_cancel.cancel();
        assert_eq!(task.await.unwrap().unwrap_err(), CANCELLED);
        drop(guard);
    }

    #[tokio::test]
    async fn slow_corrupt_decode_does_not_block_other_keys_or_delete_a_new_publication() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let cache = TtsCache::open(temp.path().to_owned()).await.unwrap();
        let key = segment_key(&identity());
        let target = temp.path().join(format!("{key}.wav"));
        let mut corrupted = wav();
        // Preserve file length: publication identity must not rely on size.
        corrupted[40..44].copy_from_slice(&6_u32.to_le_bytes());
        tokio::fs::write(&target, corrupted).await.unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let task = tokio::spawn({
            let cache = cache.clone();
            let key = key.clone();
            let entered = entered.clone();
            let release = release.clone();
            async move {
                cache
                    .lookup_with_decode(&key, &CancellationToken::new(), move |bytes, cancel| {
                        let entered = entered.clone();
                        let release = release.clone();
                        async move {
                            let result = audio_codec::decode(bytes, &cancel).await;
                            entered.notify_one();
                            release.notified().await;
                            assert!(result.as_ref().unwrap_err().is_corrupt());
                            result
                        }
                    })
                    .await
            }
        });
        entered.notified().await;

        let mut other = identity();
        other.text.push('！');
        let other_key = segment_key(&other);
        tokio::time::timeout(Duration::from_secs(1), async {
            cache
                .commit(
                    &other_key,
                    ContainerFormat::Wav,
                    &wav(),
                    &CancellationToken::new(),
                )
                .await
                .unwrap();
            assert_eq!(
                cache
                    .lookup(&other_key, &CancellationToken::new())
                    .await
                    .unwrap(),
                Some(vec![0; 4])
            );
            cache
                .commit(
                    &key,
                    ContainerFormat::Wav,
                    &wav(),
                    &CancellationToken::new(),
                )
                .await
                .unwrap();
        })
        .await
        .expect("decoding must hold neither a root nor a publication lock");

        release.notify_one();
        assert!(task.await.unwrap().unwrap().is_none());
        assert_eq!(tokio::fs::read(&target).await.unwrap(), wav());
        assert_eq!(
            cache.lookup(&key, &CancellationToken::new()).await.unwrap(),
            Some(vec![0; 4])
        );
    }

    #[tokio::test]
    async fn resource_worker_and_unsupported_decode_errors_preserve_cached_bytes() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let cache = TtsCache::open(temp.path().to_owned()).await.unwrap();
        let key = segment_key(&identity());
        let target = temp.path().join(format!("{key}.wav"));
        cache
            .commit(
                &key,
                ContainerFormat::Wav,
                &wav(),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        for error in [
            DecodeError::Resource("allocation failed".into()),
            DecodeError::Worker("worker stopped".into()),
            DecodeError::Unsupported("codec unavailable".into()),
            DecodeError::Cancelled,
        ] {
            let expected = error.to_string();
            let mut error = Some(error);
            assert_eq!(
                cache
                    .lookup_with_decode(&key, &CancellationToken::new(), |_, _| {
                        std::future::ready(Err(error.take().unwrap()))
                    })
                    .await
                    .unwrap_err(),
                expected
            );
            assert_eq!(tokio::fs::read(&target).await.unwrap(), wav());
        }
    }

    #[tokio::test]
    async fn publication_registry_tracks_in_flight_keys_instead_of_cache_history() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let cache = TtsCache::open(temp.path().to_owned()).await.unwrap();
        for index in 0..8 {
            let mut current = identity();
            current.text.push_str(&index.to_string());
            cache
                .commit(
                    &segment_key(&current),
                    ContainerFormat::Wav,
                    &wav(),
                    &CancellationToken::new(),
                )
                .await
                .unwrap();
            assert_eq!(cache.state.publications.lock().await.len(), 1);
        }
        assert_eq!(file_names(temp.path()).await.len(), 8);
    }

    #[tokio::test]
    async fn invalid_keys_never_escape_the_cache_root() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let cache = TtsCache::open(temp.path().to_owned()).await.unwrap();
        for key in ["../outside", "", &"A".repeat(64)] {
            assert!(cache
                .commit(key, ContainerFormat::Wav, &wav(), &CancellationToken::new())
                .await
                .is_err());
            assert!(cache.lookup(key, &CancellationToken::new()).await.is_err());
        }
        assert!(file_names(temp.path()).await.is_empty());
    }
}
