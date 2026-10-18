import Foundation
import QuickLook
import Tauri
import UIKit

private struct NativeFileActionResult: Encodable { let outcome: String }

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
    let purpose: String?
    let reservationToken: String?
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
            if args.purpose == "config" {
                DispatchQueue.main.async {
                    do {
                        let operation = try self.presentation.beginPicker(args.operationId, reservationToken: args.reservationToken, invoke)
                        NativeConfigBackupDirectory.export(contentBase64: args.contentBase64, fileName: args.fileName,
                                                           mimeType: args.mimeType, owner: self.presentation, operation: operation)
                    } catch { invoke.reject(error.localizedDescription) }
                }
                return
            }
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
