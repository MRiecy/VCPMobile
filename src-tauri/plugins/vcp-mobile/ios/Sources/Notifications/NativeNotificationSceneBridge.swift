import Darwin
import Foundation
import ObjectiveC
import UIKit
import UserNotifications

private struct NotificationSceneBridgeError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

/// A single process-lifetime hook for the locked tao 0.37.1 Scene entry. It
/// preserves tao's delegate and original IMP, including all window creation.
final class NativeNotificationSceneBridge {
    static let shared = NativeNotificationSceneBridge()
    private weak var owner: NativeNotifications?
    private var installed = false

    private typealias SceneConnection = @convention(c) (
        AnyObject, Selector, UIScene, UISceneSession, UIScene.ConnectionOptions
    ) -> Void

    func install(owner: NativeNotifications) throws {
        guard Thread.isMainThread else {
            throw NotificationSceneBridgeError(message: "Scene hook installation is not on the main thread")
        }
        self.owner = owner
        if installed { return }
        let selector = NSSelectorFromString("scene:willConnectToSession:options:")
        guard let sceneDelegate = NSClassFromString("TaoSceneDelegate"),
              let method = class_getInstanceMethod(sceneDelegate, selector) else {
            throw NotificationSceneBridgeError(message: "TaoSceneDelegate Scene connection method was not registered before plugin initialization")
        }
        guard method_getNumberOfArguments(method) == 5,
              Self.returnType(method) == "v",
              Self.argumentType(method, 0)?.hasPrefix("@") == true,
              Self.argumentType(method, 1) == ":",
              (2...4).allSatisfy({ Self.argumentType(method, UInt32($0))?.hasPrefix("@") == true }) else {
            throw NotificationSceneBridgeError(message: "TaoSceneDelegate Scene connection ABI differs from the audited signature")
        }
        let original = unsafeBitCast(method_getImplementation(method), to: SceneConnection.self)
        // imp_implementationWithBlock supplies self and the explicit method
        // arguments; _cmd belongs only to the original C IMP signature.
        let replacement: @convention(block) (
            AnyObject, UIScene, UISceneSession, UIScene.ConnectionOptions
        ) -> Void = { [weak self] delegate, scene, session, options in
            if let response = options.notificationResponse {
                self?.owner?.consume(response)
            }
            original(delegate, selector, scene, session, options)
        }
        let implementation = imp_implementationWithBlock(replacement as Any)
        method_setImplementation(method, implementation)
        // The Objective-C method keeps the copied block for the process lifetime.
        // Do not remove it when a WebView or plugin instance is released.
        installed = true
    }

    private static func returnType(_ method: Method) -> String {
        let encoding = method_copyReturnType(method)
        defer { free(encoding) }
        return String(cString: encoding)
    }

    private static func argumentType(_ method: Method, _ index: UInt32) -> String? {
        guard let encoding = method_copyArgumentType(method, index) else { return nil }
        defer { free(encoding) }
        return String(cString: encoding)
    }
}
