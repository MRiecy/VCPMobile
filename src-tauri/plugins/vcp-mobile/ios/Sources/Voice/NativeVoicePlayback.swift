import AVFoundation
import Foundation
import Tauri

/// Main owns playback and a ten-second frame budget, including scheduled audio.
/// The decode queue holds at most one bounded append. Tiny transport packets are
/// coalesced before scheduling, so packet count cannot prevent startup admission.
final class NativeVoicePlayback {
    private static let capacityFrames = 240_000
    private static let minimumChunkFrames = 480
    private static let maximumChunkFrames = 2_400
    private static let scheduledChunkLimit = capacityFrames / minimumChunkFrames
    private static let stopFadeSeconds = 0.060
    private struct Chunk {
        let key = UUID()
        let buffer: AVAudioPCMBuffer
        var frames: Int { Int(buffer.frameLength) }
    }
    private final class PendingFrames {
        private var samples = [Float](repeating: 0, count: NativeVoicePlayback.capacityFrames)
        private var head = 0
        private(set) var count = 0

        func append(_ input: [Float]) {
            precondition(input.count <= samples.count - count)
            for (offset, sample) in input.enumerated() {
                samples[(head + count + offset) % samples.count] = sample
            }
            count += input.count
        }

        func take(_ frames: Int, format: AVAudioFormat) throws -> Chunk {
            guard frames > 0, frames <= count,
                  let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(frames)),
                  let output = buffer.floatChannelData?[0] else {
                throw voiceAudioFailure("voice_playback_failed")
            }
            buffer.frameLength = AVAudioFrameCount(frames)
            for index in 0..<frames { output[index] = samples[(head + index) % samples.count] }
            head = (head + frames) % samples.count
            count -= frames
            return Chunk(buffer: buffer)
        }
    }
    private final class Playback {
        let id: String
        let startupFrames: Int
        let pending = PendingFrames()
        var queued: [Chunk] = []
        var queuedFrames = 0
        var bufferedFrames: Int { queuedFrames + pending.count }
        var startupReleased = false
        var segmentHasAudio = false
        var flushTail = false
        var startedReported = false
        var rendererPlaying = false
        var append: Append?
        var finish: Invoke?
        var finishing = false
        init(_ id: String, startupFrames: Int) { self.id = id; self.startupFrames = startupFrames }
    }
    private final class Append {
        weak var owner: Playback?
        let encoded: String
        var invoke: Invoke?
        var samples: [Float]?
        init(owner: Playback, encoded: String, invoke: Invoke) {
            self.owner = owner; self.encoded = encoded; self.invoke = invoke
        }
        func reject(_ reason: String) { let pending = invoke; invoke = nil; pending?.reject(reason) }
        func resolve() { let pending = invoke; invoke = nil; pending?.resolve() }
    }
    private final class Retirement {
        let id: String
        let player: AVAudioPlayerNode
        let startedAt = DispatchTime.now().uptimeNanoseconds
        let initialVolume: Float
        var timer: DispatchSourceTimer?
        var completions: [() -> Void] = []
        init(id: String, player: AVAudioPlayerNode) {
            self.id = id; self.player = player; initialVolume = player.volume
        }
    }

    private var player: AVAudioPlayerNode?
    private var rendererGeneration: UInt64 = 0
    private let format: AVAudioFormat
    private let decoder = DispatchQueue(label: "com.vcp.avatar.voice-playback-decode", qos: .userInitiated)
    private var active: Playback?
    private var retiring: Retirement?
    private var closedIds: [String] = []
    private var decoding: Append?
    private var waitingDecode: Append?
    private var disposed = false
    var onStatus: ((String?, Bool) -> Void)?
    var onStarted: ((String) -> Void)?

    init(format: AVAudioFormat) {
        self.format = format
    }

    /// Renderers may change at call entry/exit, never at in-call mic mute.
    /// Retain unconfirmed chunks; stop callbacks from the former node
    /// cannot acknowledge audio belonging to the replacement node.
    func bind(_ player: AVAudioPlayerNode?) {
        dispatchPrecondition(condition: .onQueue(.main))
        if self.player === player { return }
        rendererGeneration += 1
        // A renderer replacement cannot carry a fade on the old hardware graph.
        if let retiring = retiring { completeRetirement(retiring) }
        self.player?.stop()
        self.player = player
        if let current = active {
            current.rendererPlaying = false
            onStatus?(current.id, false)
            if current.startupReleased, player != nil {
                for chunk in current.queued { schedule(chunk, current: current) }
            }
            admit(current)
        }
    }

    var ownsPlayback: Bool {
        dispatchPrecondition(condition: .onQueue(.main))
        return active != nil || retiring != nil
    }

    var isStopping: Bool {
        dispatchPrecondition(condition: .onQueue(.main))
        return retiring != nil
    }

    func canStart(_ id: String) -> Bool { !disposed && !closedIds.contains(id) }

    func start(id: String, startupFrames: Int, invoke: Invoke) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard !disposed, !closedIds.contains(id) else { invoke.reject("cancelled:voice_playback"); return }
        guard (1...5_760).contains(startupFrames) else { invoke.reject("voice_playback_invalid_startup"); return }
        if active?.id == id { invoke.resolve(); return }
        guard active == nil, retiring == nil else { invoke.reject("voice_playback_busy"); return }
        player?.volume = 1
        active = Playback(id, startupFrames: startupFrames)
        onStatus?(id, false)
        invoke.resolve()
    }

    func append(id: String, pcmBase64: String, invoke: Invoke) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard let current = active, current.id == id, !current.finishing, !disposed else {
            invoke.reject("cancelled:voice_playback"); return
        }
        guard !pcmBase64.isEmpty, pcmBase64.utf8.count <= 43_692 else {
            invoke.reject("voice_playback_invalid_pcm"); return
        }
        guard current.append == nil, waitingDecode == nil else {
            invoke.reject("voice_playback_append_busy"); return
        }
        let request = Append(owner: current, encoded: pcmBase64, invoke: invoke)
        current.append = request
        waitingDecode = request
        decodeNext()
    }

    private func decodeNext() {
        guard decoding == nil, let request = waitingDecode else { return }
        waitingDecode = nil
        decoding = request
        decoder.async { [weak self, request] in
            let result: Result<[Float], Error>
            do {
                guard let data = Data(base64Encoded: request.encoded), !data.isEmpty,
                      data.count <= 32_768, data.count % 2 == 0 else {
                    throw voiceAudioFailure("voice_playback_invalid_pcm")
                }
                var samples = [Float](repeating: 0, count: data.count / 2)
                data.withUnsafeBytes { raw in
                    let bytes = raw.bindMemory(to: UInt8.self)
                    for index in 0..<(data.count / 2) {
                        let word = UInt16(bytes[index * 2]) | UInt16(bytes[index * 2 + 1]) << 8
                        samples[index] = Float(Int16(bitPattern: word)) / 32_768
                    }
                }
                result = .success(samples)
            } catch { result = .failure(error) }
            DispatchQueue.main.async { [weak self, request] in
                guard let self = self else { request.reject("cancelled:voice_playback"); return }
                if self.decoding === request { self.decoding = nil }
                guard let current = request.owner, self.active === current, current.append === request, !self.disposed else {
                    request.reject("cancelled:voice_playback")
                    self.decodeNext()
                    return
                }
                switch result {
                case .success(let decoded):
                    request.samples = decoded
                    self.admit(current)
                case .failure(let error):
                    let reason = voiceAudioReason(error, fallback: "voice_playback_decode_failed")
                    request.reject(reason)
                    self.cancelCurrent(reason: reason)
                }
                self.decodeNext()
            }
        }
    }

    private func admit(_ current: Playback) {
        guard active === current else { return }
        if let append = current.append, let samples = append.samples,
           player != nil, current.bufferedFrames + samples.count <= Self.capacityFrames {
            current.pending.append(samples)
            current.segmentHasAudio = true
            current.append = nil
            append.samples = nil
            append.resolve()
        }
        if !current.startupReleased,
           current.bufferedFrames >= current.startupFrames || (current.finishing && current.bufferedFrames > 0) {
            current.startupReleased = true
            current.flushTail = true
        }
        if current.startupReleased, let player = player {
            do {
                // Keep the first buffer at most 20 ms for prompt started feedback.
                // Batch already available audio into up to 100 ms buffers without
                // waiting beyond the original 20 ms minimum. A real segment
                // end/EOS flushes its smaller final block.
                // This bounds AVAudioPCMBuffer objects, not PCM admission. Even
                // tiny completed segments can wait in the frame ring while an
                // already-started player releases scheduled blocks.
                while current.queued.count < Self.scheduledChunkLimit
                    && (current.pending.count >= Self.minimumChunkFrames
                        || (current.pending.count > 0 && (current.flushTail || current.finishing))) {
                    let chunkFrames = current.startedReported || !current.queued.isEmpty
                        ? Self.maximumChunkFrames : Self.minimumChunkFrames
                    let frames = min(chunkFrames, current.pending.count)
                    let chunk = try current.pending.take(frames, format: format)
                    current.queued.append(chunk)
                    current.queuedFrames += chunk.frames
                    schedule(chunk, current: current)
                }
            } catch {
                cancelCurrent(reason: voiceAudioReason(error, fallback: "voice_playback_failed"))
                return
            }
            if current.pending.count == 0 { current.flushTail = false }
            if !current.queued.isEmpty, !player.isPlaying { player.play() }
        }
        onStatus?(current.id, current.rendererPlaying && !current.queued.isEmpty)
        if current.finishing, current.bufferedFrames == 0, current.append == nil {
            let completion = current.finish
            current.finish = nil
            rememberClosed(current.id)
            active = nil
            player?.stop()
            onStatus?(nil, false)
            completion?.resolve()
        }
    }

    private func schedule(_ chunk: Chunk, current: Playback) {
        guard let player = player else { return }
        let generation = rendererGeneration
        player.scheduleBuffer(chunk.buffer, at: nil, options: [], completionCallbackType: .dataPlayedBack) {
            [weak self, weak current] _ in
            DispatchQueue.main.async {
                guard let self = self, self.rendererGeneration == generation,
                      let current = current, self.active === current,
                      let index = current.queued.firstIndex(where: { $0.key == chunk.key }) else { return }
                // This callback confirms actual rendering, unlike append ACK.
                // Send the fact before admit can complete a very short job.
                current.rendererPlaying = true
                if !current.startedReported {
                    current.startedReported = true
                    self.onStarted?(current.id)
                }
                current.queuedFrames -= current.queued.remove(at: index).frames
                self.admit(current)
            }
        }
    }

    func endSegment(id: String, invoke: Invoke) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard let current = active, current.id == id, !current.finishing, !disposed else {
            invoke.reject("cancelled:voice_playback"); return
        }
        // Rust submits this only after the preceding append acknowledgement.
        guard current.append == nil else { invoke.reject("voice_playback_append_pending"); return }
        if current.segmentHasAudio {
            current.startupReleased = true
            current.flushTail = true
            current.segmentHasAudio = false
        }
        invoke.resolve()
        admit(current)
    }

    func finish(id: String, invoke: Invoke) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard let current = active, current.id == id, !disposed else { invoke.reject("cancelled:voice_playback"); return }
        guard !current.finishing else { invoke.reject("voice_playback_finish_busy"); return }
        current.finishing = true
        current.finish = invoke
        admit(current)
    }

    func stop(id: String, immediately: Bool = false, completion: @escaping () -> Void) throws {
        dispatchPrecondition(condition: .onQueue(.main))
        rememberClosed(id)
        if active?.id == id { cancelCurrent(reason: "cancelled:voice_playback", immediately: immediately) }
        if let retiring = retiring, retiring.id == id {
            if immediately { completeRetirement(retiring); completion(); return }
            guard retiring.completions.count < 8 else { throw voiceAudioFailure("voice_playback_stop_busy") }
            retiring.completions.append(completion)
        } else { completion() }
    }

    func cancelCurrent(reason: String, immediately: Bool = false) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard let current = active else {
            if immediately, let retiring = retiring { completeRetirement(retiring) }
            return
        }
        active = nil
        rememberClosed(current.id)
        current.append?.reject(reason)
        if waitingDecode === current.append { waitingDecode = nil }
        current.append = nil
        current.finish?.reject(reason)
        current.finish = nil
        let hadScheduledAudio = !current.queued.isEmpty
        current.queued.removeAll()
        current.queuedFrames = 0
        // Revoke logical ownership immediately, but retain the current renderer
        // until its already-scheduled audio fades. No new PCM is admitted here.
        if !immediately, hadScheduledAudio, let player = player, player.isPlaying, player.volume > 0 {
            let retirement = Retirement(id: current.id, player: player)
            retiring = retirement
            let timer = DispatchSource.makeTimerSource(queue: .main)
            retirement.timer = timer
            timer.schedule(deadline: .now(), repeating: .milliseconds(5), leeway: .milliseconds(1))
            timer.setEventHandler { [weak self, weak retirement] in
                guard let self = self, let retirement = retirement, self.retiring === retirement else { return }
                guard self.player === retirement.player else { self.completeRetirement(retirement); return }
                let elapsed = Double(DispatchTime.now().uptimeNanoseconds - retirement.startedAt) / 1_000_000_000
                let remaining = Float(max(0, 1 - elapsed / Self.stopFadeSeconds))
                // Ease the last steps towards silence before the node is stopped.
                retirement.player.volume = retirement.initialVolume * remaining * remaining
                if elapsed >= Self.stopFadeSeconds { self.completeRetirement(retirement) }
            }
            timer.resume()
            return
        }
        player?.stop()
        player?.volume = 1
        onStatus?(nil, false)
    }

    private func completeRetirement(_ retirement: Retirement) {
        guard retiring === retirement else { return }
        retirement.timer?.cancel()
        retirement.timer = nil
        retirement.player.volume = 0
        retirement.player.stop()
        retirement.player.volume = 1
        retiring = nil
        onStatus?(nil, false)
        let completions = retirement.completions
        retirement.completions.removeAll()
        for completion in completions { completion() }
    }

    func shutdown() {
        dispatchPrecondition(condition: .onQueue(.main))
        disposed = true
        cancelCurrent(reason: "cancelled:voice_session_stopped", immediately: true)
    }

    private func rememberClosed(_ id: String) {
        if closedIds.contains(id) { return }
        closedIds.append(id)
        if closedIds.count > 32 { closedIds.removeFirst() }
    }
}
