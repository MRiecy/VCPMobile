//! Bounded SSE framing. UTF-8 is decoded only after a complete line arrives.

const MAX_EVENT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Default)]
pub(super) struct SseDecoder {
    line: Vec<u8>,
    data: String,
    skip_lf: bool,
    first_line: bool,
}

impl SseDecoder {
    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, String> {
        if bytes.len() > MAX_EVENT_BYTES {
            return Err("语音服务返回的数据块过大。".into());
        }
        let mut events = Vec::new();
        for &byte in bytes {
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' {
                    continue;
                }
            }
            if byte == b'\n' || byte == b'\r' {
                self.skip_lf = byte == b'\r';
                let line = std::str::from_utf8(&self.line)
                    .map_err(|_| "语音服务返回了无效的文本编码。")?;
                let line = if !self.first_line {
                    line.strip_prefix('\u{feff}').unwrap_or(line)
                } else {
                    line
                };
                self.first_line = true;
                if line.is_empty() {
                    if !self.data.is_empty() {
                        self.data.pop();
                        events.push(std::mem::take(&mut self.data));
                    }
                } else if let Some(value) = line.strip_prefix("data:") {
                    self.data.push_str(value.strip_prefix(' ').unwrap_or(value));
                    self.data.push('\n');
                } else if line == "data" {
                    self.data.push('\n');
                }
                self.line.clear();
            } else {
                self.line.push(byte);
            }
            if self.line.len() + self.data.len() > MAX_EVENT_BYTES {
                return Err("语音服务返回的单个事件过大。".into());
            }
        }
        Ok(events)
    }

    pub(super) fn finish(&self) -> Result<(), String> {
        if self.line.is_empty() && self.data.is_empty() {
            Ok(())
        } else {
            Err("语音服务响应在完整事件到达前中断。".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_byte_boundary_preserves_utf8_crlf_json_and_base64() {
        let input = "\u{feff}: keepalive\r\ndata: {\"text\":\"你好\",\"audio\":\"AQIDBA==\"}\r\n\r\ndata: [DONE]\n\n".as_bytes();
        for split in 0..=input.len() {
            let mut decoder = SseDecoder::default();
            let mut events = decoder.push(&input[..split]).unwrap();
            events.extend(decoder.push(&input[split..]).unwrap());
            decoder.finish().unwrap();
            assert_eq!(
                events,
                ["{\"text\":\"你好\",\"audio\":\"AQIDBA==\"}", "[DONE]"]
            );
        }
    }

    #[test]
    fn incomplete_and_oversized_events_are_errors_not_partial_success() {
        let mut decoder = SseDecoder::default();
        decoder.push(b"data: {\"incomplete\":").unwrap();
        assert!(decoder.finish().is_err());
        assert!(decoder.push(&vec![b'x'; MAX_EVENT_BYTES]).is_err());
    }
}
