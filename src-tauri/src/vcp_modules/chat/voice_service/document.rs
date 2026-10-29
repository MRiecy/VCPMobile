//! A document authorizes new voice intents, not the lifetime of a call or its
//! accepted work. Capture and independent playback keep separate stop barriers.

use std::sync::Mutex;
use tauri::{AppHandle, Manager, Runtime};

const STALE_DOCUMENT: &str = "cancelled: 语音所属页面已失效，请重试。";

#[derive(Default)]
struct Documents {
    issued: u64,
    current: u64,
    valid: bool,
}

#[derive(Default)]
pub struct VoiceDocumentState(Mutex<Documents>);

fn parse_document(document_id: &str) -> Result<u64, String> {
    document_id.parse::<u64>().ok().filter(|id| *id != 0)
        .ok_or_else(|| "语音页面标识无效。".into())
}

impl VoiceDocumentState {
    pub(super) fn allocate(&self) -> Result<String, String> {
        let mut documents = self.0.lock().map_err(|error| error.to_string())?;
        documents.issued = documents.issued.checked_add(1)
            .ok_or("语音页面标识已耗尽，请重新启动应用。")?;
        Ok(documents.issued.to_string())
    }

    /// The callback takes only resource admission locks, never awaits. The
    /// shared lock order is document -> input/TTS, including normal commands.
    pub(super) fn adopt(
        &self,
        document_id: &str,
        on_changed: impl FnOnce() -> Result<(), String>,
    ) -> Result<bool, String> {
        let document = parse_document(document_id)?;
        let mut documents = self.0.lock().map_err(|error| error.to_string())?;
        if document > documents.issued {
            return Err("语音页面标识无效。".into());
        }
        if document < documents.current || (document == documents.current && !documents.valid) {
            return Err(STALE_DOCUMENT.into());
        }
        if document == documents.current {
            return Ok(false);
        }
        on_changed()?;
        documents.current = document;
        documents.valid = true;
        Ok(true)
    }

    pub(super) fn with_current<T>(
        &self,
        document_id: &str,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let document = parse_document(document_id)?;
        let documents = self.0.lock().map_err(|error| error.to_string())?;
        if !documents.valid || documents.current != document {
            return Err(STALE_DOCUMENT.into());
        }
        operation()
    }

    /// Retain the last identity after invalidation so reconnecting the same
    /// ticket cannot resurrect it. A late old attachment cannot revoke a new one.
    pub(super) fn invalidate(&self, expected: Option<&str>) -> Result<Option<String>, String> {
        let expected = expected.map(parse_document).transpose()?;
        let mut documents = self.0.lock().map_err(|error| error.to_string())?;
        if !documents.valid || expected.is_some_and(|id| id != documents.current) {
            return Ok(None);
        }
        documents.valid = false;
        Ok(Some(documents.current.to_string()))
    }
}

#[tauri::command]
pub fn allocate_voice_document(app: AppHandle) -> Result<String, String> {
    app.state::<VoiceDocumentState>().allocate()
}

/// Keep identity adoption usable independently of any native cleanup result.
/// Playback cleanup runs under its own existing stop barrier; a failure remains
/// retryable through connect_tts_document instead of blocking capture recovery.
pub(super) fn adopt_document<R: Runtime>(app: &AppHandle<R>, document_id: &str) -> Result<(), String> {
    let mut previous_playback = None;
    app.state::<VoiceDocumentState>().adopt(document_id, || {
        previous_playback = super::tts::adopt_document(app, document_id)?;
        Ok(())
    })?;
    if let Some(id) = previous_playback {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = super::tts::stop_tts(app, id).await {
                log::warn!("[VoiceDocument] Previous playback cleanup failed: {}", error.chars().take(160).collect::<String>());
            }
        });
    }
    Ok(())
}

#[tauri::command]
pub async fn connect_voice_document(app: AppHandle, document_id: String) -> Result<(), String> {
    adopt_document(&app, &document_id)?;
    let (capture, native) = tokio::join!(
        super::input::retire_other_documents(app.clone(), &document_id),
        tauri_plugin_vcp_mobile::voice::connect_voice_document(app.clone(), document_id.clone()),
    );
    // Both owners must finish their attempts even if one fails. Successful
    // repeated connects retry any retained capture without reviving old tickets.
    capture?;
    native?;
    app.state::<VoiceDocumentState>().with_current(&document_id, || Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn allocation_is_inert_and_late_activation_cannot_replace_a_new_document() {
        let state = VoiceDocumentState::default();
        let old = state.allocate().unwrap();
        let current = state.allocate().unwrap();
        assert!(state.with_current(&old, || Ok(())).is_err());
        state.adopt(&current, || Ok(())).unwrap();
        let unused = state.allocate().unwrap();
        assert!(state.with_current(&current, || Ok(())).is_ok());
        assert!(state.with_current(&unused, || Ok(())).is_err());
        assert!(state.adopt(&old, || panic!("stale adoption must not touch resources")).is_err());
        assert!(state.with_current(&old, || -> Result<(), String> {
            panic!("unpolled old command must be rejected")
        }).is_err());
    }

    #[test]
    fn invalidation_is_exact_and_an_invalid_ticket_cannot_reconnect() {
        let state = VoiceDocumentState::default();
        let first = state.allocate().unwrap();
        state.adopt(&first, || Ok(())).unwrap();
        assert_eq!(state.invalidate(Some(&first)).unwrap(), Some(first.clone()));
        assert!(state.adopt(&first, || Ok(())).is_err());
        assert!(state.with_current(&first, || Ok(())).is_err());
        let next = state.allocate().unwrap();
        state.adopt(&next, || Ok(())).unwrap();
        assert!(state.invalidate(Some(&first)).unwrap().is_none());
        assert!(state.with_current(&next, || Ok(())).is_ok());
        assert_eq!(state.invalidate(None).unwrap(), Some(next));
        assert!(state.invalidate(None).unwrap().is_none());
    }

    #[test]
    fn repeated_adoption_does_not_repeat_resource_retirement() {
        let state = VoiceDocumentState::default();
        let document = state.allocate().unwrap();
        let retired = AtomicUsize::new(0);
        for expected in [true, false, false] {
            assert_eq!(state.adopt(&document, || {
                retired.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }).unwrap(), expected);
        }
        assert_eq!(retired.load(Ordering::Relaxed), 1);
        assert!(state.adopt("0", || Ok(())).is_err());
        assert!(state.adopt("99", || Ok(())).is_err());
    }
}
