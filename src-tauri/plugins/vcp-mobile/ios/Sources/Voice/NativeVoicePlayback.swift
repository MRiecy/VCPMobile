import AVFoundation
import Foundation
import Tauri

/// Main-queue player control with a separate PCM decode queue. At most two
/// seconds of scheduled PCM, one waiting append and one decoder
/// operation are retained. Append applies asynchronous backpressure.
final class NativeVoicePlayback {
    private struct Chunk {
        let key = UUID()
        let buffer: AVAudioPCMBuffer
        let bytes: Int
    }
    private final class Playback {
        let id: String
        var queued: [Chunk] = []
        var queuedBytes = 0
        var append: Append?
        var finish: Invoke?
        var finishing = false
        init(_ id: String) { self.id = id }
    }
    private final class Append {
        weak var owner: Playback?
        let encoded: String
        var invoke: Invoke?
        var chunks: [Chunk]?
        var bytes = 0
        init(owner: Playback, encoded: String, invoke: Invoke) {
            self.owner = owner; self.encoded = encoded; self.invoke = invoke
        }
        func reject(_ reason: String) { let pending = invoke; invoke = nil; pending?.reject(reason) }
        func resolve() { let pending = invoke; invoke = nil; pending?.resolve() }
    }

    private var player: AVAudioPlayerNode?
    private var rendererGeneration: UInt64 = 0
    private let format: AVAudioFormat
    private let decoder = DispatchQueue(label: "com.vcp.avatar.voice-playback-decode", qos: .userInitiated)
    private var active: Playback?
    private var closedIds: [String] = []
    private var decoding: Append?
    private var waitingDecode: Append?
    private var disposed = false
    var onStatus: ((String?, Bool) -> Void)?

    init(format: AVAudioFormat) {
        self.format = format
    }

    /// Renderers may change at call entry/exit, never at in-call mic mute.
    /// Retain unconfirmed 20 ms chunks; stop callbacks from the former node
    /// cannot acknowledge audio belonging to the replacement node.
    func bind(_ player: AVAudioPlayerNode?) {
        dispatchPrecondition(condition: .onQueue(.main))
        if self.player === player { return }
        rendererGeneration += 1
        self.player?.stop()
        self.player = player
        if let current = active, player != nil {
            for chunk in current.queued { schedule(chunk, current: current) }
            admit(current)
        }
    }

    var ownsPlayback: Bool {
        dispatchPrecondition(condition: .onQueue(.main))
        return active != nil
    }

    func canStart(_ id: String) -> Bool { !disposed && !closedIds.contains(id) }

    func start(id: String, invoke: Invoke) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard !disposed, !closedIds.contains(id) else { invoke.reject("cancelled:voice_playback"); return }
        if active?.id == id { invoke.resolve(); return }
        guard active == nil else { invoke.reject("voice_playback_busy"); return }
        active = Playback(id)
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
        let format = self.format
        decoder.async { [weak self, request] in
            let result: Result<([Chunk], Int), Error>
            do {
                guard let data = Data(base64Encoded: request.encoded), !data.isEmpty,
                      data.count <= 32_768, data.count % 2 == 0,
                      let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(data.count / 2)),
                      let output = buffer.floatChannelData?[0] else {
                    throw voiceAudioFailure("voice_playback_invalid_pcm")
                }
                buffer.frameLength = AVAudioFrameCount(data.count / 2)
                data.withUnsafeBytes { raw in
                    let bytes = raw.bindMemory(to: UInt8.self)
                    for index in 0..<(data.count / 2) {
                        let word = UInt16(bytes[index * 2]) | UInt16(bytes[index * 2 + 1]) << 8
                        output[index] = Float(Int16(bitPattern: word)) / 32_768
                    }
                }
                var chunks: [Chunk] = []
                var offset = 0
                while offset < Int(buffer.frameLength) {
                    let count = min(480, Int(buffer.frameLength) - offset)
                    guard let part = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(count)),
                          let target = part.floatChannelData?[0] else { throw voiceAudioFailure("voice_playback_failed") }
                    part.frameLength = AVAudioFrameCount(count)
                    memcpy(target, output.advanced(by: offset), count * MemoryLayout<Float>.size)
                    chunks.append(Chunk(buffer: part, bytes: count * 2))
                    offset += count
                }
                result = .success((chunks, data.count))
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
                    request.chunks = decoded.0; request.bytes = decoded.1
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
        if let append = current.append, let chunks = append.chunks,
           player != nil, current.queuedBytes + append.bytes <= 96_000,
           current.queued.count + chunks.count <= 128 {
            current.queuedBytes += append.bytes
            current.append = nil
            append.chunks = nil
            for chunk in chunks {
                current.queued.append(chunk)
                schedule(chunk, current: current)
            }
            append.resolve()
        }
        if !current.queued.isEmpty, let player = player, !player.isPlaying { player.play() }
        onStatus?(current.id, !current.queued.isEmpty)
        if current.finishing, current.queued.isEmpty, current.append == nil {
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
                current.queuedBytes -= current.queued.remove(at: index).bytes
                self.admit(current)
            }
        }
    }

    func finish(id: String, invoke: Invoke) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard let current = active, current.id == id, !disposed else { invoke.reject("cancelled:voice_playback"); return }
        guard !current.finishing else { invoke.reject("voice_playback_finish_busy"); return }
        current.finishing = true
        current.finish = invoke
        admit(current)
    }

    func stop(id: String) {
        dispatchPrecondition(condition: .onQueue(.main))
        rememberClosed(id)
        if active?.id == id { cancelCurrent(reason: "cancelled:voice_playback") }
    }

    func cancelCurrent(reason: String) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard let current = active else { return }
        active = nil
        rememberClosed(current.id)
        player?.stop()
        current.append?.reject(reason)
        if waitingDecode === current.append { waitingDecode = nil }
        current.append = nil
        current.finish?.reject(reason)
        current.finish = nil
        current.queued.removeAll()
        current.queuedBytes = 0
        onStatus?(nil, false)
    }

    func shutdown() {
        dispatchPrecondition(condition: .onQueue(.main))
        disposed = true
        cancelCurrent(reason: "cancelled:voice_session_stopped")
    }

    private func rememberClosed(_ id: String) {
        if closedIds.contains(id) { return }
        closedIds.append(id)
        if closedIds.count > 32 { closedIds.removeFirst() }
    }
}
