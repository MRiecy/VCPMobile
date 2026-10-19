use super::*;

// Keep cancellation tests independent of the native lock contention deliberately
// exercised by the concurrency test below.
static TEST_LOCK: Mutex<()> = Mutex::new(());

fn tone(samples: usize) -> Vec<u8> {
    (0..samples)
        .flat_map(|index| {
            let phase = index as f64 * 440.0 * std::f64::consts::TAU / 16_000.0;
            ((phase.sin() * 12_000.0) as i16).to_le_bytes()
        })
        .collect()
}

// Read the actual MPEG frame headers rather than calling back into the encoder.
// MPEG-2 Layer III: 576 samples/frame, 72 * bitrate / rate bytes/frame.
fn assert_mp3_frames(mp3: &[u8], input_samples: usize) {
    let mut offset = 0;
    let mut frames = 0;
    while offset < mp3.len() {
        assert!(mp3.len() - offset >= 4, "truncated frame header");
        let header = u32::from_be_bytes(mp3[offset..offset + 4].try_into().unwrap());
        assert_eq!(header >> 21, 0x7ff, "frame sync at {offset}");
        assert_eq!((header >> 19) & 3, 2, "MPEG-2");
        assert_eq!((header >> 17) & 3, 1, "Layer III");
        assert_eq!((header >> 12) & 15, 8, "64 kbps on every frame");
        assert_eq!((header >> 10) & 3, 2, "16 kHz on every frame");
        assert_eq!((header >> 6) & 3, 3, "mono on every frame");
        let padding = ((header >> 9) & 1) as usize;
        offset += 72 * 64_000 / 16_000 + padding;
        assert!(offset <= mp3.len(), "truncated MP3 tail");
        frames += 1;
    }
    assert_eq!(offset, mp3.len(), "no leading or trailing ID3 data");
    // Full input plus the encoder's 576-sample delay must survive final flush.
    assert!(frames * 576 >= input_samples + 576);
    assert!(frames * 576 < input_samples + 4 * 576);
}

#[test]
fn real_cbr_stream_has_complete_frames_and_tail() {
    let _test = TEST_LOCK.lock().unwrap();
    // Non-frame/non-chunk-aligned length exercises the last partial PCM block.
    let samples = 16_123;
    let result = encode_pcm16_mono(&tone(samples), || false).unwrap();
    assert_mp3_frames(&result, samples);
}

#[test]
fn accepts_one_sample_and_exactly_thirty_seconds() {
    let _test = TEST_LOCK.lock().unwrap();
    assert_mp3_frames(&encode_pcm16_mono(&[0, 0], || false).unwrap(), 1);
    let pcm = vec![0; 16_000 * 30 * 2];
    let encoded = encode_pcm16_mono(&pcm, || false).unwrap();
    assert_mp3_frames(&encoded, 16_000 * 30);
    // Independently check the rate-derived size with a generous frame allowance.
    assert!((240_000..242_000).contains(&encoded.len()));
}

#[test]
fn rejects_invalid_pcm_before_calling_native_code() {
    let _test = TEST_LOCK.lock().unwrap();
    assert_eq!(
        encode_pcm16_mono(&[], || false),
        Err(EncodeError::EmptyInput)
    );
    assert_eq!(
        encode_pcm16_mono(&[0], || false),
        Err(EncodeError::InvalidPcmLength)
    );
    assert_eq!(
        encode_pcm16_mono(&vec![0; 16_000 * 30 * 2 + 2], || false),
        Err(EncodeError::DurationExceeded)
    );
}

#[test]
fn cancellation_discards_partial_audio_and_releases_the_handle() {
    let _test = TEST_LOCK.lock().unwrap();
    let pcm = tone(16_000);
    assert_eq!(
        encode_pcm16_mono(&pcm, || true),
        Err(EncodeError::Cancelled)
    );
    let mut probes = 0;
    let cancelled = encode_pcm16_mono(&pcm, || {
        probes += 1;
        probes >= 6
    });
    assert_eq!(cancelled, Err(EncodeError::Cancelled));
    let completed = encode_pcm16_mono(&pcm, || false).unwrap();
    assert_mp3_frames(&completed, 16_000);
}

#[test]
fn cancellation_remains_available_while_another_encoder_holds_the_lock() {
    let _test = TEST_LOCK.lock().unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let owner = std::thread::spawn(move || {
        let _guard = ENCODER_LOCK.lock().unwrap();
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx.recv().unwrap();
    let mut probes = 0;
    let cancelled = encode_pcm16_mono(&[0, 0], || {
        probes += 1;
        probes >= 3
    });
    release_tx.send(()).unwrap();
    owner.join().unwrap();
    assert_eq!(cancelled, Err(EncodeError::Cancelled));
    assert_eq!(probes, 3);
}

#[test]
fn concurrent_callers_produce_independent_complete_streams() {
    let _test = TEST_LOCK.lock().unwrap();
    let input = tone(8_123);
    let expected = encode_pcm16_mono(&input, || false).unwrap();
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..4)
            .map(|_| scope.spawn(|| encode_pcm16_mono(&input, || false).unwrap()))
            .collect();
        for worker in workers {
            let actual = worker.join().unwrap();
            assert_eq!(actual, expected);
            assert_mp3_frames(&actual, 8_123);
        }
    });
}
