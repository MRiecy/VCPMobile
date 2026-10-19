use super::network::MAX_TEXT_BYTES;

pub(super) const SAMPLE_RATE: u64 = 16000;
pub(super) const PRE_ROLL_SAMPLES: u64 = SAMPLE_RATE / 2;
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
    pub(super) fn new(epoch: u32, confirmed_start: u64, allowed_after: u64) -> Self {
        Self {
            epoch,
            cursor: confirmed_start
                .saturating_sub(PRE_ROLL_SAMPLES)
                .max(allowed_after),
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
}

pub(super) enum ReplyPiece {
    Text(String),
    Wait,
    Finished,
}

impl ReplyBuffer {
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
        Ok(())
    }

    pub(super) fn next(&mut self) -> ReplyPiece {
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
            while let Some((offset, ch)) = characters.next() {
                count += 1;
                let end = offset + ch.len_utf8();
                if ch.is_whitespace() {
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
            let piece = remaining[..cut].trim().to_string();
            self.emitted_bytes += cut;
            if !piece.is_empty() {
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
        let mut window = CaptureWindow::new(3, 16000, 12000);
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
}
