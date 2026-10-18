import Foundation
import Tauri
import UIKit
import WebKit

private struct GeometryRequest: Decodable {
    let sampleId: UInt64
    let viewportRevision: UInt64
}

private struct GeometryRect: Encodable {
    let x: Double
    let y: Double
    let width: Double
    let height: Double

    init(_ rect: CGRect) {
        x = Double(rect.minX)
        y = Double(rect.minY)
        width = Double(rect.width)
        height = Double(rect.height)
    }
}

private struct GeometrySnapshot: Encodable {
    let webViewGeneration: UInt64
    let geometrySequence: UInt64
    let sampleId: UInt64
    let viewportRevision: UInt64
    let boundsPoints: GeometryRect
    let keyboardVisible: Bool
    let dockedTopPoints: Double?
    let floatingRectPoints: GeometryRect?
}

private final class GeometryProbe: UIView {
    var onChange: (() -> Void)?

    override func layoutSubviews() {
        super.layoutSubviews()
        onChange?()
    }

    override func safeAreaInsetsDidChange() {
        super.safeAreaInsetsDidChange()
        onChange?()
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        onChange?()
    }
}

/// Owns only WebKit/UIKit resources. The WKWebView bounds never follow the CSS
/// usable surface, so keyboard avoidance cannot feed back into native sampling.
final class NativeWebKit {
    let resume = NativeWebViewResume()
    private static var nextGeneration: UInt64 = 0
    private static var clearingStores = Set<ObjectIdentifier>()
    private weak var webView: WKWebView?
    private var generation: UInt64 = 0
    private var sequence: UInt64 = 0
    private var probe: GeometryProbe?
    private var observations: [NSObjectProtocol] = []
    private var boundsObservation: NSKeyValueObservation?
    private var keyboardScreenFrame: CGRect?
    private var invalidationQueued = false

    func attach(_ webView: WKWebView) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard self.webView !== webView else { return }
        detach()
        Self.nextGeneration += 1
        generation = Self.nextGeneration
        self.webView = webView
        resume.attach(webView)
        let currentGeneration = generation
        // Wry owns the view's frame. viewport-fit=cover and no automatic UIKit
        // safe-area adjustment keep document width aligned with these bounds.
        webView.scrollView.contentInsetAdjustmentBehavior = .never
        let probe = GeometryProbe(frame: webView.bounds)
        probe.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        probe.isUserInteractionEnabled = false
        probe.backgroundColor = .clear
        probe.accessibilityElementsHidden = true
        probe.onChange = { [weak self] in self?.invalidate(currentGeneration) }
        webView.addSubview(probe)
        self.probe = probe
        webView.keyboardLayoutGuide.followsUndockedKeyboard = false
        // A zero-size marker tracks the docked guide without changing WebView
        // constraints. Its layout invalidates the probe during interactive moves.
        let marker = GeometryProbe(frame: .zero)
        marker.translatesAutoresizingMaskIntoConstraints = false
        marker.isUserInteractionEnabled = false
        marker.onChange = { [weak self] in self?.invalidate(currentGeneration) }
        probe.addSubview(marker)
        NSLayoutConstraint.activate([
            marker.leadingAnchor.constraint(equalTo: probe.leadingAnchor),
            marker.topAnchor.constraint(equalTo: webView.keyboardLayoutGuide.topAnchor),
            marker.widthAnchor.constraint(equalToConstant: 0),
            marker.heightAnchor.constraint(equalToConstant: 0)
        ])
        boundsObservation = webView.observe(\.bounds, options: [.new]) { [weak self] _, _ in
            DispatchQueue.main.async { self?.invalidate(currentGeneration) }
        }
        for name in [UIResponder.keyboardWillChangeFrameNotification,
                     UIResponder.keyboardDidChangeFrameNotification,
                     UIResponder.keyboardDidHideNotification] {
            observations.append(NotificationCenter.default.addObserver(
                forName: name, object: nil, queue: .main
            ) { [weak self] notification in
                guard let self = self, self.generation == currentGeneration else { return }
                // Keyboard notifications are process-wide; only this foreground
                // Scene may contribute geometry to its WebView.
                guard let scene = self.webView?.window?.windowScene,
                      scene.activationState == .foregroundActive
                        || scene.activationState == .foregroundInactive else { return }
                if name == UIResponder.keyboardDidHideNotification {
                    self.keyboardScreenFrame = nil
                } else {
                    self.keyboardScreenFrame = (notification.userInfo?[UIResponder.keyboardFrameEndUserInfoKey] as? NSValue)?.cgRectValue
                }
                self.invalidate(currentGeneration)
            })
        }
        for name in [UIScene.didActivateNotification, UIScene.willEnterForegroundNotification] {
            observations.append(NotificationCenter.default.addObserver(
                forName: name, object: nil, queue: .main
            ) { [weak self] notification in
                guard let self = self, let scene = notification.object as? UIScene,
                      scene === self.webView?.window?.windowScene else { return }
                self.invalidate(currentGeneration)
            })
        }
        invalidate(currentGeneration)
    }

    private func invalidate(_ expectedGeneration: UInt64) {
        guard generation == expectedGeneration else { return }
        resume.layoutChanged()
        guard !invalidationQueued else { return }
        invalidationQueued = true
        DispatchQueue.main.async { [weak self] in
            guard let self = self, self.generation == expectedGeneration else { return }
            self.invalidationQueued = false
            self.webView?.evaluateJavaScript(
                "window.dispatchEvent(new CustomEvent('vcp-webview-geometry-changed',{detail:{webViewGeneration:\(expectedGeneration)}}))",
                completionHandler: nil
            )
        }
    }

    func getGeometry(_ invoke: Invoke) {
        do {
            let request = try invoke.parseArgs(GeometryRequest.self)
            DispatchQueue.main.async { [self] in
                guard let webView = webView, let window = webView.window,
                      webView.bounds.width > 0, webView.bounds.height > 0 else {
                    invoke.reject("webview_not_attached: 当前窗口尚未完成布局")
                    return
                }
                webView.layoutIfNeeded()
                var intersection = CGRect.null
                if let screenFrame = keyboardScreenFrame {
                    let windowFrame = window.convert(screenFrame, from: window.screen.coordinateSpace)
                    intersection = webView.convert(windowFrame, from: window).intersection(webView.bounds)
                }
                let visible = !intersection.isNull && !intersection.isEmpty
                let guide = webView.keyboardLayoutGuide.layoutFrame
                // A hidden/floating guide ends at safe-area bottom. Require real
                // occlusion below that edge to classify a docked keyboard.
                let docked = visible && guide.minY < webView.bounds.maxY - webView.safeAreaInsets.bottom - 1
                    && intersection.maxY >= webView.bounds.maxY - 1
                sequence += 1
                invoke.resolve(GeometrySnapshot(
                    webViewGeneration: generation, geometrySequence: sequence,
                    sampleId: request.sampleId, viewportRevision: request.viewportRevision,
                    boundsPoints: GeometryRect(webView.bounds), keyboardVisible: visible,
                    dockedTopPoints: docked ? Double(guide.minY) : nil,
                    floatingRectPoints: visible && !docked ? GeometryRect(intersection) : nil
                ))
            }
        } catch {
            invoke.reject("invalid_geometry_args: \(error.localizedDescription)")
        }
    }

    func clearCache(_ invoke: Invoke) {
        DispatchQueue.main.async { [self] in
            guard let store = webView?.configuration.websiteDataStore else {
                invoke.reject("webview_unavailable: 当前 WebView 不存在")
                return
            }
            let identity = ObjectIdentifier(store)
            guard Self.clearingStores.insert(identity).inserted else {
                invoke.reject("cache_clear_in_progress: 此 WebView 的浏览缓存正在清理")
                return
            }
            // Capture the actual store through completion even if WebView is
            // replaced. Cookies, local databases and service-worker data remain.
            store.removeData(ofTypes: [WKWebsiteDataTypeDiskCache, WKWebsiteDataTypeMemoryCache],
                             modifiedSince: Date(timeIntervalSince1970: 0)) { [store] in
                DispatchQueue.main.async {
                    Self.clearingStores.remove(ObjectIdentifier(store))
                    invoke.resolve()
                }
            }
        }
    }

    private func detach() {
        resume.detach()
        for token in observations { NotificationCenter.default.removeObserver(token) }
        observations.removeAll()
        boundsObservation = nil
        probe?.onChange = nil
        probe?.removeFromSuperview()
        probe = nil
        keyboardScreenFrame = nil
        invalidationQueued = false
        sequence = 0
    }

    deinit {
        for token in observations { NotificationCenter.default.removeObserver(token) }
        boundsObservation?.invalidate()
        // UIKit removal is scheduled on its owning queue; no callback retains us.
        let oldProbe = probe
        DispatchQueue.main.async { oldProbe?.removeFromSuperview() }
    }
}

extension VcpMobilePlugin {
    @objc public func connectWebViewResume(_ invoke: Invoke) { webKit.resume.connect(invoke) }
    @objc public func completeWebViewResume(_ invoke: Invoke) { webKit.resume.complete(invoke) }
    @objc public func getWebViewGeometry(_ invoke: Invoke) { webKit.getGeometry(invoke) }
    @objc public func clearWebViewCache(_ invoke: Invoke) { webKit.clearCache(invoke) }
}
