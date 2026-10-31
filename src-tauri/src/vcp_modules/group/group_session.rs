//! Mutable JEV scheduling state belongs to the existing topic turn lease.
//! This module has no registry: ActiveGroupTurns remains the admission owner.

use crate::vcp_modules::chat::voice_service::VoiceReplyPolicy;
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const MAX_QUEUED_MEMBERS: usize = 254;
const MAX_PENDING_INPUTS: usize = 32;

pub(crate) type SelectedSpeaker = (String, Option<Arc<GroupReplyScope>>);

pub(crate) struct GroupReplyScope {
    pub origin: String,
    pub cancellation: CancellationToken,
    pub policy: Option<Arc<VoiceReplyPolicy>>,
    finished: CancellationToken,
    finished_once: AtomicBool,
    result: Mutex<Option<Result<serde_json::Value, String>>>,
    on_finished: Box<dyn Fn() + Send + Sync>,
    on_cancel: Box<dyn Fn() + Send + Sync>,
}

impl GroupReplyScope {
    pub(crate) fn new(
        origin: String,
        cancellation: CancellationToken,
        policy: Option<Arc<VoiceReplyPolicy>>,
        on_finished: impl Fn() + Send + Sync + 'static,
        on_cancel: impl Fn() + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            origin,
            cancellation,
            policy,
            finished: CancellationToken::new(),
            finished_once: AtomicBool::new(false),
            result: Mutex::new(None),
            on_finished: Box::new(on_finished),
            on_cancel: Box::new(on_cancel),
        })
    }

    pub(crate) fn finish(&self) {
        self.finish_with_result(Ok(serde_json::json!({"status":"completed"})));
    }

    fn finish_with_result(&self, result: Result<serde_json::Value, String>) {
        if !self.finished_once.swap(true, Ordering::AcqRel) {
            if let Ok(mut stored) = self.result.lock() {
                *stored = Some(result);
            }
            (self.on_finished)();
            self.finished.cancel();
        }
    }

    pub(crate) async fn wait(&self) -> Result<serde_json::Value, String> {
        self.finished.cancelled().await;
        self.result
            .lock()
            .map_err(|e| e.to_string())?
            .clone()
            .ok_or_else(|| "群聊回复没有完成结果".to_string())?
    }
    fn cancel(&self) {
        self.cancellation.cancel();
        (self.on_cancel)();
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GroupSessionSnapshot {
    pub mode: &'static str,
    pub phase: &'static str,
    pub queued_agent_ids: Vec<String>,
    pub current_agent_id: Option<String>,
    pub autonomous_rounds: u32,
    pub revision: u64,
    pub stop_reason: Option<String>,
    pub error: Option<String>,
}

struct SessionState {
    queue: VecDeque<String>,
    current: Option<String>,
    current_scope: Option<Arc<GroupReplyScope>>,
    latest_scope: Option<Arc<GroupReplyScope>>,
    latest_input_id: u64,
    current_decision_cancellation: Option<CancellationToken>,
    phase: &'static str,
    revision: u64,
    rounds: u32,
    counts: BTreeMap<String, u32>,
    force_continue: bool,
    trigger: &'static str,
    pending_inputs: BTreeMap<u64, CancellationToken>,
    closed: bool,
    stop_reason: Option<String>,
    error: Option<String>,
}

impl SessionState {
    fn advance_revision(&mut self) {
        self.revision += 1;
        if let Some(cancellation) = self.current_decision_cancellation.take() {
            cancellation.cancel();
        }
    }
}

pub(crate) struct GroupSession {
    pub turn_id: String,
    pub cancellation: CancellationToken,
    state: Mutex<SessionState>,
    changed: Notify,
}

pub(crate) struct GroupDecisionContext {
    pub revision: u64,
    pub cancellation: CancellationToken,
    pub rounds: u32,
    pub counts: BTreeMap<String, u32>,
    pub force_continue: bool,
    pub trigger: &'static str,
    pub scope: Option<Arc<GroupReplyScope>>,
}

pub(crate) struct GroupInputReservation {
    pub session: Arc<GroupSession>,
    id: u64,
    consumed: bool,
}

impl GroupInputReservation {
    pub(crate) fn commit(
        mut self,
        scope: Arc<GroupReplyScope>,
        mentions: &[String],
    ) -> Result<(), String> {
        let old = {
            let mut state = self.session.state.lock().map_err(|e| e.to_string())?;
            if state.closed || self.session.cancellation.is_cancelled() {
                return Err("群聊已停止，消息已保存；请重新发起群聊。".into());
            }
            state.pending_inputs.remove(&self.id);
            self.consumed = true;
            state.advance_revision();
            // Persistence and member reads can settle out of admission order.
            // A late origin must never take cancellation ownership from a newer input.
            let is_latest = self.id > state.latest_input_id;
            let old = if is_latest {
                state.latest_input_id = self.id;
                state.latest_scope.replace(scope.clone())
            } else {
                Some(scope.clone())
            };
            if !scope.cancellation.is_cancelled() {
                if is_latest {
                    for id in mentions.iter().rev() {
                        if state.current.as_deref() != Some(id) {
                            state.queue.retain(|queued| queued != id);
                            if state.queue.len() < MAX_QUEUED_MEMBERS {
                                state.queue.push_front(id.clone());
                            }
                        }
                    }
                } else {
                    // Keep an earlier explicit invitation without moving it ahead
                    // of newer input or manual work already accepted by the owner.
                    for id in mentions {
                        if state.current.as_deref() != Some(id)
                            && !state.queue.contains(id)
                            && state.queue.len() < MAX_QUEUED_MEMBERS
                        {
                            state.queue.push_back(id.clone());
                        }
                    }
                }
            }
            old.filter(|old| {
                !state
                    .current_scope
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(old, current))
            })
        };
        if let Some(old) = old {
            if old.cancellation.is_cancelled() {
                old.finish_with_result(Ok(serde_json::json!({"status":"cancelled"})));
            } else {
                old.finish();
            }
        }
        self.session.changed.notify_one();
        Ok(())
    }
}

impl Drop for GroupInputReservation {
    fn drop(&mut self) {
        if !self.consumed {
            if let Ok(mut state) = self.session.state.lock() {
                state.pending_inputs.remove(&self.id);
            }
            self.session.changed.notify_one();
        }
    }
}

impl GroupSession {
    pub(crate) fn new(
        turn_id: String,
        cancellation: CancellationToken,
        trigger: &'static str,
        force_continue: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            turn_id,
            cancellation,
            changed: Notify::new(),
            state: Mutex::new(SessionState {
                queue: VecDeque::new(),
                current: None,
                current_scope: None,
                latest_scope: None,
                latest_input_id: 0,
                current_decision_cancellation: None,
                phase: "waiting",
                revision: 0,
                rounds: 0,
                counts: BTreeMap::new(),
                force_continue,
                trigger,
                pending_inputs: BTreeMap::new(),
                closed: false,
                stop_reason: None,
                error: None,
            }),
        })
    }

    pub(crate) fn snapshot(&self) -> Result<GroupSessionSnapshot, String> {
        let state = self.state.lock().map_err(|e| e.to_string())?;
        Ok(GroupSessionSnapshot {
            mode: "jev",
            phase: if self.cancellation.is_cancelled() && !state.closed {
                "stopping"
            } else {
                state.phase
            },
            queued_agent_ids: state.queue.iter().cloned().collect(),
            current_agent_id: state.current.clone(),
            autonomous_rounds: state.rounds,
            revision: state.revision,
            stop_reason: state.stop_reason.clone(),
            error: state.error.clone(),
        })
    }

    pub(crate) fn reserve(self: &Arc<Self>) -> Result<GroupInputReservation, String> {
        self.reserve_with_cancellation(self.cancellation.child_token())
    }

    pub(crate) fn reserve_with_cancellation(
        self: &Arc<Self>,
        cancellation: CancellationToken,
    ) -> Result<GroupInputReservation, String> {
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        if state.closed || self.cancellation.is_cancelled() {
            return Err("群聊正在停止，请在结束后重试。".into());
        }
        if state.pending_inputs.len() >= MAX_PENDING_INPUTS {
            return Err("等待保存的群消息过多，请稍后重试。".into());
        }
        state.advance_revision(); // Admission preempts the old decision before input IO.
        let id = state.revision;
        state.pending_inputs.insert(id, cancellation);
        Ok(GroupInputReservation {
            session: self.clone(),
            id,
            consumed: false,
        })
    }

    pub(crate) fn enqueue(&self, ids: &[String]) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        if state.closed || self.cancellation.is_cancelled() {
            return Err("群聊正在停止，请在结束后重试。".into());
        }
        if state
            .latest_scope
            .as_ref()
            .is_some_and(|scope| scope.cancellation.is_cancelled())
        {
            return Err("当前回复正在取消，请结束后重新邀请成员。".into());
        }
        let mut next = state.queue.clone();
        for id in ids.iter().rev() {
            if state.current.as_deref() == Some(id) {
                continue;
            }
            next.retain(|queued| queued != id);
            next.push_front(id.clone());
        }
        if next.len() > MAX_QUEUED_MEMBERS {
            return Err("群聊待发言队列已满。".into());
        }
        state.queue = next;
        state.advance_revision();
        self.changed.notify_one();
        Ok(())
    }

    pub(crate) fn next_speaker(&self, revision: u64) -> Result<Option<SelectedSpeaker>, String> {
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        if state.closed
            || self.cancellation.is_cancelled()
            || state.revision != revision
            || !state.pending_inputs.is_empty()
        {
            return Ok(None);
        }
        let Some(id) = state.queue.pop_front() else {
            return Ok(None);
        };
        let scope = state.latest_scope.clone();
        state.current = Some(id.clone());
        state.current_scope = scope.clone();
        state.phase = "speaking";
        state.advance_revision();
        Ok(Some((id, scope)))
    }

    pub(crate) fn speaker_finished(
        &self,
        persisted: bool,
        result: Result<serde_json::Value, String>,
    ) -> Result<(), String> {
        let finished = {
            let mut state = self.state.lock().map_err(|e| e.to_string())?;
            if let Some(id) = state.current.take() {
                if persisted {
                    *state.counts.entry(id).or_default() += 1;
                }
            }
            state.advance_revision();
            state.phase = "waiting";
            state.current_scope.take().filter(|old| {
                !state
                    .latest_scope
                    .as_ref()
                    .is_some_and(|latest| Arc::ptr_eq(old, latest))
            })
        };
        if let Some(finished) = finished {
            finished.finish_with_result(result);
        }
        Ok(())
    }

    pub(crate) async fn await_inputs(&self) -> Result<(), String> {
        loop {
            let changed = self.changed.notified();
            if self
                .state
                .lock()
                .map_err(|e| e.to_string())?
                .pending_inputs
                .is_empty()
            {
                return Ok(());
            }
            tokio::select! { _ = changed => {}, _ = self.cancellation.cancelled() => return Ok(()) }
        }
    }

    pub(crate) fn decision_context(
        &self,
        revision: u64,
    ) -> Result<Option<GroupDecisionContext>, String> {
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        if state.revision != revision
            || !state.pending_inputs.is_empty()
            || !state.queue.is_empty()
            || state.closed
            || self.cancellation.is_cancelled()
        {
            return Ok(None);
        }
        state.phase = "deciding";
        state.advance_revision();
        let cancellation = self.cancellation.child_token();
        state.current_decision_cancellation = Some(cancellation.clone());
        state.current_scope = state.latest_scope.clone();
        Ok(Some(GroupDecisionContext {
            revision: state.revision,
            cancellation,
            rounds: state.rounds,
            counts: state.counts.clone(),
            force_continue: state.force_continue,
            trigger: state.trigger,
            scope: state.latest_scope.clone(),
        }))
    }

    pub(crate) fn decision_finished(&self) {
        let retired = self.state.lock().ok().and_then(|mut state| {
            state.current_decision_cancellation = None;
            state.current_scope.take().filter(|old| {
                !state
                    .latest_scope
                    .as_ref()
                    .is_some_and(|latest| Arc::ptr_eq(old, latest))
            })
        });
        if let Some(retired) = retired {
            retired.finish();
        }
    }

    pub(crate) fn accept_decision(
        &self,
        revision: u64,
        ids: &[String],
        stop_reason: Option<String>,
    ) -> Result<bool, String> {
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        if state.closed
            || self.cancellation.is_cancelled()
            || state.revision != revision
            || !state.pending_inputs.is_empty()
            || !state.queue.is_empty()
        {
            return Ok(false);
        }
        state.rounds += 1;
        state.advance_revision();
        state.force_continue = false;
        state.phase = "waiting";
        state
            .queue
            .extend(ids.iter().take(MAX_QUEUED_MEMBERS).cloned());
        if let Some(reason) = stop_reason {
            state.stop_reason = Some(reason);
            state.closed = true;
        }
        Ok(true)
    }

    pub(crate) fn revision_is_current(&self, revision: u64) -> bool {
        self.state
            .lock()
            .is_ok_and(|state| state.revision == revision && state.pending_inputs.is_empty())
    }

    pub(crate) fn invalidate(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.advance_revision();
        }
        self.changed.notify_one();
    }

    pub(crate) fn latest_scope_cancelled(&self) -> bool {
        self.state.lock().is_ok_and(|state| {
            state
                .latest_scope
                .as_ref()
                .is_some_and(|scope| scope.cancellation.is_cancelled())
        })
    }

    pub(crate) fn fail(&self, error: String) {
        if let Ok(mut state) = self.state.lock() {
            state.error = Some(error);
        }
        self.close(Some("error".into()));
    }

    pub(crate) fn close_if_current(&self, revision: u64, reason: &str) -> Result<bool, String> {
        {
            let mut state = self.state.lock().map_err(|e| e.to_string())?;
            if state.revision != revision || !state.pending_inputs.is_empty() {
                return Ok(false);
            }
            state.closed = true;
            state.stop_reason = Some(reason.into());
        }
        self.close(None);
        Ok(true)
    }

    pub(crate) fn cancel_scopes(&self) {
        if let Ok(state) = self.state.lock() {
            for cancellation in state.pending_inputs.values() {
                cancellation.cancel();
            }
        }
        let scopes = self
            .state
            .lock()
            .map(|state| [state.current_scope.clone(), state.latest_scope.clone()])
            .unwrap_or([None, None]);
        for scope in scopes.into_iter().flatten() {
            scope.cancel();
        }
    }

    pub(crate) fn finish_cancelled_scope(&self, scope: &Arc<GroupReplyScope>) {
        let owns_work = self.state.lock().is_ok_and(|mut state| {
            let owns_work = state
                .current_scope
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, scope));
            let is_latest = state
                .latest_scope
                .as_ref()
                .is_some_and(|latest| Arc::ptr_eq(latest, scope));
            if owns_work || is_latest {
                state.advance_revision();
            }
            owns_work
        });
        if !owns_work {
            scope.finish_with_result(Ok(serde_json::json!({"status":"cancelled"})));
        }
        self.changed.notify_one();
    }

    pub(crate) fn close(&self, reason: Option<String>) {
        let (scopes, result) = if let Ok(mut state) = self.state.lock() {
            state.closed = true;
            state.phase = "idle";
            state.queue.clear();
            state.current = None;
            state.advance_revision();
            if reason.is_some() {
                state.stop_reason = reason;
            }
            let result = if let Some(error) = &state.error {
                Err(error.clone())
            } else {
                let status = match state.stop_reason.as_deref() {
                    Some("cancelled_by_user" | "cancelled") => "cancelled",
                    Some("stopping") => "stopping",
                    Some("recovery_pending") => "recovery_pending",
                    Some("discarded") => "discarded",
                    _ => "completed",
                };
                Ok(serde_json::json!({"status":status}))
            };
            (
                [state.current_scope.take(), state.latest_scope.take()],
                result,
            )
        } else {
            ([None, None], Err("群聊状态已失效".into()))
        };
        for scope in scopes.into_iter().flatten() {
            if self.cancellation.is_cancelled() {
                scope.cancel();
            }
            scope.finish_with_result(result.clone());
        }
        self.changed.notify_waiters();
    }
}

/// Bridges a voice reply's cancellation to one real request. Dropping this
/// guard only ends the idle signal watcher, never the request/finalizer task.
pub(crate) struct ScopeCancellation {
    pub token: CancellationToken,
    watcher: Option<tokio::task::JoinHandle<()>>,
}

impl ScopeCancellation {
    pub(crate) fn new(parent: CancellationToken, scope: Option<&Arc<GroupReplyScope>>) -> Self {
        let token = parent.child_token();
        let watcher = scope.map(|scope| {
            let source = scope.cancellation.clone(); let target = token.clone();
            if source.is_cancelled() { target.cancel(); }
            tokio::spawn(async move { tokio::select! { _ = source.cancelled() => target.cancel(), _ = target.cancelled() => {} } })
        });
        Self { token, watcher }
    }
}

impl Drop for ScopeCancellation {
    fn drop(&mut self) {
        if let Some(watcher) = self.watcher.take() {
            watcher.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn scope(
        id: &str,
        cancel: CancellationToken,
        finished: Arc<AtomicUsize>,
    ) -> Arc<GroupReplyScope> {
        GroupReplyScope::new(
            id.into(),
            cancel,
            None,
            move || {
                finished.fetch_add(1, Ordering::SeqCst);
            },
            || {},
        )
    }

    #[test]
    fn admitted_input_blocks_scheduling_and_stale_stop_cannot_swallow_manual_intent() {
        let session = GroupSession::new("turn".into(), CancellationToken::new(), "continue", true);
        let initial = session.snapshot().unwrap().revision;
        let decision = session.decision_context(initial).unwrap().unwrap();
        let reservation = session.reserve().unwrap();
        session.enqueue(&["member".into()]).unwrap();
        assert!(session
            .next_speaker(session.snapshot().unwrap().revision)
            .unwrap()
            .is_none());
        assert!(session
            .decision_context(session.snapshot().unwrap().revision)
            .unwrap()
            .is_none());
        assert!(!session
            .accept_decision(decision.revision, &[], Some("end_option_selected".into()))
            .unwrap());
        drop(reservation);
        assert_eq!(session.snapshot().unwrap().autonomous_rounds, 0);
        let (member, _) = session
            .next_speaker(session.snapshot().unwrap().revision)
            .unwrap()
            .unwrap();
        assert_eq!(member, "member");
        session
            .speaker_finished(true, Ok(serde_json::json!({"status":"completed"})))
            .unwrap();
        let next = session
            .decision_context(session.snapshot().unwrap().revision)
            .unwrap()
            .unwrap();
        assert!(
            next.force_continue,
            "only an accepted decision consumes force continue"
        );
    }

    #[tokio::test]
    async fn voice_cancel_isolated_from_later_typed_input_and_completion_waits_for_work() {
        let root = CancellationToken::new();
        let voice_cancel = CancellationToken::new();
        let finished = Arc::new(AtomicUsize::new(0));
        let voice = scope("voice", voice_cancel.clone(), finished.clone());
        let typed = scope(
            "typed",
            CancellationToken::new(),
            Arc::new(AtomicUsize::new(0)),
        );
        let session = GroupSession::new("turn".into(), root.clone(), "user_message", false);
        session
            .reserve()
            .unwrap()
            .commit(voice.clone(), &[])
            .unwrap();
        session.enqueue(&["first".into()]).unwrap();
        session
            .next_speaker(session.snapshot().unwrap().revision)
            .unwrap()
            .unwrap();
        let bridge = ScopeCancellation::new(root.clone(), Some(&voice));
        session
            .reserve()
            .unwrap()
            .commit(typed.clone(), &[])
            .unwrap();
        voice_cancel.cancel();
        session.finish_cancelled_scope(&voice);
        assert_eq!(
            finished.load(Ordering::SeqCst),
            0,
            "live request/finalizer still owns the origin"
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), bridge.token.cancelled())
            .await
            .unwrap();
        assert!(!root.is_cancelled());
        assert!(!typed.cancellation.is_cancelled());
        session
            .speaker_finished(false, Ok(serde_json::json!({"status":"cancelled"})))
            .unwrap();
        voice.wait().await.unwrap();
        assert_eq!(finished.load(Ordering::SeqCst), 1);
        assert!(!session.latest_scope_cancelled());
    }

    #[tokio::test]
    async fn late_cancelled_voice_input_cannot_replace_a_committed_typed_input() {
        let session = GroupSession::new(
            "turn".into(),
            CancellationToken::new(),
            "user_message",
            false,
        );
        let voice_cancel = CancellationToken::new();
        let voice_finished = Arc::new(AtomicUsize::new(0));
        let typed_finished = Arc::new(AtomicUsize::new(0));
        let voice = scope("voice", voice_cancel.clone(), voice_finished.clone());
        let typed = scope("typed", CancellationToken::new(), typed_finished.clone());
        let slow_voice = session
            .reserve_with_cancellation(voice_cancel.clone())
            .unwrap();
        let later_typed = session.reserve().unwrap();

        // The text submitted after hangup can finish persistence/member reads
        // while the earlier voice submission still owns its reservation.
        later_typed
            .commit(typed.clone(), &["typed-member".into()])
            .unwrap();
        voice_cancel.cancel();
        slow_voice
            .commit(voice.clone(), &["voice-member".into()])
            .unwrap();
        assert_eq!(voice.wait().await.unwrap()["status"], "cancelled");
        assert_eq!(voice_finished.load(Ordering::SeqCst), 1);
        assert_eq!(typed_finished.load(Ordering::SeqCst), 0);
        assert!(!session.latest_scope_cancelled());
        assert_eq!(
            session.snapshot().unwrap().queued_agent_ids,
            ["typed-member"]
        );

        let (member, selected_scope) = session
            .next_speaker(session.snapshot().unwrap().revision)
            .unwrap()
            .unwrap();
        assert_eq!(member, "typed-member");
        assert!(Arc::ptr_eq(selected_scope.as_ref().unwrap(), &typed));
        session
            .speaker_finished(true, Ok(serde_json::json!({"status":"completed"})))
            .unwrap();
        let next = session
            .decision_context(session.snapshot().unwrap().revision)
            .unwrap()
            .unwrap();
        // A late cancellation watcher belonging to the retired origin must not
        // preempt the newer input's arbitration or change its revision.
        session.finish_cancelled_scope(&voice);
        assert!(session.revision_is_current(next.revision));
        assert!(!next.cancellation.is_cancelled());
        session.close(Some("end_option_selected".into()));
        assert_eq!(typed.wait().await.unwrap()["status"], "completed");
        assert_eq!(typed_finished.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn late_valid_input_keeps_newer_scope_and_invitation_priority() {
        let session = GroupSession::new(
            "turn".into(),
            CancellationToken::new(),
            "user_message",
            false,
        );
        let earlier_scope = scope(
            "earlier",
            CancellationToken::new(),
            Arc::new(AtomicUsize::new(0)),
        );
        let newer_scope = scope(
            "newer",
            CancellationToken::new(),
            Arc::new(AtomicUsize::new(0)),
        );
        let earlier = session.reserve().unwrap();
        let newer = session.reserve().unwrap();
        newer
            .commit(newer_scope.clone(), &["new".into(), "shared".into()])
            .unwrap();
        earlier
            .commit(earlier_scope.clone(), &["old".into(), "shared".into()])
            .unwrap();

        assert_eq!(earlier_scope.wait().await.unwrap()["status"], "completed");
        assert_eq!(
            session.snapshot().unwrap().queued_agent_ids,
            ["new", "shared", "old"]
        );
        let (_, selected_scope) = session
            .next_speaker(session.snapshot().unwrap().revision)
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(selected_scope.as_ref().unwrap(), &newer_scope));
        session.close(None);
    }

    #[test]
    fn accepted_input_manual_queue_and_history_changes_preempt_only_the_old_decision() {
        let root = CancellationToken::new();
        let session = GroupSession::new("turn".into(), root.clone(), "continue", true);
        let first = session.decision_context(0).unwrap().unwrap();
        let input = session.reserve().unwrap();
        assert!(first.cancellation.is_cancelled());
        assert!(!root.is_cancelled());
        assert!(!session
            .accept_decision(first.revision, &[], Some("end_option_selected".into()))
            .unwrap());
        session.decision_finished();
        drop(input);

        let second = session
            .decision_context(session.snapshot().unwrap().revision)
            .unwrap()
            .unwrap();
        assert!(!second.cancellation.is_cancelled());
        session.enqueue(&["manual".into()]).unwrap();
        assert!(second.cancellation.is_cancelled());
        assert!(!root.is_cancelled());
        session.decision_finished();
        assert_eq!(
            session
                .next_speaker(session.snapshot().unwrap().revision)
                .unwrap()
                .unwrap()
                .0,
            "manual"
        );
        session
            .speaker_finished(true, Ok(serde_json::json!({"status":"completed"})))
            .unwrap();

        let third = session
            .decision_context(session.snapshot().unwrap().revision)
            .unwrap()
            .unwrap();
        session.invalidate();
        assert!(third.cancellation.is_cancelled());
        session.decision_finished();
        let fourth = session
            .decision_context(session.snapshot().unwrap().revision)
            .unwrap()
            .unwrap();
        assert!(!fourth.cancellation.is_cancelled());
        assert_eq!(
            fourth.rounds, 0,
            "preempted decisions consume no autonomous budget"
        );
        root.cancel();
        assert!(fourth.cancellation.is_cancelled());
    }

    #[test]
    fn arbitration_holds_old_voice_scope_until_its_actual_response_settles() {
        let finished = Arc::new(AtomicUsize::new(0));
        let old = scope("old", CancellationToken::new(), finished.clone());
        let session = GroupSession::new(
            "turn".into(),
            CancellationToken::new(),
            "user_message",
            false,
        );
        session.reserve().unwrap().commit(old, &[]).unwrap();
        let decision = session
            .decision_context(session.snapshot().unwrap().revision)
            .unwrap()
            .unwrap();
        session
            .reserve()
            .unwrap()
            .commit(
                scope(
                    "new",
                    CancellationToken::new(),
                    Arc::new(AtomicUsize::new(0)),
                ),
                &[],
            )
            .unwrap();
        assert_eq!(finished.load(Ordering::SeqCst), 0);
        session.decision_finished();
        assert_eq!(finished.load(Ordering::SeqCst), 1);
        assert!(!session
            .accept_decision(decision.revision, &[], Some("end_option_selected".into()))
            .unwrap());
    }

    #[test]
    fn hard_stop_cancels_all_origins_but_finishes_each_once_after_settlement() {
        let root = CancellationToken::new();
        let finished = Arc::new(AtomicUsize::new(0));
        let current = scope("current", CancellationToken::new(), finished.clone());
        let latest = scope("latest", CancellationToken::new(), finished.clone());
        let session = GroupSession::new("turn".into(), root.clone(), "user_message", false);
        session
            .reserve()
            .unwrap()
            .commit(current.clone(), &[])
            .unwrap();
        session.enqueue(&["member".into()]).unwrap();
        session
            .next_speaker(session.snapshot().unwrap().revision)
            .unwrap()
            .unwrap();
        session
            .reserve()
            .unwrap()
            .commit(latest.clone(), &[])
            .unwrap();
        root.cancel();
        session.cancel_scopes();
        assert!(current.cancellation.is_cancelled());
        assert!(latest.cancellation.is_cancelled());
        assert_eq!(finished.load(Ordering::SeqCst), 0);
        assert!(session.reserve().is_err());
        session.close(Some("cancelled_by_user".into()));
        session.close(None);
        assert_eq!(finished.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn scope_completion_preserves_decision_errors_and_recovery_pending() {
        let session = GroupSession::new(
            "failed".into(),
            CancellationToken::new(),
            "user_message",
            false,
        );
        let reply = scope(
            "voice",
            CancellationToken::new(),
            Arc::new(AtomicUsize::new(0)),
        );
        session
            .reserve()
            .unwrap()
            .commit(reply.clone(), &[])
            .unwrap();
        session.fail("JEV 未配置".into());
        assert_eq!(reply.wait().await.unwrap_err(), "JEV 未配置");
        let session = GroupSession::new(
            "recovery".into(),
            CancellationToken::new(),
            "user_message",
            false,
        );
        let reply = scope(
            "voice2",
            CancellationToken::new(),
            Arc::new(AtomicUsize::new(0)),
        );
        session
            .reserve()
            .unwrap()
            .commit(reply.clone(), &[])
            .unwrap();
        session.close(Some("recovery_pending".into()));
        assert_eq!(reply.wait().await.unwrap()["status"], "recovery_pending");
    }

    #[tokio::test]
    async fn cancellation_during_member_read_revokes_the_unstarted_scope_revision() {
        let session = GroupSession::new(
            "turn".into(),
            CancellationToken::new(),
            "user_message",
            false,
        );
        let cancellation = CancellationToken::new();
        let reply = scope("voice", cancellation.clone(), Arc::new(AtomicUsize::new(0)));
        session
            .reserve()
            .unwrap()
            .commit(reply.clone(), &[])
            .unwrap();
        session.enqueue(&["member".into()]).unwrap();
        let reading_revision = session.snapshot().unwrap().revision;
        cancellation.cancel();
        session.finish_cancelled_scope(&reply);
        assert!(session.next_speaker(reading_revision).unwrap().is_none());
        assert_eq!(reply.wait().await.unwrap()["status"], "cancelled");
    }
}
