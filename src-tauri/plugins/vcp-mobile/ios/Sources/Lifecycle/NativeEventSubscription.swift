import Foundation
import Network
import Tauri
import UIKit
import WebKit

private struct RegisterNativeEventsArgs: Decodable {
    let channel: Channel
}

private struct NativeProtocolAcknowledgement: Encodable {
    let protocolVersion = 1
}

private enum LifecyclePhase: String, Encodable {
    case active, inactive, background
}

private struct LifecyclePayload: Encodable {
    let phase: LifecyclePhase
    let nativeSequence: UInt64
    let snapshot: Bool
}

private struct NetworkPayload: Encodable {
    // Path availability is a recovery hint, not proof a server is reachable.
    let connected: Bool
}

private struct NativeEvent<Payload: Encodable>: Encodable {
    let event: String
    let payload: Payload
}

/// Owns one process Channel subscription, its Scene/App observations and path monitor.
/// WebView replacement changes observer generation without resetting native lifecycle ordering.
final class NativeEventSubscription {
    // UIKit state, these process facts, and subscription ownership are main-queue only.
    // A Channel/WebView/plugin replacement must not reset Rust's sequence gate.
    private static var observedPhase: LifecyclePhase?
    private static var nativeSequence: UInt64 = 0

    private weak var webView: WKWebView?
    private weak var observedScene: UIScene?
    private var channel: Channel?
    private var observers: [NSObjectProtocol] = []
    private var monitor: NWPathMonitor?
    private let networkQueue = DispatchQueue(label: "com.vcp.avatar.native-network")
    private var generation = UUID()
    private var lastDeliveredPhase: LifecyclePhase?
    private var lastNetworkConnected: Bool?

    /// Record attachment before other view owners attach; refresh the existing
    /// subscription afterwards so startup and replacement ordering stays explicit.
    func attach(_ webView: WKWebView) -> Bool {
        dispatchPrecondition(condition: .onQueue(.main))
        guard self.webView !== webView else { return false }
        self.webView = webView
        return true
    }

    func refreshSubscription() {
        dispatchPrecondition(condition: .onQueue(.main))
        guard let channel = channel else { return }
        do {
            try replaceSubscription(channel)
        } catch {
            failSubscription(error)
        }
    }

    func register(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(RegisterNativeEventsArgs.self)
            DispatchQueue.main.async { [self] in
                do {
                    try replaceSubscription(args.channel)
                    invoke.resolve(NativeProtocolAcknowledgement())
                } catch {
                    failSubscription(error)
                    invoke.reject("native_events: \(error.localizedDescription)")
                }
            }
        } catch {
            invoke.reject("invalid_native_events_args: \(error.localizedDescription)")
        }
    }

    private func replaceSubscription(_ newChannel: Channel) throws {
        dispatchPrecondition(condition: .onQueue(.main))
        invalidateSubscription()
        channel = newChannel
        let subscriptionGeneration = generation
        installLifecycleObservers(generation: subscriptionGeneration)
        // Observers and this snapshot are installed in one main-queue turn.
        // There is no fabricated foreground state before UIKit is consulted.
        if let phase = currentPhase() {
            try sendLifecycle(phase, snapshot: true)
        }

        let pathMonitor = NWPathMonitor()
        pathMonitor.pathUpdateHandler = { [weak self] path in
            let connected = path.status == .satisfied
            DispatchQueue.main.async { [weak self] in
                guard let self = self, self.generation == subscriptionGeneration,
                      self.channel != nil else { return }
                guard self.lastNetworkConnected != connected else { return }
                do {
                    try self.channel?.send(NativeEvent(
                        event: "network", payload: NetworkPayload(connected: connected)
                    ))
                    self.lastNetworkConnected = connected
                } catch {
                    self.failSubscription(error)
                }
            }
        }
        monitor = pathMonitor
        // Never read currentPath before the monitor's first real callback.
        pathMonitor.start(queue: networkQueue)
    }

    private func installLifecycleObservers(generation subscriptionGeneration: UUID) {
        let applicationTransitions: [(Notification.Name, LifecyclePhase)] = [
            (UIApplication.didBecomeActiveNotification, .active),
            (UIApplication.willResignActiveNotification, .inactive),
            (UIApplication.willEnterForegroundNotification, .inactive),
            (UIApplication.didEnterBackgroundNotification, .background)
        ]
        for (name, phase) in applicationTransitions {
            observe(name, generation: subscriptionGeneration) { [weak self] _ in
                guard let self = self, self.currentScene() == nil else { return }
                self.deliverTransition(phase)
            }
        }
        let sceneTransitions: [(Notification.Name, LifecyclePhase)] = [
            (UIScene.didActivateNotification, .active),
            (UIScene.willDeactivateNotification, .inactive),
            (UIScene.willEnterForegroundNotification, .inactive),
            (UIScene.didEnterBackgroundNotification, .background)
        ]
        for (name, phase) in sceneTransitions {
            observe(name, generation: subscriptionGeneration) { [weak self] notification in
                guard let self = self, let scene = notification.object as? UIScene,
                      self.currentScene() === scene else { return }
                // The "will" notifications precede activationState's update.
                // Their named transition is authoritative for inactive.
                self.deliverTransition(phase)
            }
        }
        observe(UIApplication.willTerminateNotification, generation: subscriptionGeneration) {
            [weak self] _ in self?.invalidateSubscription()
        }
    }

    private func observe(
        _ name: Notification.Name,
        generation subscriptionGeneration: UUID,
        handler: @escaping (Notification) -> Void
    ) {
        let token = NotificationCenter.default.addObserver(
            forName: name, object: nil, queue: .main
        ) { [weak self] notification in
            guard let self = self, self.generation == subscriptionGeneration,
                  self.channel != nil else { return }
            handler(notification)
        }
        observers.append(token)
    }

    private func currentScene() -> UIScene? {
        dispatchPrecondition(condition: .onQueue(.main))
        if let scene = webView?.window?.windowScene {
            observedScene = scene
            return scene
        }
        // The application owns a single UIWindowScene. During plugin attachment
        // the WebView may not have a window yet, so use the unique application scene.
        let scenes = UIApplication.shared.connectedScenes.filter {
            $0.session.role == .windowApplication
        }
        if scenes.count == 1, let scene = scenes.first {
            observedScene = scene
            return scene
        }
        if let scene = observedScene, scenes.contains(scene) {
            return scene
        }
        return nil
    }

    private func currentPhase() -> LifecyclePhase? {
        if let scene = currentScene() {
            switch scene.activationState {
            case .foregroundActive: return .active
            case .foregroundInactive: return .inactive
            case .background: return .background
            // An unattached scene has no foreground/background fact yet.
            // Consult the real application state instead of inventing background.
            case .unattached: break
            @unknown default: return nil
            }
        }
        switch UIApplication.shared.applicationState {
        case .active: return .active
        case .inactive: return .inactive
        case .background: return .background
        @unknown default: return nil
        }
    }

    private func deliverTransition(_ phase: LifecyclePhase) {
        do {
            try sendLifecycle(phase, snapshot: false)
        } catch {
            failSubscription(error)
        }
    }

    private func sendLifecycle(_ phase: LifecyclePhase, snapshot: Bool) throws {
        guard let channel = channel else { return }
        if Self.observedPhase != phase {
            if Self.observedPhase != nil {
                Self.nativeSequence += 1
            }
            Self.observedPhase = phase
        } else if !snapshot && lastDeliveredPhase == phase {
            return
        }
        try channel.send(NativeEvent(
            event: "lifecycle",
            payload: LifecyclePayload(
                phase: phase, nativeSequence: Self.nativeSequence, snapshot: snapshot
            )
        ))
        lastDeliveredPhase = phase
    }

    private func failSubscription(_ error: Error) {
        invalidateSubscription()
        NSLog("[VcpMobilePlugin] Native event source failed: %@", error.localizedDescription)
    }

    func sendNotificationClick(_ click: NativeNotificationClick) {
        do {
            try channel?.send(NativeEvent(event: "notificationClick", payload: click))
        } catch {
            // The notification owner retains the pending intent for the getter.
            NSLog("[VcpMobilePlugin] Notification click delivery failed: %@", error.localizedDescription)
        }
    }

    var canSendVoiceAudio: Bool {
        dispatchPrecondition(condition: .onQueue(.main))
        return channel != nil
    }

    func sendVoiceAudio(_ payload: NativeVoiceAudioEvent) -> Bool {
        dispatchPrecondition(condition: .onQueue(.main))
        guard let channel = channel else { return false }
        do {
            try channel.send(NativeEvent(event: "voiceAudio", payload: payload))
            return true
        } catch {
            NSLog("[VcpVoice] Native event delivery failed: %@", error.localizedDescription)
            return false
        }
    }

    func sendVoicePlayback(_ payload: NativeVoicePlaybackEvent) -> Bool {
        dispatchPrecondition(condition: .onQueue(.main))
        guard let channel = channel else { return false }
        do {
            try channel.send(NativeEvent(event: "voicePlayback", payload: payload))
            return true
        } catch {
            NSLog("[VcpVoice] Playback event delivery failed: %@", error.localizedDescription)
            return false
        }
    }

    func sendVoiceDocument(_ payload: NativeVoiceDocumentEvent) -> Bool {
        dispatchPrecondition(condition: .onQueue(.main))
        guard let channel = channel else { return false }
        do {
            try channel.send(NativeEvent(event: "voiceDocument", payload: payload))
            return true
        } catch {
            NSLog("[VcpVoice] Document invalidation delivery failed: %@", error.localizedDescription)
            return false
        }
    }

    private func invalidateSubscription() {
        dispatchPrecondition(condition: .onQueue(.main))
        generation = UUID()
        for token in observers {
            NotificationCenter.default.removeObserver(token)
        }
        observers.removeAll()
        monitor?.cancel()
        monitor = nil
        channel = nil
        observedScene = nil
        lastDeliveredPhase = nil
        lastNetworkConnected = nil
    }

    deinit {
        // Neither operation reads UIKit. Deliveries retain only weak ownership.
        for token in observers {
            NotificationCenter.default.removeObserver(token)
        }
        monitor?.cancel()
    }
}
