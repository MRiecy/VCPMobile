import Foundation
import Darwin

/// Owns one App Group delivery's lease and filesystem protocol, not its terminal decision.
final class ShareInboxStorage {
    struct Delivery {
        let folder: URL
        let retainedBytes: Int64
    }

    private var root: URL?
    private var identity: [String: Any]?
    private var lease: FileHandle?

    func begin() throws -> Delivery {
        guard let group = Bundle.main.object(forInfoDictionaryKey: "VCPShareAppGroup") as? String,
              let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group) else {
            throw failure("当前安装无法从其他 App 接收分享。请打开 VCPMobile，直接选择文件或粘贴文字。")
        }
        root = container.appendingPathComponent("ShareInbox", isDirectory: true)
        guard let root else { throw failure("共享目录不可用") }
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
        var retainedBytes: Int64 = 0
        if let enumerator = FileManager.default.enumerator(at: root, includingPropertiesForKeys: [.fileSizeKey, .isRegularFileKey]) {
            for case let file as URL in enumerator {
                let values = try file.resourceValues(forKeys: [.fileSizeKey, .isRegularFileKey])
                if values.isRegularFile == true { retainedBytes += Int64(values.fileSize ?? 0) }
            }
        }
        return Delivery(folder: folder, retainedBytes: retainedBytes)
    }

    func checkCurrent() throws {
        if try !withSlot({ try isCurrent() }) { throw failure("这次分享已被新的分享覆盖") }
    }

    func publishReady(folder: URL, manifest: [String: Any]) throws {
        guard let root, var current = identity else { throw failure("分享状态缺失") }
        try write(manifest, to: folder.appendingPathComponent("manifest.json"))
        try withSlot {
            guard try isCurrent() else { throw failure("这次分享已被覆盖") }
            current["state"] = "ready"
            try write(current, to: root.appendingPathComponent("current.json"))
        }
    }

    func publishFailure(_ error: Error) throws {
        guard let root, var current = identity else { return }
        try withSlot {
            if try isCurrent() {
                current["state"] = "failed"; current["error"] = error.localizedDescription
                try write(current, to: root.appendingPathComponent("current.json"))
            }
        }
    }

    func releaseLease() { lease = nil }

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

    private func failure(_ message: String) -> NSError {
        NSError(domain: "VCPShare", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
    }
}
