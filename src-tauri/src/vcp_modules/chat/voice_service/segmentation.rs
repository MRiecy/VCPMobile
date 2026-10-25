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
    emitted_segments: usize,
    quotes: QuoteState,
}

#[derive(Clone, Copy, Default)]
struct QuoteState {
    single: bool,
    double: bool,
}

struct SpeechChar {
    ch: char,
    end: usize,
    closing: bool,
    quotes: QuoteState,
}

struct ChunkPolicy {
    min_sentence_chars: usize,
    clause_cut_chars: usize,
    max_chars: usize,
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

    fn policy(&self) -> ChunkPolicy {
        let (min_sentence_chars, clause_cut_chars, max_chars) = match self.emitted_segments {
            0 => (10, 24, 60),
            1 => (40, 80, 150),
            _ => (80, 120, 180),
        };
        ChunkPolicy {
            min_sentence_chars,
            clause_cut_chars,
            max_chars,
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

    pub(super) fn has_spoken_body(&self) -> bool {
        let style_end = global_style(&self.text, self.inherit_global_style).map_or(0, |style| {
            self.text.len() - self.text.trim_start().len() + style.len()
        });
        speech_chars(&self.text, style_end, QuoteState::default())
            .next()
            .is_some()
    }

    pub(super) fn next(&mut self) -> ReplyPiece {
        if !self.style_decided {
            self.global_style =
                global_style(&self.text, self.inherit_global_style).map(str::to_owned);
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
            let style_prefix_end = if self.emitted_bytes == 0 {
                self.global_style.as_ref().map_or(0, |style| {
                    remaining.len() - remaining.trim_start().len() + style.len()
                })
            } else {
                0
            };
            let policy = self.policy();
            // One segment plus one bounded punctuation run is sufficient to
            // distinguish attachment from a run that itself exceeds the limit.
            let body: Vec<_> = speech_chars(remaining, style_prefix_end, self.quotes)
                .take(2 * policy.max_chars + 1)
                .collect();
            if body.is_empty() {
                // A style or action without speech cannot occupy a TTS request.
                // Keep it available for the next stable input until finalization.
                if self.finished {
                    self.emitted_bytes = self.text.len();
                    return ReplyPiece::Finished;
                }
                return ReplyPiece::Wait;
            }
            let cut_chars = body.iter().enumerate().find_map(|(index, character)| {
                let count = index + 1;
                let sentence = count >= policy.min_sentence_chars && is_sentence_end(&body, index);
                let clause = count >= policy.clause_cut_chars && is_clause_end(character.ch);
                if sentence || clause {
                    let attached = attach_punctuation(&body, count);
                    return Some(if attached <= policy.max_chars {
                        attached
                    } else {
                        hard_cut(&body, policy.max_chars)
                    });
                }
                (count >= policy.max_chars).then(|| hard_cut(&body, policy.max_chars))
            });
            let Some(cut_chars) = cut_chars.or_else(|| self.finished.then_some(body.len())) else {
                return ReplyPiece::Wait;
            };
            let last = &body[cut_chars - 1];
            let cut = last.end;
            let mut piece = speech_pauses(&remaining[..cut]);
            let first_piece = self.emitted_segments == 0;
            self.emitted_bytes += cut;
            self.quotes = last.quotes;
            if !piece.is_empty() {
                if !first_piece {
                    if let Some(style) = &self.global_style {
                        piece.insert_str(0, style);
                    }
                }
                self.emitted_segments += 1;
                return ReplyPiece::Text(piece);
            }
        }
    }
}

fn global_style(text: &str, inherit_global_style: bool) -> Option<&str> {
    let start = text.trim_start();
    let closing = match (inherit_global_style, start.chars().next()) {
        (true, Some('(')) => ')',
        (true, Some('（')) => '）',
        _ => return None,
    };
    for (count, (offset, ch)) in start.char_indices().skip(1).enumerate() {
        if ch == closing {
            return (count > 0).then_some(&start[..offset + ch.len_utf8()]);
        }
        if count >= 80
            || matches!(
                ch,
                '\n' | '\r' | '。' | '！' | '？' | '!' | '?' | '.' | '(' | '（'
            )
        {
            return None;
        }
    }
    None
}

fn speech_chars(
    text: &str,
    style_end: usize,
    mut quotes: QuoteState,
) -> impl Iterator<Item = SpeechChar> + '_ {
    let mut previous: Option<char> = None;
    let mut tag_end = 0;
    let mut pending_tag = false;
    let mut characters = text.char_indices().peekable();
    std::iter::from_fn(move || {
        while let Some((offset, ch)) = characters.next() {
            let end = offset + ch.len_utf8();
            if end <= style_end || end <= tag_end {
                continue;
            }
            if ch == '[' {
                for (count, (inner, tag_ch)) in text[end..].char_indices().enumerate() {
                    if tag_ch == ']' {
                        tag_end = end + inner + tag_ch.len_utf8();
                        pending_tag = true;
                        break;
                    }
                    if count >= MAX_AUDIO_TAG_CHARS || matches!(tag_ch, '\n' | '\r' | '[') {
                        break;
                    }
                }
                if end <= tag_end {
                    continue;
                }
            }
            // Tags and their separating whitespace belong to the following speech.
            // Keeping cuts at body character ends leaves those tags with that speech.
            if ch.is_whitespace() && (previous.is_none() || pending_tag) {
                continue;
            }
            pending_tag = false;
            let closing = match ch {
                '"' => {
                    let closing = quotes.double;
                    quotes.double = !quotes.double;
                    closing
                }
                '\'' => {
                    let previous_is_word = previous.is_some_and(char::is_alphanumeric);
                    let next_is_word = characters
                        .peek()
                        .is_some_and(|(_, next)| next.is_alphanumeric());
                    if previous_is_word && next_is_word {
                        false
                    } else {
                        let closing = quotes.single;
                        quotes.single = !quotes.single;
                        closing
                    }
                }
                _ => is_closing_symbol(ch),
            };
            previous = Some(ch);
            return Some(SpeechChar {
                ch,
                end,
                closing,
                quotes,
            });
        }
        None
    })
}

fn is_clause_end(ch: char) -> bool {
    matches!(ch, '，' | ',' | '；' | ';' | '：' | ':' | '、')
}

fn is_pause_punctuation(ch: char) -> bool {
    matches!(ch, '。' | '！' | '？' | '!' | '?' | '…' | '.') || is_clause_end(ch)
}

fn is_closing_symbol(ch: char) -> bool {
    matches!(
        ch,
        '”' | '’' | '」' | '』' | '）' | ')' | '】' | ']' | '》' | '〉' | '}'
    )
}

fn is_sentence_end(body: &[SpeechChar], index: usize) -> bool {
    let ch = body[index].ch;
    if matches!(ch, '。' | '！' | '？' | '!' | '?' | '…' | '\n') {
        return true;
    }
    if ch != '.' || index == 0 {
        return false;
    }
    let previous = &body[index - 1];
    if !previous.ch.is_alphanumeric()
        && previous.ch != '_'
        && previous.ch != '.'
        && !previous.closing
    {
        return false;
    }
    // A period within a decimal or identifier is not a sentence boundary.
    // No abbreviation dictionary: only the stable text's actual boundaries matter.
    body[index + 1..]
        .iter()
        .find(|next| next.ch != '.' && !next.closing)
        .is_none_or(|next| next.ch.is_whitespace())
}

fn attach_punctuation(body: &[SpeechChar], mut cut: usize) -> usize {
    while body
        .get(cut)
        .is_some_and(|next| is_pause_punctuation(next.ch) || next.closing)
    {
        cut += 1;
    }
    cut
}

fn hard_cut(body: &[SpeechChar], limit: usize) -> usize {
    let limit = limit.min(body.len());
    if let Some(index) = body[..limit]
        .iter()
        .rposition(|item| item.ch.is_whitespace())
    {
        if index + 1 > limit / 2 {
            return index + 1;
        }
    }
    if body
        .get(limit)
        .is_some_and(|next| is_pause_punctuation(next.ch) || next.closing)
    {
        // Move one spoken character with its punctuation when the complete
        // punctuation/closing-symbol run would exceed this segment's budget.
        let run_start = body[..limit]
            .iter()
            .rposition(|item| !is_pause_punctuation(item.ch) && !item.closing)
            .map_or(0, |index| index + 1);
        if run_start > 1 && attach_punctuation(body, run_start) - run_start < limit {
            return run_start - 1;
        }
    }
    limit
}

fn speech_pauses(text: &str) -> String {
    let mut output = String::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if !output.is_empty() {
            let previous = output
                .chars()
                .rev()
                .find(|ch| !is_closing_symbol(*ch) && !matches!(ch, '\'' | '"'));
            output.push(if previous.is_some_and(is_pause_punctuation) {
                ' '
            } else {
                '，'
            });
        }
        output.push_str(line);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preroll_respects_capture_floor_and_long_speech_has_contiguous_cuts() {
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
        let mut reply = ReplyBuffer::new(false);
        reply
            .update("你好，我们现在开始吧。接下".into(), false)
            .unwrap();
        assert!(matches!(reply.next(), ReplyPiece::Text(text) if text == "你好，我们现在开始吧。"));
        assert!(matches!(reply.next(), ReplyPiece::Wait));
        reply
            .update("你好，我们现在开始吧。下面继续。".into(), true)
            .unwrap();
        assert!(matches!(reply.next(), ReplyPiece::Text(text) if text == "下面继续。"));
        assert!(matches!(reply.next(), ReplyPiece::Finished));
        assert!(reply
            .update("您好，我们现在开始吧。下面继续。".into(), true)
            .is_err());
    }

    #[test]
    fn decimal_and_an_unfinished_tail_wait_for_more_text() {
        let mut reply = ReplyBuffer::new(false);
        reply.update("The value is 3.14".into(), false).unwrap();
        assert!(matches!(reply.next(), ReplyPiece::Wait));
        reply
            .update("The value is 3.14. Next".into(), false)
            .unwrap();
        assert!(matches!(reply.next(), ReplyPiece::Text(text) if text == "The value is 3.14."));
    }

    #[test]
    fn long_unpunctuated_text_is_bounded_and_oversize_is_an_explicit_error() {
        let mut reply = ReplyBuffer::new(false);
        reply.update("字".repeat(200), false).unwrap();
        assert!(matches!(reply.next(), ReplyPiece::Text(text) if text.chars().count() == 60));
        assert!(reply.update("字".repeat(MAX_TEXT_BYTES), true).is_err());
    }

    #[test]
    fn tags_are_never_cut_and_global_style_repeats_without_repeating_local_actions() {
        let mut reply = ReplyBuffer::new(true);
        reply
            .update(
                "(温柔、轻松)[轻笑]这是接下来要说的第一句。[叹气]第二句。".into(),
                true,
            )
            .unwrap();
        assert!(
            matches!(reply.next(), ReplyPiece::Text(text) if text == "(温柔、轻松)[轻笑]这是接下来要说的第一句。")
        );
        assert!(
            matches!(reply.next(), ReplyPiece::Text(text) if text == "(温柔、轻松)[叹气]第二句。")
        );
        let mut reply = ReplyBuffer::new(false);
        reply
            .update(format!("{}[轻声，说得慢一点]尾句。", "字".repeat(55)), true)
            .unwrap();
        assert!(
            matches!(reply.next(), ReplyPiece::Text(text) if text == format!("{}[轻声，说得慢一点]尾句。", "字".repeat(55)))
        );
        assert!(matches!(reply.next(), ReplyPiece::Finished));
        let mut boundary = ReplyBuffer::new(false);
        boundary
            .update(format!("{}[轻声][慢一点]尾句。", "字".repeat(60)), true)
            .unwrap();
        assert!(matches!(boundary.next(), ReplyPiece::Text(text) if text == "字".repeat(60)));
        assert!(
            matches!(boundary.next(), ReplyPiece::Text(text) if text == "[轻声][慢一点]尾句。")
        );
        let mut spacing = ReplyBuffer::new(false);
        spacing
            .update(format!("{} [低声][轻笑] 尾句。", "字".repeat(59)), true)
            .unwrap();
        assert!(matches!(spacing.next(), ReplyPiece::Text(text) if text == "字".repeat(59)));
        assert!(matches!(spacing.next(), ReplyPiece::Text(text) if text == "[低声][轻笑] 尾句。"));
    }

    #[test]
    fn complete_global_style_has_a_bound_and_does_not_inherit_a_new_message() {
        let mut reply = ReplyBuffer::new(true);
        reply.update("（温柔".into(), false).unwrap();
        assert!(matches!(reply.next(), ReplyPiece::Wait));
        reply
            .update("（温柔）这是接下来要说的第一句。第二句。".into(), true)
            .unwrap();
        assert!(
            matches!(reply.next(), ReplyPiece::Text(text) if text == "（温柔）这是接下来要说的第一句。")
        );
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
    fn standalone_parenthetical_prose_is_not_repeated_as_global_style() {
        let mut reply = ReplyBuffer::new(false);
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
            let mut reply = ReplyBuffer::new(false);
            reply.update(text, false).unwrap();
            assert!(matches!(reply.next(), ReplyPiece::Text(text) if text.chars().count() == 60));
        }
        let mut reply = ReplyBuffer::new(false);
        reply.update("[轻声，".into(), false).unwrap();
        assert!(matches!(reply.next(), ReplyPiece::Wait));
        reply
            .update("[轻声，说得慢一点]正文。".into(), true)
            .unwrap();
        assert!(
            matches!(reply.next(), ReplyPiece::Text(text) if text == "[轻声，说得慢一点]正文。")
        );
    }

    #[test]
    fn short_sentences_merge_and_finalization_releases_the_last_short_piece() {
        let mut reply = ReplyBuffer::new(true);
        reply.update("嗯。好的。".into(), false).unwrap();
        assert!(matches!(reply.next(), ReplyPiece::Wait));
        reply
            .update("嗯。好的。我们开始吧。再见。".into(), false)
            .unwrap();
        assert!(matches!(reply.next(), ReplyPiece::Text(text) if text == "嗯。好的。我们开始吧。"));
        assert!(matches!(reply.next(), ReplyPiece::Wait));
        reply
            .update("嗯。好的。我们开始吧。再见。".into(), true)
            .unwrap();
        assert!(matches!(reply.next(), ReplyPiece::Text(text) if text == "再见。"));
        assert!(matches!(reply.next(), ReplyPiece::Finished));
    }

    #[test]
    fn all_transport_modes_share_one_staircase_window() {
        let sizes = [60, 150, 180];
        let mut reply = ReplyBuffer::new(false);
        reply.update("字".repeat(500), false).unwrap();
        for expected in sizes {
            assert!(
                matches!(reply.next(), ReplyPiece::Text(text) if text.chars().count() == expected)
            );
        }
        assert!(matches!(reply.next(), ReplyPiece::Wait));
        reply.update("字".repeat(500), true).unwrap();
        assert!(
            matches!(reply.next(), ReplyPiece::Text(text) if text.chars().count() == 500 - sizes.iter().sum::<usize>())
        );
        assert!(matches!(reply.next(), ReplyPiece::Finished));
    }

    #[test]
    fn sentence_and_clause_thresholds_advance_only_after_a_spoken_piece() {
        for (sizes, punctuation) in [([10, 40, 80], '。'), ([24, 80, 120], '，')] {
            let mut reply = ReplyBuffer::new(true);
            let mut cumulative = "(轻松)[轻笑]".to_string();
            reply.update(cumulative.clone(), false).unwrap();
            assert!(matches!(reply.next(), ReplyPiece::Wait));
            assert_eq!(reply.emitted_segments, 0);
            for expected in sizes {
                let body = format!("{}{}", "字".repeat(expected - 1), punctuation);
                cumulative.push_str(&body);
                reply.update(cumulative.clone(), false).unwrap();
                assert!(matches!(reply.next(), ReplyPiece::Text(text) if text.ends_with(&body)));
                assert!(matches!(reply.next(), ReplyPiece::Wait));
            }
            assert_eq!(reply.emitted_segments, 3);
        }
    }

    #[test]
    fn consuming_completed_paragraphs_matches_the_full_readable_input() {
        let paragraphs = [
            format!("(温柔)[轻笑]“{}？！”", "字".repeat(58)),
            format!("{}，{}。", "次".repeat(79), "文".repeat(39)),
            format!("[轻声]{}", "后".repeat(181)),
            "Use config.json for version 3.14. 最后的短句。".to_owned(),
        ];
        let drain = |reply: &mut ReplyBuffer| {
            let mut pieces = Vec::new();
            while let ReplyPiece::Text(text) = reply.next() {
                pieces.push(text);
            }
            pieces
        };
        let text = paragraphs.join("\n");
        let mut complete = ReplyBuffer::new(true);
        complete.update(text.clone(), true).unwrap();
        let expected = drain(&mut complete);
        let mut incremental = ReplyBuffer::new(true);
        let mut actual = Vec::new();
        let mut stable = String::new();
        for paragraph in paragraphs {
            if !stable.is_empty() {
                stable.push('\n');
            }
            stable.push_str(&paragraph);
            incremental.update(stable.clone(), false).unwrap();
            actual.extend(drain(&mut incremental));
        }
        assert!(!actual.is_empty());
        incremental.update(text, true).unwrap();
        actual.extend(drain(&mut incremental));
        assert_eq!(actual, expected);
        assert!(matches!(incremental.next(), ReplyPiece::Finished));
    }

    #[test]
    fn style_and_actions_never_become_standalone_requests() {
        for input in ["(温柔)", "[轻笑][低声]", "(温柔)[轻笑]\n ", " \n "] {
            let mut reply = ReplyBuffer::new(true);
            reply.update(input.into(), true).unwrap();
            assert!(!reply.has_spoken_body());
            assert!(matches!(reply.next(), ReplyPiece::Finished));
            assert_eq!(reply.emitted_segments, 0);
        }
        let mut reply = ReplyBuffer::new(true);
        reply
            .update("(温柔)这里是足够长的第一句。[轻笑]".into(), true)
            .unwrap();
        assert!(reply.has_spoken_body());
        assert!(
            matches!(reply.next(), ReplyPiece::Text(text) if text == "(温柔)这里是足够长的第一句。")
        );
        assert!(matches!(reply.next(), ReplyPiece::Finished));
        assert_eq!(reply.emitted_segments, 1);
    }

    #[test]
    fn punctuation_and_closing_quotes_attach_but_opening_quotes_stay_with_following_text() {
        for (input, expected) in [
            (
                "“这是第一段足够长的正文？！”“第二段。”",
                "“这是第一段足够长的正文？！”",
            ),
            (
                "这是第一段足够长的正文……（第二段。）",
                "这是第一段足够长的正文……",
            ),
            (
                "\"This first sentence is enough!\"\"Next sentence.\"",
                "\"This first sentence is enough!\"",
            ),
        ] {
            let mut reply = ReplyBuffer::new(false);
            reply.update(input.into(), true).unwrap();
            assert!(matches!(reply.next(), ReplyPiece::Text(text) if text == expected));
        }
        let mut bounded = ReplyBuffer::new(false);
        bounded
            .update(format!("{}？！”", "字".repeat(59)), true)
            .unwrap();
        assert!(matches!(bounded.next(), ReplyPiece::Text(text) if text == "字".repeat(58)));
        assert!(matches!(bounded.next(), ReplyPiece::Text(text) if text == "字？！”"));
    }

    #[test]
    fn english_periods_preserve_decimal_and_identifier_boundaries() {
        let mut reply = ReplyBuffer::new(false);
        let sentence = "Use config.json for version 3.14.";
        reply.update(sentence.into(), false).unwrap();
        assert!(matches!(reply.next(), ReplyPiece::Text(text) if text == sentence));
        assert!(matches!(reply.next(), ReplyPiece::Wait));
    }

    #[test]
    fn punctuation_longer_than_a_segment_keeps_the_hard_limit() {
        for punctuation_chars in [59, 60, 20_000] {
            let mut reply = ReplyBuffer::new(false);
            reply
                .update(
                    format!("{}{}后文", "字".repeat(59), ".".repeat(punctuation_chars)),
                    true,
                )
                .unwrap();
            let expected = if punctuation_chars < 60 {
                "字".repeat(58)
            } else {
                format!("{}.", "字".repeat(59))
            };
            assert!(matches!(reply.next(), ReplyPiece::Text(text) if text == expected));
        }
    }

    #[test]
    fn internal_newlines_add_pauses_without_changing_the_emitted_source_prefix() {
        let mut reply = ReplyBuffer::new(false);
        let first = "第一段已经有足够多的文字\n";
        let text = format!("{first}标题\n第一项。\n第二项\n第三项");
        reply.update(text.clone(), true).unwrap();
        assert!(matches!(reply.next(), ReplyPiece::Text(text) if text == first.trim_end()));
        assert!(
            matches!(reply.next(), ReplyPiece::Text(text) if text == "标题，第一项。 第二项，第三项")
        );
        assert!(matches!(reply.next(), ReplyPiece::Finished));
        assert_eq!(reply.text, text);
        assert!(reply.update(text.replace('\n', "，"), true).is_err());
    }
}
