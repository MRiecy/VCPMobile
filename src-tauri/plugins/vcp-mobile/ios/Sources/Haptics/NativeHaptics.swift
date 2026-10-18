import Foundation
import Tauri
import UIKit

private struct NativeHapticArgs: Decodable {
    let type: String
    let effect: String?
    let intensity: Double?
}

private struct NativeHapticResult: Encodable {
    let submitted: Bool
    let type: String
    let effect: String?
    var resolvedEffect: String?
    var intensity: Double?
    var reason: String?
}

/// UIKit owns hardware availability and system haptics policy. These generators
/// belong to the attached WebView; replacing it discards the old interactions.
final class NativeHaptics {
    private weak var view: UIView?
    private var impacts: [String: UIImpactFeedbackGenerator] = [:]
    private var selection: UISelectionFeedbackGenerator?
    private var notification: UINotificationFeedbackGenerator?

    func attach(_ view: UIView) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard self.view !== view else { return }
        impacts.removeAll()
        selection = nil
        notification = nil
        self.view = view
    }

    func trigger(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(NativeHapticArgs.self)
            DispatchQueue.main.async { [self] in
                guard let view = self.view else {
                    invoke.resolve(NativeHapticResult(submitted: false, type: args.type,
                        effect: args.effect, reason: "haptic_view_unavailable"))
                    return
                }
                do {
                    var result = NativeHapticResult(submitted: true, type: args.type, effect: args.effect)
                    switch args.type {
                    case "message":
                        switch args.effect {
                        case "send":
                            impact("light", style: .light, view: view).impactOccurred(intensity: 0.5)
                            result.intensity = 0.5
                        case "complete":
                            impact("light", style: .light, view: view).impactOccurred()
                        default:
                            throw invalid("unknown message haptic")
                        }
                        result.resolvedEffect = "light"
                    case "impact":
                        let effect = args.effect ?? "light"
                        let style: UIImpactFeedbackGenerator.FeedbackStyle
                        switch effect {
                        case "light": style = .light
                        case "medium": style = .medium
                        case "heavy": style = .heavy
                        case "soft": style = .soft
                        case "rigid": style = .rigid
                        default: throw invalid("unknown impact style")
                        }
                        let generator = impact(effect, style: style, view: view)
                        if let intensity = args.intensity {
                            guard intensity.isFinite && (0.0...1.0).contains(intensity) else {
                                throw invalid("intensity must be between 0 and 1")
                            }
                            generator.impactOccurred(intensity: CGFloat(intensity))
                            result.intensity = intensity
                        } else {
                            generator.impactOccurred()
                        }
                        result.resolvedEffect = effect
                    case "selection":
                        let generator = selection ?? UISelectionFeedbackGenerator(view: view)
                        selection = generator
                        generator.selectionChanged()
                        result.resolvedEffect = "selection"
                    case "notification":
                        let feedback: UINotificationFeedbackGenerator.FeedbackType
                        switch args.effect {
                        case "success": feedback = .success
                        case "warning": feedback = .warning
                        case "error": feedback = .error
                        default: throw invalid("unknown notification feedback")
                        }
                        let generator = notification ?? UINotificationFeedbackGenerator(view: view)
                        notification = generator
                        generator.notificationOccurred(feedback)
                        result.resolvedEffect = args.effect
                    default:
                        invoke.resolve(NativeHapticResult(submitted: false, type: args.type,
                            effect: args.effect, reason: "unsupported_haptic_type"))
                        return
                    }
                    // UIFeedbackGenerator reports an event, not whether the motor played.
                    // No Web/notification vibration fallback bypasses system suppression.
                    invoke.resolve(result)
                } catch {
                    invoke.reject("haptic_request: \(error.localizedDescription)")
                }
            }
        } catch {
            invoke.reject("invalid_haptic_args: \(error.localizedDescription)")
        }
    }

    private func impact(_ name: String, style: UIImpactFeedbackGenerator.FeedbackStyle,
                        view: UIView) -> UIImpactFeedbackGenerator {
        if let generator = impacts[name] { return generator }
        let generator = UIImpactFeedbackGenerator(style: style, view: view)
        impacts[name] = generator
        return generator
    }

    private func invalid(_ message: String) -> NSError {
        NSError(domain: "VCPHaptics", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
    }
}

extension VcpMobilePlugin {
    @objc public func triggerHapticFeedback(_ invoke: Invoke) {
        haptics.trigger(invoke)
    }
}
