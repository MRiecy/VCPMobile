//! Strict PCM16 mono RIFF reader for the native ASR export.

pub(super) fn pcm16_mono(bytes: &[u8], sample_rate: u32) -> Result<Vec<u8>, String> {
    let invalid = || "语音服务返回的 WAV 音频格式无效。".to_string();
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(invalid());
    }
    let read_u16 = |slice: &[u8]| u16::from_le_bytes([slice[0], slice[1]]);
    let read_u32 = |slice: &[u8]| u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]);
    let end = (read_u32(&bytes[4..8]) as usize)
        .checked_add(8)
        .ok_or_else(invalid)?;
    if end > bytes.len() || end < 12 {
        return Err(invalid());
    }
    let mut position = 12usize;
    let mut format_found = false;
    let mut pcm = Vec::new();
    while position < end {
        if end - position < 8 {
            return Err(invalid());
        }
        let id = &bytes[position..position + 4];
        let size = read_u32(&bytes[position + 4..position + 8]) as usize;
        let start = position + 8;
        let chunk_end = start
            .checked_add(size)
            .filter(|value| *value <= end)
            .ok_or_else(invalid)?;
        if id == b"fmt " {
            if size < 16 || format_found {
                return Err(invalid());
            }
            let format = &bytes[start..chunk_end];
            if read_u16(format) != 1
                || read_u16(&format[2..]) != 1
                || read_u32(&format[4..]) != sample_rate
                || read_u16(&format[12..]) != 2
                || read_u16(&format[14..]) != 16
            {
                return Err("语音服务需要单声道 PCM16 音频及约定采样率。".into());
            }
            format_found = true;
        } else if id == b"data" {
            if size % 2 != 0 {
                return Err(invalid());
            }
            pcm.extend_from_slice(&bytes[start..chunk_end]);
        }
        position = chunk_end
            .checked_add(size % 2)
            .filter(|value| *value <= end)
            .ok_or_else(invalid)?;
    }
    if !format_found || pcm.is_empty() {
        return Err(invalid());
    }
    Ok(pcm)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn wav_fixture() -> Vec<u8> {
        let mut bytes = b"RIFF\0\0\0\0WAVEJUNK\x03\0\0\0abc\0fmt \x10\0\0\0".to_vec();
        bytes.extend_from_slice(&[1, 0, 1, 0]);
        bytes.extend_from_slice(&24000u32.to_le_bytes());
        bytes.extend_from_slice(&48000u32.to_le_bytes());
        bytes.extend_from_slice(&[2, 0, 16, 0]);
        bytes.extend_from_slice(b"data\x04\0\0\0\x01\x02\x03\x04");
        let size = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&size.to_le_bytes());
        bytes
    }

    #[test]
    fn finds_pcm_after_unknown_odd_sized_chunk_and_padding() {
        assert_eq!(pcm16_mono(&wav_fixture(), 24000).unwrap(), [1, 2, 3, 4]);
    }

    #[test]
    fn truncated_chunks_and_wrong_rates_are_rejected() {
        let bytes = wav_fixture();
        assert!(pcm16_mono(&bytes[..bytes.len() - 1], 24000).is_err());
        assert!(pcm16_mono(&bytes, 16000).is_err());
    }
}
