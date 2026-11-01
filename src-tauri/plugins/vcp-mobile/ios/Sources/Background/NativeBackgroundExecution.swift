import Foundation
import Tauri
import UIKit

private struct BackgroundExecutionArgs: Decodable {
    let tag: String
    let screenKeepOn: Bool
}

private struct BackgroundExecutionReleaseArgs: Decodable {
    let tag: String
}

/// Applies the existing screen-on leases only while the application is active.
final class NativeBackgroundExecution {
    static let shared = NativeBackgroundExecution()

    private var screenTags = Set<String>()
    private var observations: [NSObjectProtocol] = []

    private init() {
        dispatchPrecondition(condition: .onQueue(.main))
        observe(UIApplication.didBecomeActiveNotification) { [weak self] _ in
            self?.updateScreen(active: true)
        }
        for name in [UIApplication.willResignActiveNotification, UIApplication.didEnterBackgroundNotification] {
            observe(name) { [weak self] _ in self?.updateScreen(active: false) }
        }
    }

    func acquire(tag: String, screenKeepOn: Bool) {
        dispatchPrecondition(condition: .onQueue(.main))
        if screenKeepOn { screenTags.insert(tag) }
        updateScreen()
    }

    func release(tag: String) {
        dispatchPrecondition(condition: .onQueue(.main))
        screenTags.remove(tag)
        updateScreen()
    }

    private func updateScreen(active: Bool? = nil) {
        let foreground = active ?? (UIApplication.shared.applicationState == .active)
        UIApplication.shared.isIdleTimerDisabled = foreground && !screenTags.isEmpty
    }

    private func observe(_ name: Notification.Name, handler: @escaping (Notification) -> Void) {
        observations.append(NotificationCenter.default.addObserver(
            forName: name, object: nil, queue: .main, using: handler
        ))
    }
}

extension VcpMobilePlugin {
    @objc public func acquireBackgroundExecution(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(BackgroundExecutionArgs.self)
            DispatchQueue.main.async {
                NativeBackgroundExecution.shared.acquire(tag: args.tag, screenKeepOn: args.screenKeepOn)
                invoke.resolve()
            }
        } catch { invoke.reject("invalid_background_resource_args: \(error.localizedDescription)") }
    }

    @objc public func releaseBackgroundExecution(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(BackgroundExecutionReleaseArgs.self)
            DispatchQueue.main.async {
                NativeBackgroundExecution.shared.release(tag: args.tag)
                invoke.resolve()
            }
        } catch { invoke.reject("invalid_background_resource_args: \(error.localizedDescription)") }
    }
}
