import CryptoKit
import Foundation
import PhotosUI
import QuickLook
import Tauri
import UIKit
import UniformTypeIdentifiers

struct NativePickedFile: Encodable {
    let path: String
    let name: String
    let mime: String
    let size: UInt64
    let hash: String
    let thumbnailPath: String? = nil
}

private struct NativePickedFileFailure: Encodable { let name: String; let message: String }
private struct NativePickedFiles: Encodable {
    var files: [NativePickedFile] = []
    var errors: [NativePickedFileFailure] = []
}

struct NativeFileActionResult: Encodable { let outcome: String }
struct NativeFileSaveResult: Encodable {
    let uri: String
    let displayName: String
    let mimeType: String
    let size: UInt64
}

private struct PickArgs: Decodable {
    let operationId: String
    let mode: String
    let stagingDir: String
    let maxBytes: UInt64
    let maxFiles: Int

    var selectionLimit: Int { mode == "file" || mode == "gallery" ? max(1, min(maxFiles, 16)) : 1 }
}
private struct CancelFileArgs: Decodable { let operationId: String }
private struct OpenArgs: Decodable { let path: String; let action: String; let operationId: String; let fileName: String? }
private struct ExportArgs: Decodable {
    let filePath: String
    let fileName: String
    let mimeType: String?
    let maxBytes: UInt64
    let operationId: String
}
private struct ExportBytesArgs: Decodable {
    let fileName: String; let contentBase64: String; let mimeType: String?; let operationId: String
}

enum NativeFileIO {
    static let queue = DispatchQueue(label: "com.vcp.avatar.file-io", qos: .userInitiated)
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

private final class FilePickerDelegate: NSObject, UIDocumentPickerDelegate, PHPickerViewControllerDelegate, UIAdaptivePresentationControllerDelegate {
    let owner: NativePresentation
    let operation: NativeFileOperation
    let args: PickArgs
    init(_ owner: NativePresentation, _ operation: NativeFileOperation, _ args: PickArgs) {
        self.owner = owner; self.operation = operation; self.args = args
    }
    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        operation.cancel()
        if !operation.working { owner.fail(operation, "cancelled: file picker") }
    }
    func presentationControllerDidDismiss(_ presentationController: UIPresentationController) {
        if !operation.working { owner.fail(operation, "cancelled: file picker") }
    }
    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        guard !operation.working, !operation.finished else { return }
        guard !urls.isEmpty else { owner.fail(operation, "cancelled: no file selected"); return }
        operation.working = true
        NativeFileIO.queue.async {
            var batch = self.initialBatch(count: urls.count)
            let selected = Array(urls.prefix(self.args.selectionLimit))
            for (index, url) in selected.enumerated() {
                if self.operation.cancelled { break }
                let scoped = url.startAccessingSecurityScopedResource()
                defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                let coordinator = NSFileCoordinator()
                var error: NSError?
                var result: Result<NativePickedFile, Error>?
                let remaining = self.remainingBytes(batch)
                coordinator.coordinate(readingItemAt: url, options: .withoutChanges, error: &error) { readable in
                    result = Result { try self.acquire(readable, name: url.lastPathComponent, index: index, count: selected.count, limit: remaining) }
                }
                self.append(result ?? .failure(error ?? (NativeFileError.failure("file_provider: no coordinated file") as NSError)), name: url.lastPathComponent, to: &batch)
            }
            self.complete(batch)
        }
    }
    func picker(_ picker: PHPickerViewController, didFinishPicking results: [PHPickerResult]) {
        guard !operation.working, !operation.finished else { return }
        guard !results.isEmpty else { owner.fail(operation, "cancelled: photo picker"); return }
        operation.working = true
        picker.dismiss(animated: true)
        loadPhoto(Array(results.prefix(args.selectionLimit)), index: 0, batch: initialBatch(count: results.count))
    }
    private func initialBatch(count: Int) -> NativePickedFiles {
        var batch = NativePickedFiles()
        if count > args.selectionLimit {
            batch.errors.append(NativePickedFileFailure(name: "其余 \(count - args.selectionLimit) 项", message: "每次最多选择 \(args.selectionLimit) 个附件"))
        }
        return batch
    }
    private func remainingBytes(_ batch: NativePickedFiles) -> UInt64 {
        args.maxBytes - min(args.maxBytes, batch.files.reduce(0) { $0 + $1.size })
    }
    private func append(_ result: Result<NativePickedFile, Error>, name: String, to batch: inout NativePickedFiles) {
        switch result {
        case .success(let file): batch.files.append(file)
        case .failure(let error): batch.errors.append(NativePickedFileFailure(name: name, message: error.localizedDescription))
        }
    }
    private func loadPhoto(_ results: [PHPickerResult], index: Int, batch: NativePickedFiles) {
        // Only one provider is active; the next starts on main after this callback
        // has finished copying its borrowed URL and published ownership.
        guard !operation.cancelled, index < results.count else { complete(batch); return }
        let provider = results[index].itemProvider
        let type = provider.registeredTypeIdentifiers.first { UTType($0)?.conforms(to: .image) == true } ?? UTType.image.identifier
        operation.progress = provider.loadFileRepresentation(forTypeIdentifier: type) { url, error in
            // This copy MUST finish before the provider callback returns: the URL
            // is borrowed only for this callback's lifetime.
            var next = batch
            let name = url?.lastPathComponent ?? provider.suggestedName ?? "照片 \(index + 1)"
            let result: Result<NativePickedFile, Error> = Result {
                guard let url = url else { throw error ?? NativeFileError.failure("file_provider: missing photo") }
                return try self.acquire(url, name: name, index: index, count: results.count, limit: self.remainingBytes(batch))
            }
            self.append(result, name: name, to: &next)
            let completed = next
            DispatchQueue.main.async {
                self.operation.progress = nil
                self.loadPhoto(results, index: index + 1, batch: completed)
            }
        }
    }
    private func acquire(_ source: URL, name: String, index: Int, count: Int, limit: UInt64) throws -> NativePickedFile {
        try operation.check()
        let ownedStart = operation.owned.count
        var succeeded = false
        defer {
            if !succeeded {
                operation.owned.suffix(from: ownedStart).forEach { try? FileManager.default.removeItem(at: $0) }
                operation.owned.removeSubrange(ownedStart...)
            }
        }
        let suffix = source.pathExtension.filter { $0.isASCII && ($0.isLetter || $0.isNumber) }.prefix(12)
        let target = URL(fileURLWithPath: args.stagingDir, isDirectory: true)
            .appendingPathComponent("\(args.operationId)-\(UUID().uuidString)" + (suffix.isEmpty ? "" : ".\(suffix)"))
        operation.owned.append(target)
        let declaredSize = (try? source.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0
        let mime = NativeFileIO.mime(source)
        owner.progress(operation, event: "vcp-mobile-file-start", detail: ["name": name, "size": declaredSize, "mime": mime, "selectionIndex": index, "selectionCount": count])
        let (size, hash) = try NativeFileIO.copy(source, to: target, limit: limit, operation: operation) { loaded in
            self.owner.progress(self.operation, event: "vcp-mobile-file-progress", detail: ["name": name, "mime": mime, "loaded": loaded, "total": declaredSize, "progress": declaredSize > 0 ? min(100, Double(loaded) * 100 / Double(declaredSize)) : 0, "selectionIndex": index, "selectionCount": count])
        }
        if args.mode == "avatar" {
            let output = target.deletingPathExtension().appendingPathExtension("avatar")
            let image = try NativeImages.derive(source: target, output: output, dimension: 2048, pixels: 4_194_304, bytes: 20 * 1024 * 1024, quality: 0.9, preserveAlpha: true)
            operation.owned.append(URL(fileURLWithPath: image.path))
            try operation.check()
            succeeded = true
            return NativePickedFile(path: image.path, name: "avatar_work." + (image.mime == "image/png" ? "png" : "jpg"), mime: image.mime, size: image.size, hash: "")
        }
        succeeded = true
        return NativePickedFile(path: target.path, name: NativeFileIO.leaf(name), mime: NativeFileIO.mime(source), size: size, hash: hash)
    }
    private func complete(_ batch: NativePickedFiles) {
        DispatchQueue.main.async {
            self.operation.working = false
            if self.operation.cancelled { self.owner.fail(self.operation, "cancelled: file acquisition"); return }
            if self.args.selectionLimit > 1 {
                self.owner.resolve(self.operation, batch, keepingFiles: batch.files.map { URL(fileURLWithPath: $0.path) })
            } else if let file = batch.files.first {
                self.owner.resolve(self.operation, file, keeping: URL(fileURLWithPath: file.path))
            } else {
                self.owner.fail(self.operation, batch.errors.first?.message ?? "file_provider: no file selected")
            }
        }
    }
}

private final class PreviewDelegate: NSObject, QLPreviewControllerDataSource, QLPreviewControllerDelegate {
    let url: URL; let owner: NativePresentation; let operation: NativeFileOperation
    init(_ url: URL, _ owner: NativePresentation, _ operation: NativeFileOperation) { self.url = url; self.owner = owner; self.operation = operation }
    func numberOfPreviewItems(in controller: QLPreviewController) -> Int { 1 }
    func previewController(_ controller: QLPreviewController, previewItemAt index: Int) -> QLPreviewItem { url as NSURL }
    func previewController(_ controller: QLPreviewController, editingModeFor previewItem: QLPreviewItem) -> QLPreviewItemEditingMode { .disabled }
    func previewControllerDidDismiss(_ controller: QLPreviewController) { owner.resolve(operation, NativeFileActionResult(outcome: "completed")) }
}

private final class ExportDelegate: NSObject, UIDocumentPickerDelegate, UIAdaptivePresentationControllerDelegate {
    let result: NativeFileSaveResult; let owner: NativePresentation; let operation: NativeFileOperation
    init(_ result: NativeFileSaveResult, _ owner: NativePresentation, _ operation: NativeFileOperation) { self.result = result; self.owner = owner; self.operation = operation }
    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) { owner.fail(operation, "cancelled: Files export") }
    func presentationControllerDidDismiss(_ presentationController: UIPresentationController) { owner.fail(operation, "cancelled: Files export") }
    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        owner.resolve(operation, NativeFileSaveResult(uri: urls.first?.absoluteString ?? "", displayName: result.displayName, mimeType: result.mimeType, size: result.size))
    }
}

extension VcpMobilePlugin {
    @objc public func cancelFileOperation(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(CancelFileArgs.self)
            DispatchQueue.main.async { self.presentation.cancel(args.operationId); invoke.resolve() }
        } catch { invoke.reject(error.localizedDescription) }
    }
    @objc public func pickFile(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(PickArgs.self)
            DispatchQueue.main.async {
                do {
                    let operation = try self.presentation.begin(args.operationId, invoke)
                    if args.mode == "camera" {
                        NativeCamera.start(owner: self.presentation, operation: operation, stagingDir: args.stagingDir, maxBytes: args.maxBytes)
                        return
                    }
                    let delegate = FilePickerDelegate(self.presentation, operation, args)
                    operation.delegate = delegate
                    let controller: UIViewController
                    if args.mode == "gallery" || args.mode == "avatar" {
                        var configuration = PHPickerConfiguration()
                        configuration.selectionLimit = args.selectionLimit
                        configuration.selection = .ordered
                        configuration.filter = .images
                        configuration.preferredAssetRepresentationMode = .current
                        let picker = PHPickerViewController(configuration: configuration)
                        picker.delegate = delegate; controller = picker
                    } else {
                        let types: [UTType] = args.mode == "config"
                            ? [.json, UTType(exportedAs: "com.vcp.avatar.configuration", conformingTo: .json)]
                            : [.item]
                        let picker = UIDocumentPickerViewController(forOpeningContentTypes: types, asCopy: false)
                        picker.allowsMultipleSelection = args.selectionLimit > 1; picker.delegate = delegate; controller = picker
                    }
                    do { try self.presentation.present(controller, for: operation); controller.presentationController?.delegate = delegate }
                    catch { self.presentation.fail(operation, error.localizedDescription) }
                } catch { invoke.reject(error.localizedDescription) }
            }
        } catch { invoke.reject(error.localizedDescription) }
    }

    @objc public func openFile(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(OpenArgs.self)
            DispatchQueue.main.async {
                do {
                    let operation = try self.presentation.begin(args.operationId, invoke)
                    operation.working = true
                    let source = URL(fileURLWithPath: args.path)
                    let copy = NativeFileIO.temporary(args.fileName ?? source.lastPathComponent)
                    operation.owned = [copy.deletingLastPathComponent()]
                    NativeFileIO.queue.async {
                        let result = Result { try NativeFileIO.copy(source, to: copy, limit: 512 * 1024 * 1024, operation: operation) }
                        DispatchQueue.main.async {
                            operation.working = false
                            do {
                                _ = try result.get(); try operation.check()
                                if args.action == "share" {
                                    let controller = UIActivityViewController(activityItems: [copy], applicationActivities: nil)
                                    controller.completionWithItemsHandler = { _, completed, _, error in
                                        operation.systemOwnsFiles = false
                                        if let error = error { self.presentation.fail(operation, error.localizedDescription) }
                                        else { self.presentation.resolve(operation, NativeFileActionResult(outcome: completed ? "completed" : "cancelled")) }
                                    }
                                    try self.presentation.present(controller, for: operation)
                                    operation.systemOwnsFiles = true
                                } else {
                                    guard QLPreviewController.canPreview(copy as NSURL) else { self.presentation.resolve(operation, NativeFileActionResult(outcome: "cannotPreview")); return }
                                    let controller = QLPreviewController()
                                    let delegate = PreviewDelegate(copy, self.presentation, operation)
                                    operation.delegate = delegate; controller.dataSource = delegate; controller.delegate = delegate
                                    try self.presentation.present(controller, for: operation)
                                }
                            } catch { self.presentation.fail(operation, error.localizedDescription) }
                        }
                    }
                } catch { invoke.reject(error.localizedDescription) }
            }
        } catch { invoke.reject(error.localizedDescription) }
    }

    @objc public func saveFileToDownloads(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(ExportArgs.self)
            exportFile(args, invoke: invoke, ownedInput: nil)
        } catch { invoke.reject(error.localizedDescription) }
    }

    @objc public func saveToDownloads(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(ExportBytesArgs.self)
            guard args.contentBase64.utf8.count <= 28 * 1024 * 1024 else { throw NativeFileError.failure("file_too_large: export exceeds byte budget") }
            NativeFileIO.queue.async {
                let source = NativeFileIO.temporary(args.fileName)
                do {
                    guard let data = Data(base64Encoded: args.contentBase64), data.count <= 20 * 1024 * 1024 else { throw NativeFileError.failure("invalid_export: invalid or oversized base64") }
                    try FileManager.default.createDirectory(at: source.deletingLastPathComponent(), withIntermediateDirectories: true)
                    try data.write(to: source, options: .atomic)
                    self.exportFile(ExportArgs(filePath: source.path, fileName: args.fileName, mimeType: args.mimeType, maxBytes: 20 * 1024 * 1024, operationId: args.operationId), invoke: invoke, ownedInput: source.deletingLastPathComponent())
                } catch { try? FileManager.default.removeItem(at: source.deletingLastPathComponent()); invoke.reject(error.localizedDescription) }
            }
        } catch { invoke.reject(error.localizedDescription) }
    }

    private func exportFile(_ args: ExportArgs, invoke: Invoke, ownedInput: URL?) {
            DispatchQueue.main.async {
                do {
                    let operation = try self.presentation.begin(args.operationId, invoke)
                    operation.working = true
                    let copy = NativeFileIO.temporary(args.fileName)
                    operation.owned = [copy.deletingLastPathComponent()]
                    if let ownedInput = ownedInput { operation.owned.append(ownedInput) }
                    NativeFileIO.queue.async {
                        let result = Result { try NativeFileIO.copy(URL(fileURLWithPath: args.filePath), to: copy, limit: args.maxBytes, operation: operation) }
                        DispatchQueue.main.async {
                            operation.working = false
                            do {
                                let (size, _) = try result.get(); try operation.check()
                                let value = NativeFileSaveResult(uri: "", displayName: copy.lastPathComponent, mimeType: args.mimeType ?? NativeFileIO.mime(copy), size: size)
                                let delegate = ExportDelegate(value, self.presentation, operation)
                                operation.delegate = delegate
                                let picker = UIDocumentPickerViewController(forExporting: [copy], asCopy: true)
                                picker.delegate = delegate
                                try self.presentation.present(picker, for: operation)
                                operation.systemOwnsFiles = true
                                picker.presentationController?.delegate = delegate
                            } catch { self.presentation.fail(operation, error.localizedDescription) }
                        }
                    }
                } catch {
                    if let ownedInput = ownedInput { try? FileManager.default.removeItem(at: ownedInput) }
                    invoke.reject(error.localizedDescription)
                }
            }
    }
}
