import AVFoundation
import Foundation
import ImageIO
import Tauri
import UniformTypeIdentifiers

private struct NativeMediaOperationArgs: Decodable { let operationId: String }
private struct NativeMediaProcessArgs: Decodable {
    let operationId: String
    let path: String
    let outputDir: String
    let maxBytes: UInt64
}
private struct NativeProcessedAudio: Encodable {
    let path: String
    let mime = "audio/mp4"
    let codec = "aac"
    let container = "m4a"
    let sampleRate = 16_000
    let channels = 1
}
private struct NativeProcessedVideo: Encodable { let paths: [String] }
private struct NativeMediaAcknowledgement: Encodable {}

/// A reservation exists before Rust starts a blocking native call. Cancellation
/// removes an unstarted reservation; a running operation owns its files until its
/// final callback, so cancellation never races a writer with Rust's cleanup.
private final class NativeMediaOperation {
    let id: String
    var started = false // NativeMedia registry lock
    private let lock = NSLock()
    private var cancelled = false
    private var finished = false
    private var cancelWork: (() -> Void)?

    init(_ id: String) { self.id = id }

    func check() throws {
        lock.lock(); let value = cancelled; lock.unlock()
        if value { throw NativeFileError.failure("cancelled: media operation") }
    }

    func setCancellation(_ handler: (() -> Void)?) {
        lock.lock()
        if finished { lock.unlock(); return }
        cancelWork = handler
        let value = cancelled
        lock.unlock()
        if value { handler?() }
    }

    func cancel() {
        lock.lock()
        if finished { lock.unlock(); return }
        cancelled = true
        let handler = cancelWork
        lock.unlock()
        handler?()
    }

    /// Commits one native result against a concurrent cancellation. A successful
    /// handoff transfers file ownership to Rust; later cancellation is a no-op.
    func complete() -> Bool {
        lock.lock(); defer { lock.unlock() }
        finished = true
        cancelWork = nil
        return !cancelled
    }
}

final class NativeMedia {
    private let lock = NSLock()
    private var operations: [String: NativeMediaOperation] = [:]
    private var stopped = false
    private let audioQueue = DispatchQueue(label: "com.vcp.avatar.media-audio", qos: .userInitiated)

    func prepare(_ id: String) throws {
        lock.lock(); defer { lock.unlock() }
        guard !stopped else { throw NativeFileError.failure("cancelled: media owner stopped") }
        guard !id.isEmpty, operations[id] == nil else {
            throw NativeFileError.failure("media_operation: invalid or duplicate reservation")
        }
        operations[id] = NativeMediaOperation(id)
    }

    func cancel(_ id: String) {
        lock.lock()
        let operation = operations[id]
        if operation?.started == false { operations.removeValue(forKey: id) }
        lock.unlock()
        // This does not wait behind the audio worker. Its next buffer boundary
        // observes cancellation; AVFoundation image generation is cancelled now.
        operation?.cancel()
    }

    func shutdown() {
        lock.lock()
        stopped = true
        let pending = Array(operations.values)
        operations.removeAll()
        lock.unlock()
        pending.forEach { $0.cancel() }
    }

    private func claim(_ id: String) throws -> NativeMediaOperation {
        lock.lock(); defer { lock.unlock() }
        guard !stopped, let operation = operations[id], !operation.started else {
            throw NativeFileError.failure("cancelled: media reservation is no longer available")
        }
        operation.started = true
        return operation
    }

    private func remove(_ operation: NativeMediaOperation) {
        lock.lock(); defer { lock.unlock() }
        if operations[operation.id] === operation { operations.removeValue(forKey: operation.id) }
    }

    fileprivate func processAudio(_ args: NativeMediaProcessArgs, _ invoke: Invoke) throws {
        let operation = try claim(args.operationId)
        audioQueue.async { [weak self] in
            var owned: [URL] = []
            do {
                let result = try Self.convertAudio(args, operation: operation, owned: &owned)
                guard operation.complete() else { throw NativeFileError.failure("cancelled: audio processing") }
                self?.remove(operation)
                invoke.resolve(result)
            } catch {
                _ = operation.complete()
                owned.forEach { try? FileManager.default.removeItem(at: $0) }
                self?.remove(operation)
                invoke.reject(error.localizedDescription)
            }
        }
    }

    fileprivate func processVideo(_ args: NativeMediaProcessArgs, _ invoke: Invoke) throws {
        let operation = try claim(args.operationId)
        // The detached task never captures the plugin or touches UIKit. Decode
        // requests are sequential: at most one generated CGImage per operation.
        Task.detached(priority: .userInitiated) { [weak self] in
            var owned: [URL] = []
            do {
                let result = try await Self.extractVideo(args, operation: operation, owned: &owned)
                guard operation.complete() else { throw NativeFileError.failure("cancelled: video processing") }
                self?.remove(operation)
                invoke.resolve(result)
            } catch {
                _ = operation.complete()
                owned.forEach { try? FileManager.default.removeItem(at: $0) }
                self?.remove(operation)
                invoke.reject(error.localizedDescription)
            }
        }
    }

    private static func sourceAndDirectory(_ args: NativeMediaProcessArgs) throws -> (URL, URL) {
        guard args.path.hasPrefix("/"), args.outputDir.hasPrefix("/"), args.maxBytes > 0 else {
            throw NativeFileError.failure("media_input: local paths and a positive byte budget are required")
        }
        let source = URL(fileURLWithPath: args.path)
        guard try source.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile == true else {
            throw NativeFileError.failure("media_input: source is not a regular file")
        }
        let directory = URL(fileURLWithPath: args.outputDir, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return (source, directory)
    }

    private static func currentFileSize(_ url: URL) throws -> UInt64 {
        // A reused URL can cache resourceValues while an encoder grows its file.
        // Query the filesystem each time; zero bytes is valid before writer flush.
        let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
        guard let size = attributes[.size] as? NSNumber else {
            throw NativeFileError.failure("media_output: file size unavailable")
        }
        return size.uint64Value
    }

    private static func encodedSize(_ url: URL, limit: UInt64) throws -> UInt64 {
        let size = try currentFileSize(url)
        guard size > 0, size <= limit else {
            throw NativeFileError.failure("media_too_large: encoded media is empty or exceeds the byte budget")
        }
        return size
    }

    private static func convertAudio(
        _ args: NativeMediaProcessArgs, operation: NativeMediaOperation, owned: inout [URL]
    ) throws -> NativeProcessedAudio {
        try operation.check()
        let (source, directory) = try sourceAndDirectory(args)
        let output = directory.appendingPathComponent("audio.m4a")
        guard source.standardizedFileURL != output.standardizedFileURL else {
            throw NativeFileError.failure("media_input: output would overwrite source")
        }
        owned.append(output)

        // File processing neither plays/records audio nor changes AVAudioSession.
        let input = try AVAudioFile(forReading: source, commonFormat: .pcmFormatFloat32, interleaved: false)
        defer { input.close() }
        let writer = try AVAudioFile(forWriting: output, settings: [
            AVFormatIDKey: kAudioFormatMPEG4AAC,
            AVSampleRateKey: 16_000,
            AVNumberOfChannelsKey: 1,
            AVEncoderBitRateKey: 32_000
        ], commonFormat: .pcmFormatFloat32, interleaved: false)
        defer { writer.close() }

        guard let converter = AVAudioConverter(from: input.processingFormat, to: writer.processingFormat),
              let inputBuffer = AVAudioPCMBuffer(pcmFormat: input.processingFormat, frameCapacity: 4096),
              let outputBuffer = AVAudioPCMBuffer(pcmFormat: writer.processingFormat, frameCapacity: 4096) else {
            throw NativeFileError.failure("audio_convert: unsupported PCM conversion")
        }
        converter.downmix = true
        var inputEnded = false
        var totalFrames: UInt64 = 0
        var lastSizeCheckFrames: UInt64 = 0

        while true {
            try operation.check()
            outputBuffer.frameLength = 0
            var readError: Error?
            var conversionError: NSError?
            var readFrames: UInt64 = 0
            let status = converter.convert(to: outputBuffer, error: &conversionError) { requested, state in
                do {
                    try operation.check()
                    if inputEnded { state.pointee = .endOfStream; return nil }
                    guard requested > 0 else { throw NativeFileError.failure("audio_convert: empty input request") }
                    try input.read(into: inputBuffer, frameCount: min(requested, inputBuffer.frameCapacity))
                    if inputBuffer.frameLength == 0 {
                        inputEnded = true
                        state.pointee = .endOfStream
                        return nil
                    }
                    readFrames += UInt64(inputBuffer.frameLength)
                    state.pointee = .haveData
                    return inputBuffer
                } catch {
                    readError = error
                    state.pointee = .endOfStream
                    return nil
                }
            }
            if let error = readError { throw error }
            if status == .error {
                if let error = conversionError { throw error }
                throw NativeFileError.failure("audio_convert: conversion failed")
            }
            // Both .inputRanDry and .endOfStream can carry a final nonempty
            // buffer. Write it before interpreting the status and keep draining
            // the same converter after input EOF to preserve its resampling tail.
            if outputBuffer.frameLength > 0 {
                try writer.write(from: outputBuffer)
                totalFrames += UInt64(outputBuffer.frameLength)
            }
            // Encoded bytes, not source duration, enforce Rust's request budget.
            // Poll once per second of output; close() below flushes final packets.
            if totalFrames - lastSizeCheckFrames >= 16_000 {
                lastSizeCheckFrames = totalFrames
                let size = try currentFileSize(output)
                if size > args.maxBytes {
                    throw NativeFileError.failure("media_too_large: encoded audio exceeds the byte budget")
                }
            }
            if status == .endOfStream { break }
            if readFrames == 0 && outputBuffer.frameLength == 0 {
                throw NativeFileError.failure("audio_convert: converter stopped making progress")
            }
        }
        guard totalFrames > 0 else { throw NativeFileError.failure("audio_decode: source contains no audio samples") }
        try operation.check()
        writer.close() // iOS 18+: flush encoder and finalize M4A before handing it to Rust.
        _ = try encodedSize(output, limit: args.maxBytes)
        let completed = try AVAudioFile(forReading: output)
        defer { completed.close() }
        guard completed.length > 0,
              completed.fileFormat.streamDescription.pointee.mFormatID == kAudioFormatMPEG4AAC,
              completed.fileFormat.sampleRate == 16_000,
              completed.fileFormat.channelCount == 1 else {
            throw NativeFileError.failure("audio_encode: completed file does not match AAC 16kHz mono")
        }
        return NativeProcessedAudio(path: output.path)
    }

    private static func extractVideo(
        _ args: NativeMediaProcessArgs, operation: NativeMediaOperation, owned: inout [URL]
    ) async throws -> NativeProcessedVideo {
        try operation.check()
        let (source, directory) = try sourceAndDirectory(args)
        let asset = AVURLAsset(url: source)
        operation.setCancellation { asset.cancelLoading() }
        let durationTime = try await asset.load(.duration)
        let duration = durationTime.seconds
        try operation.check()
        guard duration.isFinite, duration > 0,
              !(try await asset.loadTracks(withMediaType: .video)).isEmpty else {
            throw NativeFileError.failure("video_decode: source has no finite video timeline")
        }
        let generator = AVAssetImageGenerator(asset: asset)
        generator.appliesPreferredTrackTransform = true
        generator.maximumSize = CGSize(width: 1280, height: 720)
        // Preserve representative-frame sampling rather than requiring costly
        // frame-accurate seeking. Actual timestamps below suppress repeat frames.
        let spacing = max(2, duration / 300)
        let tolerance = CMTime(seconds: spacing / 2, preferredTimescale: 600)
        generator.requestedTimeToleranceBefore = tolerance
        generator.requestedTimeToleranceAfter = tolerance
        operation.setCancellation { generator.cancelAllCGImageGeneration(); asset.cancelLoading() }
        defer {
            operation.setCancellation(nil)
            generator.cancelAllCGImageGeneration()
            asset.cancelLoading()
        }
        let count = duration > 600 ? 300 : max(1, Int(ceil(duration / 2)))
        var paths: [String] = []
        var actualTimes: [CMTime] = []
        var totalBytes: UInt64 = 0
        for index in 0..<count {
            try operation.check()
            let time = CMTime(seconds: Double(index) * spacing, preferredTimescale: 600)
            let (image, actualTime) = try await generator.image(at: time)
            try operation.check()
            guard actualTime.isNumeric else { throw NativeFileError.failure("video_decode: invalid frame timestamp") }
            if actualTimes.contains(where: { CMTimeCompare($0, actualTime) == 0 }) { continue }
            let output = directory.appendingPathComponent(String(format: "frame_%04d.jpg", index + 1))
            owned.append(output)
            let bytes: UInt64 = try autoreleasepool {
                guard image.width > 0, image.height > 0,
                      let destination = CGImageDestinationCreateWithURL(output as CFURL, UTType.jpeg.identifier as CFString, 1, nil) else {
                    throw NativeFileError.failure("video_encode: JPEG encoder unavailable")
                }
                CGImageDestinationAddImage(destination, image, [kCGImageDestinationLossyCompressionQuality: 0.9] as CFDictionary)
                guard CGImageDestinationFinalize(destination) else {
                    throw NativeFileError.failure("video_encode: could not finalize JPEG frame")
                }
                return try encodedSize(output, limit: args.maxBytes - totalBytes)
            }
            totalBytes += bytes
            actualTimes.append(actualTime)
            paths.append(output.path)
        }
        try operation.check()
        guard !paths.isEmpty else { throw NativeFileError.failure("video_decode: no frames could be extracted") }
        return NativeProcessedVideo(paths: paths)
    }
}

extension VcpMobilePlugin {
    @objc public func prepareMediaOperation(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(NativeMediaOperationArgs.self)
            try media.prepare(args.operationId)
            invoke.resolve(NativeMediaAcknowledgement())
        } catch { invoke.reject(error.localizedDescription) }
    }

    @objc public func cancelMediaOperation(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(NativeMediaOperationArgs.self)
            media.cancel(args.operationId)
            invoke.resolve(NativeMediaAcknowledgement())
        } catch { invoke.reject(error.localizedDescription) }
    }

    @objc public func processAudio(_ invoke: Invoke) {
        do { try media.processAudio(invoke.parseArgs(NativeMediaProcessArgs.self), invoke) }
        catch { invoke.reject(error.localizedDescription) }
    }

    @objc public func processVideo(_ invoke: Invoke) {
        do { try media.processVideo(invoke.parseArgs(NativeMediaProcessArgs.self), invoke) }
        catch { invoke.reject(error.localizedDescription) }
    }
}
