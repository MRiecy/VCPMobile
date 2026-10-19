import AVFoundation
import Foundation

/// Main-queue ownership of the app's shared audio session. The real renderer
/// (VP call or output-only speech) takes priority over silent keepalive, including
/// setup/teardown when engine.isRunning cannot identify the current owner.
final class NativeAudioSessionCoordinator {
    static let shared = NativeAudioSessionCoordinator()
    let session = AVAudioSession.sharedInstance()
    private(set) var voiceOwner: UUID?
    private var keepaliveActivated = false

    var ownsKeepaliveConfiguration: Bool {
        voiceOwner == nil && session.category == .playback && session.mode == .default
            && session.categoryOptions.contains(.mixWithOthers)
    }

    func acquireVoice(_ owner: UUID, recording: Bool = true) throws {
        dispatchPrecondition(condition: .onQueue(.main))
        guard voiceOwner == nil || voiceOwner == owner else { throw voiceAudioFailure("audio_session_busy") }
        voiceOwner = owner
        NativeBackgroundExecution.shared.yieldToVoice()
        keepaliveActivated = false
        do {
            if recording {
                try session.setCategory(.playAndRecord, mode: .voiceChat, options: [.defaultToSpeaker, .allowBluetoothHFP])
            } else {
                try session.setCategory(.playback, mode: .default, options: [])
            }
            try session.setActive(true)
        } catch {
            voiceOwner = nil
            NativeBackgroundExecution.shared.voiceDidReleaseSession()
            throw error
        }
    }

    func releaseVoice(_ owner: UUID) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard voiceOwner == owner else { return }
        // The caller has already stopped input and output before relinquishing
        // its reservation. Never deactivate a session configured by another user.
        if session.category == .playAndRecord || session.category == .playback {
            do { try session.setActive(false, options: [.notifyOthersOnDeactivation]) }
            catch { NSLog("[VcpVoice] Session deactivation: %@", error.localizedDescription) }
        }
        voiceOwner = nil
        NativeBackgroundExecution.shared.voiceDidReleaseSession()
    }

    func activateKeepalive() throws -> Bool {
        dispatchPrecondition(condition: .onQueue(.main))
        guard voiceOwner == nil else { return false }
        try session.setCategory(.playback, mode: .default, options: [.mixWithOthers])
        try session.setActive(true)
        keepaliveActivated = true
        return true
    }

    func releaseKeepalive(deactivate: Bool) throws {
        dispatchPrecondition(condition: .onQueue(.main))
        guard deactivate, keepaliveActivated, ownsKeepaliveConfiguration else {
            keepaliveActivated = false
            return
        }
        try session.setActive(false, options: [.notifyOthersOnDeactivation])
        keepaliveActivated = false
    }
}

func voiceAudioFailure(_ reason: String) -> NSError {
    NSError(domain: "VcpVoiceAudio", code: 1, userInfo: [NSLocalizedDescriptionKey: reason])
}

func voiceAudioReason(_ error: Error, fallback: String) -> String {
    let native = error as NSError
    if native.domain == "VcpVoiceAudio" { return native.localizedDescription }
    NSLog("[VcpVoice] %@: %@", fallback, native.localizedDescription)
    return fallback
}
