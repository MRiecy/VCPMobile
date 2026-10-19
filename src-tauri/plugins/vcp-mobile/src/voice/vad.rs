use super::{SpeechFrame, SpeechTransition, VoiceFrame};

const FRAME_SAMPLES: u64 = 512;
const ONSET_SAMPLES: u64 = 1536; // 96 ms at 16 kHz
const PLAYBACK_ONSET_SAMPLES: u64 = 3072; // 192 ms
const OFFSET_SAMPLES: u64 = 8192; // 512 ms

#[derive(Default)]
pub(super) struct VadDetector {
    epoch: Option<u32>,
    last_sample: Option<u64>,
    speaking: bool,
    onset_samples: u64,
    silence_samples: u64,
    onset_playback: bool,
}

impl VadDetector {
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
                if self.silence_samples >= OFFSET_SAMPLES {
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
            let (threshold, duration) = if frame.playback_active {
                (0.7, PLAYBACK_ONSET_SAMPLES)
            } else {
                (0.6, ONSET_SAMPLES)
            };
            if frame.probability >= threshold {
                self.onset_samples += FRAME_SAMPLES;
                if self.onset_samples >= duration {
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
