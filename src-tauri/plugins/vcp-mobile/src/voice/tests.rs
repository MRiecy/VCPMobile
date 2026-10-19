use super::*;

fn frame(index: u64, probability: f32, playback: bool) -> VoiceFrame {
    VoiceFrame {
        session_id: "call-a".into(),
        audio_epoch: 1,
        sample_index: index,
        probability,
        level: 0.25,
        bands: [0.1; 32],
        aec_state: AecState::Enabled,
        route: "speaker".into(),
        playback_active: playback,
        probe_revision: 0,
    }
}

fn notice(id: &str, epoch: u32) -> VoiceNotice {
    VoiceNotice {
        session_id: id.into(),
        audio_epoch: epoch,
        reason: "background".into(),
    }
}

#[test]
fn onset_uses_three_audio_frames_not_arrival_time() {
    let mut detector = VadDetector::default();
    for index in [512, 1024] {
        let result = detector.process(frame(index, 0.6, false)).unwrap();
        assert!(!result.speech_active);
        assert_eq!(result.speech_transition, None);
    }
    let result = detector.process(frame(1536, 0.6, false)).unwrap();
    assert!(result.speech_active);
    assert_eq!(result.speech_transition, Some(SpeechTransition::Start));
    assert_eq!(
        detector
            .process(frame(2048, 0.9, false))
            .unwrap()
            .speech_transition,
        None
    );
}

#[test]
fn playback_requires_six_consecutive_frames_at_the_higher_threshold() {
    let mut detector = VadDetector::default();
    for step in 1..=10 {
        assert!(
            !detector
                .process(frame(step * 512, 0.69, true))
                .unwrap()
                .speech_active
        );
    }
    for step in 11..=15 {
        assert!(
            !detector
                .process(frame(step * 512, 0.7, true))
                .unwrap()
                .speech_active
        );
    }
    assert_eq!(
        detector
            .process(frame(16 * 512, 0.7, true))
            .unwrap()
            .speech_transition,
        Some(SpeechTransition::Start)
    );
}

#[test]
fn short_silence_and_hysteresis_do_not_end_an_active_utterance() {
    let mut detector = VadDetector::default();
    for step in 1..=3 {
        detector.process(frame(step * 512, 0.9, false));
    }
    for step in 4..=18 {
        assert!(
            detector
                .process(frame(step * 512, 0.2, false))
                .unwrap()
                .speech_active
        );
    }
    // The exit threshold is strictly below 0.35; a speech continuation breaks
    // the silence run without requiring the entry threshold again.
    assert!(
        detector
            .process(frame(19 * 512, 0.35, false))
            .unwrap()
            .speech_active
    );
    for step in 20..=34 {
        assert!(
            detector
                .process(frame(step * 512, 0.0, false))
                .unwrap()
                .speech_active
        );
    }
    let ended = detector.process(frame(35 * 512, 0.0, false)).unwrap();
    assert!(!ended.speech_active);
    assert_eq!(ended.speech_transition, Some(SpeechTransition::End));
}

#[test]
fn missing_frames_reset_evidence_instead_of_counting_the_gap_as_speech() {
    let mut detector = VadDetector::default();
    detector.process(frame(512, 0.9, false));
    detector.process(frame(1024, 0.9, false));
    let after_gap = detector.process(frame(8192, 0.9, false)).unwrap();
    assert!(after_gap.discontinuity);
    assert!(!after_gap.speech_active);
    assert!(
        !detector
            .process(frame(8704, 0.9, false))
            .unwrap()
            .speech_active
    );
    assert!(
        detector
            .process(frame(9216, 0.9, false))
            .unwrap()
            .speech_active
    );
}

#[test]
fn route_epoch_reset_ends_old_speech_and_rejects_delayed_old_frames() {
    let mut detector = VadDetector::default();
    for step in 1..=3 {
        detector.process(frame(step * 512, 0.9, false));
    }
    let mut replacement = frame(512, 0.9, false);
    replacement.audio_epoch = 2;
    let result = detector.process(replacement.clone()).unwrap();
    assert!(result.discontinuity);
    assert!(!result.speech_active);
    assert_eq!(result.speech_transition, Some(SpeechTransition::End));
    assert!(detector.process(frame(2048, 0.9, false)).is_none());
    assert!(detector.process(replacement.clone()).is_none());
    replacement.sample_index = 1024;
    assert!(!detector.process(replacement.clone()).unwrap().speech_active);
    replacement.sample_index = 1536;
    assert!(detector.process(replacement).unwrap().speech_active);
}

#[test]
fn playback_change_starts_a_new_confirmation_window() {
    let mut detector = VadDetector::default();
    detector.process(frame(512, 0.9, false));
    detector.process(frame(1024, 0.9, false));
    for step in 3..=7 {
        assert!(
            !detector
                .process(frame(step * 512, 0.9, true))
                .unwrap()
                .speech_active
        );
    }
    assert!(
        detector
            .process(frame(4096, 0.9, true))
            .unwrap()
            .speech_active
    );
}

#[test]
fn invalid_measurements_and_repeated_samples_cannot_supply_onset_evidence() {
    let mut detector = VadDetector::default();
    detector.process(frame(512, 0.9, false));
    assert!(detector.process(frame(512, 0.9, false)).is_none());
    assert!(detector.process(frame(1024, f32::NAN, false)).is_none());
    let mut invalid = frame(1024, 0.9, false);
    invalid.bands[8] = f32::INFINITY;
    assert!(detector.process(invalid).is_none());
    let result = detector.process(frame(1536, 0.9, false)).unwrap();
    assert!(result.discontinuity);
    assert!(!result.speech_active);
}

#[test]
fn cancel_during_start_and_old_completion_cannot_revoke_new_session() {
    let mut sessions = Sessions::default();
    let old = sessions.begin("call-a".into()).unwrap();
    assert_eq!(sessions.cancel("call-a", None), Some(0));
    assert!(!sessions.owns("call-a", old));
    let current = sessions.begin("call-b".into()).unwrap();
    assert!(sessions.cancel("call-a", Some(old)).is_none());
    assert!(sessions.consume_frame(frame(512, 0.9, false)).is_none());
    assert!(sessions
        .consume_terminal(notice("call-a", 8), true)
        .is_none());
    assert!(sessions.owns("call-b", current));
}

#[test]
fn stop_before_start_tombstone_keeps_a_later_active_capture_intact() {
    let mut sessions = Sessions::default();
    assert_eq!(sessions.cancel("call-a", None), None);
    let current = sessions.begin("call-b".into()).unwrap();
    let mut sample = frame(512, 0.9, false);
    sample.session_id = "call-b".into();
    assert!(sessions.consume_frame(sample.clone()).is_some());

    // A's start invocation is only now polled by Rust. Its rejection must not
    // erase B or reset the speech evidence already accumulated for B.
    assert!(sessions
        .begin("call-a".into())
        .unwrap_err()
        .starts_with("cancelled:"));
    assert!(sessions.cancel("call-a", None).is_none());
    assert!(sessions.owns("call-b", current));
    sample.sample_index = 1024;
    sessions.consume_frame(sample.clone()).unwrap();
    sample.sample_index = 1536;
    let (event, _) = sessions.consume_frame(sample).unwrap();
    assert!(matches!(
        event,
        VoiceAudioEvent::Frame(SpeechFrame {
            speech_active: true,
            ..
        })
    ));
    assert!(sessions
        .consume_terminal(notice("call-b", 1), false)
        .is_some());
}

#[test]
fn an_unrelated_start_is_busy_until_the_owner_is_explicitly_cancelled() {
    let mut sessions = Sessions::default();
    let old = sessions.begin("call-a".into()).unwrap();
    assert!(sessions
        .begin("call-b".into())
        .unwrap_err()
        .starts_with("voice_session_busy:"));
    assert!(sessions.owns("call-a", old));
    // This is the synchronous cancellation phase, before any native stop ack.
    sessions.cancel("call-a", None);
    let current = sessions.begin("call-b".into()).unwrap();
    assert!(sessions.owns("call-b", current));
    assert!(sessions.cancel("call-a", Some(old)).is_none());
    assert!(sessions.owns("call-b", current));
}

#[test]
fn cancellation_memory_is_bounded_and_eviction_never_allows_replacing_an_owner() {
    let mut sessions = Sessions::default();
    for index in 0..=CANCELLED_SESSION_LIMIT {
        sessions.cancel(&format!("cancelled-{index}"), None);
    }
    sessions.cancel(&format!("cancelled-{CANCELLED_SESSION_LIMIT}"), None);
    assert_eq!(sessions.cancelled.len(), CANCELLED_SESSION_LIMIT);
    assert!(sessions
        .begin(format!("cancelled-{CANCELLED_SESSION_LIMIT}"))
        .is_err());
    let generation = sessions.begin("live-call".into()).unwrap();
    // Even an ID older than the bounded cancellation memory cannot take over.
    assert!(sessions
        .begin("cancelled-0".into())
        .unwrap_err()
        .starts_with("voice_session_busy:"));
    assert!(sessions.owns("live-call", generation));
}

#[test]
fn native_terminal_closes_only_the_current_audio_epoch() {
    let mut sessions = Sessions::default();
    sessions.begin("call-a".into()).unwrap();
    let mut current = frame(512, 0.9, false);
    current.audio_epoch = 2;
    sessions.consume_frame(current);
    assert!(sessions
        .consume_terminal(notice("call-a", 1), false)
        .is_none());
    assert!(matches!(
        sessions.consume_terminal(notice("call-a", 2), true),
        Some(VoiceAudioEvent::Error(_))
    ));
    assert!(sessions.consume_frame(frame(1024, 0.9, false)).is_none());
}

#[test]
fn probe_stop_is_requested_once_on_confirmed_speech_during_playback() {
    let mut sessions = Sessions::default();
    let generation = sessions.begin("call-a".into()).unwrap();
    for step in 1..=5 {
        assert!(sessions
            .consume_frame(frame(step * 512, 0.8, true))
            .unwrap()
            .1
            .is_none());
    }
    assert_eq!(
        sessions.consume_frame(frame(3072, 0.8, true)).unwrap().1,
        Some((generation, 1))
    );
    assert!(sessions
        .consume_frame(frame(3584, 0.8, true))
        .unwrap()
        .1
        .is_none());
}

#[test]
fn a_new_probe_intent_invalidates_late_setup_without_changing_the_capture_owner() {
    let mut sessions = Sessions::default();
    let generation = sessions.begin("call-a".into()).unwrap();
    let (_, enable_revision) = sessions.probe_intent("call-a", true).unwrap();
    let (_, disable_revision) = sessions.probe_intent("call-a", false).unwrap();
    assert!(!sessions.owns_control("call-a", generation, Some(enable_revision)));
    assert!(sessions.owns_control("call-a", generation, Some(disable_revision)));
    assert_eq!(
        sessions.latest_probe("call-a", generation),
        Some((disable_revision, false))
    );
    assert!(sessions.owns("call-a", generation));
    sessions.cancel("call-a", None);
    sessions.begin("call-b".into()).unwrap();
    assert!(sessions.latest_probe("call-a", generation).is_none());
    assert!(sessions.probe_intent("call-a", false).is_err());
}

#[test]
fn frames_from_an_older_probe_do_not_cancel_a_newer_playback_intent() {
    let mut sessions = Sessions::default();
    let generation = sessions.begin("call-a".into()).unwrap();
    let (_, old_probe) = sessions.probe_intent("call-a", true).unwrap();
    sessions.probe_intent("call-a", false).unwrap();
    let (_, new_probe) = sessions.probe_intent("call-a", true).unwrap();
    for step in 1..=6 {
        let mut old_frame = frame(step * 512, 0.9, true);
        old_frame.probe_revision = old_probe;
        assert!(sessions.consume_frame(old_frame).unwrap().1.is_none());
    }
    assert_eq!(
        sessions.latest_probe("call-a", generation),
        Some((new_probe, true))
    );
}

#[test]
fn automatic_probe_stop_has_a_revision_that_a_later_explicit_play_can_supersede() {
    let mut sessions = Sessions::default();
    let generation = sessions.begin("call-a".into()).unwrap();
    let (_, playing_revision) = sessions.probe_intent("call-a", true).unwrap();
    let mut stop = None;
    for step in 1..=6 {
        let mut current_frame = frame(step * 512, 0.9, true);
        current_frame.probe_revision = playing_revision;
        stop = sessions.consume_frame(current_frame).unwrap().1;
    }
    let (stop_generation, stop_revision) = stop.unwrap();
    assert_eq!(stop_generation, generation);
    assert!(stop_revision > playing_revision);
    assert_eq!(
        sessions.latest_probe("call-a", generation),
        Some((stop_revision, false))
    );
    let (_, next_revision) = sessions.probe_intent("call-a", true).unwrap();
    assert!(next_revision > stop_revision);
    assert!(!sessions.owns_control("call-a", generation, Some(stop_revision)));
}

#[test]
fn event_contract_is_flat_camel_case_and_requires_exactly_32_bands() {
    let mut detector = VadDetector::default();
    let event = VoiceAudioEvent::Frame(detector.process(frame(512, 0.8, false)).unwrap());
    let value = serde_json::to_value(event).unwrap();
    assert_eq!(value["kind"], "frame");
    assert_eq!(value["sessionId"], "call-a");
    assert_eq!(value["audioEpoch"], 1);
    assert_eq!(value["sampleIndex"], 512);
    assert_eq!(value["speechActive"], false);
    assert_eq!(value["speechTransition"], serde_json::Value::Null);
    assert_eq!(value["aecState"], "enabled");
    assert_eq!(value["probeRevision"], 0);
    assert!(value.get("frame").is_none());
    let native: crate::events::NativeEvent = serde_json::from_value(serde_json::json!({
        "event": "voiceAudio", "payload": value,
    }))
    .unwrap();
    assert!(matches!(
        native,
        crate::events::NativeEvent::VoiceAudio(NativeVoiceAudioEvent::Frame(_))
    ));
    let mut invalid = serde_json::to_value(frame(512, 0.8, false)).unwrap();
    invalid["bands"] = serde_json::json!([0.1, 0.2]);
    assert!(serde_json::from_value::<VoiceFrame>(invalid).is_err());
}
