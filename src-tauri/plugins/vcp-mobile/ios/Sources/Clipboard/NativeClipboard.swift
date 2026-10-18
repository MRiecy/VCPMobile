import Foundation
import Tauri
import UIKit

private struct WriteClipboardArgs: Decodable {
    let content: String
}

private struct ClipboardContent: Encodable {
    let content: String
}

extension VcpMobilePlugin {
    @objc public func writeClipboard(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(WriteClipboardArgs.self)
            DispatchQueue.main.async {
                guard UIApplication.shared.applicationState == .active else {
                    invoke.reject("clipboard_unavailable: bring the application to the foreground to write the clipboard")
                    return
                }
                UIPasteboard.general.string = args.content
                invoke.resolve()
            }
        } catch {
            invoke.reject("invalid_clipboard_args: \(error.localizedDescription)")
        }
    }

    @objc public func readClipboard(_ invoke: Invoke) {
        DispatchQueue.main.async {
            guard UIApplication.shared.applicationState == .active else {
                invoke.reject("clipboard_unavailable: bring the application to the foreground to read the clipboard")
                return
            }
            let pasteboard = UIPasteboard.general
            guard pasteboard.hasStrings else {
                invoke.resolve(ClipboardContent(content: ""))
                return
            }
            guard let content = pasteboard.string else {
                invoke.reject("clipboard_unavailable: the system did not provide the clipboard text")
                return
            }
            invoke.resolve(ClipboardContent(content: content))
        }
    }
}
