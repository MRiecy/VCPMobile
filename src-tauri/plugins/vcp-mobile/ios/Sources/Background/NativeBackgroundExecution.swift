import AVFoundation
import Foundation
import Tauri
import UIKit
import WebKit

private struct BackgroundExecutionArgs: Decodable {
    let tag: String
    let audioRequired: Bool
    let screenKeepOn: Bool
}

private struct BackgroundExecutionReleaseArgs: Decodable {
    let tag: String
}

/// Experimental, best-effort resources for the existing Rust task owners.
/// This object never owns a request, reply, retry or persisted terminal result.
/// Main-queue serialization and process lifetime survive WebView replacement.
final class NativeBackgroundExecution: NSObject, AVAudioPlayerDelegate {
    static let shared = NativeBackgroundExecution()

    private struct Lease {
        let audioRequired: Bool
        let screenKeepOn: Bool
    }

    private let session = AVAudioSession.sharedInstance()
    private weak var webKit: NativeWebKit?
    private var leases: [String: Lease] = [:]
    private var player: AVAudioPlayer?
    private var ownsAudioActivation = false
    private var interrupted = false
    private var mediaServicesUnavailable = false
    private var requiresNewForegroundTask = false
    private var observations: [NSObjectProtocol] = []
    private var failureHandler: ((String) -> Bool)?
    private var pendingFailure: String?

    private override init() {
        super.init()
        dispatchPrecondition(condition: .onQueue(.main))
        observe(AVAudioSession.interruptionNotification) { [weak self] in
            self?.handleInterruption($0)
        }
        observe(AVAudioSession.routeChangeNotification) { [weak self] in
            self?.handleRouteChange($0)
        }
        observe(AVAudioSession.mediaServicesWereLostNotification) { [weak self] _ in
            self?.mediaServicesChanged(available: false)
        }
        observe(AVAudioSession.mediaServicesWereResetNotification) { [weak self] _ in
            self?.mediaServicesChanged(available: true)
        }
        observe(UIApplication.didBecomeActiveNotification) { [weak self] _ in
            // Becoming active updates visibility only. In particular, a media
            // reset is not permission to silently restart audio on any active event.
            self?.updateScreen(active: true)
        }
        for name in [UIApplication.willResignActiveNotification, UIApplication.didEnterBackgroundNotification] {
            observe(name) { [weak self] _ in self?.updateScreen(active: false) }
        }
    }

    func setFailureHandler(_ handler: @escaping (String) -> Bool) {
        dispatchPrecondition(condition: .onQueue(.main))
        failureHandler = handler
        if let reason = pendingFailure, handler(reason) { pendingFailure = nil }
    }

    func attachWebKit(_ webKit: NativeWebKit) {
        dispatchPrecondition(condition: .onQueue(.main))
        self.webKit = webKit
    }

    func acquire(tag: String, audioRequired: Bool, screenKeepOn: Bool) throws {
        dispatchPrecondition(condition: .onQueue(.main))
        guard !tag.isEmpty else { throw failure("background_resource_invalid_tag") }
        let previous = leases.updateValue(
            Lease(audioRequired: audioRequired, screenKeepOn: screenKeepOn), forKey: tag
        )
        do {
            if audioRequired {
                try startPlayback(newForegroundTask: UIApplication.shared.applicationState == .active)
            }
            updateScreen()
        } catch {
            // A failed Stream acquisition must not discard another operation's
            // audio lease or a Sync/Prerender screen owner.
            leases[tag] = previous
            updateScreen()
            if !needsAudio {
                do { try stopPlayback(deactivate: true) }
                catch { NSLog("[VcpBackgroundExecution] Acquisition cleanup: %@", error.localizedDescription) }
            }
            throw error
        }
    }

    func release(tag: String) throws {
        dispatchPrecondition(condition: .onQueue(.main))
        guard leases.removeValue(forKey: tag) != nil else { return }
        updateScreen()
        if !needsAudio {
            // An interruption is not guaranteed to deliver its ended event.
            // Once this audio ownership ends, a future independent task may
            // attempt real activation; this never resumes the completed task.
            defer { interrupted = false }
            try stopPlayback(deactivate: true)
        }
    }

    private var needsAudio: Bool { leases.values.contains { $0.audioRequired } }

    private var ownsSessionConfiguration: Bool {
        session.category == .playback && session.mode == .default
            && session.categoryOptions.contains(.mixWithOthers)
    }

    private func updateScreen(active: Bool? = nil) {
        let foreground = active ?? (UIApplication.shared.applicationState == .active)
        UIApplication.shared.isIdleTimerDisabled = foreground && leases.values.contains { $0.screenKeepOn }
    }

    private func startPlayback(newForegroundTask: Bool = false) throws {
        dispatchPrecondition(condition: .onQueue(.main))
        guard needsAudio else { return }
        let captureState = webKit?.microphoneCaptureState ?? .none
        if captureState == .active || captureState == .muted {
            // Read the attached WebView's actual capture state. Its audio session
            // category may remain playAndRecord after the tracks have stopped.
            // Keep the Stream lease, but yield its best-effort silent audio
            // without deactivating WebKit's active or temporarily muted capture.
            try stopPlayback(deactivate: false)
            return
        }
        guard !mediaServicesUnavailable else { throw failure("background_audio_media_services_unavailable") }
        guard !interrupted else { throw failure("background_audio_interrupted") }
        if requiresNewForegroundTask && !newForegroundTask {
            throw failure("background_audio_waiting_for_foreground_task")
        }
        if player?.isPlaying == true && ownsSessionConfiguration { return }
        do {
            // Only Stream leases request audio. Sync and Prerender never borrow
            // this resource, and no recording category/microphone is requested.
            try session.setCategory(.playback, mode: .default, options: [.mixWithOthers])
            try session.setActive(true)
            ownsAudioActivation = true
            let next = try AVAudioPlayer(data: Self.silentWave, fileTypeHint: AVFileType.wav.rawValue)
            next.numberOfLoops = -1
            next.delegate = self
            player?.delegate = nil
            player?.stop()
            player = next
            guard next.prepareToPlay(), next.play() else {
                throw failure("background_audio_playback_failed")
            }
            interrupted = false
            requiresNewForegroundTask = false
        } catch {
            do { try stopPlayback(deactivate: true) }
            catch { NSLog("[VcpBackgroundExecution] Playback cleanup: %@", error.localizedDescription) }
            throw error
        }
    }

    private func stopPlayback(deactivate: Bool) throws {
        player?.delegate = nil
        player?.stop()
        player = nil
        // System interruption/media reset or another in-app category owner has
        // already taken activation away from us. Otherwise retain ownership if
        // deactivation throws, so a later real cleanup can still release it.
        guard deactivate && ownsSessionConfiguration else {
            ownsAudioActivation = false
            return
        }
        guard ownsAudioActivation else { return }
        try session.setActive(false, options: [.notifyOthersOnDeactivation])
        ownsAudioActivation = false
    }

    private func handleInterruption(_ notification: Notification) {
        guard let raw = notification.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
              let type = AVAudioSession.InterruptionType(rawValue: raw) else { return }
        switch type {
        case .began:
            interrupted = true
            ownsAudioActivation = false
            player?.pause()
            NSLog("[VcpBackgroundExecution] Audio interrupted; existing tasks retain their own lifecycle")
        case .ended:
            interrupted = false
            let options = AVAudioSession.InterruptionOptions(
                rawValue: notification.userInfo?[AVAudioSessionInterruptionOptionKey] as? UInt ?? 0
            )
            guard options.contains(.shouldResume) else {
                requiresNewForegroundTask = true
                return
            }
            guard needsAudio, !requiresNewForegroundTask, !mediaServicesUnavailable else { return }
            resumePlayback()
        @unknown default: break
        }
    }

    private func handleRouteChange(_ notification: Notification) {
        guard needsAudio else { return }
        if !ownsSessionConfiguration {
            // Another in-app audio use changed the shared session. Do not fight
            // it by continually resetting its category or deactivating its audio.
            requiresNewForegroundTask = true
            try? stopPlayback(deactivate: false)
            return
        }
        if session.currentRoute.outputs.isEmpty {
            player?.pause()
            return
        }
        guard !interrupted, !mediaServicesUnavailable, !requiresNewForegroundTask,
              player?.isPlaying != true else { return }
        resumePlayback()
    }

    private func mediaServicesChanged(available: Bool) {
        mediaServicesUnavailable = !available
        requiresNewForegroundTask = true
        interrupted = false
        try? stopPlayback(deactivate: false)
        // Apple requires recreating orphaned audio objects after a reset and
        // waiting for user action. A new foreground Stream task may try again;
        // neither the reset nor a lifecycle notification replays an LLM request.
        NSLog("[VcpBackgroundExecution] Media services %@; awaiting a new foreground Stream task",
              available ? "reset" : "unavailable")
    }

    private func resumePlayback() {
        do { try startPlayback() }
        catch {
            requiresNewForegroundTask = true
            reportFailure("background_audio_resume_failed: \(error.localizedDescription)")
        }
    }

    func audioPlayerDidFinishPlaying(_ player: AVAudioPlayer, successfully flag: Bool) {
        DispatchQueue.main.async { [weak self] in
            guard let self = self, self.player === player, self.needsAudio else { return }
            self.requiresNewForegroundTask = true
            try? self.stopPlayback(deactivate: true)
            self.reportFailure("background_audio_stopped: continuous playback ended (success=\(flag))")
        }
    }

    func audioPlayerDecodeErrorDidOccur(_ player: AVAudioPlayer, error: Error?) {
        DispatchQueue.main.async { [weak self] in
            guard let self = self, self.player === player, self.needsAudio else { return }
            self.requiresNewForegroundTask = true
            try? self.stopPlayback(deactivate: true)
            self.reportFailure("background_audio_decode_failed: \(error?.localizedDescription ?? "unknown")")
        }
    }

    private func reportFailure(_ reason: String) {
        NSLog("[VcpBackgroundExecution] %@", reason)
        pendingFailure = reason
        if failureHandler?(reason) == true { pendingFailure = nil }
    }

    private func observe(_ name: Notification.Name, handler: @escaping (Notification) -> Void) {
        observations.append(NotificationCenter.default.addObserver(
            forName: name, object: nil, queue: .main, using: handler
        ))
    }

    private func failure(_ reason: String) -> NSError {
        NSError(domain: "VcpBackgroundExecution", code: 1, userInfo: [NSLocalizedDescriptionKey: reason])
    }

    // One second of mono 16-bit PCM silence. Generated in memory, with no bundle
    // asset, filesystem staging, source microphone or unbounded audio buffers.
    private static let silentWave: Data = {
        let sampleRate: UInt32 = 8_000
        let byteCount = sampleRate * 2
        var data = Data("RIFF".utf8)
        func word(_ value: UInt32) {
            data.append(contentsOf: [UInt8(truncatingIfNeeded: value), UInt8(truncatingIfNeeded: value >> 8),
                                     UInt8(truncatingIfNeeded: value >> 16), UInt8(truncatingIfNeeded: value >> 24)])
        }
        word(36 + byteCount)
        data.append(contentsOf: "WAVEfmt ".utf8)
        word(16)
        data.append(contentsOf: [1, 0, 1, 0]) // PCM, one channel
        word(sampleRate)
        word(sampleRate * 2)
        data.append(contentsOf: [2, 0, 16, 0]) // block alignment, bits per sample
        data.append(contentsOf: "data".utf8)
        word(byteCount)
        data.append(Data(count: Int(byteCount)))
        return data
    }()
}

extension VcpMobilePlugin {
    @objc public func acquireBackgroundExecution(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(BackgroundExecutionArgs.self)
            DispatchQueue.main.async {
                do {
                    try NativeBackgroundExecution.shared.acquire(
                        tag: args.tag, audioRequired: args.audioRequired, screenKeepOn: args.screenKeepOn
                    )
                    invoke.resolve()
                } catch {
                    invoke.reject("background_resource_acquire_failed: \(error.localizedDescription)")
                }
            }
        } catch { invoke.reject("invalid_background_resource_args: \(error.localizedDescription)") }
    }

    @objc public func releaseBackgroundExecution(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(BackgroundExecutionReleaseArgs.self)
            DispatchQueue.main.async {
                do {
                    try NativeBackgroundExecution.shared.release(tag: args.tag)
                    invoke.resolve()
                } catch {
                    invoke.reject("background_resource_release_failed: \(error.localizedDescription)")
                }
            }
        } catch { invoke.reject("invalid_background_resource_args: \(error.localizedDescription)") }
    }
}
