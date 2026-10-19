import Foundation

struct VoiceSegmentExport: Encodable {
    let path: String
    let sampleRate = 16_000
    let sampleCount: UInt64
}

struct VoiceSegmentSnapshot {
    let pcm: Data
    let sampleCount: UInt64
}

/// Inference-queue only. One fixed 32-second PCM16 ring, in the same 16 kHz
/// sample domain as VAD frames. No filesystem work runs on this hot queue.
final class VoiceSegmentCache {
    private static let capacity = 32 * 16_000
    private var samples: [Int16]
    private var endSample: UInt64 = 0

    init() { samples = [Int16](repeating: 0, count: Self.capacity) }

    func append(_ frame: UnsafePointer<Float>, count: Int) {
        for index in 0..<count {
            let value = max(-1, min(1, frame[index]))
            let pcm = Int16(max(-32_768, min(32_767, Int((Double(value) * 32_768).rounded()))))
            samples[Int(endSample % UInt64(Self.capacity))] = pcm.littleEndian
            endSample += 1
        }
    }

    func snapshot(start: UInt64, end: UInt64) throws -> VoiceSegmentSnapshot {
        let oldest = endSample > UInt64(Self.capacity) ? endSample - UInt64(Self.capacity) : 0
        guard start < end, end - start <= 30 * 16_000 else { throw voiceAudioFailure("voice_segment_invalid_range") }
        guard start >= oldest, end <= endSample else { throw voiceAudioFailure("voice_segment_expired") }
        let count = Int(end - start)
        let offset = Int(start % UInt64(Self.capacity))
        let first = min(count, Self.capacity - offset)
        var data = Data(count: count * 2)
        data.withUnsafeMutableBytes { target in
            samples.withUnsafeBufferPointer { source in
                if let output = target.baseAddress, let input = source.baseAddress {
                    memcpy(output, input.advanced(by: offset), first * 2)
                    if first < count { memcpy(output.advanced(by: first * 2), input, (count - first) * 2) }
                }
            }
        }
        return VoiceSegmentSnapshot(pcm: data, sampleCount: UInt64(count))
    }

    func clear() {
        samples.withUnsafeMutableBufferPointer { buffer in
            if let base = buffer.baseAddress { memset(base, 0, buffer.count * MemoryLayout<Int16>.size) }
        }
        endSample = 0
    }
}

/// File-queue only. Each session has a separate generated cache directory; no
/// user-supplied identifier becomes a path. Idle files expire after one minute.
final class VoiceSegmentFiles {
    private let queue: DispatchQueue
    private let timer: DispatchSourceTimer
    private var directory: URL?
    private var exports: [(url: URL, expires: Date)] = []
    private var closed = false

    init(queue: DispatchQueue) {
        self.queue = queue
        timer = DispatchSource.makeTimerSource(queue: queue)
        timer.setEventHandler { [weak self] in self?.removeExpiredExports() }
        timer.schedule(deadline: .now() + .seconds(15), repeating: .seconds(15))
        timer.resume()
    }

    func write(_ snapshot: VoiceSegmentSnapshot) throws -> VoiceSegmentExport {
        dispatchPrecondition(condition: .onQueue(queue))
        guard !closed else { throw voiceAudioFailure("cancelled:voice_segment") }
        let fileManager = FileManager.default
        removeExpiredExports()
        guard exports.count < 4 else { throw voiceAudioFailure("voice_segment_exports_busy") }
        let folder: URL
        if let existing = directory { folder = existing }
        else {
            guard let caches = fileManager.urls(for: .cachesDirectory, in: .userDomainMask).first else {
                throw voiceAudioFailure("voice_segment_cache_unavailable")
            }
            folder = caches.appendingPathComponent("VCPVoiceSegments", isDirectory: true)
                .appendingPathComponent(UUID().uuidString, isDirectory: true)
            try fileManager.createDirectory(at: folder, withIntermediateDirectories: true)
            directory = folder
        }
        let count = snapshot.sampleCount
        let pcmBytes = UInt32(count * 2)
        var data = Data(capacity: Int(pcmBytes) + 44)
        func word(_ value: UInt32) {
            data.append(contentsOf: [UInt8(truncatingIfNeeded: value), UInt8(truncatingIfNeeded: value >> 8),
                                     UInt8(truncatingIfNeeded: value >> 16), UInt8(truncatingIfNeeded: value >> 24)])
        }
        data.append(contentsOf: "RIFF".utf8); word(36 + pcmBytes)
        data.append(contentsOf: "WAVEfmt ".utf8); word(16)
        data.append(contentsOf: [1, 0, 1, 0]); word(16_000); word(32_000)
        data.append(contentsOf: [2, 0, 16, 0])
        data.append(contentsOf: "data".utf8); word(pcmBytes)
        data.append(snapshot.pcm)
        let url = folder.appendingPathComponent(UUID().uuidString + ".wav")
        try data.write(to: url, options: .atomic)
        exports.append((url, Date().addingTimeInterval(60)))
        return VoiceSegmentExport(path: url.path, sampleCount: count)
    }

    func clear() {
        dispatchPrecondition(condition: .onQueue(queue))
        closed = true
        timer.cancel()
        if let directory = directory {
            do { try FileManager.default.removeItem(at: directory) }
            catch { NSLog("[VcpVoice] Segment cleanup: %@", error.localizedDescription) }
        }
        directory = nil
        exports.removeAll()
    }

    private func removeExpiredExports() {
        dispatchPrecondition(condition: .onQueue(queue))
        let now = Date()
        exports.removeAll { item in
            if item.expires <= now {
                try? FileManager.default.removeItem(at: item.url)
                return true
            }
            return !FileManager.default.fileExists(atPath: item.url.path)
        }
    }

    deinit { timer.cancel() }
}
