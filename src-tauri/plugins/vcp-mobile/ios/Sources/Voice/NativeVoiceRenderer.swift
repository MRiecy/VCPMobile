import AVFoundation
import Foundation

/// Sole hardware/session owner. A call reservation itself does not instantiate
/// an input node. Once authorized, the VP graph remains fixed through mute;
/// output-only playback never touches input or requests record permission.
final class NativeVoiceRenderer {
    private let token = UUID()
    private(set) var engine: AVAudioEngine?
    private(set) var speechPlayer: AVAudioPlayerNode?
    private(set) var isVoice = false
    private var observations: [NSObjectProtocol] = []
    var onPlayer: ((AVAudioPlayerNode?) -> Void)?
    var onFault: ((String) -> Void)?

    func ensure(voice: Bool) throws {
        dispatchPrecondition(condition: .onQueue(.main))
        if let engine = engine, engine.isRunning, isVoice == voice { return }
        destroy(releaseSession: false)
        do {
            try NativeAudioSessionCoordinator.shared.acquireVoice(token, recording: voice)
            let engine = AVAudioEngine()
            self.engine = engine
            isVoice = voice
            if voice {
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
            installObservers(engine)
            onPlayer?(speech)
        } catch {
            destroy()
            throw error
        }
    }

    func mute() {
        dispatchPrecondition(condition: .onQueue(.main))
        if isVoice { engine?.inputNode.isVoiceProcessingInputMuted = true }
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
        isVoice = false
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
            if raw.flatMap(AVAudioSession.RouteChangeReason.init(rawValue:)) == .categoryChange,
               let self = self,
               (self.isVoice && session.category == .playAndRecord && session.mode == .voiceChat)
                || (!self.isVoice && session.category == .playback && session.mode == .default) { return nil }
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
