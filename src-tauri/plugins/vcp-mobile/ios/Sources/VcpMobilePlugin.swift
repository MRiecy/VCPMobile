import Foundation
import Tauri
import WebKit

final class VcpMobilePlugin: Plugin {
    let notifications = NativeNotifications()
    let presentation = NativePresentation()
    let webKit = NativeWebKit()
    let media = NativeMedia()
    let haptics = NativeHaptics()
    let voice = NativeVoiceAudio()

    private let nativeEvents = NativeEventSubscription()

    override init() {
        super.init()
        notifications.onClick = { [weak self] click in
            self?.nativeEvents.sendNotificationClick(click)
        }
        notifications.install()
        voice.eventsReady = { [weak self] in self?.nativeEvents.canSendVoiceAudio ?? false }
        voice.onEvent = { [weak self] payload in self?.nativeEvents.sendVoiceAudio(payload) ?? false }
    }

    @objc override func load(webview: WKWebView) {
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            guard self.nativeEvents.attach(webview) else { return }
            self.presentation.attach(webview)
            self.webKit.attach(webview)
            self.haptics.attach(webview)
            self.voice.attach(webview)
            NativeShareInbox.configure(webView: webview)
            self.nativeEvents.refreshSubscription()
        }
    }

    @objc public func registerNativeEvents(_ invoke: Invoke) {
        nativeEvents.register(invoke)
    }

    @objc public func shareInboxLocation(_ invoke: Invoke) {
        NativeShareInbox.location(invoke)
    }

    deinit {
        voice.shutdown()
        media.shutdown()
    }
}

@_cdecl("init_plugin_vcp_mobile")
public func initPlugin() -> Plugin {
    VcpMobilePlugin()
}
