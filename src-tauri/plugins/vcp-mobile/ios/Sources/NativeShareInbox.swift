import Foundation
import Tauri
import WebKit

enum NativeShareInbox {
    private static weak var webView: WKWebView?
    private static var observing = false
    static func configure(webView: WKWebView) {
        self.webView = webView
        guard !observing else { return }
        observing = true
        CFNotificationCenterAddObserver(CFNotificationCenterGetDarwinNotifyCenter(), nil, { _, _, _, _, _ in
            DispatchQueue.main.async {
                NativeShareInbox.webView?.evaluateJavaScript("window.dispatchEvent(new Event('vcp-share-inbox-changed'))", completionHandler: nil)
            }
        }, "com.vcp.avatar.share.changed" as CFString, nil, .deliverImmediately)
    }
    static func location(_ invoke: Invoke) {
        guard let group = Bundle.main.object(forInfoDictionaryKey: "VCPShareAppGroup") as? String,
              let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group) else {
            invoke.reject("share_container_unavailable: 请检查主应用与分享扩展的 App Group 签名")
            return
        }
        invoke.resolve(["path": container.appendingPathComponent("ShareInbox", isDirectory: true).path])
    }
}
