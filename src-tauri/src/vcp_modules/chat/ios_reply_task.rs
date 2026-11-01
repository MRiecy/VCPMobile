//! Best-effort iOS display for the lifetime of one chat turn.
use std::future::Future;
use tokio::sync::oneshot;

pub(crate) struct ReplyTask(oneshot::Sender<bool>);

impl ReplyTask {
    #[cfg(target_os = "ios")]
    pub(crate) fn begin<R: tauri::Runtime>(app: &tauri::AppHandle<R>, title: &str) -> Self {
        use tauri_plugin_vcp_mobile::stream::ios_chat;
        let (finished, receiver) = oneshot::channel();
        let app = app.clone();
        let title = title.to_owned();
        tauri::async_runtime::spawn(async move {
            let tag = uuid::Uuid::new_v4().to_string();
            if let Some(success) =
                display_outcome(receiver, ios_chat::begin(&app, &tag, &title)).await
            {
                ios_chat::finish(&app, &tag, success).await;
            }
        });
        Self(finished)
    }

    pub(crate) fn finish(self, success: bool) {
        let _ = self.0.send(success);
    }
}

// Skip a turn that ended before submission. Once submitted, wait for its
// acknowledgement so a late native launch still receives the actual outcome.
async fn display_outcome(
    mut finished: oneshot::Receiver<bool>,
    begin: impl Future<Output = bool>,
) -> Option<bool> {
    if finished.try_recv() != Err(oneshot::error::TryRecvError::Empty) || !begin.await {
        return None;
    }
    Some(finished.await.unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn completed_turn_skips_display_submission_if_the_observer_has_not_started() {
        let (finished, receiver) = oneshot::channel();
        ReplyTask(finished).finish(true);
        assert_eq!(
            display_outcome(receiver, async { panic!("late begin") }).await,
            None
        );
    }

    #[tokio::test]
    async fn a_late_submission_acknowledgement_uses_the_finished_turns_outcome() {
        for (accepted, committed) in [(true, true), (true, false), (false, true)] {
            let (finished, receiver) = oneshot::channel();
            let task = ReplyTask(finished);
            let (started, wait_for_start) = oneshot::channel();
            let (release, wait_for_release) = oneshot::channel();
            let begin = async {
                started.send(()).unwrap();
                wait_for_release.await.unwrap();
                accepted
            };
            let (outcome, ()) = tokio::join!(display_outcome(receiver, begin), async {
                wait_for_start.await.unwrap();
                if committed {
                    task.finish(true);
                } else {
                    drop(task);
                }
                release.send(()).unwrap();
            });
            assert_eq!(outcome, accepted.then_some(committed));
        }
    }
}
