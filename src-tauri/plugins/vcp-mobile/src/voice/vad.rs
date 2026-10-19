use super::{SpeechFrame, SpeechTransition, VoiceFrame};
use serde::{Deserialize, Serialize};

const FRAME_SAMPLES: u64 = 512;
const ONSET_SAMPLES: u64 = 1536; // 96 ms at 16 kHz
const SAMPLES_PER_MS: u64 = 16;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "config-export", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum VoiceVadSensitivity {
    Low,
    #[default]
    Standard,
    High,
}

impl VoiceVadSensitivity {
    pub const ALL: [Self; 3] = [Self::Low, Self::Standard, Self::High];

    fn onset_threshold(self, playback: bool) -> f32 {
        match (self, playback) {
            (Self::Low, false) | (Self::Standard, true) => 0.7,
            (Self::Standard, false) | (Self::High, true) => 0.6,
            (Self::High, false) => 0.5,
            (Self::Low, true) => 0.8,
        }
    }
}

/// A microphone session's immutable VAD policy. Millisecond values are kept as
/// entered; decisions occur on the first 32 ms frame that meets the duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "config-export", derive(ts_rs::TS))]
#[serde(default, rename_all = "camelCase")]
pub struct VoiceVadConfig {
    pub sensitivity: VoiceVadSensitivity,
    pub end_silence_ms: u32,
    pub playback_onset_ms: u32,
}

impl VoiceVadConfig {
    pub const MIN_DURATION_MS: u32 = 32;
    pub const MAX_END_SILENCE_MS: u32 = 5000;
    pub const MAX_PLAYBACK_ONSET_MS: u32 = 2000;
    pub const DEFAULT_END_SILENCE_MS: u32 = 512;
    pub const DEFAULT_PLAYBACK_ONSET_MS: u32 = 192;

    /// Minimum evidence in the native 16 kHz sample domain, before frame rounding.
    pub fn onset_samples(&self, playback: bool) -> u64 {
        if playback {
            u64::from(self.playback_onset_ms) * SAMPLES_PER_MS
        } else {
            ONSET_SAMPLES
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(Self::MIN_DURATION_MS..=Self::MAX_END_SILENCE_MS).contains(&self.end_silence_ms) {
            return Err("说完等待时间须为 32–5000 ms 的整数。".into());
        }
        if !(Self::MIN_DURATION_MS..=Self::MAX_PLAYBACK_ONSET_MS).contains(&self.playback_onset_ms) {
            return Err("朗读时插话确认须为 32–2000 ms 的整数。".into());
        }
        Ok(())
    }
}

impl Default for VoiceVadConfig {
    fn default() -> Self {
        Self {
            sensitivity: VoiceVadSensitivity::default(),
            end_silence_ms: Self::DEFAULT_END_SILENCE_MS,
            playback_onset_ms: Self::DEFAULT_PLAYBACK_ONSET_MS,
        }
    }
}

#[derive(Default)]
pub(super) struct VadDetector {
    config: VoiceVadConfig,
    epoch: Option<u32>,
    last_sample: Option<u64>,
    speaking: bool,
    onset_samples: u64,
    silence_samples: u64,
    onset_playback: bool,
}

impl VadDetector {
    pub(super) fn new(config: VoiceVadConfig) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }

    pub(super) fn audio_epoch(&self) -> Option<u32> {
        self.epoch
    }

    pub(super) fn process(&mut self, frame: VoiceFrame) -> Option<SpeechFrame> {
        let valid_unit = |value: f32| value.is_finite() && (0.0..=1.0).contains(&value);
        if frame.sample_index < FRAME_SAMPLES
            || !valid_unit(frame.probability)
            || !valid_unit(frame.level)
            || !frame.bands.iter().copied().all(valid_unit)
        {
            return None;
        }
        if self.epoch.is_some_and(|epoch| frame.audio_epoch < epoch) {
            return None;
        }
        let same_epoch = self.epoch == Some(frame.audio_epoch);
        if same_epoch
            && self
                .last_sample
                .is_some_and(|last| frame.sample_index <= last)
        {
            return None;
        }
        let discontinuity = self.epoch.is_some()
            && (!same_epoch
                || self.last_sample.is_some_and(|last| {
                    last.checked_add(FRAME_SAMPLES) != Some(frame.sample_index)
                }));
        let mut transition = None;
        if discontinuity {
            if self.speaking {
                transition = Some(SpeechTransition::End);
            }
            self.speaking = false;
            self.onset_samples = 0;
            self.silence_samples = 0;
        }
        self.epoch = Some(frame.audio_epoch);
        self.last_sample = Some(frame.sample_index);

        if self.speaking {
            if frame.probability < 0.35 {
                self.silence_samples += FRAME_SAMPLES;
                if self.silence_samples >= u64::from(self.config.end_silence_ms) * SAMPLES_PER_MS {
                    self.speaking = false;
                    self.silence_samples = 0;
                    transition = Some(SpeechTransition::End);
                }
            } else {
                self.silence_samples = 0;
            }
        } else {
            if self.onset_playback != frame.playback_active {
                self.onset_samples = 0;
                self.onset_playback = frame.playback_active;
            }
            let threshold = self.config.sensitivity.onset_threshold(frame.playback_active);
            let duration = self.config.onset_samples(frame.playback_active);
            if frame.probability >= threshold {
                self.onset_samples += FRAME_SAMPLES;
                // A one-frame onset must not replace the End already emitted
                // for a discontinuity. Keep this evidence for the next frame.
                if self.onset_samples >= duration && transition.is_none() {
                    self.speaking = true;
                    self.onset_samples = 0;
                    transition = Some(SpeechTransition::Start);
                }
            } else {
                self.onset_samples = 0;
            }
        }
        Some(SpeechFrame {
            frame,
            speech_active: self.speaking,
            speech_transition: transition,
            discontinuity,
        })
    }
}
