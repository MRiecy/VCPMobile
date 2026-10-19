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
}

/// Main publishes intent before altering playback; the tap only takes atomic
/// snapshots. PCM keeps its original stamp through resampling and inference.
final class VoiceProbeCaptureState {
    let revision = Atomic<UInt64>(0)
    let playing = Atomic<Bool>(false)
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
    private let readIndex = Atomic<UInt64>(0)
    private let writeIndex = Atomic<UInt64>(0)
    private let cancelled = Atomic<Bool>(false)
    private let inputFault = Atomic<Bool>(false)
    private let signal: DispatchSourceUserDataAdd
    private let inputRate: Double
    private let inputBuffer: AVAudioPCMBuffer
    private let outputBuffer: AVAudioPCMBuffer
    private let converter: AVAudioConverter?
    private let model: SileroVoiceModel
    private let probeState: VoiceProbeCaptureState
    private let onFrame: (VoiceAnalysisFrame) -> Bool
    private let onError: (String) -> Void
    private let frame: UnsafeMutablePointer<Float>
    private var frameCount = 0
    private var sampleIndex: UInt64 = 0
    private var frameProbeRevision: UInt64 = 0
    private var framePlaybackActive = false
    private let meter = VoiceAudioMeter()

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
        frame = .allocate(capacity: 512)
        samples.initialize(repeating: 0, count: Self.capacity * Self.maxFrames)
        lengths.initialize(repeating: 0, count: Self.capacity)
        probeRevisions.initialize(repeating: 0, count: Self.capacity)
        probePlaying.initialize(repeating: false, count: Self.capacity)
        frame.initialize(repeating: 0, count: 512)
        signal = DispatchSource.makeUserDataAddSource(queue: queue)
        model.reset()
        signal.setEventHandler { [weak self] in self?.drain() }
        signal.resume()
    }

    func accept(_ buffer: AVAudioPCMBuffer) {
        if cancelled.load(ordering: .acquiring) { return }
        let probeRevision = probeState.revision.load(ordering: .acquiring)
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
        writeIndex.store(write + 1, ordering: .releasing)
        signal.add(data: 1)
    }

    func stop() {
        cancelled.store(true, ordering: .releasing)
        signal.cancel()
    }

    private func drain() {
        do {
            while !cancelled.load(ordering: .acquiring) {
                if inputFault.load(ordering: .acquiring) { throw voiceAudioFailure("audio_input_discontinuity") }
                let read = readIndex.load(ordering: .relaxed)
                if read == writeIndex.load(ordering: .acquiring) { return }
                let slot = Int(read % UInt64(Self.capacity))
                let count = lengths[slot]
                let probeRevision = probeRevisions[slot]
                let playbackActive = probePlaying[slot]
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
                    try consume(converted, count: Int(outputBuffer.frameLength), probeRevision: probeRevision, playbackActive: playbackActive)
                } else {
                    try consume(target, count: count, probeRevision: probeRevision, playbackActive: playbackActive)
                }
            }
        } catch {
            if !cancelled.exchange(true, ordering: .acquiringAndReleasing) {
                signal.cancel()
                onError(voiceAudioReason(error, fallback: "audio_processing_failed"))
            }
        }
    }

    private func consume(_ input: UnsafePointer<Float>, count: Int, probeRevision: UInt64, playbackActive: Bool) throws {
        for index in 0..<count {
            if cancelled.load(ordering: .relaxed) { return }
            guard input[index].isFinite else { throw voiceAudioFailure("audio_sample_invalid") }
            if frameCount == 0 {
                frameProbeRevision = probeRevision
                framePlaybackActive = playbackActive
            }
            frame[frameCount] = input[index]
            frameCount += 1
            if frameCount != 512 { continue }
            frameCount = 0
            let probability = try model.probability(UnsafePointer(frame))
            sampleIndex += 512
            let reading = meter.measure(UnsafePointer(frame))
            if !cancelled.load(ordering: .acquiring) {
                guard onFrame(VoiceAnalysisFrame(sampleIndex: sampleIndex, probability: probability, level: reading.level, bands: reading.bands,
                                                probeRevision: frameProbeRevision, playbackActive: framePlaybackActive)) else {
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
        frame.deinitialize(count: 512)
        samples.deallocate()
        lengths.deallocate()
        probeRevisions.deallocate()
        probePlaying.deallocate()
        frame.deallocate()
    }
}
