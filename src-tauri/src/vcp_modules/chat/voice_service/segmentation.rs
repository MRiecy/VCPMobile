use super::{network::MAX_TEXT_BYTES, readable::MAX_AUDIO_TAG_CHARS};

pub(super) const SAMPLE_RATE: u64 = 16000;
const PRE_ROLL_SAMPLES: u64 = SAMPLE_RATE / 2;
const LEADING_PAD_SAMPLES: u64 = SAMPLE_RATE * 300 / 1000;
const CUT_SAMPLES: u64 = SAMPLE_RATE * 25;

#[derive(Debug, Clone, Copy)]
pub(super) struct SegmentWindow {
    pub epoch: u32,
    pub sequence: u32,
    pub start: u64,
    pub end: u64,
    pub final_segment: bool,
}

pub(super) struct CaptureWindow {
    epoch: u32,
    cursor: u64,
    next_cut: u64,
    sequence: u32,
}

impl CaptureWindow {
    pub(super) fn new(
        epoch: u32,
        confirmed_start: u64,
        allowed_after: u64,
        onset_samples: u64,
    ) -> Self {
        // Keep the original half-second for normal settings. Longer custom
        // confirmation includes every onset frame and a short leading margin.
        let pre_roll =
            PRE_ROLL_SAMPLES.max(onset_samples.div_ceil(512) * 512 + LEADING_PAD_SAMPLES);
        Self {
            epoch,
            cursor: confirmed_start.saturating_sub(pre_roll).max(allowed_after),
            next_cut: confirmed_start.saturating_add(CUT_SAMPLES),
            sequence: 0,
        }
    }

    pub(super) fn advance(&mut self, end: u64, finished: bool) -> Vec<SegmentWindow> {
        let mut result = Vec::new();
        if end >= self.next_cut {
            result.push(self.take(self.next_cut, finished && end == self.next_cut));
            self.next_cut = self.next_cut.saturating_add(CUT_SAMPLES);
        }
        if finished && (end > self.cursor || result.is_empty()) {
            result.push(self.take(end, true));
        }
        result
    }

    fn take(&mut self, end: u64, final_segment: bool) -> SegmentWindow {
        let window = SegmentWindow {
            epoch: self.epoch,
            sequence: self.sequence,
            start: self.cursor,
            end,
            final_segment,
        };
        self.cursor = end;
        self.sequence = self.sequence.saturating_add(1);
        window
    }
}

#[derive(Default)]
pub(super) struct ReplyBuffer {
    text: String,
    emitted_bytes: usize,
    finished: bool,
    global_style: Option<String>,
    style_decided: bool,
    inherit_global_style: bool,
}

pub(super) enum ReplyPiece {
    Text(String),
    Wait,
    Finished,
}

impl ReplyBuffer {
    pub(super) fn new(inherit_global_style: bool) -> Self {
        Self {
            inherit_global_style,
            ..Default::default()
        }
    }

    pub(super) fn update(&mut self, text: String, finished: bool) -> Result<(), String> {
        if text.len() > MAX_TEXT_BYTES {
            return Err("语音回复超过 64 KiB，已停止本次朗读。".into());
        }
        if !text.starts_with(&self.text[..self.emitted_bytes]) {
            return Err("已开始朗读的回复正文发生变化，已停止本次朗读。".into());
        }
        if self.finished && (text != self.text || !finished) {
            return Err("已结束的语音回复不能继续追加。".into());
        }
        self.text = text;
        self.finished = finished;
        if self.emitted_bytes == 0 {
            self.global_style = None;
            self.style_decided = false;
        }
        Ok(())
    }

    pub(super) fn next(&mut self) -> ReplyPiece {
        if !self.style_decided {
            let start = self.text.trim_start();
            let closing = match (self.inherit_global_style, start.chars().next()) {
                (true, Some('(')) => Some(')'),
                (true, Some('（')) => Some('）'),
                _ => None,
            };
            if let Some(closing) = closing {
                let mut valid = true;
                let mut count = 0;
                let mut end = None;
                for (offset, ch) in start.char_indices().skip(1) {
                    if ch == closing {
                        end = Some(offset + ch.len_utf8());
                        break;
                    }
                    count += 1;
                    if count > 80
                        || matches!(
                            ch,
                            '\n' | '\r' | '。' | '！' | '？' | '!' | '?' | '.' | '(' | '（'
                        )
                    {
                        valid = false;
                        break;
                    }
                }
                if valid {
                    if let Some(end) = end {
                        if count > 0 {
                            self.global_style = Some(start[..end].to_owned());
                        }
                    } else if !self.finished {
                        return ReplyPiece::Wait;
                    }
                }
            }
            if self.text.is_empty() && !self.finished {
                return ReplyPiece::Wait;
            }
            self.style_decided = true;
        }
        loop {
            let remaining = &self.text[self.emitted_bytes..];
            if remaining.is_empty() {
                return if self.finished {
                    ReplyPiece::Finished
                } else {
                    ReplyPiece::Wait
                };
            }
            let mut cut = None;
            let mut last_space = None;
            let mut characters = remaining.char_indices().peekable();
            let mut count = 0;
            let mut tag_end = 0;
            let mut pending_tags_start = None;
            let style_prefix_end = if self.emitted_bytes == 0 {
                self.global_style.as_ref().map_or(0, |style| {
                    remaining.len() - remaining.trim_start().len() + style.len()
                })
            } else {
                0
            };
            while let Some((offset, ch)) = characters.next() {
                let end = offset + ch.len_utf8();
                if ch == '[' && end > tag_end {
                    let tail = &remaining[end..];
                    let mut bounded = true;
                    let mut closing = None;
                    for (count, (inner, tag_ch)) in tail.char_indices().enumerate() {
                        if tag_ch == ']' {
                            closing = Some(end + inner + 1);
                            break;
                        }
                        if count >= MAX_AUDIO_TAG_CHARS || matches!(tag_ch, '\n' | '\r' | '[') {
                            bounded = false;
                            break;
                        }
                    }
                    if let Some(closing) = closing {
                        tag_end = closing;
                        pending_tags_start.get_or_insert(offset);
                    } else if bounded && !self.finished {
                        return ReplyPiece::Wait;
                    }
                }
                // Tags describe the following speech. They neither consume
                // its text budget nor form a cut point at the closing bracket.
                if end <= tag_end || end <= style_prefix_end {
                    continue;
                }
                count += 1;
                if !ch.is_whitespace() {
                    pending_tags_start = None;
                } else if pending_tags_start.is_none() {
                    last_space = Some(end);
                }
                let sentence_end = matches!(ch, '。' | '！' | '？' | '!' | '?')
                    || (ch == '.'
                        && characters
                            .peek()
                            .is_some_and(|(_, next)| next.is_whitespace()));
                let clause_end =
                    count >= 24 && matches!(ch, '，' | ',' | '；' | ';' | '：' | ':' | '\n');
                if sentence_end || clause_end {
                    cut = Some(end);
                    break;
                }
                if count >= 160 {
                    if let Some(tag_start) = pending_tags_start {
                        if tag_start > 0 {
                            cut = Some(tag_start);
                            break;
                        }
                        // A leading tag still needs its first spoken content;
                        // spaces alone cannot become a detached tag segment.
                        continue;
                    }
                    cut = Some(
                        last_space
                            .filter(|space| *space > remaining.len().min(end) / 2)
                            .unwrap_or(end),
                    );
                    break;
                }
            }
            let Some(cut) = cut.or_else(|| self.finished.then_some(remaining.len())) else {
                return ReplyPiece::Wait;
            };
            let mut piece = remaining[..cut].trim().to_string();
            let first_piece = self.emitted_bytes == 0;
            self.emitted_bytes += cut;
            if !piece.is_empty() {
                if !first_piece {
                    if let Some(style) = &self.global_style {
                        piece.insert_str(0, style);
                    }
                }
                return ReplyPiece::Text(piece);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preroll_respects_local_probe_floor_and_long_speech_has_contiguous_cuts() {
        let mut window = CaptureWindow::new(3, 16000, 12000, 192 * 16);
        assert!(window.advance(415999, false).is_empty());
        let first = window.advance(416000, false)[0];
        assert_eq!(
            (first.start, first.end, first.sequence, first.epoch),
            (12000, 416000, 0, 3)
        );
        assert!(!first.final_segment);
        let last = window.advance(420000, true)[0];
        assert_eq!((last.start, last.end, last.sequence), (416000, 420000, 1));
        assert!(last.final_segment);
    }

    #[test]
    fn long_confirmation_keeps_the_onset_and_stays_within_native_export_budget() {
        let confirmed = SAMPLE_RATE * 3;
        let mut window = CaptureWindow::new(1, confirmed, 0, 2000 * 16);
        let first = window.advance(confirmed + CUT_SAMPLES, false)[0];
        // 2000 ms requires 63 frames (2016 ms), with 300 ms of leading audio.
        assert_eq!(first.start, confirmed - (63 * 512 + LEADING_PAD_SAMPLES));
        assert!(first.end - first.start < SAMPLE_RATE * 30);
        let last = window.advance(first.end + 512, true)[0];
        assert_eq!(last.start, first.end);
    }

    #[test]
    fn playback_ending_during_confirmation_does_not_discard_the_earlier_speech() {
        let config = tauri_plugin_vcp_mobile::voice::VoiceVadConfig {
            playback_onset_ms: 2000,
            ..Default::default()
        };
        // 1.8 s of speech while playback was active, then ordinary VAD
        // confirms 96 ms after playback ends. The full prefix still belongs
        // to this utterance, despite its final frame being output-inactive.
        let speech_start = SAMPLE_RATE;
        let confirmed = speech_start + 30_720;
        let mut window = CaptureWindow::new(
            1,
            confirmed,
            0,
            config.onset_samples(true).max(config.onset_samples(false)),
        );
        let segment = window.advance(confirmed + SAMPLE_RATE, true)[0];
        assert!(segment.start <= speech_start);
    }

    #[test]
    fn short_pause_or_long_confirmation_never_reexports_a_previous_segment() {
        let mut previous = CaptureWindow::new(1, 16000, 0, 96 * 16);
        let previous_end = 32000;
        let first = previous.advance(previous_end, true)[0];
        for onset in [96 * 16, 2000 * 16] {
            let mut next = CaptureWindow::new(1, previous_end + 3 * 512, first.end, onset);
            let second = next.advance(previous_end + 8 * 512, true)[0];
            assert_eq!(second.start, first.end);
        }
    }

    #[test]
    fn cumulative_unicode_updates_emit_each_sentence_once_and_allow_unspoken_tail_edits() {
        let mut reply = ReplyBuffer::default();
        reply.update("你好。接下".into(), false).unwrap();
        assert!(matches!(reply.next(), ReplyPiece::Text(text) if text == "你好。"));
        assert!(matches!(reply.next(), ReplyPiece::Wait));
        reply.update("你好。下面继续。".into(), true).unwrap();
        assert!(matches!(reply.next(), ReplyPiece::Text(text) if text == "下面继续。"));
        assert!(matches!(reply.next(), ReplyPiece::Finished));
        assert!(reply.update("您好。下面继续。".into(), true).is_err());
    }

    #[test]
    fn decimal_and_an_unfinished_tail_wait_for_more_text() {
        let mut reply = ReplyBuffer::default();
        reply.update("The value is 3.14".into(), false).unwrap();
        assert!(matches!(reply.next(), ReplyPiece::Wait));
        reply
            .update("The value is 3.14. Next".into(), false)
            .unwrap();
        assert!(matches!(reply.next(), ReplyPiece::Text(text) if text == "The value is 3.14."));
    }

    #[test]
    fn long_unpunctuated_text_is_bounded_and_oversize_is_an_explicit_error() {
        let mut reply = ReplyBuffer::default();
        reply.update("字".repeat(200), false).unwrap();
        assert!(matches!(reply.next(), ReplyPiece::Text(text) if text.chars().count() == 160));
        assert!(reply.update("字".repeat(MAX_TEXT_BYTES), true).is_err());
    }

    #[test]
    fn tags_are_never_cut_and_global_style_repeats_without_repeating_local_actions() {
        let mut reply = ReplyBuffer::new(true);
        reply
            .update("(温柔、轻松)[轻笑]第一句。[叹气]第二句。".into(), true)
            .unwrap();
        assert!(
            matches!(reply.next(), ReplyPiece::Text(text) if text == "(温柔、轻松)[轻笑]第一句。")
        );
        assert!(
            matches!(reply.next(), ReplyPiece::Text(text) if text == "(温柔、轻松)[叹气]第二句。")
        );
        let mut reply = ReplyBuffer::default();
        reply
            .update(
                format!("{}[轻声，说得慢一点]尾句。", "字".repeat(155)),
                true,
            )
            .unwrap();
        assert!(
            matches!(reply.next(), ReplyPiece::Text(text) if text == format!("{}[轻声，说得慢一点]尾句。", "字".repeat(155)))
        );
        assert!(matches!(reply.next(), ReplyPiece::Finished));
        let mut boundary = ReplyBuffer::default();
        boundary
            .update(format!("{}[轻声][慢一点]尾句。", "字".repeat(160)), true)
            .unwrap();
        assert!(matches!(boundary.next(), ReplyPiece::Text(text) if text == "字".repeat(160)));
        assert!(
            matches!(boundary.next(), ReplyPiece::Text(text) if text == "[轻声][慢一点]尾句。")
        );
        let mut spacing = ReplyBuffer::default();
        spacing
            .update(format!("{}[低声][轻笑] 尾句。", "字".repeat(159)), true)
            .unwrap();
        assert!(matches!(spacing.next(), ReplyPiece::Text(text) if text == "字".repeat(159)));
        assert!(matches!(spacing.next(), ReplyPiece::Text(text) if text == "[低声][轻笑] 尾句。"));
    }

    #[test]
    fn partial_global_style_waits_with_a_bound_and_does_not_inherit_a_new_message() {
        let mut reply = ReplyBuffer::new(true);
        reply.update("（温柔".into(), false).unwrap();
        assert!(matches!(reply.next(), ReplyPiece::Wait));
        reply
            .update("（温柔）第一句。第二句。".into(), true)
            .unwrap();
        assert!(matches!(reply.next(), ReplyPiece::Text(text) if text == "（温柔）第一句。"));
        assert!(matches!(reply.next(), ReplyPiece::Text(text) if text == "（温柔）第二句。"));
        let mut next = ReplyBuffer::new(true);
        next.update("[轻笑]不同消息。".into(), true).unwrap();
        assert!(matches!(next.next(), ReplyPiece::Text(text) if text == "[轻笑]不同消息。"));
        let mut unclosed = ReplyBuffer::new(true);
        unclosed
            .update(format!("({}", "字".repeat(200)), false)
            .unwrap();
        assert!(matches!(unclosed.next(), ReplyPiece::Text(_)));
    }

    #[test]
    fn ordinary_parenthetical_prose_is_not_repeated_outside_design_mode() {
        let mut reply = ReplyBuffer::default();
        reply
            .update("(注：这是一段说明)第一句。第二句。".into(), true)
            .unwrap();
        assert!(
            matches!(reply.next(), ReplyPiece::Text(text) if text == "(注：这是一段说明)第一句。")
        );
        assert!(matches!(reply.next(), ReplyPiece::Text(text) if text == "第二句。"));
    }

    #[test]
    fn oversized_or_unclosed_square_text_cannot_bypass_the_segment_limit() {
        for text in [
            format!("[{}", "字".repeat(300)),
            format!("[{}]结尾。", "字".repeat(300)),
        ] {
            let mut reply = ReplyBuffer::default();
            reply.update(text, false).unwrap();
            assert!(matches!(reply.next(), ReplyPiece::Text(text) if text.chars().count() == 160));
        }
        let mut reply = ReplyBuffer::default();
        reply.update("[轻声，".into(), false).unwrap();
        assert!(matches!(reply.next(), ReplyPiece::Wait));
        reply
            .update("[轻声，说得慢一点]正文。".into(), true)
            .unwrap();
        assert!(
            matches!(reply.next(), ReplyPiece::Text(text) if text == "[轻声，说得慢一点]正文。")
        );
    }
}
