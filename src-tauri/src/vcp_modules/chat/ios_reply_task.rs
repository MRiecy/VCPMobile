//! iOS execution grants belong to one user turn, even when a group changes speaker.
//! Only live cancellation handles are retained; the HTTP request and its data stay in Rust.
use crate::vcp_modules::vcp_client::{VcpRequestFailure, VcpResponse};
use dashmap::DashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const EXPIRED: &str = "系统已结束本次后台回复任务";

// Keep user stop and the OS expiration distinguishable while waking the same
// request/turn tree (including attachment preparation or a group decision).
struct ExecutionGrant {
    tag: String,
    expired: CancellationToken,
}

static GRANTS: LazyLock<DashMap<String, (CancellationToken, CancellationToken)>> =
    LazyLock::new(DashMap::new);

impl ExecutionGrant {
    fn new(cancel: CancellationToken) -> Self {
        let tag = Uuid::new_v4().to_string();
        let expired = CancellationToken::new();
        GRANTS.insert(tag.clone(), (expired.clone(), cancel));
        Self { tag, expired }
    }

    fn transport_result(
        &self,
        result: Result<VcpResponse, VcpRequestFailure>,
    ) -> Result<VcpResponse, VcpRequestFailure> {
        match result {
            Ok(reply) if reply.is_aborted && self.expired.is_cancelled() => {
                Err(VcpRequestFailure::streaming(EXPIRED, reply.full_content))
            }
            result => result,
        }
    }
}

impl Drop for ExecutionGrant {
    fn drop(&mut self) {
        GRANTS.remove(&self.tag);
    }
}

pub(crate) fn expire(tag: &str) {
    if let Some(grant) = GRANTS.get(tag) {
        let (expired, cancel) = grant.value();
        if !cancel.is_cancelled() {
            expired.cancel();
            cancel.cancel();
        }
    }
}

#[derive(Default)]
pub(crate) struct ReceivedCharacters {
    bytes: usize,
    characters: u64,
    last_report: Option<Instant>,
}

impl ReceivedCharacters {
    // Aurora's full_text is append-only. Scan each newly received byte once,
    // and cross the Swift bridge only on new data, at most once per second.
    pub(crate) fn observe(&mut self, text: &str, now: Instant) -> Option<u64> {
        if text.len() == self.bytes
            || self
                .last_report
                .is_some_and(|last| now.duration_since(last) < Duration::from_secs(1))
        {
            return None;
        }
        self.characters += text[self.bytes..].chars().count() as u64;
        self.bytes = text.len();
        self.last_report = Some(now);
        Some(self.characters)
    }
}

#[cfg(target_os = "ios")]
mod native {
    use super::*;
    use tauri::{AppHandle, Runtime};
    use tauri_plugin_vcp_mobile::stream::ios_chat;
    use tokio::sync::watch;

    pub(crate) struct ReplyTask<R: Runtime> {
        app: AppHandle<R>,
        grant: ExecutionGrant,
        display: Option<watch::Sender<(String, u64)>>,
        pub(crate) success: bool,
    }

    impl<R: Runtime> ReplyTask<R> {
        pub(crate) fn begin(
            app: &AppHandle<R>,
            title: &str,
            speaker: &str,
            cancel: CancellationToken,
        ) -> Self {
            let grant = ExecutionGrant::new(cancel);
            // Never hold up the HTTP reader waiting for UIKit to redraw. A slow
            // bridge needs only the latest speaker and count, in order, for this tag.
            let display = ios_chat::begin(app, &grant.tag, title, speaker).then(|| {
                let (sender, mut receiver) = watch::channel((speaker.to_owned(), 0));
                let app = app.clone();
                let tag = grant.tag.clone();
                tauri::async_runtime::spawn(async move {
                    while receiver.changed().await.is_ok() {
                        let (speaker, count) = receiver.borrow_and_update().clone();
                        ios_chat::update(&app, &tag, &speaker, count).await;
                    }
                });
                sender
            });
            Self {
                app: app.clone(),
                grant,
                display,
                success: false,
            }
        }

        pub(crate) fn is_running(&self) -> bool {
            self.display.is_some() && !self.grant.expired.is_cancelled()
        }

        pub(crate) fn update(&self, speaker: &str, received: u64) {
            if let Some(display) = &self.display {
                display.send_if_modified(|(current, count)| {
                    if current.as_str() == speaker && *count == received {
                        return false;
                    }
                    speaker.clone_into(current);
                    *count = received;
                    true
                });
            }
        }

        pub(crate) fn check_expiration(&self) -> Result<(), String> {
            if self.grant.expired.is_cancelled() {
                Err(EXPIRED.to_string())
            } else {
                Ok(())
            }
        }

        pub(crate) fn transport_result(
            &self,
            result: Result<VcpResponse, VcpRequestFailure>,
        ) -> Result<VcpResponse, VcpRequestFailure> {
            self.grant.transport_result(result)
        }
    }

    impl<R: Runtime> Drop for ReplyTask<R> {
        fn drop(&mut self) {
            if !self.is_running() {
                return;
            }
            let app = self.app.clone();
            let tag = self.grant.tag.clone();
            let success = self.success;
            tauri::async_runtime::spawn_blocking(move || ios_chat::finish(&app, &tag, success));
        }
    }

    pub(crate) struct Finalization<R: Runtime> {
        app: AppHandle<R>,
        tag: String,
    }

    impl<R: Runtime> Finalization<R> {
        pub(crate) fn begin(app: &AppHandle<R>) -> Option<Self> {
            let tag = Uuid::new_v4().to_string();
            ios_chat::begin_finalization(app, &tag).then(|| Self {
                app: app.clone(),
                tag,
            })
        }
    }

    impl<R: Runtime> Drop for Finalization<R> {
        fn drop(&mut self) {
            let app = self.app.clone();
            let tag = self.tag.clone();
            tauri::async_runtime::spawn_blocking(move || ios_chat::end_finalization(&app, &tag));
        }
    }
}

#[cfg(target_os = "ios")]
pub(crate) use native::{Finalization, ReplyTask};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry_targets_only_the_matching_live_turn_and_wakes_its_speaker() {
        let group = CancellationToken::new();
        let speaker = group.child_token();
        let old = ExecutionGrant::new(group);
        let newer = ExecutionGrant::new(CancellationToken::new());
        expire(&old.tag);
        assert!(speaker.is_cancelled() && old.expired.is_cancelled());
        assert!(!newer.expired.is_cancelled());
        let old_tag = old.tag.clone();
        drop(old);
        expire(&old_tag);
        assert!(!newer.expired.is_cancelled());
    }

    #[test]
    fn a_user_stop_does_not_become_an_expiration() {
        let stop = CancellationToken::new();
        let task = ExecutionGrant::new(stop.clone());
        stop.cancel();
        expire(&task.tag);
        assert!(!task.expired.is_cancelled());
        assert!(
            task.transport_result(Ok(VcpResponse::cancelled()))
                .unwrap()
                .is_aborted
        );
    }

    #[test]
    fn expired_sse_preserves_current_partial_and_completed_http_is_not_reclassified() {
        let task = ExecutionGrant::new(CancellationToken::new());
        expire(&task.tag);
        let interrupted = VcpResponse {
            full_content: "已有正文".into(),
            ..VcpResponse::cancelled()
        };
        let failure = task.transport_result(Ok(interrupted)).unwrap_err();
        assert!(!failure.is_recovery_pending());
        assert_eq!(
            failure.into_parts(),
            (EXPIRED.into(), Some("已有正文".into()))
        );
        let completed = VcpResponse {
            full_content: "全部".into(),
            is_aborted: false,
            finish_reason: Some("stop".into()),
        };
        assert_eq!(
            task.transport_result(Ok(completed)).unwrap().full_content,
            "全部"
        );
    }

    #[test]
    fn progress_counts_new_unicode_text_and_skips_unchanged_or_rapid_updates() {
        let mut count = ReceivedCharacters::default();
        let start = Instant::now();
        assert_eq!(count.observe("", start), None);
        assert_eq!(count.observe("你好", start), Some(2));
        assert_eq!(count.observe("你好👋", start), None);
        let next = start + Duration::from_secs(1);
        assert_eq!(count.observe("你好👋！", next), Some(4));
        assert_eq!(
            count.observe("你好👋！", next + Duration::from_secs(2)),
            None
        );
    }
}
