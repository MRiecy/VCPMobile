//! Best-effort iOS progress display for one user turn, including changes of group speaker.
use std::future::Future;
use std::time::{Duration, Instant};
use tokio::sync::watch;

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

#[derive(Clone)]
enum DisplayUpdate {
    Progress { speaker: String, characters: u64 },
    Finished(bool),
}

pub(crate) struct ReplyTask {
    display: watch::Sender<DisplayUpdate>,
    pub(crate) success: bool,
}

impl ReplyTask {
    fn channel(speaker: &str) -> (Self, watch::Receiver<DisplayUpdate>) {
        let (display, receiver) = watch::channel(DisplayUpdate::Progress {
            speaker: speaker.to_owned(),
            characters: 0,
        });
        (
            Self {
                display,
                success: false,
            },
            receiver,
        )
    }

    #[cfg(target_os = "ios")]
    pub(crate) fn begin<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        title: &str,
        speaker: &str,
    ) -> Self {
        use tauri_plugin_vcp_mobile::stream::ios_chat;
        let (task, receiver) = Self::channel(speaker);
        let app = app.clone();
        let title = title.to_owned();
        let speaker = speaker.to_owned();
        tauri::async_runtime::spawn(async move {
            let tag = uuid::Uuid::new_v4().to_string();
            let app = &app;
            let tag = tag.as_str();
            let outcome = observe(
                receiver,
                ios_chat::begin(app, tag, &title, &speaker),
                |speaker, count| async move { ios_chat::update(app, tag, &speaker, count).await },
            )
            .await;
            if let Some(success) = outcome {
                ios_chat::finish(app, tag, success).await;
            }
        });
        task
    }

    pub(crate) fn is_observing(&self) -> bool {
        !self.display.is_closed()
    }

    pub(crate) fn update(&self, speaker: &str, characters: u64) {
        let _ = self.display.send(DisplayUpdate::Progress {
            speaker: speaker.to_owned(),
            characters,
        });
    }
}

impl Drop for ReplyTask {
    fn drop(&mut self) {
        let _ = self.display.send(DisplayUpdate::Finished(self.success));
    }
}

// The observer alone calls Swift. Dropping the sole sender replaces any pending
// progress with the final result, even while native begin or update is awaiting a reply.
async fn observe<F: Future<Output = bool>>(
    mut receiver: watch::Receiver<DisplayUpdate>,
    begin: impl Future<Output = bool>,
    mut update: impl FnMut(String, u64) -> F,
) -> Option<bool> {
    if receiver.has_changed().is_err() || !begin.await {
        return None;
    }
    while receiver.changed().await.is_ok() {
        let latest = receiver.borrow_and_update().clone();
        match latest {
            DisplayUpdate::Finished(success) => return Some(success),
            DisplayUpdate::Progress {
                speaker,
                characters,
            } => {
                if !update(speaker, characters).await {
                    return Some(false);
                }
            }
        }
    }
    Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn completed_turn_skips_display_submission_if_the_observer_has_not_started() {
        let (task, receiver) = ReplyTask::channel("agent");
        drop(task);
        let outcome = observe(receiver, async { panic!("late begin") }, |_, _| async {
            panic!("late update")
        })
        .await;
        assert_eq!(outcome, None);
    }

    #[tokio::test]
    async fn finish_supersedes_progress_while_begin_awaits_the_scheduler() {
        for accepted in [true, false] {
            let (mut task, receiver) = ReplyTask::channel("first");
            let (started, wait_for_start) = oneshot::channel();
            let (release, wait_for_release) = oneshot::channel();
            let begin = async {
                started.send(()).unwrap();
                wait_for_release.await.unwrap();
                accepted
            };
            let (outcome, ()) = tokio::join!(
                observe(receiver, begin, |_, _| async {
                    panic!("obsolete progress")
                }),
                async {
                    wait_for_start.await.unwrap();
                    task.update("second", 7);
                    task.success = true;
                    drop(task);
                    release.send(()).unwrap();
                }
            );
            assert_eq!(outcome, accepted.then_some(true));
        }
    }

    #[tokio::test]
    async fn an_expired_display_receives_only_the_latest_progress_and_stops_observing() {
        let (task, receiver) = ReplyTask::channel("first");
        task.update("first", 15);
        task.update("second", 2);
        let mut updates = Vec::new();
        let outcome = observe(receiver, async { true }, |speaker, count| {
            updates.push((speaker, count));
            std::future::ready(false)
        })
        .await;
        assert_eq!(updates, [("second".to_string(), 2)]);
        assert_eq!(outcome, Some(false));
        assert!(!task.is_observing());
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
