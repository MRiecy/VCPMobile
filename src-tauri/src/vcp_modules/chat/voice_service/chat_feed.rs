use super::{
    network::MAX_TEXT_BYTES,
    readable,
    segmentation::{ReplyBuffer, ReplyPiece},
};
use crate::vcp_modules::{
    chat::stream_block_parser::{StreamBlock, StreamBlockParser},
    settings_manager::VOICE_GUIDANCE_MAX_CHARS,
    vcp_client::StreamEvent,
};
use std::sync::Arc;

const TRUNCATION_NOTICE: &str = "语音内容过长，剩余内容已截断";

fn bounded_prefix(text: &str, byte_limit: usize) -> &str {
    let mut end = byte_limit.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// One message owns its parser, accepted guidance, readable text and emission
/// position. Both call replies and complete bubble narration use this state.
pub(super) struct MessageSpeech {
    text: String,
    parser: StreamBlockParser,
    stable_spoken: String,
    reply: ReplyBuffer,
    instruction: Option<Arc<str>>,
    guidance_locked: bool,
    finished: bool,
    truncated: bool,
    truncation_notice_pending: bool,
}

impl MessageSpeech {
    pub(super) fn new() -> Self {
        Self {
            text: String::new(),
            parser: StreamBlockParser::new(),
            stable_spoken: String::new(),
            reply: ReplyBuffer::new(true),
            instruction: None,
            guidance_locked: false,
            finished: false,
            truncated: false,
            truncation_notice_pending: false,
        }
    }

    pub(super) fn update(&mut self, text: &str, finished: bool) -> Result<(), String> {
        // Bubble narration shares the raw-input cap with chat narration. Its
        // fixed notice is a separate piece, outside the accepted-text budget.
        if self.truncated {
            return Ok(());
        }
        let truncated = text.len() > MAX_TEXT_BYTES;
        self.replace_accepted(bounded_prefix(text, MAX_TEXT_BYTES), finished || truncated)?;
        if truncated {
            self.truncated = true;
            self.truncation_notice_pending = true;
        }
        Ok(())
    }

    fn replace_accepted(&mut self, text: &str, finished: bool) -> Result<(), String> {
        if self.finished {
            return if text == self.text && finished {
                Ok(())
            } else {
                Err("已结束的语音回复不能继续追加。".into())
            };
        }
        let rebuild = !text.starts_with(&self.text);
        if rebuild {
            // Rebuild only parsing/projection caches. Guidance and the already
            // emitted prefix remain owned by this message and cannot rewind.
            self.parser.reset();
            self.stable_spoken.clear();
            self.text.clear();
            self.text.push_str(text);
        } else {
            self.text.push_str(&text[self.text.len()..]);
        }
        self.parse_accepted(finished, rebuild)
    }

    fn append_accepted(&mut self, delta: &str, finished: bool) -> Result<(), String> {
        if self.finished {
            return if delta.is_empty() && finished {
                Ok(())
            } else {
                Err("已结束的语音回复不能继续追加。".into())
            };
        }
        self.text.push_str(delta);
        self.parse_accepted(finished, false)
    }

    fn finish_accepted(&mut self) -> Result<(), String> {
        if !self.finished {
            self.parse_accepted(true, false)?;
        }
        Ok(())
    }

    fn parse_accepted(&mut self, finished: bool, rebuild: bool) -> Result<(), String> {
        let previous_stable = self.stable_spoken.len();
        if finished {
            let (blocks, terminal) = self.parser.finish_for_speech(&self.text);
            self.accept_blocks(blocks);
            if let Some(terminal) = terminal {
                let readable = if terminal.unclosed_block.is_some() {
                    readable::project_terminal_contents(&terminal.content)
                } else {
                    readable::project_markdown_block(&terminal.content)
                };
                self.append_readable(&readable);
            }
            self.guidance_locked = true;
        } else {
            let blocks = self.parser.process_stable(&self.text);
            self.accept_blocks(blocks);
        }
        let finished = finished || self.truncated;
        if rebuild {
            self.reply.update(self.stable_spoken.clone(), finished)?;
        } else {
            self.reply.append(&self.stable_spoken[previous_stable..], finished)?;
        }
        self.finished = finished;
        Ok(())
    }

    fn accept_blocks(&mut self, blocks: Vec<StreamBlock>) {
        for block in blocks {
            if !self.guidance_locked {
                if let StreamBlock::VoiceGuidance { content, .. } = &block {
                    if !content.is_empty() && content.chars().count() <= VOICE_GUIDANCE_MAX_CHARS {
                        self.instruction = Some(Arc::from(content.as_str()));
                    }
                }
                // The parser alone decides whether a block is first-line
                // guidance. No later block gets another chance to supply it.
                self.guidance_locked = true;
            }
            if let StreamBlock::Markdown { content, .. } = block {
                self.append_readable(&readable::project_markdown_block(&content));
            }
        }
    }

    fn append_readable(&mut self, text: &str) {
        if text.is_empty() || self.truncated {
            return;
        }
        let separator = usize::from(!self.stable_spoken.is_empty());
        let remaining = MAX_TEXT_BYTES - self.stable_spoken.len();
        let accepted = bounded_prefix(text, remaining.saturating_sub(separator));
        if remaining >= separator {
            if separator != 0 { self.stable_spoken.push('\n'); }
            self.stable_spoken.push_str(accepted);
        }
        // Markdown projection can expand bytes (e.g. table separators become
        // Chinese pauses). It obeys its own cap without turning normal drain
        // into an error or charging the fixed notice to this buffer.
        if accepted.len() != text.len() {
            self.truncated = true;
            self.truncation_notice_pending = true;
        }
    }

    #[cfg(test)]
    pub(super) fn next(&mut self) -> (ReplyPiece, Option<Arc<str>>) {
        let (piece, instruction, _) = self.next_piece();
        (piece, instruction)
    }

    pub(super) fn next_piece(&mut self) -> (ReplyPiece, Option<Arc<str>>, bool) {
        let piece = self.reply.next();
        if matches!(piece, ReplyPiece::Finished)
            && std::mem::take(&mut self.truncation_notice_pending)
        {
            // Do not inherit model style/guidance for this fixed explanation.
            return (ReplyPiece::Text(TRUNCATION_NOTICE.into()), None, true);
        }
        (piece, self.instruction.clone(), false)
    }

    pub(super) fn has_spoken_text(&self) -> bool {
        self.truncation_notice_pending || self.reply.has_spoken_body()
    }
}

struct MessageText {
    id: String,
    agent_id: Option<String>,
    speech: MessageSpeech,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct SpeechPiece {
    pub message_id: String,
    pub agent_id: Option<String>,
    pub text: String,
    pub instruction: Option<Arc<str>>,
    pub notice: bool,
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
    cursor: usize,
    accepted_text_bytes: usize,
    truncated: bool,
    truncation_notice_queued: bool,
}

impl ChatFeed {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn is_group(&self) -> bool {
        self.group
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
            let agent_id = context.and_then(|value| value["agentId"].as_str())
                .filter(|id| !id.is_empty()).map(str::to_owned);
            let index = self
                .messages
                .iter()
                .position(|message| message.id == event.message_id);
            if let Some(index) = index {
                if agent_id.is_some() && self.messages[index].agent_id != agent_id {
                    return Err("朗读消息的发言者发生变化，已停止朗读。".into());
                }
            }
            let previous = index
                .map(|index| self.messages[index].speech.text.len())
                .unwrap_or(0);
            let other_bytes = self.accepted_text_bytes - previous;
            let message_budget = MAX_TEXT_BYTES - other_bytes;
            let index = index.unwrap_or_else(|| {
                self.messages.push(MessageText {
                    id: event.message_id.clone(),
                    agent_id,
                    speech: MessageSpeech::new(),
                });
                self.messages.len() - 1
            });
            let speech = &mut self.messages[index].speech;
            let truncated = if let Some(text) = replacement {
                let accepted = bounded_prefix(text, message_budget);
                let truncated = accepted.len() != text.len();
                speech.replace_accepted(accepted, normal_end || truncated)?;
                truncated
            } else if let Some(text) = delta {
                let accepted = bounded_prefix(text, message_budget - previous);
                let truncated = accepted.len() != text.len();
                speech.append_accepted(accepted, normal_end || truncated)?;
                truncated
            } else {
                unreachable!("replacement or delta was present")
            };
            self.accepted_text_bytes = other_bytes + speech.text.len();
            let truncated = truncated || speech.truncated;
            if truncated {
                // Seal the whole task at its accepted prefix. Previously queued
                // text/PCM stays owned by normal playback and drains in order.
                self.finished = true;
                self.truncated = true;
                speech.truncation_notice_pending = false;
            }
            self.dirty = true;
        }
        if normal_end {
            if let Some(message) = self
                .messages
                .iter_mut()
                .find(|message| message.id == event.message_id)
            {
                message.speech.finish_accepted()?;
                if message.speech.truncated {
                    self.truncated = true;
                    self.finished = true;
                }
            }
            self.dirty = true;
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
            if self.finished && !message.speech.finished {
                message.speech.finish_accepted()?;
            }
            if std::mem::take(&mut message.speech.truncation_notice_pending) {
                self.truncated = true;
            }
        }
        if self.truncated && !self.truncation_notice_queued {
            if let Some(last) = self.messages.last_mut() {
                last.speech.truncation_notice_pending = true;
                self.truncation_notice_queued = true;
            }
        }
        let mut pieces = Vec::new();
        // A member's unfinished tail must not be prepended to the next member,
        // nor may later members interleave with that member's speech.
        while let Some(message) = self.messages.get_mut(self.cursor) {
            let (piece, instruction, notice) = message.speech.next_piece();
            match piece {
                ReplyPiece::Text(text) => pieces.push(SpeechPiece {
                    message_id: message.id.clone(),
                    agent_id: message.agent_id.clone(),
                    text,
                    instruction,
                    notice,
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
            agent_id: None,
            text: text.into(),
            instruction: instruction.map(Arc::from),
            notice: text == TRUNCATION_NOTICE,
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
        format!("<voice_guidance>\n{text}\n</voice_guidance>\n")
    }

    fn delta(id: &str, text: &str, group: bool) -> StreamEvent {
        let mut event = update(id, "", group);
        event.aurora = Some(serde_json::from_value(json!({
            "kind": "delta", "chunk": text,
        })).unwrap());
        event
    }

    fn drain_feed(feed: &mut ChatFeed) -> Vec<SpeechPiece> {
        feed.take_update().unwrap().map_or_else(Vec::new, |update| update.pieces)
    }

    #[test]
    fn delta_fragments_match_the_complete_speech_without_replacing_stable_prefixes() {
        let raw = format!(
            "{}第一段已经完整。\n\n<think>不朗读</think>\n第二段带有 **强调** 和中文。\n\n最后一句。",
            guidance("自然清晰"),
        );
        let mut complete = ChatFeed::new();
        complete.accept(&member("message", &raw)).unwrap();
        let expected = drain_feed(&mut complete);
        let mut incremental = ChatFeed::new();
        let mut actual = Vec::new();
        for ch in raw.chars() {
            incremental.accept(&delta("message", &ch.to_string(), true)).unwrap();
            actual.extend(drain_feed(&mut incremental));
        }
        incremental.accept(&member("message", &raw)).unwrap();
        actual.extend(drain_feed(&mut incremental));
        assert_eq!(actual, expected);
        assert_eq!(incremental.accepted_text_bytes, raw.len());
    }

    #[test]
    fn bubble_limit_keeps_a_utf8_prefix_and_emits_one_notice_outside_its_budget() {
        for tail in ["中", "😀"] {
            let prefix = "a".repeat(MAX_TEXT_BYTES - 1);
            let raw = format!("{prefix}{tail}后续内容");
            let mut speech = MessageSpeech::new();
            speech.update(&raw, false).unwrap();
            assert_eq!(speech.text, prefix);
            assert!(speech.finished);
            let spoken = drain_speech(&mut speech);
            assert_eq!(spoken.last(), Some(&(TRUNCATION_NOTICE.into(), None)));
            assert_eq!(spoken[..spoken.len() - 1].iter().map(|(text, _)| text.as_str()).collect::<String>(), prefix);
            speech.update(&raw, true).unwrap();
            speech.update("迟到的不同终态", true).unwrap();
            assert!(drain_speech(&mut speech).is_empty());
            assert!(matches!(speech.next().0, ReplyPiece::Finished));
        }
    }

    #[test]
    fn group_limit_is_shared_and_late_terminals_cannot_repeat_the_notice() {
        let mut feed = ChatFeed::new();
        let first = "a".repeat(MAX_TEXT_BYTES - 7);
        feed.accept(&member("first", &first)).unwrap();
        let mut pieces = drain_feed(&mut feed);
        feed.accept(&delta("second", "中文😀之后", true)).unwrap();
        assert_eq!(feed.accepted_text_bytes, MAX_TEXT_BYTES - 1);
        assert_eq!(feed.messages[1].speech.text, "中文");
        let final_update = feed.take_update().unwrap().unwrap();
        assert!(final_update.finished);
        pieces.extend(final_update.pieces);
        assert_eq!(pieces.last(), Some(&piece("second", TRUNCATION_NOTICE, None)));
        assert_eq!(pieces.iter().filter(|piece| piece.text == TRUNCATION_NOTICE).count(), 1);
        assert!(!feed.accept(&member("second", "中文😀之后的完整终态")).unwrap());
        assert!(!feed.accept(&member("third", "后续成员")).unwrap());
        feed.finish_submission();
        assert!(drain_feed(&mut feed).is_empty());
    }

    #[test]
    fn group_truncation_drains_an_earlier_unclosed_tail_before_the_last_member_notice() {
        let mut feed = ChatFeed::new();
        let first = format!("<think>{}", "a".repeat(MAX_TEXT_BYTES - 14));
        feed.accept(&delta("first", &first, true)).unwrap();
        assert!(drain_feed(&mut feed).is_empty());
        feed.accept(&delta("second", "中文😀之后", true)).unwrap();
        assert!(feed.cursor < feed.messages.len());
        let update = feed.take_update().unwrap().unwrap();
        assert!(update.finished);
        assert_eq!(update.pieces.first().unwrap().message_id, "first");
        assert_eq!(update.pieces.last(), Some(&piece("second", TRUNCATION_NOTICE, None)));
        assert!(update.pieces.iter().any(|piece| piece.message_id == "second" && piece.text == "中文"));
        assert!(!feed.accept(&delta("second", "迟到增量", true)).unwrap());
        assert!(feed.take_update().unwrap().is_none());
    }

    #[test]
    fn expanded_table_projection_seals_group_on_contentless_end_and_coalesces_notices() {
        let table = format!("{}|\n{}|\n{}", "|a".repeat(16), "|---".repeat(16),
            format!("{}\n", "|".repeat(17)).repeat(1500));
        assert!(table.len() * 2 < MAX_TEXT_BYTES);
        assert!(readable::project_markdown_block(&table).len() > MAX_TEXT_BYTES);
        let mut feed = ChatFeed::new();
        feed.accept(&delta("first", &table, true)).unwrap();
        feed.accept(&delta("second", &table, true)).unwrap();
        assert!(drain_feed(&mut feed).is_empty());
        let mut end = member("first", "");
        end.content = None;
        feed.accept(&end).unwrap();
        let update = feed.take_update().unwrap().unwrap();
        assert!(update.finished);
        assert_eq!(update.pieces.iter().filter(|piece| piece.notice).count(), 1);
        assert!(update.pieces.last().unwrap().notice);
        assert_eq!(update.pieces.last().unwrap().message_id, "second");
        assert!(feed.messages.iter().all(|message| message.speech.stable_spoken.len() <= MAX_TEXT_BYTES));
        assert!(!feed.accept(&delta("third", "迟到成员", true)).unwrap());
        feed.finish_submission();
        assert!(drain_feed(&mut feed).is_empty());
    }

    #[test]
    fn truncation_preserves_emitted_prefix_checks_for_full_replacements() {
        let mut feed = ChatFeed::new();
        let prefix = "这段已经稳定并且已经被朗读。\n\n";
        feed.accept(&delta("message", prefix, true)).unwrap();
        assert!(!drain_feed(&mut feed).is_empty());
        let replacement = format!("改写已经朗读的内容。{}", "a".repeat(MAX_TEXT_BYTES));
        assert!(feed.accept(&update("message", &replacement, true)).unwrap_err().contains("已开始朗读"));

        let mut feed = ChatFeed::new();
        feed.accept(&delta("message", prefix, true)).unwrap();
        let mut pieces = drain_feed(&mut feed);
        let replacement = format!("{prefix}{}", "b".repeat(MAX_TEXT_BYTES));
        feed.accept(&update("message", &replacement, true)).unwrap();
        pieces.extend(drain_feed(&mut feed));
        assert_eq!(feed.accepted_text_bytes, MAX_TEXT_BYTES);
        assert_eq!(pieces.last().unwrap().text, TRUNCATION_NOTICE);
        assert_eq!(pieces.iter().filter(|piece| piece.text == TRUNCATION_NOTICE).count(), 1);
    }

    #[test]
    fn message_author_is_carried_with_every_piece_and_cannot_change_midstream() {
        let mut feed = ChatFeed::new();
        let mut first = update("message", &"这是第一位成员正在说的话。\n\n".repeat(30), true);
        first.context = Some(json!({"isGroupMessage":true,"agentId":"first"}));
        feed.accept(&first).unwrap();
        let pieces = feed.take_update().unwrap().unwrap().pieces;
        assert!(!pieces.is_empty());
        assert!(pieces.iter().all(|piece| piece.agent_id.as_deref() == Some("first")));
        let mut changed = member("message", "替换后的正文。");
        changed.context = Some(json!({"isGroupMessage":true,"agentId":"second"}));
        assert!(feed.accept(&changed).unwrap_err().contains("发言者"));
    }

    fn drain_speech(speech: &mut MessageSpeech) -> Vec<(String, Option<Arc<str>>)> {
        let mut pieces = Vec::new();
        while let (ReplyPiece::Text(text), instruction) = speech.next() {
            pieces.push((text, instruction));
        }
        pieces
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
        let mut feed = ChatFeed::new();
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
                piece("a", "(温柔)[轻笑]第一句。第一位尾音", Some("清亮自然")),
                piece("b", "第二位。", Some("沉稳缓慢")),
            ]
        );
        assert!(!result.finished);
    }

    #[test]
    fn only_stable_text_is_accepted_before_message_completion() {
        let mut feed = ChatFeed::new();
        feed.accept(&update("a", "<voice_guid", false)).unwrap();
        assert!(feed.take_update().unwrap().unwrap().pieces.is_empty());
        let raw = format!("{}第一句话还没有形成稳定段落。", guidance("首个指导"));
        feed.accept(&update("a", &raw, false)).unwrap();
        assert!(feed.take_update().unwrap().unwrap().pieces.is_empty());
        let raw = format!("{raw}\n\n剩余正文");
        feed.accept(&update("a", &raw, false)).unwrap();
        assert_eq!(
            feed.take_update().unwrap().unwrap().pieces,
            vec![piece("a", "第一句话还没有形成稳定段落。", Some("首个指导"))]
        );
        feed.accept(&member("a", &raw)).unwrap();
        assert_eq!(
            feed.take_update().unwrap().unwrap().pieces,
            vec![piece("a", "剩余正文", Some("首个指导"))]
        );
    }

    #[test]
    fn missing_late_or_invalid_guidance_leaves_plain_speech_without_instruction() {
        let cases = [
            "正文。".to_owned(),
            format!("正文。\n{}", guidance("迟到")),
            format!("{}正文。", guidance("")),
            format!(
                "{}正文。",
                guidance(&"字".repeat(VOICE_GUIDANCE_MAX_CHARS + 1))
            ),
        ];
        for raw in cases {
            let mut feed = ChatFeed::new();
            feed.accept(&member("a", &raw)).unwrap();
            let pieces = feed.take_update().unwrap().unwrap().pieces;
            assert_eq!(pieces, vec![piece("a", "正文。", None)]);
        }
    }

    #[test]
    fn a_thought_prefix_does_not_promote_later_guidance_to_the_first_line() {
        let mut feed = ChatFeed::new();
        let raw = format!(
            "<think>私密推理</think>\n```text\n示例代码\n```\n{}正文。",
            guidance("有效指导")
        );
        feed.accept(&member("a", &raw)).unwrap();
        assert_eq!(
            feed.take_update().unwrap().unwrap().pieces,
            vec![piece("a", "正文。", None)]
        );
    }

    #[test]
    fn group_messages_cannot_interleave_or_rewrite_a_spoken_prefix() {
        let mut feed = ChatFeed::new();
        feed.accept(&update(
            "a",
            &format!("{}这是第一位已经稳定的句子。\n\n等尾句", guidance("第一位")),
            true,
        ))
        .unwrap();
        feed.accept(&member("b", &format!("{}第二位。", guidance("第二位"))))
            .unwrap();
        assert_eq!(
            feed.take_update().unwrap().unwrap().pieces,
            vec![piece("a", "这是第一位已经稳定的句子。", Some("第一位"))]
        );
        feed.accept(&member(
            "a",
            &format!(
                "{}这是第一位已经稳定的句子。\n\n等尾句完成。",
                guidance("第一位")
            ),
        ))
        .unwrap();
        assert_eq!(
            feed.take_update().unwrap().unwrap().pieces,
            vec![
                piece("a", "等尾句完成。", Some("第一位")),
                piece("b", "第二位。", Some("第二位")),
            ]
        );
        assert!(feed.accept(&member("a", "改写已播内容。")).is_err());
    }

    #[test]
    fn first_body_locks_even_while_the_playback_worker_has_not_consumed_a_frame() {
        let mut feed = ChatFeed::new();
        feed.accept(&update("a", "正文已经形成稳定段落。\n\n", false))
            .unwrap();
        // A later snapshot can replace an unspoken tail, but cannot change
        // which guidance this response fixed when its body first arrived.
        feed.accept(&update(
            "a",
            &format!("{}正文已经形成稳定段落。\n\n", guidance("后来补写")),
            false,
        ))
        .unwrap();
        assert_eq!(
            feed.take_update().unwrap().unwrap().pieces,
            vec![piece("a", "正文已经形成稳定段落。", None)]
        );
    }

    #[test]
    fn invalid_guidance_cannot_promote_a_subsequent_tool_example() {
        let first = guidance(&format!(
            "{}\n<<<[TOOL_REQUEST]>>>",
            "字".repeat(VOICE_GUIDANCE_MAX_CHARS + 1)
        ));
        let raw = format!(
            "{first}<<<[TOOL_REQUEST]>>>\n{}<<<[END_TOOL_REQUEST]>>>\n正文。",
            guidance("工具内示例")
        );
        let mut feed = ChatFeed::new();
        feed.accept(&member("a", &raw)).unwrap();
        assert_eq!(
            feed.take_update().unwrap().unwrap().pieces,
            vec![piece("a", "正文。", None)]
        );
    }

    #[test]
    fn closed_examples_are_skipped_without_changing_later_guidance_eligibility() {
        for prefix in [
            "<<<[TOOL_REQUEST]>>>\ncontent:「始」<think>「末」\n<<<[END_TOOL_REQUEST]>>>\n",
            "<think>\n<<<[TOOL_REQUEST]>>>\n</think>\n",
            "```text\n<<<[TOOL_REQUEST]>>>\n```\n",
            "<<<[TOOL_REQUEST]>>>\ncontent:「始」\n```\n示例\n「末」\n<<<[END_TOOL_REQUEST]>>>\n",
        ] {
            let raw = format!("{prefix}{}正文。", guidance("真正指导"));
            let mut feed = ChatFeed::new();
            feed.accept(&member("a", &raw)).unwrap();
            assert_eq!(
                feed.take_update().unwrap().unwrap().pieces,
                vec![piece("a", "正文。", None)]
            );
        }
    }

    #[test]
    fn complete_and_incremental_input_share_projection_and_instruction() {
        let raw = format!(
            "{}### 标题\n\n正文含 `true`。\n\n<think>闭合推理</think>\n最后一段。",
            guidance("自然清晰")
        );
        let mut complete = MessageSpeech::new();
        complete.update(&raw, true).unwrap();
        let complete_pieces = drain_speech(&mut complete);
        let mut incremental = MessageSpeech::new();
        let mut incremental_pieces = Vec::new();
        for (offset, ch) in raw.char_indices() {
            incremental
                .update(&raw[..offset + ch.len_utf8()], false)
                .unwrap();
            incremental_pieces.extend(drain_speech(&mut incremental));
        }
        assert!(!incremental_pieces.is_empty());
        incremental.update(&raw, true).unwrap();
        incremental_pieces.extend(drain_speech(&mut incremental));
        assert_eq!(incremental_pieces, complete_pieces);
        assert_eq!(incremental.stable_spoken, complete.stable_spoken);
        assert_eq!(incremental.instruction, complete.instruction);
        assert_eq!(complete.instruction.as_deref(), Some("自然清晰"));
        assert_eq!(complete.stable_spoken, "标题\n正文含 true。\n最后一段。");
    }

    #[test]
    fn consuming_each_stable_update_matches_complete_message_segments() {
        let cases = [
            format!(
                "{}(温柔)[轻笑]{}。\n\n{}。\n\n[轻声]{}。\n\n结束。",
                guidance("自然清晰"),
                "字".repeat(9),
                "次".repeat(39),
                "后".repeat(79),
            ),
            format!(
                "(轻松){}，\n\n{}，\n\n{}，\n\n末尾没有标点",
                "字".repeat(23),
                "次".repeat(79),
                "后".repeat(119),
            ),
            format!(
                "（平静）“{}？！”\n\n\"{}!\"\n\n[低声]{}\n\n末尾。",
                "字".repeat(58),
                "next ".repeat(34),
                "后".repeat(181),
            ),
            format!(
                "{}(清晰)Use **config.json** for version `3.14`.\n\n<think>不朗读</think>\n{}\n\n~~~text\n忽略代码。\n~~~\n\n最后一句。",
                guidance("从容"),
                "第二段包含多个短句。".repeat(20),
            ),
        ];
        for raw in cases {
            let mut complete = MessageSpeech::new();
            complete.update(&raw, true).unwrap();
            let expected = drain_speech(&mut complete);
            let mut incremental = MessageSpeech::new();
            let mut actual = Vec::new();
            // Consume after every update, as the live ChatFeed does; merely
            // comparing the final projection would miss emitted-boundary drift.
            for (offset, ch) in raw.char_indices() {
                incremental
                    .update(&raw[..offset + ch.len_utf8()], false)
                    .unwrap();
                actual.extend(drain_speech(&mut incremental));
            }
            assert!(!actual.is_empty(), "no live speech for {raw}");
            incremental.update(&raw, true).unwrap();
            actual.extend(drain_speech(&mut incremental));
            assert_eq!(actual, expected, "{raw}");
            assert!(matches!(incremental.next().0, ReplyPiece::Finished));
        }
    }

    #[test]
    fn terminal_unclosed_blocks_are_released_once_without_becoming_guidance() {
        for raw in [
            "<voice_guidance>模型漏写闭合后的正文。",
            "<think>模型漏写闭合后的正文。",
            "<<<[TOOL_REQUEST]>>>\n模型漏写闭合后的正文。",
            "```text\n模型漏写闭合后的正文。",
            "~~~text\n模型漏写闭合后的正文。",
            "<div><p>模型漏写闭合后的正文。</p>",
        ] {
            let mut speech = MessageSpeech::new();
            speech.update(raw, false).unwrap();
            assert!(!speech.has_spoken_text(), "{raw}");
            assert!(matches!(speech.next().0, ReplyPiece::Wait));
            speech.update(raw, true).unwrap();
            assert_eq!(speech.stable_spoken, "模型漏写闭合后的正文。", "{raw}");
            assert!(speech.instruction.is_none());
            assert!(matches!(speech.next().0, ReplyPiece::Text(_)));
            speech.update(raw, true).unwrap();
            assert!(matches!(speech.next().0, ReplyPiece::Finished));
        }
    }

    #[test]
    fn guidance_style_and_audio_tags_alone_are_not_speakable_body() {
        let mut speech = MessageSpeech::new();
        speech
            .update(&format!("{}(温柔)[轻笑]", guidance("自然")), true)
            .unwrap();
        assert!(!speech.has_spoken_text());
        assert!(matches!(speech.next().0, ReplyPiece::Finished));
    }

    #[test]
    fn replacement_rebuilds_unspoken_cache_but_keeps_guidance_and_sent_prefix() {
        let mut speech = MessageSpeech::new();
        let prefix = "这段已经稳定并且将要朗读。";
        speech
            .update(
                &format!("{}{prefix}\n\n尚未完成", guidance("最初指导")),
                false,
            )
            .unwrap();
        assert!(matches!(speech.next().0, ReplyPiece::Text(text) if text == prefix));
        speech
            .update(
                &format!("{}{prefix}\n\n修改剩余正文。", guidance("替换指导")),
                true,
            )
            .unwrap();
        assert_eq!(speech.instruction.as_deref(), Some("最初指导"));
        assert!(matches!(speech.next().0, ReplyPiece::Text(text) if text == "修改剩余正文。"));

        let mut speech = MessageSpeech::new();
        speech.update(&format!("{prefix}\n\n"), false).unwrap();
        assert!(matches!(speech.next().0, ReplyPiece::Text(_)));
        assert!(speech
            .update("改写已经播放的稳定正文。\n\n", false)
            .is_err());
    }
}
