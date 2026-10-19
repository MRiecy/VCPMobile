use super::{
    network::MAX_TEXT_BYTES,
    readable,
    segmentation::{ReplyBuffer, ReplyPiece},
};
use crate::vcp_modules::{chat::voice_guidance, vcp_client::StreamEvent};
use std::sync::Arc;

struct MessageText {
    id: String,
    text: String,
    reply: ReplyBuffer,
    instruction: Option<Arc<str>>,
    guidance_locked: bool,
    finished: bool,
    dirty: bool,
}

impl MessageText {
    fn lock_guidance(
        &mut self,
        auto_guidance: bool,
        projection: &voice_guidance::GuidanceProjection,
    ) {
        if self.guidance_locked {
            return;
        }
        if !auto_guidance {
            self.guidance_locked = true;
            return;
        }
        let mut prefix = String::new();
        let mut previous = 0;
        for block in &projection.blocks {
            prefix.push_str(&self.text[previous..block.range.start]);
            previous = block.range.end;
            if !readable::text(&prefix).is_empty() {
                break;
            }
            if block.valid {
                self.instruction = Some(Arc::from(block.content.as_str()));
                self.guidance_locked = true;
                break;
            }
        }
        self.guidance_locked |= !readable::text(&projection.body).is_empty() || self.finished;
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct SpeechPiece {
    pub message_id: String,
    pub text: String,
    pub instruction: Option<Arc<str>>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct FeedUpdate {
    pub pieces: Vec<SpeechPiece>,
    pub finished: bool,
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
    auto_guidance: bool,
    inherit_global_style: bool,
    cursor: usize,
}

impl ChatFeed {
    pub(super) fn new(auto_guidance: bool, inherit_global_style: bool) -> Self {
        Self {
            auto_guidance,
            inherit_global_style,
            ..Default::default()
        }
    }

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
                    reply: ReplyBuffer::new(self.inherit_global_style),
                    instruction: None,
                    guidance_locked: false,
                    finished: false,
                    dirty: true,
                });
                self.messages.len() - 1
            });
            if let Some(text) = replacement {
                self.messages[index].text = text.into();
            } else if let Some(text) = delta {
                self.messages[index].text.push_str(text);
            }
            self.messages[index].dirty = true;
            self.dirty = true;
        }
        if normal_end {
            if let Some(message) = self
                .messages
                .iter_mut()
                .find(|message| message.id == event.message_id)
            {
                message.finished = true;
                message.dirty = true;
            }
            self.dirty = true;
        }
        if let Some(message) = self
            .messages
            .iter_mut()
            .find(|message| message.id == event.message_id && !message.guidance_locked)
        {
            // Lock on output admission, not when the playback worker happens
            // to be free. Network latency must not choose a later instruction.
            let projection = voice_guidance::scan(&message.text, message.finished);
            message.lock_guidance(self.auto_guidance, &projection);
        }
        if normal_end && !self.group {
            self.finished = true;
            self.dirty = true;
        }
        Ok(self.dirty)
    }

    pub(super) fn take_update(&mut self) -> Result<Option<FeedUpdate>, String> {
        if !std::mem::take(&mut self.dirty) {
            return Ok(None);
        }
        for message in &mut self.messages {
            if self.finished && !message.finished {
                message.finished = true;
                message.dirty = true;
            }
            if !std::mem::take(&mut message.dirty) {
                continue;
            }
            let projection = voice_guidance::scan(&message.text, message.finished);
            if projection.exhausted {
                return Err("语音指导格式过于复杂，已停止本次朗读；聊天正文已保留。".into());
            }
            let text = readable::project(&projection.body, message.finished)?;
            message.lock_guidance(self.auto_guidance, &projection);
            message.reply.update(text, message.finished)?;
        }
        let mut pieces = Vec::new();
        // A member's unfinished tail must not be prepended to the next member,
        // nor may later members interleave with that member's speech.
        while let Some(message) = self.messages.get_mut(self.cursor) {
            match message.reply.next() {
                ReplyPiece::Text(text) => pieces.push(SpeechPiece {
                    message_id: message.id.clone(),
                    text,
                    instruction: message.instruction.clone(),
                }),
                ReplyPiece::Wait => break,
                ReplyPiece::Finished => self.cursor += 1,
            }
        }
        Ok(Some(FeedUpdate {
            pieces,
            finished: self.finished && self.cursor == self.messages.len(),
        }))
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

    fn piece(id: &str, text: &str, instruction: Option<&str>) -> SpeechPiece {
        SpeechPiece {
            message_id: id.into(),
            text: text.into(),
            instruction: instruction.map(Arc::from),
        }
    }

    fn member(id: &str, text: &str) -> StreamEvent {
        StreamEvent::end(
            id.into(),
            Some(json!({"isGroupMessage":true})),
            Some("completed".into()),
            Some(text.into()),
            None,
            None,
            None,
        )
    }

    fn update(id: &str, text: &str, group: bool) -> StreamEvent {
        // A snapshot replacement uses the same event branch as production
        // Aurora snapshots, without duplicating its unrelated UI payload.
        let mut event = member(id, text);
        event.finish_reason = Some("in_progress".into());
        event.aurora = Some(
            serde_json::from_value(json!({
                "kind":"snapshot", "content":text,
            }))
            .unwrap(),
        );
        event.context = group.then(|| json!({"isGroupMessage":true}));
        event
    }

    fn guidance(text: &str) -> String {
        format!(
            "{}\n{text}\n{}\n",
            voice_guidance::GUIDANCE_START,
            voice_guidance::GUIDANCE_END
        )
    }

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
        assert_eq!(
            feed.take_update().unwrap(),
            Some(FeedUpdate {
                pieces: vec![piece("new", "新消息。", None)],
                finished: true
            })
        );
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
        assert_eq!(
            feed.take_update().unwrap(),
            Some(FeedUpdate {
                pieces: vec![piece("a", "第一位。", None)],
                finished: false
            })
        );
        feed.accept(&member("b", "第二位。")).unwrap();
        assert_eq!(
            feed.take_update().unwrap(),
            Some(FeedUpdate {
                pieces: vec![piece("b", "第二位。", None)],
                finished: false
            })
        );
        feed.accept(&StreamEvent {
            r#type: "group_turn".into(),
            context: Some(json!({"turnId":"group-turn"})),
            chunk: Some(json!({"active":false})),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            feed.take_update().unwrap(),
            Some(FeedUpdate {
                pieces: vec![],
                finished: true
            })
        );
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
        assert!(feed.take_update().unwrap().is_none());
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
        assert_eq!(
            feed.take_update().unwrap(),
            Some(FeedUpdate {
                pieces: vec![piece("member", "实际回复。", None)],
                finished: false
            })
        );
        feed.accept(&group_event("invited", false)).unwrap();
        assert_eq!(
            feed.take_update().unwrap(),
            Some(FeedUpdate {
                pieces: vec![],
                finished: true
            })
        );
    }

    #[test]
    fn explicit_chat_finish_seals_an_empty_job_without_overwriting_text() {
        let mut feed = ChatFeed::default();
        feed.finish_submission();
        assert_eq!(
            feed.take_update().unwrap(),
            Some(FeedUpdate {
                pieces: vec![],
                finished: true
            })
        );
    }

    #[test]
    fn each_group_message_owns_its_guidance_style_and_unpunctuated_tail() {
        let mut feed = ChatFeed::new(true, true);
        feed.accept(&member(
            "a",
            &format!("{}(温柔)[轻笑]第一句。第一位尾音", guidance("清亮自然")),
        ))
        .unwrap();
        feed.accept(&member("b", &format!("{}第二位。", guidance("沉稳缓慢"))))
            .unwrap();
        let result = feed.take_update().unwrap().unwrap();
        assert_eq!(
            result.pieces,
            vec![
                piece("a", "(温柔)[轻笑]第一句。", Some("清亮自然")),
                piece("a", "(温柔)第一位尾音", Some("清亮自然")),
                piece("b", "第二位。", Some("沉稳缓慢")),
            ]
        );
        assert!(!result.finished);
    }

    #[test]
    fn partial_guidance_is_never_spoken_and_the_first_valid_instruction_stays_fixed() {
        let mut feed = ChatFeed::new(true, true);
        feed.accept(&update("a", "<<<[VOICE_GUID", false)).unwrap();
        assert!(feed.take_update().unwrap().unwrap().pieces.is_empty());
        let raw = format!("{}第一句。", guidance("首个指导"));
        feed.accept(&update("a", &raw, false)).unwrap();
        assert_eq!(
            feed.take_update().unwrap().unwrap().pieces,
            vec![piece("a", "第一句。", Some("首个指导"))]
        );
        let raw = format!("{raw}\n{}第二句。", guidance("迟到指导"));
        feed.accept(&update("a", &raw, false)).unwrap();
        assert_eq!(
            feed.take_update().unwrap().unwrap().pieces,
            vec![piece("a", "第二句。", Some("首个指导"))]
        );
    }

    #[test]
    fn missing_late_invalid_or_disabled_guidance_yields_none_for_the_configured_fallback() {
        let cases = [
            (true, "普通正文。".to_owned()),
            (true, format!("正文。\n{}", guidance("迟到"))),
            (true, format!("{}正文。", guidance(""))),
            (
                true,
                format!(
                    "{}正文。",
                    guidance(&"字".repeat(voice_guidance::MAX_GUIDANCE_CHARS + 1))
                ),
            ),
            (false, format!("{}正文。", guidance("自动开关已关闭"))),
        ];
        for (enabled, raw) in cases {
            let mut feed = ChatFeed::new(enabled, true);
            feed.accept(&member("a", &raw)).unwrap();
            let pieces = feed.take_update().unwrap().unwrap().pieces;
            assert_eq!(pieces.len(), 1);
            assert!(pieces[0].instruction.is_none());
            assert!(!pieces[0].text.contains("VOICE_GUIDANCE"));
        }
    }

    #[test]
    fn opaque_prefix_does_not_count_as_the_start_of_spoken_prose() {
        let mut feed = ChatFeed::new(true, true);
        let raw = format!(
            "<think>私密推理</think>\n```text\n示例代码\n```\n{}正文。",
            guidance("有效指导")
        );
        feed.accept(&member("a", &raw)).unwrap();
        assert_eq!(
            feed.take_update().unwrap().unwrap().pieces,
            vec![piece("a", "正文。", Some("有效指导"))]
        );
    }

    #[test]
    fn group_messages_cannot_interleave_or_rewrite_a_spoken_prefix() {
        let mut feed = ChatFeed::new(true, true);
        feed.accept(&update(
            "a",
            &format!("{}第一句。等尾句", guidance("第一位")),
            true,
        ))
        .unwrap();
        feed.accept(&member("b", &format!("{}第二位。", guidance("第二位"))))
            .unwrap();
        assert_eq!(
            feed.take_update().unwrap().unwrap().pieces,
            vec![piece("a", "第一句。", Some("第一位"))]
        );
        feed.accept(&member(
            "a",
            &format!("{}第一句。等尾句完成。", guidance("第一位")),
        ))
        .unwrap();
        assert_eq!(
            feed.take_update().unwrap().unwrap().pieces,
            vec![
                piece("a", "等尾句完成。", Some("第一位")),
                piece("b", "第二位。", Some("第二位")),
            ]
        );
        feed.accept(&member("a", "改写已播内容。")).unwrap();
        assert!(feed.take_update().is_err());
    }

    #[test]
    fn first_body_locks_even_while_the_playback_worker_has_not_consumed_a_frame() {
        let mut feed = ChatFeed::new(true, true);
        feed.accept(&update("a", "正文已经开始", false)).unwrap();
        // A later snapshot can replace an unspoken tail, but cannot change
        // which guidance this response fixed when its body first arrived.
        feed.accept(&update(
            "a",
            &format!("{}正文已经开始。", guidance("后来补写")),
            false,
        ))
        .unwrap();
        assert_eq!(
            feed.take_update().unwrap().unwrap().pieces,
            vec![piece("a", "正文已经开始。", None)]
        );
    }

    #[test]
    fn invalid_guidance_cannot_promote_a_subsequent_tool_example() {
        let first = guidance(&format!(
            "{}\n<<<[TOOL_REQUEST]>>>",
            "字".repeat(voice_guidance::MAX_GUIDANCE_CHARS + 1)
        ));
        let raw = format!(
            "{first}<<<[TOOL_REQUEST]>>>\n{}<<<[END_TOOL_REQUEST]>>>\n正文。",
            guidance("工具内示例")
        );
        let mut feed = ChatFeed::new(true, true);
        feed.accept(&member("a", &raw)).unwrap();
        assert_eq!(
            feed.take_update().unwrap().unwrap().pieces,
            vec![piece("a", "正文。", None)]
        );
    }

    #[test]
    fn recovery_budget_failure_stops_speech_without_changing_stored_text() {
        let raw = format!("{}正常正文。", guidance("自然\n<think>").repeat(18));
        let mut feed = ChatFeed::new(true, true);
        feed.accept(&member("a", &raw)).unwrap();
        assert!(feed
            .take_update()
            .unwrap_err()
            .contains("语音指导格式过于复杂"));
        assert_eq!(feed.messages[0].text, raw);
        assert!(voice_guidance::strip_guidance_for_history(&raw).ends_with("正常正文。"));
    }

    #[test]
    fn opaque_examples_neither_hide_real_guidance_nor_swallow_following_speech() {
        for prefix in [
            "<<<[TOOL_REQUEST]>>>\ncontent:「始」<think>「末」\n<<<[END_TOOL_REQUEST]>>>\n",
            "<think>\n<<<[TOOL_REQUEST]>>>\n</think>\n",
            "```text\n<<<[TOOL_REQUEST]>>>\n```\n",
            "<<<[TOOL_REQUEST]>>>\ncontent:「始」\n```\n示例\n「末」\n<<<[END_TOOL_REQUEST]>>>\n",
        ] {
            let raw = format!("{prefix}{}正文。", guidance("真正指导"));
            let mut feed = ChatFeed::new(true, true);
            feed.accept(&member("a", &raw)).unwrap();
            assert_eq!(
                feed.take_update().unwrap().unwrap().pieces,
                vec![piece("a", "正文。", Some("真正指导"))]
            );
        }
    }
}
