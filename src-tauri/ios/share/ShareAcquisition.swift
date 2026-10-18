import Foundation
import UniformTypeIdentifiers
import CryptoKit

/// Owns provider acquisition, cancellation, read draining and the one terminal decision.
final class ShareAcquisition {
    enum Outcome {
        case ready
        case failed(Error)
        case cancelled(Error)
    }

    private enum Phase { case acquiring, finishing, finished }
    private enum Completion {
        case ready(folder: URL, manifest: [String: Any])
        case failure(Error)
    }

    private let queue = DispatchQueue(label: "com.vcp.avatar.share.acquire")
    private let stateLock = NSLock()
    private var cancelled = false
    private var activeReads = 0
    private var pendingCompletion: Completion?
    private var phase: Phase = .acquiring
    private var progress: Progress?
    private var activeCoordinator: NSFileCoordinator?
    private var started = false
    private let maxBytes: Int64 = 512 * 1024 * 1024
    private var totalBytes: Int64 = 0
    private var retainedBytes: Int64 = 0

    private let storage = ShareInboxStorage()
    private let completionHandler: (Outcome) -> Void

    init(completion: @escaping (Outcome) -> Void) { completionHandler = completion }

    func start(_ providers: [NSItemProvider], initialText: [String]) {
        stateLock.lock()
        guard !started, !cancelled, phase == .acquiring else { stateLock.unlock(); return }
        started = true
        stateLock.unlock()
        queue.async { self.begin(providers, initialText: initialText) }
    }

    private func checkCurrent() throws {
        stateLock.lock(); let stopped = cancelled || phase != .acquiring; stateLock.unlock()
        if stopped { throw failure("分享已取消") }
        try storage.checkCurrent()
    }

    private func begin(_ providers: [NSItemProvider], initialText: [String]) {
        do {
            stateLock.lock()
            let stopped = cancelled || phase != .acquiring
            stateLock.unlock()
            guard !stopped else { throw failure("分享已取消") }
            let delivery = try storage.begin()
            let folder = delivery.folder
            retainedBytes = delivery.retainedBytes
            guard providers.count <= 16 else { throw failure("一次分享最多包含 16 项") }
            guard initialText.reduce(0, { $0 + $1.utf8.count }) <= 1_048_576 else { throw failure("分享文字超过 1 MiB") }
            acquire(providers, index: 0, folder: folder, text: initialText, items: [])
        } catch { finish(error: error) }
    }

    private func acquire(_ providers: [NSItemProvider], index: Int, folder: URL, text: [String], items: [[String: Any]]) {
        do { try checkCurrent() } catch { finish(error: error); return }
        if index == providers.count {
            requestCompletion(.ready(folder: folder, manifest: ["text": text.joined(separator: "\n"), "items": items]))
            return
        }
        let provider = providers[index]
        let prefersFile = [UTType.image, .pdf, .movie, .audio].contains { provider.hasItemConformingToTypeIdentifier($0.identifier) }
        if !prefersFile && provider.hasItemConformingToTypeIdentifier(UTType.url.identifier) && !provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) {
            setProgress(provider.loadObject(ofClass: NSURL.self) { value, error in
                self.queue.async {
                    if let error { self.finish(error: error); return }
                    guard let url = value as? URL else { self.finish(error: self.failure("无法读取分享链接")); return }
                    self.nextText(url.absoluteString, providers, index, folder, text, items)
                }
            })
        } else if !prefersFile && provider.hasItemConformingToTypeIdentifier(UTType.plainText.identifier) && !provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) {
            setProgress(provider.loadObject(ofClass: NSString.self) { value, error in
                self.queue.async {
                    if let error { self.finish(error: error); return }
                    guard let value = value as? String else { self.finish(error: self.failure("无法读取分享文字")); return }
                    self.nextText(value, providers, index, folder, text, items)
                }
            })
        } else if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) {
            setProgress(provider.loadObject(ofClass: NSURL.self) { value, error in
                guard self.beginRead() else { return }
                defer { self.endRead() }
                do {
                    if let error { throw error }
                    guard let url = value as? URL else { throw self.failure("提供方未返回文件地址") }
                    let type = UTType(filenameExtension: url.pathExtension) ?? .data
                    let item = try self.copyFile(url, type: type, folder: folder)
                    self.queue.async { self.acquire(providers, index: index + 1, folder: folder, text: text, items: items + [item]) }
                } catch { self.failedItem(error, provider, providers, index, folder, text, items) }
            })
        } else if let type = provider.registeredTypeIdentifiers.first(where: { UTType($0)?.conforms(to: .data) == true && $0 != UTType.fileURL.identifier }) {
            setProgress(provider.loadFileRepresentation(forTypeIdentifier: type) { url, error in
                // The provider URL expires when this callback returns: copy here, not in a later async block.
                guard self.beginRead() else { return }
                defer { self.endRead() }
                do {
                    if let error { throw error }
                    guard let url else { throw self.failure("提供方未返回文件") }
                    let item = try self.copyFile(url, type: UTType(type) ?? .data, folder: folder)
                    self.queue.async { self.acquire(providers, index: index + 1, folder: folder, text: text, items: items + [item]) }
                } catch { self.failedItem(error, provider, providers, index, folder, text, items) }
            })
        } else {
            // Keep the failure visible; the containing app offers an explicit continue-with-successful-items choice.
            let item: [String: Any] = ["itemId": UUID().uuidString.lowercased(), "fileName": provider.suggestedName ?? "分享项目", "error": "提供方没有可读取的文件或文字表示"]
            acquire(providers, index: index + 1, folder: folder, text: text, items: items + [item])
        }
    }

    private func copyFile(_ url: URL, type: UTType, folder: URL) throws -> [String: Any] {
        try checkCurrent()
        let scope = url.startAccessingSecurityScopedResource()
        defer { if scope { url.stopAccessingSecurityScopedResource() } }
        let itemId = UUID().uuidString.lowercased()
        let destination = folder.appendingPathComponent(itemId)
        var result: Result<[String: Any], Error> = .failure(failure("提供方未能协调文件读取"))
        var coordinationError: NSError?
        let coordinator = NSFileCoordinator()
        stateLock.lock()
        activeCoordinator = coordinator
        let stopped = cancelled || phase != .acquiring
        stateLock.unlock()
        defer {
            stateLock.lock()
            if activeCoordinator === coordinator { activeCoordinator = nil }
            stateLock.unlock()
        }
        if stopped {
            coordinator.cancel()
            throw failure("分享已取消")
        }
        // Keep the provider's Progress separate: its callback can already be
        // running while file coordination is still waiting for another process.
        coordinator.coordinate(readingItemAt: url, options: .withoutChanges, error: &coordinationError) { readable in
            result = Result {
                try checkCurrent()
                let values = try readable.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey])
                guard values.isRegularFile == true else { throw failure("只能分享普通文件") }
                guard Int64(values.fileSize ?? 0) + totalBytes <= maxBytes else { throw failure("单次分享总大小不能超过 512 MiB") }
                let input = try FileHandle(forReadingFrom: readable)
                defer { try? input.close() }
                guard FileManager.default.createFile(atPath: destination.path, contents: nil) else { throw failure("无法创建分享副本") }
                let output = try FileHandle(forWritingTo: destination)
                defer { try? output.close() }
                var hash = SHA256(); var size: Int64 = 0
                while let bytes = try input.read(upToCount: 64 * 1024), !bytes.isEmpty {
                    try checkCurrent()
                    size += Int64(bytes.count)
                    guard totalBytes + size <= maxBytes else { throw failure("单次分享总大小不能超过 512 MiB") }
                    guard retainedBytes + totalBytes + size <= 1024 * 1024 * 1024 else { throw failure("分享暂存超过 1 GiB，请等待上一操作结束后重试") }
                    let free = try folder.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey]).volumeAvailableCapacityForImportantUsage ?? 0
                    guard free > 64 * 1024 * 1024 else { throw failure("可用空间不足，至少需要保留 64 MiB") }
                    try output.write(contentsOf: bytes); hash.update(data: bytes)
                }
                try output.synchronize()
                totalBytes += size
                return ["itemId": itemId, "fileName": readable.lastPathComponent, "mimeType": type.preferredMIMEType ?? "application/octet-stream", "size": size, "sha256": hash.finalize().map { String(format: "%02x", $0) }.joined()]
            }
        }
        if let coordinationError { throw coordinationError }
        do { return try result.get() }
        catch { try? FileManager.default.removeItem(at: destination); throw error }
    }
    private func failedItem(_ error: Error, _ provider: NSItemProvider, _ providers: [NSItemProvider], _ index: Int, _ folder: URL, _ text: [String], _ items: [[String: Any]]) {
        let item: [String: Any] = ["itemId": UUID().uuidString.lowercased(), "fileName": provider.suggestedName ?? "分享文件", "error": error.localizedDescription]
        queue.async { self.acquire(providers, index: index + 1, folder: folder, text: text, items: items + [item]) }
    }
    private func beginRead() -> Bool {
        stateLock.lock(); defer { stateLock.unlock() }
        if cancelled || phase != .acquiring { return false }
        activeReads += 1; return true
    }
    private func endRead() {
        stateLock.lock(); activeReads -= 1
        let shouldFinish = activeReads == 0 && pendingCompletion != nil
        stateLock.unlock()
        if shouldFinish { queue.async { self.finishPendingCompletion() } }
    }

    private func nextText(_ value: String, _ providers: [NSItemProvider], _ index: Int, _ folder: URL, _ text: [String], _ items: [[String: Any]]) {
        guard text.reduce(0, { $0 + $1.utf8.count }) + value.utf8.count <= 1_048_576 else { finish(error: failure("分享文字超过 1 MiB")); return }
        acquire(providers, index: index + 1, folder: folder, text: text + [value], items: items)
    }
    private func setProgress(_ value: Progress) {
        stateLock.lock()
        let stopped = cancelled || phase != .acquiring
        if !stopped { progress = value }
        stateLock.unlock()
        if stopped { value.cancel() }
    }
    private func finish(error: Error) {
        requestCompletion(.failure(error))
    }

    private func requestCompletion(_ completion: Completion) {
        dispatchPrecondition(condition: .onQueue(queue))
        stateLock.lock()
        guard phase == .acquiring else { stateLock.unlock(); return }
        // A queued success must not replace a failure waiting for a provider
        // callback to release its file handles and security scope.
        if pendingCompletion == nil { pendingCompletion = completion }
        else if case .failure = completion { pendingCompletion = completion }
        stateLock.unlock()
        finishPendingCompletion()
    }

    private func finishPendingCompletion() {
        dispatchPrecondition(condition: .onQueue(queue))
        stateLock.lock()
        guard phase == .acquiring, activeReads == 0, let pending = pendingCompletion else {
            stateLock.unlock()
            return
        }
        let wasCancelled = cancelled
        let completion: Completion = wasCancelled ? .failure(failure("分享已取消")) : pending
        // This is the terminal linearization point. A prior cancellation wins;
        // later close actions cannot end the extension while this write owns
        // its lease. Disk/flock work runs without holding the state lock.
        phase = .finishing
        pendingCompletion = nil
        progress = nil
        stateLock.unlock()

        var completionError: Error?
        switch completion {
        case .ready(let folder, let manifest):
            do {
                try storage.publishReady(folder: folder, manifest: manifest)
            } catch { completionError = error }
        case .failure(let error):
            completionError = error
        }
        if let error = completionError {
            do { try storage.publishFailure(error) }
            catch {
                NSLog("[VCPShare] Failed to publish acquisition failure: %@", error.localizedDescription)
            }
        }
        storage.releaseLease()
        stateLock.lock(); phase = .finished; stateLock.unlock()
        let outcome: Outcome
        if let error = completionError { outcome = wasCancelled ? .cancelled(error) : .failed(error) }
        else { outcome = .ready }
        DispatchQueue.main.async { self.completionHandler(outcome) }
    }

    func cancel() {
        stateLock.lock()
        guard phase == .acquiring else { stateLock.unlock(); return }
        cancelled = true
        let active = progress
        let coordinator = activeCoordinator
        stateLock.unlock()
        coordinator?.cancel()
        active?.cancel()
        queue.async { self.finish(error: self.failure("分享已取消")) }
    }
    private func failure(_ message: String) -> NSError {
        NSError(domain: "VCPShare", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
    }
}
