use super::{network::MAX_TEXT_BYTES, readable};
use crate::vcp_modules::vcp_client::StreamEvent;

struct MessageText {
    id: String,
    text: String,
}

#[derive(Default)]
pub(super) struct ChatFeed {
    messages: Vec<MessageText>,
    group: bool,
    group_turn: Option<String>,
    agent_message: Option<String>,
    agent_request: Option<String>,
    finished: bool,
    dirty: bool,
}

impl ChatFeed {
    pub(super) fn accept(&mut self, event: &StreamEvent) -> Result<bool, String> {
        let context = event.context.as_ref();
        if context.and_then(|value| value["replayed"].as_bool()) == Some(true) || self.finished {
            return Ok(false);
        }
        self.group |= event.r#type == "group_turn"
            || context.is_some_and(|value| {
                value["ownerType"] == "group" || value["isGroupMessage"] == true
            });
        if event.r#type == "group_turn" {
            let Some(id) = context.and_then(|value| value["turnId"].as_str()) else {
                return Ok(false);
            };
            if self
                .group_turn
                .as_deref()
                .is_some_and(|current| current != id)
            {
                return Ok(false);
            }
            self.group_turn.get_or_insert_with(|| id.into());
            if event
                .chunk
                .as_ref()
                .and_then(|value| value["active"].as_bool())
                == Some(false)
            {
                if self.messages.is_empty() {
                    // invite_only has an initial empty persistence turn before
                    // the separately invited members produce their replies.
                    self.group_turn = None;
                } else {
                    self.finished = true;
                    self.dirty = true;
                }
            }
            return Ok(self.dirty);
        }
        if event.r#type == "request_activity" {
            if self.group {
                return Ok(false);
            }
            let Some(id) = context.and_then(|value| value["requestId"].as_str()) else {
                return Ok(false);
            };
            if self
                .agent_request
                .as_deref()
                .is_some_and(|current| current != id)
            {
                return Ok(false);
            }
            self.agent_request.get_or_insert_with(|| id.into());
            if event
                .chunk
                .as_ref()
                .and_then(|value| value["active"].as_bool())
                == Some(false)
            {
                self.finished = true;
                self.dirty = true;
            }
            return Ok(self.dirty);
        }
        if event.message_id.is_empty() {
            return Ok(false);
        }
        if !self.group {
            if self
                .agent_message
                .as_deref()
                .is_some_and(|id| id != event.message_id)
            {
                return Ok(false);
            }
            self.agent_message
                .get_or_insert_with(|| event.message_id.clone());
        }
        if !matches!(event.r#type.as_str(), "aurora" | "end") {
            return Ok(false);
        }
        let normal_end = event.r#type == "end"
            && event
                .finish_reason
                .as_deref()
                .is_none_or(|reason| matches!(reason, "completed" | "stop" | "length"));
        let replacement = if normal_end {
            event.content.as_deref()
        } else {
            event
                .aurora
                .as_ref()
                .and_then(|update| update.content.as_deref())
        };
        let delta = event
            .aurora
            .as_ref()
            .and_then(|update| update.chunk.as_deref());
        if replacement.is_some() || delta.is_some() {
            let index = self
                .messages
                .iter()
                .position(|message| message.id == event.message_id);
            let previous = index
                .map(|index| self.messages[index].text.len())
                .unwrap_or(0);
            let next_size = replacement
                .map(str::len)
                .unwrap_or_else(|| previous.saturating_add(delta.map(str::len).unwrap_or(0)));
            if self
                .messages
                .iter()
                .map(|message| message.text.len())
                .sum::<usize>()
                .saturating_sub(previous)
                .saturating_add(next_size)
                > MAX_TEXT_BYTES
            {
                return Err("模型回复超过语音朗读大小限制，已停止朗读。".into());
            }
            let index = index.unwrap_or_else(|| {
                self.messages.push(MessageText {
                    id: event.message_id.clone(),
                    text: String::new(),
                });
                self.messages.len() - 1
            });
            if let Some(text) = replacement {
                self.messages[index].text = text.into();
            } else if let Some(text) = delta {
                self.messages[index].text.push_str(text);
            }
            self.dirty = true;
        }
        if normal_end && !self.group {
            self.finished = true;
            self.dirty = true;
        }
        Ok(self.dirty)
    }

    pub(super) fn take_update(&mut self) -> Option<(String, bool)> {
        if !std::mem::take(&mut self.dirty) {
            return None;
        }
        let text = self
            .messages
            .iter()
            .map(|message| readable::text(&message.text))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        Some((text, self.finished))
    }

    pub(super) fn finish_submission(&mut self) {
        self.finished = true;
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_replayed_message_and_another_agent_request_do_not_enter_the_narration() {
        let mut feed = ChatFeed::default();
        let mut replay = StreamEvent::end(
            "old".into(),
            Some(json!({"replayed":true})),
            Some("completed".into()),
            Some("旧消息".into()),
            None,
            None,
            None,
        );
        assert!(!feed.accept(&replay).unwrap());
        replay.context = None;
        replay.message_id = "new".into();
        replay.content = Some("新消息。".into());
        feed.accept(&replay).unwrap();
        assert_eq!(feed.take_update(), Some(("新消息。".into(), true)));
        replay.message_id = "regenerated".into();
        assert!(!feed.accept(&replay).unwrap());
    }

    #[test]
    fn group_member_ends_do_not_finish_the_shared_tts_job() {
        let mut feed = ChatFeed::default();
        let member = |id: &str, text: &str| {
            StreamEvent::end(
                id.into(),
                Some(json!({"isGroupMessage":true})),
                Some("completed".into()),
                Some(text.into()),
                None,
                None,
                None,
            )
        };
        feed.accept(&member("a", "第一位。")).unwrap();
        assert_eq!(feed.take_update(), Some(("第一位。".into(), false)));
        feed.accept(&member("b", "第二位。")).unwrap();
        assert_eq!(
            feed.take_update(),
            Some(("第一位。 第二位。".into(), false))
        );
        feed.accept(&StreamEvent {
            r#type: "group_turn".into(),
            context: Some(json!({"turnId":"group-turn"})),
            chunk: Some(json!({"active":false})),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(feed.take_update(), Some(("第一位。 第二位。".into(), true)));
    }

    #[test]
    fn an_empty_invite_only_persistence_turn_allows_the_actual_invited_turn() {
        let mut feed = ChatFeed::default();
        let group_event = |id: &str, active: bool| StreamEvent {
            r#type: "group_turn".into(),
            context: Some(json!({"turnId":id})),
            chunk: Some(json!({"active":active})),
            ..Default::default()
        };
        feed.accept(&group_event("empty", true)).unwrap();
        feed.accept(&group_event("empty", false)).unwrap();
        assert!(feed.take_update().is_none());
        feed.accept(&group_event("invited", true)).unwrap();
        feed.accept(&StreamEvent::end(
            "member".into(),
            Some(json!({"isGroupMessage":true})),
            Some("completed".into()),
            Some("实际回复。".into()),
            None,
            None,
            None,
        ))
        .unwrap();
        assert_eq!(feed.take_update(), Some(("实际回复。".into(), false)));
        feed.accept(&group_event("invited", false)).unwrap();
        assert_eq!(feed.take_update(), Some(("实际回复。".into(), true)));
    }

    #[test]
    fn explicit_chat_finish_seals_an_empty_job_without_overwriting_text() {
        let mut feed = ChatFeed::default();
        feed.finish_submission();
        assert_eq!(feed.take_update(), Some((String::new(), true)));
    }
}
