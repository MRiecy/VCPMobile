import Foundation
import Tauri
import UIKit
import WebKit

private struct ResumeConnectArgs: Decodable { let documentId: String }
private struct ResumeCompleteArgs: Decodable { let documentId: String; let token: String }
private struct ResumeConnection: Encodable {
    let token: String?
    private enum CodingKeys: String, CodingKey { case token }
    func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        try values.encode(token, forKey: .token)
    }
}

/// A visual handoff only: one already-rendered UIKit snapshot, one sibling cover,
/// and one small WebKit presentation barrier. The actual WKWebView stays visible.
final class NativeWebViewResume {
    private weak var webView: WKWebView?
    private weak var scene: UIWindowScene?
    private var attachment = UUID()
    private var document = UUID()
    private var observations: [NSObjectProtocol] = []
    private var loadingObservation: NSKeyValueObservation?
    private var snapshot: UIView?
    private var cover: ResumeCover?
    private var backgroundSeen = false
    private var active = false
    private var size = CGSize.zero
    private var token: String?
    private var readyDocumentId: String?
    private var authorization = UUID()
    private var barrierInFlight: UUID?
    private var deadline: DispatchWorkItem?

    func attach(_ webView: WKWebView) {
        dispatchPrecondition(condition: .onQueue(.main))
        detach()
        self.webView = webView
        layoutChanged()
        let owner = attachment
        for name in [UIScene.willDeactivateNotification, UIScene.didEnterBackgroundNotification,
                     UIScene.willEnterForegroundNotification, UIScene.didActivateNotification,
                     UIScene.didDisconnectNotification] {
            observations.append(NotificationCenter.default.addObserver(
                forName: name, object: nil, queue: .main
            ) { [weak self] notification in
                guard let self = self, self.attachment == owner,
                      let scene = notification.object as? UIWindowScene,
                      scene === self.scene else { return }
                self.transition(name)
            })
        }
        observations.append(NotificationCenter.default.addObserver(
            forName: UIApplication.didReceiveMemoryWarningNotification, object: nil, queue: .main
        ) { [weak self] _ in
            guard let self = self, self.attachment == owner else { return }
            self.releaseSnapshotForMemoryPressure()
        })
        loadingObservation = webView.observe(\.isLoading, options: [.new]) { [weak self] _, change in
            let loading = change.newValue == true
            DispatchQueue.main.async {
                guard let self = self, self.attachment == owner else { return }
                if !loading {
                    self.beginBarrierIfReady()
                    return
                }
                // Loading invalidates old document work; it is never evidence
                // that a frame is ready. Tauri keeps its navigation delegate.
                self.document = UUID()
                self.barrierInFlight = nil
                self.readyDocumentId = nil
                self.authorization = UUID()
                if self.token != nil { self.restartHandshake() }
            }
        }
    }

    func detach() {
        dispatchPrecondition(condition: .onQueue(.main))
        attachment = UUID()
        document = UUID()
        for observation in observations { NotificationCenter.default.removeObserver(observation) }
        observations.removeAll()
        loadingObservation?.invalidate()
        loadingObservation = nil
        clearRecovery()
        barrierInFlight = nil
        backgroundSeen = false
        active = false
        scene = nil
        webView = nil
        size = .zero
    }

    func layoutChanged() {
        dispatchPrecondition(condition: .onQueue(.main))
        guard let webView = webView, let currentScene = webView.window?.windowScene else {
            clearRecovery()
            scene = nil
            return
        }
        if scene !== currentScene {
            clearRecovery()
            scene = currentScene
            active = currentScene.activationState == .foregroundActive
            backgroundSeen = currentScene.activationState == .background
        }
        let changed = size != webView.bounds.size
        size = webView.bounds.size
        positionCover()
        if changed && token != nil { restartHandshake() }
    }

    private func transition(_ name: Notification.Name) {
        switch name {
        case UIScene.willDeactivateNotification:
            active = false
            deadline?.cancel()
            deadline = nil
            authorization = UUID()
            if token == nil, let webView = webView, webView.window != nil,
               !webView.isHidden, webView.alpha > 0, !webView.bounds.isEmpty {
                // Capture the committed surface before WebKit detaches its root
                // layer. No expensive rasterization or second bitmap is retained.
                snapshot = webView.snapshotView(afterScreenUpdates: false)
            }
        case UIScene.didEnterBackgroundNotification:
            backgroundSeen = true
            active = false
            deadline?.cancel()
            deadline = nil
            token = nil
            readyDocumentId = nil
            authorization = UUID()
            barrierInFlight = nil
            snapshot?.removeFromSuperview()
            cover?.removeFromSuperview()
            cover = nil
        case UIScene.willEnterForegroundNotification:
            guard backgroundSeen else { return }
            backgroundSeen = false
            // No captured surface means there is nothing to hand over. Preserve
            // the framework's existing display instead of adding a blank blocker.
            guard snapshot != nil || cover != nil else { return }
            restartHandshake()
        case UIScene.didActivateNotification:
            active = true
            if backgroundSeen {
                backgroundSeen = false
                if snapshot != nil || cover != nil { restartHandshake() }
            }
            if token != nil {
                emitRequest()
                scheduleDeadline()
                beginBarrierIfReady()
            } else {
                // Control Center/permission prompts were only inactive: no cover.
                snapshot = nil
            }
        case UIScene.didDisconnectNotification:
            clearRecovery()
            backgroundSeen = false
            active = false
            scene = nil
            document = UUID()
        default: break
        }
    }

    private func restartHandshake() {
        token = UUID().uuidString
        readyDocumentId = nil
        authorization = UUID()
        // A resized snapshot may temporarily cover the new bounds. It never
        // authorizes reveal; the new geometry requires a new frontend handshake.
        if snapshot != nil || cover != nil { installCover() }
        emitRequest()
        scheduleDeadline()
    }

    private func installCover() {
        guard let webView = webView, let host = webView.superview else { return }
        if cover == nil {
            let next = ResumeCover(snapshot: snapshot)
            next.retry = { [weak self] in self?.restartHandshake() }
            next.reload = { [weak self] in
                guard let self = self else { return }
                self.document = UUID()
                self.restartHandshake()
                self.webView?.reload()
            }
            cover = next
        }
        guard let cover = cover else { return }
        if cover.superview !== host {
            cover.removeFromSuperview()
            host.insertSubview(cover, aboveSubview: webView)
        }
        positionCover()
    }

    private func positionCover() {
        guard let webView = webView, let host = webView.superview, let cover = cover,
              cover.superview === host else { return }
        let frame = webView.convert(webView.bounds, to: host)
        guard cover.frame != frame else { return }
        cover.frame = frame
        cover.setNeedsLayout()
    }

    private func scheduleDeadline() {
        deadline?.cancel()
        deadline = nil
        guard active, let expected = token else { return }
        let owner = attachment
        let work = DispatchWorkItem { [weak self] in
            guard let self = self, self.attachment == owner, self.token == expected, self.active else { return }
            self.showRecoveryControls()
        }
        deadline = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 5, execute: work)
    }

    private func showRecoveryControls() {
        guard token != nil, active else { return }
        installCover()
        cover?.showControls()
    }

    private func releaseSnapshotForMemoryPressure() {
        // Public WebKit snapshots cannot be cancelled. Retire this callback's
        // authority so a new context never waits for a suspended/lost operation.
        // The old closure holds neither the cover nor the full-screen snapshot.
        barrierInFlight = nil
        snapshot?.removeFromSuperview()
        snapshot = nil
        cover?.releaseSnapshot()
        if active && token != nil {
            // Release the large surface, but retain explicit recovery controls.
            // The WKWebView/theme background itself is never altered.
            restartHandshake()
            showRecoveryControls()
        } else {
            cover?.removeFromSuperview()
            cover = nil
        }
    }

    private func emitRequest() {
        guard let token = token, let json = json(["token": token]) else { return }
        webView?.evaluateJavaScript(
            "window.dispatchEvent(new CustomEvent('vcp-webview-resume',{detail:\(json)}))",
            completionHandler: nil
        )
    }

    func connect(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(ResumeConnectArgs.self)
            DispatchQueue.main.async { [self] in
                let expected = token
                invoke.resolve(ResumeConnection(token: expected))
                verifyMarker(documentId: args.documentId, token: expected, requireReady: false) { [weak self] valid in
                    guard let self = self, valid, self.token == expected else { return }
                    self.emitRequest()
                }
            }
        } catch { invoke.reject("invalid_webview_resume_args: \(error.localizedDescription)") }
    }

    func complete(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(ResumeCompleteArgs.self)
            DispatchQueue.main.async { [self] in
                invoke.resolve()
                guard token == args.token else { return }
                verifyMarker(documentId: args.documentId, token: args.token, requireReady: true) { [weak self] valid in
                    guard let self = self, valid, self.token == args.token else { return }
                    self.readyDocumentId = args.documentId
                    self.authorization = UUID()
                    // Keep a validated ACK received while inactive. didActivate
                    // starts the barrier; Vue need not repeat its completed token.
                    self.beginBarrierIfReady()
                }
            }
        } catch { invoke.reject("invalid_webview_resume_args: \(error.localizedDescription)") }
    }

    private func verifyMarker(documentId: String, token: String?, requireReady: Bool,
                              completion: @escaping (Bool) -> Void) {
        guard let webView = webView,
              let expected = json(["documentId": documentId, "token": token as Any? ?? NSNull(), "requireReady": requireReady]) else { return }
        let owner = attachment
        let page = document
        let script = """
        (function(e){const m=window.__vcpWebViewResume;return !!m&&m.documentId===e.documentId&&
        (e.requireReady?m.readyToken===e.token:(m.readyToken===null||m.readyToken===e.token));})(\(expected))
        """
        webView.evaluateJavaScript(script) { [weak self] value, error in
            DispatchQueue.main.async {
                guard let self = self, self.attachment == owner, self.document == page else { return }
                completion(error == nil && (value as? Bool) == true)
            }
        }
    }

    private func beginBarrierIfReady() {
        guard active, scene?.activationState == .foregroundActive,
              barrierInFlight == nil, let expected = token, let documentId = readyDocumentId,
              let webView = webView, webView.window?.windowScene === scene,
              !webView.isLoading, !webView.bounds.isEmpty else { return }
        let owner = attachment
        let page = document
        let accepted = authorization
        let geometry = webView.bounds.size
        let barrier = UUID()
        barrierInFlight = barrier
        let configuration = WKSnapshotConfiguration()
        configuration.rect = CGRect(x: webView.bounds.minX, y: webView.bounds.minY,
                                    width: min(1, webView.bounds.width), height: min(1, webView.bounds.height))
        configuration.snapshotWidth = NSNumber(value: 1)
        configuration.afterScreenUpdates = true
        webView.takeSnapshot(with: configuration) { [weak self] image, error in
            DispatchQueue.main.async {
                guard let self = self, self.attachment == owner, self.barrierInFlight == barrier else { return }
                self.barrierInFlight = nil
                guard self.token == expected, self.document == page, self.authorization == accepted,
                      self.webView?.bounds.size == geometry, self.active else {
                    self.beginBarrierIfReady()
                    return
                }
                guard error == nil, let image = image, image.size.width > 0, image.size.height > 0 else {
                    self.showRecoveryControls()
                    return
                }
                // The tiny image is discarded. Marker verification after the
                // native presentation barrier protects theme/document invalidation.
                self.verifyMarker(documentId: documentId, token: expected, requireReady: true) { [weak self] valid in
                    guard let self = self, valid, self.token == expected, self.document == page,
                          self.authorization == accepted, self.active,
                          self.scene?.activationState == .foregroundActive,
                          self.webView?.window?.windowScene === self.scene,
                          self.webView?.isLoading == false,
                          self.webView?.bounds.size == geometry else { return }
                    self.clearRecovery()
                    // Retire only this successfully presented token in Vue.
                    // Later geometry changes must not re-ack a completed handoff.
                    if let detail = self.json(["token": expected, "finished": true]) {
                        self.webView?.evaluateJavaScript(
                            "window.dispatchEvent(new CustomEvent('vcp-webview-resume',{detail:\(detail)}))",
                            completionHandler: nil
                        )
                    }
                }
            }
        }
    }

    private func clearRecovery() {
        deadline?.cancel()
        deadline = nil
        token = nil
        readyDocumentId = nil
        authorization = UUID()
        barrierInFlight = nil
        snapshot?.removeFromSuperview()
        snapshot = nil
        cover?.removeFromSuperview()
        cover = nil
    }

    private func json(_ value: [String: Any]) -> String? {
        guard let data = try? JSONSerialization.data(withJSONObject: value) else { return nil }
        return String(data: data, encoding: .utf8)
    }

    deinit {
        for observation in observations { NotificationCenter.default.removeObserver(observation) }
        loadingObservation?.invalidate()
        deadline?.cancel()
        let oldCover = cover
        let oldSnapshot = snapshot
        DispatchQueue.main.async {
            oldSnapshot?.removeFromSuperview()
            oldCover?.removeFromSuperview()
        }
    }
}
