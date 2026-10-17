import Foundation
import Tauri
import UIKit
import WebKit

enum NativeFileError: LocalizedError {
    case failure(String)
    var errorDescription: String? { if case .failure(let value) = self { return value }; return nil }
}

final class NativeFileOperation {
    let id: String
    let invoke: Invoke
    private let lock = NSLock()
    private var cancelledValue = false
    var cancelled: Bool { lock.lock(); defer { lock.unlock() }; return cancelledValue }
    private var progressValue: Progress?
    var progress: Progress? {
        get { lock.lock(); defer { lock.unlock() }; return progressValue }
        set {
            lock.lock(); progressValue = newValue; let wasCancelled = cancelledValue; lock.unlock()
            if wasCancelled { newValue?.cancel() }
        }
    }
    var controller: UIViewController?
    var anchorView: UIView?
    var delegate: AnyObject?
    var working = false // main queue only
    var systemOwnsFiles = false // wait for the system's final read callback
    var cancellationHandler: (() -> Void)? // main queue, resource-specific shutdown
    var finished = false // main queue only
    var owned: [URL] = [] // worker publishes ownership before returning to main
    init(id: String, invoke: Invoke) { self.id = id; self.invoke = invoke }
    func cancel() { lock.lock(); cancelledValue = true; let progress = progressValue; lock.unlock(); progress?.cancel() }
    func check() throws { if cancelled { throw NativeFileError.failure("cancelled: file operation") } }
}

/// One modal owner for the attached Scene. Inactive notifications from system
/// pickers/permissions do not cancel a user's operation.
final class NativePresentation: NSObject {
    weak var webView: WKWebView?
    private(set) var current: NativeFileOperation?
    private var sceneObserver: NSObjectProtocol?

    override init() {
        super.init()
        // This queue precedes all work for the new process. Only operation-owned
        // exports are removed; CAS, uploads and the Share Inbox have other owners.
        NativeFileIO.queue.async {
            let root = FileManager.default.temporaryDirectory.appendingPathComponent("vcp-native-exports", isDirectory: true)
            if let children = try? FileManager.default.contentsOfDirectory(at: root, includingPropertiesForKeys: nil) {
                children.forEach { try? FileManager.default.removeItem(at: $0) }
            }
        }
    }

    func attach(_ view: WKWebView) {
        if webView !== view, let operation = current { cancel(operation.id) }
        webView = view
        if sceneObserver == nil {
            sceneObserver = NotificationCenter.default.addObserver(
                forName: UIScene.didDisconnectNotification, object: nil, queue: .main
            ) { [weak self] notification in
                guard let self = self, let scene = notification.object as? UIScene,
                      self.webView?.window?.windowScene === scene,
                      let operation = self.current else { return }
                self.cancel(operation.id)
            }
        }
    }

    func begin(_ id: String, _ invoke: Invoke) throws -> NativeFileOperation {
        dispatchPrecondition(condition: .onQueue(.main))
        guard current == nil else { throw NativeFileError.failure("busy: another native file operation is active") }
        guard webView?.window?.windowScene != nil else { throw NativeFileError.failure("presentation_unavailable: no attached Scene") }
        let operation = NativeFileOperation(id: id, invoke: invoke)
        current = operation
        return operation
    }

    func progress(_ operation: NativeFileOperation, event: String, detail: [String: Any]) {
        var payload = detail
        payload["operationId"] = operation.id
        guard let data = try? JSONSerialization.data(withJSONObject: payload),
              let json = String(data: data, encoding: .utf8) else { return }
        DispatchQueue.main.async {
            guard self.current === operation, !operation.finished, !operation.cancelled else { return }
            self.webView?.evaluateJavaScript("window.dispatchEvent(new CustomEvent('\(event)',{detail:\(json)}))", completionHandler: nil)
        }
    }

    func present(_ controller: UIViewController, for operation: NativeFileOperation) throws {
        try operation.check()
        guard current === operation, let window = webView?.window,
              var presenter = window.rootViewController else {
            throw NativeFileError.failure("presentation_unavailable: Scene was detached")
        }
        while let next = presenter.presentedViewController, !next.isBeingDismissed { presenter = next }
        guard presenter.viewIfLoaded?.window != nil, !presenter.isBeingDismissed else {
            throw NativeFileError.failure("presentation_unavailable: controller is transitioning")
        }
        operation.controller = controller
        if let popover = controller.popoverPresentationController, let view = webView {
            guard view.bounds.width > 0, view.bounds.height > 0 else { throw NativeFileError.failure("presentation_unavailable: empty WebView bounds") }
            // A constrained source view follows rotation and split-view changes;
            // its one-point local sourceRect cannot become a stale screen point.
            let anchor = UIView(frame: CGRect(x: 0, y: 0, width: 1, height: 1))
            anchor.translatesAutoresizingMaskIntoConstraints = false
            anchor.isUserInteractionEnabled = false
            anchor.accessibilityElementsHidden = true
            view.addSubview(anchor)
            NSLayoutConstraint.activate([
                anchor.centerXAnchor.constraint(equalTo: view.safeAreaLayoutGuide.centerXAnchor),
                anchor.centerYAnchor.constraint(equalTo: view.safeAreaLayoutGuide.centerYAnchor),
                anchor.widthAnchor.constraint(equalToConstant: 1),
                anchor.heightAnchor.constraint(equalToConstant: 1)
            ])
            view.layoutIfNeeded()
            operation.anchorView = anchor
            popover.sourceView = anchor
            popover.sourceRect = anchor.bounds
            popover.permittedArrowDirections = []
        }
        presenter.present(controller, animated: true)
    }

    func cancel(_ id: String) {
        guard let operation = current, operation.id == id else { return }
        operation.cancel()
        if let cancel = operation.cancellationHandler { cancel(); return }
        // Once Files/activity is reading a file, only its real terminal callback
        // can release that file. Page cancellation does not revoke system I/O.
        if operation.systemOwnsFiles { return }
        if !operation.working { fail(operation, "cancelled: file operation") }
        else { operation.controller?.dismiss(animated: true) }
    }

    func resolve<T: Encodable>(_ operation: NativeFileOperation, _ result: T, keeping: URL? = nil) {
        resolve(operation, result, keepingFiles: keeping.map { [$0] } ?? [])
    }

    func resolve<T: Encodable>(_ operation: NativeFileOperation, _ result: T, keepingFiles: [URL]) {
        finish(operation, keeping: Set(keepingFiles)) { operation.invoke.resolve(result) }
    }

    func fail(_ operation: NativeFileOperation, _ message: String) {
        finish(operation) { operation.invoke.reject(message) }
    }

    private func finish(_ operation: NativeFileOperation, keeping: Set<URL> = [], response: @escaping () -> Void) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard !operation.finished else { return }
        operation.finished = true
        let cleanup = {
            operation.owned.filter { !keeping.contains($0) }.forEach { try? FileManager.default.removeItem(at: $0) }
            operation.owned.removeAll()
            operation.delegate = nil
            operation.anchorView?.removeFromSuperview()
            operation.anchorView = nil
            operation.cancellationHandler = nil
            operation.controller = nil
            if self.current === operation { self.current = nil }
            response()
        }
        if let controller = operation.controller, controller.isBeingDismissed,
           let transition = controller.transitionCoordinator {
            transition.animate(alongsideTransition: nil) { _ in cleanup() }
        } else if let controller = operation.controller, controller.presentingViewController != nil {
            controller.dismiss(animated: true, completion: cleanup)
        } else { cleanup() }
    }

    deinit { if let observer = sceneObserver { NotificationCenter.default.removeObserver(observer) } }
}
