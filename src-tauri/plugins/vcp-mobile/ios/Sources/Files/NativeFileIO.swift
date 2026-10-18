import CryptoKit
import Foundation
import UniformTypeIdentifiers

enum NativeFileIO {
    static let queue = DispatchQueue(label: "com.vcp.avatar.file-io", qos: .userInitiated)

    static func coordinated<T>(_ url: URL, writing: Bool, operation: NativeFileOperation,
                               missingResult: String = "file_provider: no coordinated file",
                               body: (URL) throws -> T) throws -> T {
        try operation.check()
        let coordinator = NSFileCoordinator()
        let progress = Progress(totalUnitCount: 1)
        progress.cancellationHandler = { coordinator.cancel() }
        // The operation replays cancellation if it happened before this registration.
        operation.progress = progress
        defer { operation.progress = nil }
        try operation.check()
        var coordinationError: NSError?
        var result: Result<T, Error>?
        if writing {
            coordinator.coordinate(writingItemAt: url, options: .forMerging, error: &coordinationError) { coordinatedURL in
                result = Result { try operation.check(); return try body(coordinatedURL) }
            }
        } else {
            coordinator.coordinate(readingItemAt: url, options: .withoutChanges, error: &coordinationError) { coordinatedURL in
                result = Result { try operation.check(); return try body(coordinatedURL) }
            }
        }
        // A completed write remains committed even if cancellation arrives afterwards.
        if let result = result { return try result.get() }
        try operation.check()
        if let coordinationError = coordinationError { throw coordinationError }
        throw NativeFileError.failure(missingResult)
    }

    static func copy(_ source: URL, to destination: URL, limit: UInt64, operation: NativeFileOperation, progress: ((UInt64) -> Void)? = nil) throws -> (UInt64, String) {
        try operation.check()
        let values = try source.resourceValues(forKeys: [.isRegularFileKey])
        guard values.isRegularFile == true else { throw NativeFileError.failure("invalid_file: regular file required") }
        try FileManager.default.createDirectory(at: destination.deletingLastPathComponent(), withIntermediateDirectories: true)
        guard FileManager.default.createFile(atPath: destination.path, contents: nil) else { throw NativeFileError.failure("file_write: could not create staging file") }
        let input = try FileHandle(forReadingFrom: source)
        defer { try? input.close() }
        let output = try FileHandle(forWritingTo: destination)
        defer { try? output.close() }
        var count: UInt64 = 0
        var hash = SHA256()
        var lastProgress = Date.distantPast
        while true {
            try operation.check()
            guard let data = try input.read(upToCount: 65_536), !data.isEmpty else { break }
            guard UInt64(data.count) <= limit - min(count, limit) else { throw NativeFileError.failure("file_too_large: input exceeds byte budget") }
            count += UInt64(data.count)
            try output.write(contentsOf: data)
            hash.update(data: data)
            if Date().timeIntervalSince(lastProgress) >= 0.2 { lastProgress = Date(); progress?(count) }
        }
        try output.synchronize()
        try operation.check()
        return (count, hash.finalize().map { String(format: "%02x", $0) }.joined())
    }

    static func leaf(_ name: String) -> String {
        let leaf = URL(fileURLWithPath: name).lastPathComponent
        guard !leaf.isEmpty, leaf != ".", leaf != ".." else { return "file" }
        var bounded = ""
        for character in leaf {
            let next = bounded + String(character)
            if next.utf8.count > 150 { break }
            bounded = next
        }
        return bounded.isEmpty ? "file" : bounded
    }
    static func mime(_ url: URL) -> String {
        UTType(filenameExtension: url.pathExtension)?.preferredMIMEType ?? "application/octet-stream"
    }
    static func temporary(_ name: String) -> URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("vcp-native-exports", isDirectory: true)
            .appendingPathComponent(UUID().uuidString, isDirectory: true).appendingPathComponent(leaf(name))
    }
}
