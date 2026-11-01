//! Navigation and voice reservation share the call registry's admission lock.
//! This state contains no database handles, native resources or event listeners.
use super::topic_types::{OwnerKey, TopicKey};
use serde::{Deserialize, Serialize};

pub(crate) const STALE: &str = "stale_navigation";
pub(crate) const CALL_IN_PROGRESS: &str = "call_in_progress";
pub(crate) const DELETING: &str = "target_deleting";

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FocusTarget {
    #[default]
    Welcome,
    Owner {
        owner_type: String,
        owner_id: String,
    },
    Topic {
        owner_type: String,
        owner_id: String,
        topic_id: String,
    },
}

impl FocusTarget {
    pub fn matches_topic(&self, topic: &TopicKey) -> bool {
        matches!(self, Self::Topic { owner_type, owner_id, topic_id }
            if owner_type == &topic.owner_type && owner_id == &topic.owner_id && topic_id == &topic.topic_id)
    }

    pub(crate) fn scope(&self) -> Option<DeleteScope> {
        match self {
            Self::Welcome => None,
            Self::Owner {
                owner_type,
                owner_id,
            } => Some(DeleteScope::Owner(OwnerKey::new(owner_type, owner_id))),
            Self::Topic {
                owner_type,
                owner_id,
                topic_id,
            } => Some(DeleteScope::Topic(TopicKey::new(
                owner_type, owner_id, topic_id,
            ))),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusSnapshot {
    pub target: FocusTarget,
    pub focus_revision: u64,
    pub window_revision: u64,
    pub anchor_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DeleteScope {
    Owner(OwnerKey),
    Topic(TopicKey),
}

impl DeleteScope {
    pub(crate) fn contains(&self, topic: &TopicKey) -> bool {
        match self {
            Self::Owner(owner) => {
                owner.owner_type == topic.owner_type && owner.owner_id == topic.owner_id
            }
            Self::Topic(key) => key == topic,
        }
    }

    fn overlaps(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Owner(a), Self::Owner(b)) => a == b,
            (_, Self::Topic(topic)) => self.contains(topic),
            (Self::Topic(topic), _) => other.contains(topic),
        }
    }
}

#[derive(Default)]
pub(crate) struct Navigation {
    snapshot: FocusSnapshot,
    issued: u64,
    document: u64,
    sequence: u64,
    // None after completion or invalidation. Some(None) can select welcome.
    pending: Option<Option<DeleteScope>>,
    // A newer user action can still be missing or rejected. Until an action
    // actually changes focus, cold restore can establish the initial view.
    restoring: Option<Option<DeleteScope>>,
    deleting: Vec<DeleteScope>,
}

impl Navigation {
    pub(crate) fn attach(&mut self) -> (u64, FocusSnapshot) {
        // A dead WebView can leave its allocation IPC in flight. Only a view
        // which receives the ticket can adopt it on its next request.
        self.issued += 1;
        (self.issued, self.snapshot())
    }

    pub(crate) fn snapshot(&self) -> FocusSnapshot {
        self.snapshot.clone()
    }

    fn adopt(&mut self, document: u64) -> Result<(), String> {
        if document == 0 || document > self.issued || document < self.document {
            return Err(STALE.into());
        }
        if document != self.document {
            self.document = document;
            self.sequence = 0;
            self.pending = None;
            self.restoring = None;
        }
        Ok(())
    }

    pub(crate) fn document_focus(&mut self, document: u64) -> Result<FocusSnapshot, String> {
        self.adopt(document)?;
        Ok(self.snapshot())
    }

    pub(crate) fn current_document_focus(&self, document: u64) -> Result<FocusSnapshot, String> {
        if document == 0 || self.document != document {
            return Err(STALE.into());
        }
        Ok(self.snapshot())
    }

    pub(crate) fn begin_restore(
        &mut self,
        document: u64,
        scope: Option<DeleteScope>,
    ) -> Result<FocusSnapshot, String> {
        self.adopt(document)?;
        if self.snapshot.focus_revision == 0 {
            self.check_deleting(scope.as_ref())?;
            self.restoring = Some(scope);
        }
        Ok(self.snapshot())
    }

    pub(crate) fn commit_restore(
        &mut self,
        document: u64,
        target: FocusTarget,
    ) -> Result<FocusSnapshot, String> {
        self.current_document_focus(document)?;
        if self.snapshot.focus_revision == 0 {
            if self.restoring.take().is_none() {
                return Err(STALE.into());
            }
            self.check_deleting(target.scope().as_ref())?;
            self.set(target, None);
        }
        Ok(self.snapshot())
    }

    pub(crate) fn begin(&mut self, document: u64, sequence: u64) -> Result<FocusSnapshot, String> {
        self.adopt(document)?;
        if sequence <= self.sequence {
            return Err(STALE.into());
        }
        self.sequence = sequence;
        self.pending = Some(None);
        Ok(self.snapshot())
    }

    fn check_intent(&self, document: u64, sequence: u64) -> Result<(), String> {
        if self.document != document || self.sequence != sequence || self.pending.is_none() {
            return Err(STALE.into());
        }
        Ok(())
    }

    fn check_deleting(&self, scope: Option<&DeleteScope>) -> Result<(), String> {
        if scope.is_some_and(|scope| self.deleting.iter().any(|item| item.overlaps(scope))) {
            return Err(DELETING.into());
        }
        Ok(())
    }

    pub(crate) fn validate(
        &mut self,
        document: u64,
        sequence: u64,
        scope: Option<DeleteScope>,
    ) -> Result<(), String> {
        self.check_intent(document, sequence)?;
        self.check_deleting(scope.as_ref())?;
        self.pending = Some(scope);
        Ok(())
    }

    pub(crate) fn commit(
        &mut self,
        document: u64,
        sequence: u64,
        target: FocusTarget,
        anchor: Option<String>,
        call: Option<&TopicKey>,
    ) -> Result<FocusSnapshot, String> {
        self.check_intent(document, sequence)?;
        self.pending = None;
        if call.is_some_and(|call| !target.matches_topic(call)) {
            return Err(CALL_IN_PROGRESS.into());
        }
        self.check_deleting(target.scope().as_ref())?;
        self.set(target, anchor);
        Ok(self.snapshot())
    }

    fn set(&mut self, target: FocusTarget, anchor: Option<String>) {
        self.restoring = None;
        if self.snapshot.focus_revision == 0 || self.snapshot.target != target {
            self.snapshot.focus_revision += 1;
        }
        self.snapshot.target = target;
        self.snapshot.window_revision += 1;
        self.snapshot.anchor_id = anchor;
    }

    pub(crate) fn check_call(
        &self,
        document: u64,
        revision: u64,
        target: &TopicKey,
    ) -> Result<(), String> {
        if document == 0
            || self.document != document
            || self.snapshot.focus_revision != revision
            || !self.snapshot.target.matches_topic(target)
        {
            return Err(STALE.into());
        }
        if self.deleting.iter().any(|scope| scope.contains(target)) {
            return Err(DELETING.into());
        }
        Ok(())
    }

    pub(crate) fn reserve_delete(
        &mut self,
        scope: DeleteScope,
        call: Option<&TopicKey>,
    ) -> Result<(), String> {
        if call.is_some_and(|target| scope.contains(target)) {
            return Err(CALL_IN_PROGRESS.into());
        }
        self.check_deleting(Some(&scope))?;
        self.deleting.push(scope);
        Ok(())
    }

    pub(crate) fn release_delete(&mut self, scope: &DeleteScope) {
        self.deleting.retain(|entry| entry != scope);
    }

    pub(crate) fn deleted(&mut self, scope: &DeleteScope) -> Option<FocusSnapshot> {
        if self
            .restoring
            .as_ref()
            .and_then(Option::as_ref)
            .is_some_and(|target| scope.overlaps(target))
        {
            self.restoring = None;
        }
        if self
            .pending
            .as_ref()
            .and_then(Option::as_ref)
            .is_some_and(|target| scope.overlaps(target))
        {
            self.pending = None;
        }
        let target = match (&self.snapshot.target, scope) {
            (
                FocusTarget::Topic {
                    owner_type,
                    owner_id,
                    ..
                },
                DeleteScope::Topic(topic),
            ) if self.snapshot.target.matches_topic(topic) => FocusTarget::Owner {
                owner_type: owner_type.clone(),
                owner_id: owner_id.clone(),
            },
            (target, DeleteScope::Owner(_))
                if target
                    .scope()
                    .is_some_and(|current| scope.overlaps(&current)) =>
            {
                FocusTarget::Welcome
            }
            _ => return None,
        };
        self.set(target, None);
        Some(self.snapshot())
    }
}
