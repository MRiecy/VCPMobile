import AVFoundation
import Foundation
import Synchronization

struct VoiceAnalysisFrame {
    let sampleIndex: UInt64
    let probability: Float
    let level: Float
    let bands: [Float]
    let probeRevision: UInt64
    let playbackActive: Bool
    let playbackMarker: UInt64
}

/// Main publishes intent before altering playback; the tap only takes atomic
/// snapshots. PCM keeps its original stamp through resampling and inference.
final class VoiceProbeCaptureState {
    let revision = Atomic<UInt64>(0)
    let playing = Atomic<Bool>(false)
    let playbackMarker = Atomic<UInt64>(0)
    func replaceIntent(_ revision: UInt64) {
        playing.store(false, ordering: .releasing)
        self.revision.store(revision, ordering: .releasing)
    }
}

/// A single-producer (audio tap), single-consumer (inference queue) ring.
/// The tap never invokes ORT, JSON, a main-queue block, a lock or an allocator.
/// An overrun is a discontinuity and stops the operation instead of silently
/// concatenating noncontiguous samples into Silero's recurrent state.
final class VoiceAudioPipe {
    private static let capacity = 8
    private static let maxFrames = 8192
    private let samples: UnsafeMutablePointer<Float>
    private let lengths: UnsafeMutablePointer<Int>
    private let probeRevisions: UnsafeMutablePointer<UInt64>
    private let probePlaying: UnsafeMutablePointer<Bool>
    private let playbackMarkers: UnsafeMutablePointer<UInt64>
    private let readIndex = Atomic<UInt64>(0)
    private let writeIndex = Atomic<UInt64>(0)
    private let cancelled = Atomic<Bool>(false)
    private let accepting = Atomic<Bool>(true)
    private let callbacks = Atomic<UInt32>(0)
    private let released = Atomic<Bool>(false)
    private let inputFault = Atomic<Bool>(false)
    private let signal: DispatchSourceUserDataAdd
    private let inputRate: Double
    private let inputBuffer: AVAudioPCMBuffer
    private let outputBuffer: AVAudioPCMBuffer
    private let converter: AVAudioConverter?
    private var model: SileroVoiceModel?
    private let probeState: VoiceProbeCaptureState
    private let onFrame: (VoiceAnalysisFrame) -> Bool
    private let onError: (String) -> Void
    private let frame: UnsafeMutablePointer<Float>
    private var frameCount = 0
    private var sampleIndex: UInt64 = 0
    private var processingFailure: String?
    private var frameProbeRevision: UInt64 = 0
    private var framePlaybackActive = false
    private var framePlaybackMarker: UInt64 = 0
    private let meter = VoiceAudioMeter()
    private let segments = VoiceSegmentCache()

    init(inputRate: Double, model: SileroVoiceModel, probeState: VoiceProbeCaptureState, queue: DispatchQueue,
         onFrame: @escaping (VoiceAnalysisFrame) -> Bool, onError: @escaping (String) -> Void) throws {
        guard inputRate.isFinite, inputRate >= 8_000, inputRate <= 192_000,
              let inputFormat = AVAudioFormat(standardFormatWithSampleRate: inputRate, channels: 1),
              let outputFormat = AVAudioFormat(standardFormatWithSampleRate: 16_000, channels: 1),
              let inputBuffer = AVAudioPCMBuffer(pcmFormat: inputFormat, frameCapacity: AVAudioFrameCount(Self.maxFrames)),
              let outputBuffer = AVAudioPCMBuffer(pcmFormat: outputFormat, frameCapacity: 16_512) else {
            throw voiceAudioFailure("audio_format_unavailable")
        }
        self.inputRate = inputRate
        self.inputBuffer = inputBuffer
        self.outputBuffer = outputBuffer
        converter = inputRate == 16_000 ? nil : AVAudioConverter(from: inputFormat, to: outputFormat)
        if inputRate != 16_000 && converter == nil { throw voiceAudioFailure("audio_converter_unavailable") }
        self.model = model
        self.probeState = probeState
        self.onFrame = onFrame
        self.onError = onError
        samples = .allocate(capacity: Self.capacity * Self.maxFrames)
        lengths = .allocate(capacity: Self.capacity)
        probeRevisions = .allocate(capacity: Self.capacity)
        probePlaying = .allocate(capacity: Self.capacity)
        playbackMarkers = .allocate(capacity: Self.capacity)
        frame = .allocate(capacity: 512)
        samples.initialize(repeating: 0, count: Self.capacity * Self.maxFrames)
        lengths.initialize(repeating: 0, count: Self.capacity)
        probeRevisions.initialize(repeating: 0, count: Self.capacity)
        probePlaying.initialize(repeating: false, count: Self.capacity)
        playbackMarkers.initialize(repeating: 0, count: Self.capacity)
        frame.initialize(repeating: 0, count: 512)
        signal = DispatchSource.makeUserDataAddSource(queue: queue)
        model.reset()
        signal.setEventHandler { [weak self] in self?.drain() }
        signal.resume()
    }

    func accept(_ buffer: AVAudioPCMBuffer) {
        callbacks.wrappingAdd(1, ordering: .acquiringAndReleasing)
        defer { callbacks.wrappingSubtract(1, ordering: .acquiringAndReleasing) }
        if !accepting.load(ordering: .acquiring) || cancelled.load(ordering: .acquiring) { return }
        let probeRevision = probeState.revision.load(ordering: .acquiring)
        let playbackMarker = probeState.playbackMarker.load(ordering: .acquiring)
        let playbackActive = probeState.playing.load(ordering: .acquiring)
        let count = Int(buffer.frameLength)
        let channels = Int(buffer.format.channelCount)
        guard count > 0 else { return }
        let write = writeIndex.load(ordering: .relaxed)
        let read = readIndex.load(ordering: .acquiring)
        guard count <= Self.maxFrames, channels > 0, channels <= 8,
              buffer.format.sampleRate == inputRate,
              buffer.format.commonFormat == .pcmFormatFloat32,
              let source = buffer.floatChannelData,
              write - read < UInt64(Self.capacity) else {
            inputFault.store(true, ordering: .releasing)
            signal.add(data: 1)
            return
        }
        let slot = Int(write % UInt64(Self.capacity))
        let target = samples.advanced(by: slot * Self.maxFrames)
        let interleaved = buffer.format.isInterleaved
        for index in 0..<count {
            var value: Float = 0
            for channel in 0..<channels {
                value += interleaved ? source[0][index * channels + channel] : source[channel][index]
            }
            target[index] = value / Float(channels)
        }
        lengths[slot] = count
        probeRevisions[slot] = probeRevision
        probePlaying[slot] = playbackActive
        playbackMarkers[slot] = playbackMarker
        writeIndex.store(write + 1, ordering: .releasing)
        signal.add(data: 1)
    }

    func stop() {
        accepting.store(false, ordering: .releasing)
        cancelled.store(true, ordering: .releasing)
        signal.cancel()
    }

    func stopAccepting() { accepting.store(false, ordering: .releasing) }

    /// Called after the input callback has been removed/stopped, on worker.
    /// Drain already-admitted PCM without publishing new VAD transitions, and
    /// keep even the final sub-512-sample tail for the last ASR export.
    func seal(on queue: DispatchQueue, completion: @escaping (UInt64, String?) -> Void) {
        // removeTap prevents new delivery, but do not assume an already-entered
        // real-time callback has returned. Wait asynchronously off main.
        if callbacks.load(ordering: .acquiring) != 0 {
            queue.asyncAfter(deadline: .now() + .milliseconds(1)) { [self] in seal(on: queue, completion: completion) }
            return
        }
        drain(publish: false)
        // noDataNow during live input preserves converter state; endOfStream
        // is needed to retrieve its final resampled samples before sealing.
        if let converter = converter, !cancelled.load(ordering: .acquiring) {
            do {
                var ended = false
                for _ in 0..<4 {
                    var error: NSError?
                    outputBuffer.frameLength = 0
                    let status = converter.convert(to: outputBuffer, error: &error) { _, inputStatus in
                        inputStatus.pointee = .endOfStream
                        return nil
                    }
                    if status == .error { throw error ?? voiceAudioFailure("audio_conversion_failed") }
                    guard let samples = outputBuffer.floatChannelData?[0] else { throw voiceAudioFailure("audio_output_buffer_missing") }
                    try consume(samples, count: Int(outputBuffer.frameLength), probeRevision: frameProbeRevision,
                                playbackActive: framePlaybackActive, playbackMarker: framePlaybackMarker, publish: false)
                    if status == .endOfStream { ended = true; break }
                }
                if !ended { throw voiceAudioFailure("audio_conversion_drain_failed") }
            } catch { processingFailure = voiceAudioReason(error, fallback: "audio_conversion_failed") }
        }
        if frameCount > 0 {
            segments.append(UnsafePointer(frame), count: frameCount)
            sampleIndex += UInt64(frameCount)
            frameCount = 0
        }
        stop()
        model = nil
        completion(sampleIndex, processingFailure)
    }

    // Called only on the same serial queue that drains and converts input.
    func snapshotSegment(start: UInt64, end: UInt64) throws -> VoiceSegmentSnapshot {
        guard !released.load(ordering: .acquiring) else { throw voiceAudioFailure("cancelled:voice_segment") }
        return try segments.snapshot(start: start, end: end)
    }

    func clearSegments() { released.store(true, ordering: .releasing); segments.clear() }

    private func drain(publish: Bool = true) {
        do {
            while !cancelled.load(ordering: .acquiring) {
                if inputFault.load(ordering: .acquiring) { throw voiceAudioFailure("audio_input_discontinuity") }
                let read = readIndex.load(ordering: .relaxed)
                if read == writeIndex.load(ordering: .acquiring) { return }
                let slot = Int(read % UInt64(Self.capacity))
                let count = lengths[slot]
                let probeRevision = probeRevisions[slot]
                let playbackActive = probePlaying[slot]
                let playbackMarker = playbackMarkers[slot]
                inputBuffer.frameLength = AVAudioFrameCount(count)
                guard let target = inputBuffer.floatChannelData?[0] else { throw voiceAudioFailure("audio_input_buffer_missing") }
                memcpy(target, samples.advanced(by: slot * Self.maxFrames), count * MemoryLayout<Float>.size)
                readIndex.store(read + 1, ordering: .releasing)
                if let converter = converter {
                    var supplied = false
                    var error: NSError?
                    outputBuffer.frameLength = 0
                    let status = converter.convert(to: outputBuffer, error: &error) { [inputBuffer] _, inputStatus in
                        if supplied {
                            inputStatus.pointee = .noDataNow
                            return nil
                        }
                        supplied = true
                        inputStatus.pointee = .haveData
                        return inputBuffer
                    }
                    if status == .error { throw error ?? voiceAudioFailure("audio_conversion_failed") }
                    guard let converted = outputBuffer.floatChannelData?[0] else { throw voiceAudioFailure("audio_output_buffer_missing") }
                    try consume(converted, count: Int(outputBuffer.frameLength), probeRevision: probeRevision, playbackActive: playbackActive, playbackMarker: playbackMarker, publish: publish)
                } else {
                    try consume(target, count: count, probeRevision: probeRevision, playbackActive: playbackActive, playbackMarker: playbackMarker, publish: publish)
                }
            }
        } catch {
            processingFailure = voiceAudioReason(error, fallback: "audio_processing_failed")
            if !cancelled.exchange(true, ordering: .acquiringAndReleasing) {
                signal.cancel()
                onError(processingFailure ?? "audio_processing_failed")
            }
        }
    }

    private func consume(_ input: UnsafePointer<Float>, count: Int, probeRevision: UInt64, playbackActive: Bool, playbackMarker: UInt64, publish: Bool) throws {
        for index in 0..<count {
            if cancelled.load(ordering: .relaxed) { return }
            guard input[index].isFinite else { throw voiceAudioFailure("audio_sample_invalid") }
            if frameCount == 0 {
                frameProbeRevision = probeRevision
                framePlaybackActive = playbackActive
                framePlaybackMarker = playbackMarker
            }
            frame[frameCount] = input[index]
            frameCount += 1
            if frameCount != 512 { continue }
            frameCount = 0
            segments.append(UnsafePointer(frame), count: 512)
            sampleIndex += 512
            if !publish || !accepting.load(ordering: .acquiring) { continue }
            guard let model = model else { throw voiceAudioFailure("vad_model_released") }
            let probability = try model.probability(UnsafePointer(frame))
            let reading = meter.measure(UnsafePointer(frame))
            if accepting.load(ordering: .acquiring) && !cancelled.load(ordering: .acquiring) {
                guard onFrame(VoiceAnalysisFrame(sampleIndex: sampleIndex, probability: probability, level: reading.level, bands: reading.bands,
                                                probeRevision: frameProbeRevision, playbackActive: framePlaybackActive, playbackMarker: framePlaybackMarker)) else {
                    throw voiceAudioFailure("audio_event_backpressure")
                }
            }
        }
    }

    deinit {
        signal.cancel()
        samples.deinitialize(count: Self.capacity * Self.maxFrames)
        lengths.deinitialize(count: Self.capacity)
        probeRevisions.deinitialize(count: Self.capacity)
        probePlaying.deinitialize(count: Self.capacity)
        playbackMarkers.deinitialize(count: Self.capacity)
        frame.deinitialize(count: 512)
        samples.deallocate()
        lengths.deallocate()
        probeRevisions.deallocate()
        probePlaying.deallocate()
        playbackMarkers.deallocate()
        frame.deallocate()
    }
}
