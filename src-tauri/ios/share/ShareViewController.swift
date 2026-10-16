import UIKit
import UniformTypeIdentifiers
import CryptoKit
import Darwin

/// The extension owns acquisition only. The containing app owns CAS, drafts and messages.
final class ShareViewController: UIViewController {
    private let queue = DispatchQueue(label: "com.vcp.avatar.share.acquire")
    private let stateLock = NSLock()
    private var cancelled = false
    private var activeReads = 0
    private var deferredFailure: Error?
    private var finished = false
    private var progress: Progress?
    private var root: URL?
    private var identity: [String: Any]?
    private var lease: FileHandle?
    private let label = UILabel()
    private let button = UIButton(type: .system)
    private var started = false
    private let maxBytes: Int64 = 512 * 1024 * 1024
    private var totalBytes: Int64 = 0
    private var retainedBytes: Int64 = 0

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground
        label.numberOfLines = 0
        label.textAlignment = .center
        label.text = "正在保存分享…"
        button.setTitle("取消", for: .normal)
        button.addTarget(self, action: #selector(close), for: .touchUpInside)
        let stack = UIStackView(arrangedSubviews: [label, button])
        stack.axis = .vertical
        stack.spacing = 24
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 24),
            stack.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -24),
            stack.centerYAnchor.constraint(equalTo: view.safeAreaLayoutGuide.centerYAnchor)
        ])
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        guard !started else { return }
        started = true
        let items = extensionContext?.inputItems as? [NSExtensionItem] ?? []
        let providers = items.flatMap { $0.attachments ?? [] }
        let initialText = items.filter { item in
            !(item.attachments ?? []).contains { $0.hasItemConformingToTypeIdentifier(UTType.plainText.identifier) || $0.hasItemConformingToTypeIdentifier(UTType.url.identifier) }
        }.compactMap { $0.attributedContentText?.string }
        queue.async { self.begin(providers, initialText: initialText) }
    }

    private func withSlot<T>(_ body: () throws -> T) throws -> T {
        guard let root else { throw failure("共享目录不可用") }
        let path = root.appendingPathComponent("current.lock").path
        let fd = Darwin.open(path, O_CREAT | O_RDWR, S_IRUSR | S_IWUSR)
        guard fd >= 0 else { throw failure("无法打开分享锁") }
        defer { Darwin.close(fd) }
        guard flock(fd, LOCK_EX) == 0 else { throw failure("无法锁定当前分享") }
        defer { flock(fd, LOCK_UN) }
        return try body()
    }

    private func write(_ object: [String: Any], to url: URL) throws {
        let data = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
        try data.write(to: url, options: .atomic)
        let handle = try FileHandle(forWritingTo: url)
        try handle.synchronize()
        try handle.close()
        let fd = Darwin.open(url.deletingLastPathComponent().path, O_RDONLY)
        if fd >= 0 { _ = fsync(fd); Darwin.close(fd) }
        if url.lastPathComponent == "current.json" {
            CFNotificationCenterPostNotification(CFNotificationCenterGetDarwinNotifyCenter(), CFNotificationName("com.vcp.avatar.share.changed" as CFString), nil, nil, true)
        }
    }

    private func isCurrent() throws -> Bool {
        guard let root, let identity else { return false }
        let value = try JSONSerialization.jsonObject(with: Data(contentsOf: root.appendingPathComponent("current.json"))) as? [String: Any]
        return value?["deliveryId"] as? String == identity["deliveryId"] as? String
    }

    private func checkCurrent() throws {
        stateLock.lock(); let stopped = cancelled; stateLock.unlock()
        if stopped { throw failure("分享已取消") }
        if try !withSlot({ try isCurrent() }) { throw failure("这次分享已被新的分享覆盖") }
    }

    private func begin(_ providers: [NSItemProvider], initialText: [String]) {
        do {
            guard let group = Bundle.main.object(forInfoDictionaryKey: "VCPShareAppGroup") as? String,
                  let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group) else {
                throw failure("本次安装的共享容器不可用，请检查主应用与分享扩展的 App Group 签名。")
            }
            root = container.appendingPathComponent("ShareInbox", isDirectory: true)
            guard let root else { return }
            try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
            let delivery = UUID().uuidString.lowercased()
            let folder = root.appendingPathComponent(delivery, isDirectory: true)
            try withSlot {
                // Creation, lease acquisition and slot publication are ordered against app cleanup.
                try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
                let fd = Darwin.open(folder.appendingPathComponent("lease.lock").path, O_CREAT | O_RDWR, S_IRUSR | S_IWUSR)
                guard fd >= 0 else { throw failure("无法准备分享文件") }
                guard flock(fd, LOCK_EX | LOCK_NB) == 0 else { Darwin.close(fd); throw failure("分享文件正在使用") }
                lease = FileHandle(fileDescriptor: fd, closeOnDealloc: true)
                let old = try? JSONSerialization.jsonObject(with: Data(contentsOf: root.appendingPathComponent("current.json"))) as? [String: Any]
                let generation = (old?["generation"] as? Int64 ?? 0) + 1
                let next: [String: Any] = ["generation": generation, "deliveryId": delivery, "state": "receiving"]
                identity = next
                try write(next, to: root.appendingPathComponent("current.json"))
                // Old directories are cleanup only. Never remove a provider/import with a live lease.
                for oldFolder in try FileManager.default.contentsOfDirectory(at: root, includingPropertiesForKeys: [.isDirectoryKey]) where oldFolder != folder {
                    guard (try? oldFolder.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true else { continue }
                    let oldFD = Darwin.open(oldFolder.appendingPathComponent("lease.lock").path, O_RDWR)
                    if oldFD >= 0 {
                        if flock(oldFD, LOCK_EX | LOCK_NB) == 0 { try? FileManager.default.removeItem(at: oldFolder) }
                        Darwin.close(oldFD)
                    }
                }
            }
            if let enumerator = FileManager.default.enumerator(at: root, includingPropertiesForKeys: [.fileSizeKey, .isRegularFileKey]) {
                for case let file as URL in enumerator {
                    let values = try file.resourceValues(forKeys: [.fileSizeKey, .isRegularFileKey])
                    if values.isRegularFile == true { retainedBytes += Int64(values.fileSize ?? 0) }
                }
            }
            guard providers.count <= 16 else { throw failure("一次分享最多包含 16 项") }
            guard initialText.reduce(0, { $0 + $1.utf8.count }) <= 1_048_576 else { throw failure("分享文字超过 1 MiB") }
            acquire(providers, index: 0, folder: folder, text: initialText, items: [])
        } catch { finish(error: error) }
    }

    private func acquire(_ providers: [NSItemProvider], index: Int, folder: URL, text: [String], items: [[String: Any]]) {
        do { try checkCurrent() } catch { finish(error: error); return }
        if index == providers.count {
            do {
                guard let root, var current = identity else { throw failure("分享状态缺失") }
                let manifest: [String: Any] = ["text": text.joined(separator: "\n"), "items": items]
                try write(manifest, to: folder.appendingPathComponent("manifest.json"))
                try withSlot {
                    guard try isCurrent() else { throw failure("这次分享已被覆盖") }
                    current["state"] = "ready"
                    try write(current, to: root.appendingPathComponent("current.json"))
                }
                lease = nil
                DispatchQueue.main.async {
                    self.label.text = "已保存。打开 VCPMobile，选择助手后继续。"
                    self.button.setTitle("完成", for: .normal)
                    self.button.removeTarget(self, action: #selector(self.close), for: .touchUpInside)
                    self.button.addTarget(self, action: #selector(self.complete), for: .touchUpInside)
                }
            } catch { finish(error: error) }
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
        NSFileCoordinator().coordinate(readingItemAt: url, options: .withoutChanges, error: &coordinationError) { readable in
            result = Result {
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
        if cancelled || finished { return false }
        activeReads += 1; return true
    }
    private func endRead() {
        stateLock.lock(); activeReads -= 1
        let deferred = activeReads == 0 ? deferredFailure : nil
        if deferred != nil { deferredFailure = nil }
        stateLock.unlock()
        if let deferred { queue.async { self.finish(error: deferred) } }
    }

    private func nextText(_ value: String, _ providers: [NSItemProvider], _ index: Int, _ folder: URL, _ text: [String], _ items: [[String: Any]]) {
        guard text.reduce(0, { $0 + $1.utf8.count }) + value.utf8.count <= 1_048_576 else { finish(error: failure("分享文字超过 1 MiB")); return }
        acquire(providers, index: index + 1, folder: folder, text: text + [value], items: items)
    }
    private func setProgress(_ value: Progress) {
        stateLock.lock(); progress = value; let stopped = cancelled; stateLock.unlock()
        if stopped { value.cancel() }
    }
    private func finish(error: Error) {
        stateLock.lock()
        if finished { stateLock.unlock(); return }
        if activeReads > 0 { deferredFailure = error; stateLock.unlock(); return }
        finished = true
        let wasCancelled = cancelled
        stateLock.unlock()
        if let root, var current = identity {
            try? withSlot {
                if try isCurrent() {
                    current["state"] = "failed"; current["error"] = error.localizedDescription
                    try write(current, to: root.appendingPathComponent("current.json"))
                }
            }
        }
        lease = nil
        DispatchQueue.main.async {
            if wasCancelled { self.extensionContext?.cancelRequest(withError: error); return }
            self.label.text = error.localizedDescription; self.button.setTitle("关闭", for: .normal)
            self.button.removeTarget(self, action: #selector(self.close), for: .touchUpInside)
            self.button.addTarget(self, action: #selector(self.complete), for: .touchUpInside)
        }
    }
    @objc private func close() {
        stateLock.lock(); cancelled = true; let active = progress; stateLock.unlock()
        active?.cancel()
        queue.async { self.finish(error: self.failure("分享已取消")) }
    }
    @objc private func complete() { extensionContext?.completeRequest(returningItems: nil) }
    private func failure(_ message: String) -> NSError { NSError(domain: "VCPShare", code: 1, userInfo: [NSLocalizedDescriptionKey: message]) }
}
