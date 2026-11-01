import Foundation
import Tauri
import UIKit
import UserNotifications
import WebKit

struct NativeNotificationPermission: Encodable {
    let authorization: String

    init(_ status: UNAuthorizationStatus) {
        switch status {
        case .notDetermined: authorization = "notDetermined"
        case .denied: authorization = "denied"
        case .authorized: authorization = "authorized"
        case .provisional: authorization = "provisional"
        case .ephemeral: authorization = "ephemeral"
        @unknown default: authorization = "unknown"
        }
    }
}

private struct NativeNotificationClick: Encodable {
    let kind: String
    let ownerType: String?
    let ownerId: String?
    let topicId: String?
    let messageId: String?
}

private struct LocalNotificationArgs: Decodable {
    let title: String
    let body: String
    let reply: [String: String]?
}

/// Holds the latest notification tap until a ready WebView takes it.
final class NativeNotifications: NSObject, UNUserNotificationCenterDelegate {
    private static let requestPrefix = "com.vcp.avatar.local."
    private let center = UNUserNotificationCenter.current()
    private weak var webView: WKWebView?
    private var pendingClick: NativeNotificationClick?
    private var recentResponses: [String] = []
    private var permissionRequestInFlight = false
    private var installationError: String?

    /// Tauri constructs this plugin before tao calls UIApplicationMain. Installing
    /// here is deliberately synchronous; load(webview:) is too late for launch.
    func install() {
        guard Thread.isMainThread else {
            installationError = "notification_initialization_failed: notification delegate installation requires the main thread"
            NSLog("[VcpMobilePlugin] %@", installationError ?? "Notification initialization failed")
            return
        }
        center.delegate = self
        do {
            try NativeNotificationSceneBridge.shared.install(owner: self)
        } catch {
            installationError = "notification_initialization_failed: cold notification click bridge: \(error.localizedDescription)"
            NSLog("[VcpMobilePlugin] %@", installationError ?? "Notification initialization failed")
        }
    }

    func permission(_ invoke: Invoke) {
        center.getNotificationSettings { settings in
            invoke.resolve(NativeNotificationPermission(settings.authorizationStatus))
        }
    }

    func requestPermission(_ invoke: Invoke) {
        DispatchQueue.main.async { [self] in
            guard UIApplication.shared.applicationState == .active else {
                invoke.reject("notification_permission_unavailable: bring the application to the foreground to request notification permission")
                return
            }
            guard !permissionRequestInFlight else {
                invoke.reject("notification_permission_pending: a notification permission request is already in progress")
                return
            }
            permissionRequestInFlight = true
            center.requestAuthorization(options: [.alert, .sound]) { [self] _, error in
                if let error = error {
                    DispatchQueue.main.async { [self] in
                        permissionRequestInFlight = false
                        invoke.reject("notification_permission_failed: \(error.localizedDescription)")
                    }
                    return
                }
                // The completion Bool is not the complete authorization state.
                center.getNotificationSettings { [self] settings in
                    let result = NativeNotificationPermission(settings.authorizationStatus)
                    DispatchQueue.main.async { [self] in
                        permissionRequestInFlight = false
                        invoke.resolve(result)
                    }
                }
            }
        }
    }

    func openSettings(_ invoke: Invoke) {
        DispatchQueue.main.async {
            guard let url = URL(string: UIApplication.openNotificationSettingsURLString) else {
                invoke.reject("notification_settings_failed: the system settings URL is unavailable")
                return
            }
            UIApplication.shared.open(url, options: [:]) { opened in
                if opened {
                    invoke.resolve()
                } else {
                    invoke.reject("notification_settings_failed: the system could not open notification settings")
                }
            }
        }
    }

    func send(_ invoke: Invoke) {
        let args: LocalNotificationArgs
        do {
            args = try invoke.parseArgs(LocalNotificationArgs.self)
        } catch {
            invoke.reject("invalid_notification_args: \(error.localizedDescription)")
            return
        }
        DispatchQueue.main.async { [self] in
            if args.reply != nil && UIApplication.shared.applicationState != .background {
                invoke.resolve()
                return
            }
            if let error = installationError {
                invoke.reject(error)
                return
            }
            center.getNotificationSettings { [self] settings in
                switch settings.authorizationStatus {
                case .authorized, .provisional, .ephemeral:
                    break
                case .notDetermined:
                    invoke.reject("notification_permission_required: allow notifications in the application permission page first")
                    return
                case .denied:
                    invoke.reject("notification_permission_denied: notifications are disabled in system settings")
                    return
                @unknown default:
                    invoke.reject("notification_permission_unknown: the system returned an unknown notification authorization state")
                    return
                }
                let content = UNMutableNotificationContent()
                content.title = args.title
                content.body = args.body
                content.sound = .default
                if let reply = args.reply {
                    content.userInfo = ["reply": reply]
                }
                let request = UNNotificationRequest(
                    identifier: Self.requestPrefix + UUID().uuidString,
                    content: content,
                    trigger: nil
                )
                center.add(request) { error in
                    if let error = error {
                        invoke.reject("notification_send_failed: \(error.localizedDescription)")
                    } else {
                        // Accepted by the system; visibility still follows its settings.
                        invoke.resolve()
                    }
                }
            }
        }
    }

    func attach(_ webView: WKWebView) {
        self.webView = webView
        if pendingClick != nil { signalPendingClick() }
    }

    private func signalPendingClick() {
        webView?.evaluateJavaScript("window.dispatchEvent(new Event('vcp-notification-pending'))", completionHandler: nil)
    }

    func takePendingClick(_ invoke: Invoke) {
        DispatchQueue.main.async { [self] in
            if let error = installationError {
                invoke.reject(error)
                return
            }
            if let click = pendingClick {
                pendingClick = nil
                invoke.resolve(click)
            } else {
                invoke.resolve([String: String]())
            }
        }
    }

    /// Both Scene connection options and the notification-center delegate feed
    /// this same owner. The Scene bridge calls synchronously before tao setup.
    func consume(_ response: UNNotificationResponse) {
        guard Thread.isMainThread else {
            DispatchQueue.main.async { [weak self] in self?.consume(response) }
            return
        }
        let notification = response.notification
        guard response.actionIdentifier == UNNotificationDefaultActionIdentifier,
              notification.request.identifier.hasPrefix(Self.requestPrefix) else { return }
        // A rescheduled notification may reuse an identifier; its delivery date
        // distinguishes it from the duplicate Scene/delegate report of one tap.
        let identity = "\(notification.request.identifier):\(response.actionIdentifier):\(notification.date.timeIntervalSince1970.bitPattern)"
        guard !recentResponses.contains(identity) else { return }
        recentResponses.append(identity)
        if recentResponses.count > 64 { recentResponses.removeFirst() }
        let reply = notification.request.content.userInfo["reply"] as? [String: String]
        pendingClick = NativeNotificationClick(
            kind: reply == nil ? "app" : "chat",
            ownerType: reply?["ownerType"], ownerId: reply?["ownerId"],
            topicId: reply?["topicId"], messageId: reply?["messageId"]
        )
        signalPendingClick()
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        let ours = notification.request.identifier.hasPrefix(Self.requestPrefix)
        // A reply queued in the background can arrive just as the chat becomes visible.
        let isReply = notification.request.content.userInfo["reply"] != nil
        completionHandler(ours && !isReply ? [.banner, .list, .sound] : [])
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        DispatchQueue.main.async { [weak self] in
            self?.consume(response)
            completionHandler()
        }
    }

    deinit {
        if center.delegate === self { center.delegate = nil }
    }
}

extension VcpMobilePlugin {
    @objc public func getNotificationPermission(_ invoke: Invoke) {
        notifications.permission(invoke)
    }

    @objc public func requestNotificationPermission(_ invoke: Invoke) {
        notifications.requestPermission(invoke)
    }

    @objc public func openNotificationSettings(_ invoke: Invoke) {
        notifications.openSettings(invoke)
    }

    @objc public func sendLocalNotification(_ invoke: Invoke) {
        notifications.send(invoke)
    }

    @objc public func getPendingNotification(_ invoke: Invoke) {
        notifications.takePendingClick(invoke)
    }
}
