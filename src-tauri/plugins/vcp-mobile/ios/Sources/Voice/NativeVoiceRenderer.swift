import AVFoundation
import Foundation

/// Sole hardware/session owner. A call reservation itself does not instantiate
/// an input node. The communication graph stays hardware-muted across turns;
/// output-only playback never touches input or requests record permission.
final class NativeVoiceRenderer {
    private let token = UUID()
    private(set) var engine: AVAudioEngine?
    private(set) var speechPlayer: AVAudioPlayerNode?
    private(set) var route: NativeVoiceRoute = .playback
    var isVoice: Bool { route != .playback }
    private var observations: [NSObjectProtocol] = []
    var onPlayer: ((AVAudioPlayerNode?) -> Void)?
    var onFault: ((String) -> Void)?

    func ensure(voice: Bool) throws {
        dispatchPrecondition(condition: .onQueue(.main))
        let coordinator = NativeAudioSessionCoordinator.shared
        if let engine = engine, engine.isRunning, isVoice == voice {
            guard route.matches(coordinator.session) else {
                onFault?("audio_route_changed")
                throw voiceAudioFailure("audio_route_changed")
            }
            return
        }
        destroy(releaseSession: false)
        do {
            route = try coordinator.acquireVoice(token, recording: voice)
            let engine = AVAudioEngine()
            self.engine = engine
            if voice {
                // Attach the hardware input before preparing either capture graph.
                _ = engine.inputNode
            }
            if route == .communication {
                do { try engine.inputNode.setVoiceProcessingEnabled(true) }
                catch { throw voiceAudioFailure(voiceAudioReason(error, fallback: "voice_processing_failed")) }
                guard engine.inputNode.isVoiceProcessingEnabled, engine.outputNode.isVoiceProcessingEnabled else {
                    throw voiceAudioFailure("voice_processing_unavailable")
                }
                engine.inputNode.isVoiceProcessingBypassed = false
                engine.inputNode.isVoiceProcessingInputMuted = true
                engine.inputNode.voiceProcessingOtherAudioDuckingConfiguration = .init(
                    enableAdvancedDucking: false, duckingLevel: .min)
            }
            guard let speechFormat = AVAudioFormat(standardFormatWithSampleRate: 24_000, channels: 1) else {
                throw voiceAudioFailure("voice_playback_format_unavailable")
            }
            let speech = AVAudioPlayerNode()
            speechPlayer = speech
            engine.attach(speech)
            engine.connect(speech, to: engine.mainMixerNode, format: speechFormat)
            engine.prepare()
            try engine.start()
            guard engine.isRunning else { throw voiceAudioFailure("audio_engine_not_running") }
            guard route.matches(coordinator.session) else { throw voiceAudioFailure("audio_route_changed") }
            installObservers(engine)
            onPlayer?(speech)
        } catch {
            destroy()
            throw error
        }
    }

    func setInputMuted(_ muted: Bool) {
        dispatchPrecondition(condition: .onQueue(.main))
        if route == .communication { engine?.inputNode.isVoiceProcessingInputMuted = muted }
    }

    func destroy(releaseSession: Bool = true) {
        dispatchPrecondition(condition: .onQueue(.main))
        observations.forEach { NotificationCenter.default.removeObserver($0) }
        observations.removeAll()
        onPlayer?(nil)
        speechPlayer = nil
        engine?.stop()
        engine?.reset()
        engine = nil
        route = .playback
        if releaseSession { NativeAudioSessionCoordinator.shared.releaseVoice(token) }
    }

    private func installObservers(_ engine: AVAudioEngine) {
        watch(AVAudioSession.interruptionNotification, engine: engine) { note in
            guard let raw = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
                  AVAudioSession.InterruptionType(rawValue: raw) == .began else { return nil }
            return "audio_interrupted"
        }
        watch(AVAudioSession.mediaServicesWereLostNotification, engine: engine) { _ in "audio_services_lost" }
        watch(AVAudioSession.mediaServicesWereResetNotification, engine: engine) { _ in "audio_services_reset" }
        watch(AVAudioSession.routeChangeNotification, engine: engine) { [weak self] note in
            let session = NativeAudioSessionCoordinator.shared.session
            let raw = note.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt
            if let route = self?.route, route.matches(session) {
                // Our category/input requests may deliver notifications after the graph starts.
                if case .headphones = route { return nil }
                if raw.flatMap(AVAudioSession.RouteChangeReason.init(rawValue:)) == .categoryChange { return nil }
            }
            return "audio_route_changed"
        }
        watch(.AVAudioEngineConfigurationChange, engine: engine, object: engine) { _ in "audio_configuration_changed" }
    }

    private func watch(_ name: Notification.Name, engine: AVAudioEngine, object: Any? = nil,
                       reason: @escaping (Notification) -> String?) {
        observations.append(NotificationCenter.default.addObserver(forName: name, object: object, queue: nil) {
            [weak self, weak engine] note in
            DispatchQueue.main.async {
                guard let self = self, let engine = engine, self.engine === engine,
                      let failure = reason(note) else { return }
                self.onFault?(failure)
            }
        })
    }
}
