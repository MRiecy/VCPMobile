//! Call-scoped, pre-send ASR aggregation. An offer is not a sent chat message;
//! only a successful claim crosses that boundary.
use super::network::MAX_TEXT_BYTES;
use std::{collections::BTreeMap, sync::Mutex};
use tokio_util::sync::CancellationToken;

pub(super) const MAX_BATCH_PARTS: usize = 128;

struct Part {
    active: bool,
    partial: String,
    final_text: Option<String>,
}
struct Batch {
    id: String,
    parts: BTreeMap<u64, Part>,
    offer: Option<Offer>,
    needs_retry: bool,
}
#[derive(Default)]
struct BatchState {
    current: Option<Batch>,
    next_sequence: u64,
    last_claimed: Option<String>,
}

pub(super) struct CallInput {
    pub id: String,
    pub cancel: CancellationToken,
    state: Mutex<BatchState>,
}

#[derive(Clone)]
pub(super) struct Offer {
    pub call_id: String,
    pub offer_id: String,
    pub text: String,
}

pub(super) struct Began {
    pub batch_id: String,
    pub sequence: u64,
    pub barge: Option<String>,
}

pub(super) enum BatchCompletion {
    Pending,
    Empty,
    Offered(Offer),
}

impl CallInput {
    pub fn new(id: String) -> Self {
        Self {
            id,
            cancel: CancellationToken::new(),
            state: Mutex::new(BatchState::default()),
        }
    }

    pub fn begin(&self) -> Result<Began, String> {
        if self.cancel.is_cancelled() {
            return Err(super::network::CANCELLED.into());
        }
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        if state.current.as_ref().is_some_and(|batch| batch.parts.len() >= MAX_BATCH_PARTS) {
            return Err("连续语音片段已达 128 段，请等待当前输入发送；发送失败后可重新提交。".into());
        }
        let barge = if state.current.is_none() {
            state.last_claimed.clone()
        } else {
            None
        };
        if state.current.is_none() {
            state.current = Some(Batch {
                id: uuid::Uuid::new_v4().to_string(),
                parts: BTreeMap::new(),
                offer: None,
                needs_retry: false,
            });
        }
        let sequence = state.next_sequence;
        state.next_sequence = state.next_sequence.saturating_add(1);
        let batch = state
            .current
            .as_mut()
            .ok_or_else(|| "语音聚合状态已失效。".to_string())?;
        batch.offer = None;
        batch.needs_retry = false;
        batch.parts.insert(
            sequence,
            Part {
                active: true,
                partial: String::new(),
                final_text: None,
            },
        );
        Ok(Began {
            batch_id: batch.id.clone(),
            sequence,
            barge,
        })
    }

    pub fn sent_turn(&self) -> Option<String> {
        let state = self.state.lock().ok()?;
        if state.current.is_none() {
            state.last_claimed.clone()
        } else {
            None
        }
    }

    pub fn end_speech(&self, batch_id: &str, sequence: u64) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(part) = state
                .current
                .as_mut()
                .filter(|batch| batch.id == batch_id)
                .and_then(|batch| batch.parts.get_mut(&sequence))
            {
                part.active = false;
            }
        }
    }

    pub fn partial(
        &self,
        batch_id: &str,
        sequence: u64,
        text: String,
    ) -> Result<Option<String>, String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        let Some(batch) = state.current.as_mut().filter(|batch| batch.id == batch_id) else {
            return Ok(None);
        };
        let previous = batch
            .parts
            .get(&sequence)
            .map(|part| part.partial.len())
            .unwrap_or(0);
        if batch
            .parts
            .values()
            .map(|part| part.final_text.as_ref().unwrap_or(&part.partial).len())
            .sum::<usize>()
            .saturating_sub(previous)
            .saturating_add(text.len())
            > MAX_TEXT_BYTES
        {
            return Err("待发送的语音转写过长，请分段重试。".into());
        }
        let Some(part) = batch
            .parts
            .get_mut(&sequence)
            .filter(|part| part.final_text.is_none())
        else {
            return Ok(None);
        };
        part.partial = text;
        Ok(Some(join_parts(batch)))
    }

    pub fn complete(
        &self,
        batch_id: &str,
        sequence: u64,
        text: String,
    ) -> Result<BatchCompletion, String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        let Some(batch) = state.current.as_mut().filter(|batch| batch.id == batch_id) else {
            return Ok(BatchCompletion::Pending);
        };
        let Some(part) = batch.parts.get_mut(&sequence) else {
            return Ok(BatchCompletion::Pending);
        };
        part.final_text = Some(text);
        part.partial.clear();
        part.active = false;
        let text = join_parts(batch);
        if text.len() > MAX_TEXT_BYTES {
            return Err("待发送的语音转写过长，请分段重试。".into());
        }
        if batch.needs_retry
            || batch
                .parts
                .values()
                .any(|part| part.active || part.final_text.is_none())
            || batch.offer.is_some()
        {
            return Ok(BatchCompletion::Pending);
        }
        if text.trim().is_empty() {
            state.current = None;
            return Ok(BatchCompletion::Empty);
        }
        let offer = Offer {
            call_id: self.id.clone(),
            offer_id: uuid::Uuid::new_v4().to_string(),
            text,
        };
        batch.offer = Some(offer.clone());
        Ok(BatchCompletion::Offered(offer))
    }

    pub fn fail(&self, batch_id: &str, sequence: u64) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(batch) = state.current.as_mut().filter(|batch| batch.id == batch_id) {
                batch.parts.remove(&sequence);
                batch.offer = None;
                // Preserve already recognized preceding speech. A fresh spoken
                // retry can complete it; failure must not silently send a gap.
                batch.needs_retry = true;
            }
        }
    }

    pub fn has_offer(&self, offer_id: &str) -> bool {
        self.state.lock().is_ok_and(|state| {
            state
                .current
                .as_ref()
                .and_then(|batch| batch.offer.as_ref())
                .is_some_and(|offer| offer.offer_id == offer_id && !self.cancel.is_cancelled())
        })
    }

    pub fn offered(&self, offer_id: &str) -> Result<Option<Offer>, String> {
        let state = self.state.lock().map_err(|error| error.to_string())?;
        Ok(state.current.as_ref().and_then(|batch| batch.offer.as_ref())
            .filter(|offer| offer.offer_id == offer_id && !self.cancel.is_cancelled())
            .cloned())
    }

    /// Only the call coordinator uses this, while holding its admission lock.
    pub fn claim(&self, offer_id: &str) -> Result<bool, String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        let current = state
            .current
            .as_ref()
            .and_then(|batch| batch.offer.as_ref())
            .filter(|offer| offer.offer_id == offer_id && !self.cancel.is_cancelled())
            .cloned();
        if let Some(offer) = current {
            state.current = None;
            state.last_claimed = Some(offer.offer_id.clone());
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

fn join_parts(batch: &Batch) -> String {
    batch
        .parts
        .values()
        .map(|part| part.final_text.as_ref().unwrap_or(&part.partial).trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn offered(completion: BatchCompletion) -> Offer {
        match completion {
            BatchCompletion::Offered(offer) => offer,
            _ => panic!("expected a transcript offer"),
        }
    }
    #[test]
    fn out_of_order_recognition_merges_in_recording_order_and_claim_is_unique() {
        let call = CallInput::new("call".into());
        let first = call.begin().unwrap();
        call.end_speech(&first.batch_id, first.sequence);
        let second = call.begin().unwrap();
        call.end_speech(&second.batch_id, second.sequence);
        assert!(second.barge.is_none());
        assert!(matches!(
            call.complete(&second.batch_id, second.sequence, "第二句".into())
                .unwrap(),
            BatchCompletion::Pending
        ));
        let offer = offered(
            call.complete(&first.batch_id, first.sequence, "第一句".into())
                .unwrap(),
        );
        assert_eq!(offer.text, "第一句 第二句");
        assert!(call.claim(&offer.offer_id).unwrap());
        assert!(!call.claim(&offer.offer_id).unwrap());
        assert_eq!(call.begin().unwrap().barge, Some(offer.offer_id));
    }
    #[test]
    fn speech_before_claim_invalidates_offer_without_losing_its_text() {
        let call = CallInput::new("call".into());
        let first = call.begin().unwrap();
        let old = offered(
            call.complete(&first.batch_id, first.sequence, "前半句".into())
                .unwrap(),
        );
        let more = call.begin().unwrap();
        assert!(more.barge.is_none());
        assert!(!call.claim(&old.offer_id).unwrap());
        let new = offered(
            call.complete(&more.batch_id, more.sequence, "后半句".into())
                .unwrap(),
        );
        assert_ne!(new.offer_id, old.offer_id);
        assert_eq!(new.text, "前半句 后半句");
    }

    #[test]
    fn completed_empty_recognition_does_not_fill_up_future_batches() {
        let call = CallInput::new("call".into());
        for _ in 0..100 {
            let speech = call.begin().unwrap();
            assert!(matches!(
                call.complete(&speech.batch_id, speech.sequence, "  ".into())
                    .unwrap(),
                BatchCompletion::Empty
            ));
        }
        let speech = call.begin().unwrap();
        let offer = offered(
            call.complete(&speech.batch_id, speech.sequence, "真正的下一句".into())
                .unwrap(),
        );
        assert_eq!(offer.text, "真正的下一句");
    }

    #[test]
    fn saturation_rejects_only_new_speech_and_accepted_recognition_can_still_be_claimed() {
        let call = CallInput::new("call".into());
        let parts: Vec<_> = (0..MAX_BATCH_PARTS).map(|_| call.begin().unwrap()).collect();
        assert_eq!(parts.len(), 128);
        assert!(call.begin().is_err());
        for part in &parts[..MAX_BATCH_PARTS - 1] {
            assert!(matches!(call.complete(&part.batch_id, part.sequence, "已接受".into()).unwrap(),
                BatchCompletion::Pending));
        }
        let last = &parts[MAX_BATCH_PARTS - 1];
        let ready = offered(call.complete(&last.batch_id, last.sequence, "末句".into()).unwrap());
        assert_eq!(ready.text.split_whitespace().count(), 128);
        // Rejection must also preserve an offer that already exists, including
        // when reply preparation has not reached the claim boundary yet.
        assert!(call.begin().is_err());
        assert!(call.has_offer(&ready.offer_id));
        assert!(call.claim(&ready.offer_id).unwrap());
        let next = call.begin().unwrap();
        assert_eq!(next.sequence, 128);
        assert_ne!(next.batch_id, last.batch_id);
        assert_eq!(offered(call.complete(&next.batch_id, next.sequence, "下一批".into()).unwrap()).text, "下一批");
    }
}
