import Foundation
import Tauri
import UIKit
import UniformTypeIdentifiers

/// A user-selected parent directory is the only persistent grant. The app's
/// VCPMobile child is resolved inside that grant on each configuration operation.
enum NativeConfigBackupDirectory {
    private static let bookmarkKey = "vcp.configBackup.parentBookmark"
    private enum DirectoryError: Error { case chooseAgain }

    final class Access {
        let parent: URL
        let directory: URL
        init(parent: URL, create: Bool, operation: NativeFileOperation) throws {
            guard parent.startAccessingSecurityScopedResource() else { throw DirectoryError.chooseAgain }
            self.parent = parent
            do {
                self.directory = try NativeFileIO.coordinated(parent, writing: create, operation: operation,
                                                              missingResult: "file_provider: directory coordination failed") { coordinatedParent in
                    try operation.check()
                    guard try coordinatedParent.resourceValues(forKeys: [.isDirectoryKey]).isDirectory == true else {
                        throw DirectoryError.chooseAgain
                    }
                    // A reinstall may have the user select the existing backup folder.
                    if coordinatedParent.lastPathComponent == "VCPMobile" { return coordinatedParent }
                    let child = coordinatedParent.appendingPathComponent("VCPMobile", isDirectory: true)
                    if FileManager.default.fileExists(atPath: child.path) {
                        guard try child.resourceValues(forKeys: [.isDirectoryKey]).isDirectory == true else {
                            throw NativeFileError.failure("config_directory: VCPMobile 已存在且不是文件夹")
                        }
                        return child
                    }
                    if create {
                        try FileManager.default.createDirectory(at: child, withIntermediateDirectories: false)
                        return child
                    }
                    // Import before the first export can still browse the selected parent.
                    return coordinatedParent
                }
            } catch {
                parent.stopAccessingSecurityScopedResource()
                throw error
            }
        }
        deinit { parent.stopAccessingSecurityScopedResource() }
    }

    /// All bookmark resolution and provider I/O use the existing serial file queue.
    /// The returned Access stays alive until the picker/copy's terminal callback.
    static func prepare(owner: NativePresentation, operation: NativeFileOperation, create: Bool,
                        ready: @escaping (Access) -> Void) {
        operation.working = true
        NativeFileIO.queue.async {
            let result: Result<Access?, Error> = Result {
                try operation.check()
                guard let bookmark = UserDefaults.standard.data(forKey: bookmarkKey) else { return nil }
                let parent: URL
                var stale = false
                do {
                    parent = try URL(resolvingBookmarkData: bookmark, options: [.withoutUI], relativeTo: nil, bookmarkDataIsStale: &stale)
                } catch {
                    if needsSelection(error) || ((error as NSError).domain == NSCocoaErrorDomain && (error as NSError).code == CocoaError.fileReadCorruptFile.rawValue) {
                        UserDefaults.standard.removeObject(forKey: bookmarkKey)
                        return nil
                    }
                    throw error
                }
                do {
                    let access = try Access(parent: parent, create: create, operation: operation)
                    if stale { try saveBookmark(parent) }
                    return access
                } catch {
                    if needsSelection(error) {
                        UserDefaults.standard.removeObject(forKey: bookmarkKey)
                        return nil
                    }
                    throw error
                }
            }
            DispatchQueue.main.async {
                operation.working = false
                do {
                    try operation.check()
                    if let access = try result.get() { ready(access) }
                    else { try chooseParent(owner: owner, operation: operation, create: create, ready: ready) }
                } catch { owner.fail(operation, error.localizedDescription) }
            }
        }
    }

    private static func needsSelection(_ error: Error) -> Bool {
        if case DirectoryError.chooseAgain = error { return true }
        let error = error as NSError
        return error.domain == NSCocoaErrorDomain && [
            CocoaError.fileNoSuchFile.rawValue, CocoaError.fileReadNoSuchFile.rawValue,
            CocoaError.fileReadNoPermission.rawValue, CocoaError.fileWriteNoPermission.rawValue
        ].contains(error.code)
    }

    private static func saveBookmark(_ parent: URL) throws {
        // minimalBookmark is the iOS directory-picker contract; withSecurityScope
        // is a macOS option and must not be used in this mobile target.
        let bookmark = try parent.bookmarkData(options: [.minimalBookmark], includingResourceValuesForKeys: nil, relativeTo: nil)
        UserDefaults.standard.set(bookmark, forKey: bookmarkKey)
    }

    private static func chooseParent(owner: NativePresentation, operation: NativeFileOperation,
                                     create: Bool, ready: @escaping (Access) -> Void) throws {
        let delegate = DirectoryDelegate(owner: owner, operation: operation, create: create, ready: ready)
        operation.delegate = delegate
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.folder], asCopy: false)
        picker.title = "选择配置备份的父目录"
        picker.allowsMultipleSelection = false
        picker.delegate = delegate
        try owner.present(picker, for: operation)
        picker.presentationController?.delegate = delegate
    }

    private final class DirectoryDelegate: NSObject, UIDocumentPickerDelegate, UIAdaptivePresentationControllerDelegate {
        let owner: NativePresentation
        let operation: NativeFileOperation
        let create: Bool
        let ready: (Access) -> Void
        private var selected = false
        init(owner: NativePresentation, operation: NativeFileOperation, create: Bool, ready: @escaping (Access) -> Void) {
            self.owner = owner; self.operation = operation; self.create = create; self.ready = ready
        }
        func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
            operation.cancel()
            if !operation.working { owner.fail(operation, "cancelled: config directory selection") }
        }
        func presentationControllerDidDismiss(_ presentationController: UIPresentationController) {
            if !selected && !operation.working { owner.fail(operation, "cancelled: config directory selection") }
        }
        func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
            guard !operation.working, !operation.finished else { return }
            guard let parent = urls.first else { owner.fail(operation, "cancelled: no config directory selected"); return }
            selected = true
            operation.working = true
            NativeFileIO.queue.async {
                let result = Result {
                    try self.operation.check()
                    let access = try Access(parent: parent, create: self.create, operation: self.operation)
                    try self.operation.check()
                    try NativeConfigBackupDirectory.saveBookmark(parent)
                    return access
                }
                DispatchQueue.main.async {
                    // A second picker must wait for the folder picker's dismissal.
                    let proceed = {
                        self.operation.controller = nil
                        self.operation.anchorView?.removeFromSuperview()
                        self.operation.anchorView = nil
                        self.operation.working = false
                        do { try self.operation.check(); self.ready(try result.get()) }
                        catch { self.owner.fail(self.operation, error.localizedDescription) }
                    }
                    if controller.isBeingDismissed, let transition = controller.transitionCoordinator {
                        transition.animate(alongsideTransition: nil) { _ in proceed() }
                    } else if controller.presentingViewController != nil {
                        controller.dismiss(animated: true, completion: proceed)
                    } else { proceed() }
                }
            }
        }
    }

    static func export(contentBase64: String, fileName: String, mimeType: String?, owner: NativePresentation,
                       operation: NativeFileOperation) {
        prepare(owner: owner, operation: operation, create: true) { access in
            operation.working = true
            NativeFileIO.queue.async {
                let result: Result<NativeFileSaveResult, Error> = Result {
                    try operation.check()
                    guard contentBase64.utf8.count <= 1_398_104,
                          let data = Data(base64Encoded: contentBase64), !data.isEmpty, data.count <= 1024 * 1024 else {
                        throw NativeFileError.failure("invalid_export: 配置备份无效或超过 1 MiB")
                    }
                    return try NativeFileIO.coordinated(access.directory, writing: true, operation: operation,
                                                       missingResult: "file_provider: directory coordination failed") { directory in
                        try operation.check()
                        let name = NativeFileIO.leaf(fileName)
                        let suffix = name.hasSuffix(".vcpcfg.json") ? ".vcpcfg.json" : "." + URL(fileURLWithPath: name).pathExtension
                        let stem = String(name.dropLast(suffix.count))
                        let temporary = directory.appendingPathComponent(".vcp-config-\(UUID().uuidString).tmp")
                        defer { try? FileManager.default.removeItem(at: temporary) }
                        try data.write(to: temporary, options: .atomic)
                        // moveItem refuses to overwrite. Only this operation's temporary
                        // file is removed on cancellation/failure; committed backups remain.
                        for index in 0..<1000 {
                            try operation.check()
                            let candidate = directory.appendingPathComponent(index == 0 ? name : "\(stem)-\(index)\(suffix)")
                            do {
                                try FileManager.default.moveItem(at: temporary, to: candidate)
                                return NativeFileSaveResult(uri: candidate.absoluteString, displayName: candidate.lastPathComponent,
                                                            mimeType: mimeType ?? "application/json", size: UInt64(data.count))
                            } catch {
                                let error = error as NSError
                                if error.domain != NSCocoaErrorDomain || error.code != CocoaError.fileWriteFileExists.rawValue { throw error }
                            }
                        }
                        throw NativeFileError.failure("config_export: 同名备份过多，请更换文件名")
                    }
                }
                // Keep the grant until coordination and the bounded write have finished.
                withExtendedLifetime(access) {}
                DispatchQueue.main.async {
                    operation.working = false
                    switch result {
                    case .success(let value): owner.resolve(operation, value)
                    case .failure(let error): owner.fail(operation, error.localizedDescription)
                    }
                }
            }
        }
    }

}
