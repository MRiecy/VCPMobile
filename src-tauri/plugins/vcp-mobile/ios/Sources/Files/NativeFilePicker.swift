import Foundation
import PhotosUI
import Tauri
import UIKit
import UniformTypeIdentifiers

private struct NativePickedFileFailure: Encodable { let name: String; let message: String }
private struct NativePickedFiles: Encodable {
    var files: [NativePickedFile] = []
    var errors: [NativePickedFileFailure] = []
}

private struct PickArgs: Decodable {
    let operationId: String
    let reservationToken: String?
    let mode: String
    let stagingDir: String
    let maxBytes: UInt64
    let maxFiles: Int

    var selectionLimit: Int { mode == "file" || mode == "gallery" ? max(1, min(maxFiles, 16)) : 1 }
}
private struct PreparePickerArgs: Decodable { let operationId: String; let sourceWebView: String }
private struct CancelFileArgs: Decodable { let operationId: String }

private final class FilePickerDelegate: NSObject, UIDocumentPickerDelegate, PHPickerViewControllerDelegate, UIAdaptivePresentationControllerDelegate {
    let owner: NativePresentation
    let operation: NativeFileOperation
    let args: PickArgs
    var configDirectoryAccess: NativeConfigBackupDirectory.Access?
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
                let remaining = self.remainingBytes(batch)
                let result = Result {
                    try NativeFileIO.coordinated(url, writing: false, operation: self.operation) { readable in
                        try self.acquire(readable, name: url.lastPathComponent, index: index, count: selected.count, limit: remaining)
                    }
                }
                self.append(result, name: url.lastPathComponent, to: &batch)
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

extension VcpMobilePlugin {
    @objc public func prepareFileOperation(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(PreparePickerArgs.self)
            DispatchQueue.main.async {
                do { invoke.resolve(["reservationToken": try self.presentation.preparePicker(args.operationId, sourceWebView: args.sourceWebView)]) }
                catch { invoke.reject(error.localizedDescription) }
            }
        } catch { invoke.reject(error.localizedDescription) }
    }

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
                    let operation = try self.presentation.beginPicker(args.operationId, reservationToken: args.reservationToken, invoke)
                    if args.mode == "camera" {
                        NativeCamera.start(owner: self.presentation, operation: operation, stagingDir: args.stagingDir, maxBytes: args.maxBytes)
                        return
                    }
                    let delegate = FilePickerDelegate(self.presentation, operation, args)
                    operation.delegate = delegate
                    if args.mode == "config" {
                        NativeConfigBackupDirectory.prepare(owner: self.presentation, operation: operation, create: false) { access in
                            delegate.configDirectoryAccess = access
                            operation.delegate = delegate
                            let types: [UTType] = [
                                .json, UTType(exportedAs: "com.vcp.avatar.configuration", conformingTo: .json)
                            ]
                            let picker = UIDocumentPickerViewController(forOpeningContentTypes: types, asCopy: false)
                            picker.directoryURL = access.directory
                            picker.shouldShowFileExtensions = true
                            picker.allowsMultipleSelection = false
                            picker.delegate = delegate
                            do { try self.presentation.present(picker, for: operation); picker.presentationController?.delegate = delegate }
                            catch { self.presentation.fail(operation, error.localizedDescription) }
                        }
                        return
                    }
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
                        let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.item], asCopy: false)
                        picker.allowsMultipleSelection = args.selectionLimit > 1; picker.delegate = delegate; controller = picker
                    }
                    do { try self.presentation.present(controller, for: operation); controller.presentationController?.delegate = delegate }
                    catch { self.presentation.fail(operation, error.localizedDescription) }
                } catch { invoke.reject(error.localizedDescription) }
            }
        } catch { invoke.reject(error.localizedDescription) }
    }
}
