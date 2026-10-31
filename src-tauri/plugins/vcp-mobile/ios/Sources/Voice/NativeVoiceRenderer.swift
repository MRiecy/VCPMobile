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
    private(set) var inputDeviceId: String?
    var isVoice: Bool { route != .playback }
    private var routeTimer: DispatchSourceTimer?
    var awaitingInputRoute: Bool { routeTimer != nil }
    var acceptsPlayback: Bool { awaitingInputRoute || engine?.isRunning == true }
    private var observations: [NSObjectProtocol] = []
    var onPlayer: ((AVAudioPlayerNode?) -> Void)?
    var onFault: ((String) -> Void)?

    @discardableResult
    func ensure(voice: Bool, inputDeviceId: String? = nil,
                onRouteReady: ((Error?) -> Void)? = nil) throws -> Bool {
        dispatchPrecondition(condition: .onQueue(.main))
        let coordinator = NativeAudioSessionCoordinator.shared
        if let engine = engine, engine.isRunning, isVoice == voice, self.inputDeviceId == inputDeviceId {
            guard route.matches(coordinator.session) else {
                onFault?("audio_route_changed")
                throw voiceAudioFailure("audio_route_changed")
            }
            return true
        }
        let returningMediaUID = coordinator.mediaUIDLeavingHFP()
        destroy(releaseSession: false)
        do {
            route = try coordinator.acquireVoice(token, recording: voice, inputDeviceId: inputDeviceId,
                                                 returningMediaUID: returningMediaUID)
            self.inputDeviceId = inputDeviceId
            let engine = AVAudioEngine()
            self.engine = engine
            if voice {
                // Attach the hardware input before preparing either capture graph.
                _ = engine.inputNode
            }
            if route.usesVoiceProcessing {
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
            engine.attach(speech)
            engine.connect(speech, to: engine.mainMixerNode, format: speechFormat)
            engine.prepare()
            var canWaitForRoute = inputDeviceId != nil
            if returningMediaUID != nil, case .headphones = route { canWaitForRoute = true }
            do { try engine.start() }
            catch {
                guard canWaitForRoute, !route.matches(coordinator.session), onRouteReady != nil else { throw error }
            }
            if canWaitForRoute, !route.matches(coordinator.session), let onRouteReady {
                waitForInputRoute(engine, speech: speech, completion: onRouteReady)
                return false
            }
            guard engine.isRunning else { throw voiceAudioFailure("audio_engine_not_running") }
            guard route.matches(coordinator.session) else {
                throw voiceAudioFailure(inputDeviceId == nil ? "audio_route_changed" : "voice_input_unavailable")
            }
            publishGraph(engine, speech)
            return true
        } catch {
            destroy()
            throw error
        }
    }

    private func publishGraph(_ engine: AVAudioEngine, _ speech: AVAudioPlayerNode) {
        installObservers(engine)
        speechPlayer = speech
        onPlayer?(speech)
    }

    // No player is bound and no tap is installed until the actual pair is
    // confirmed. Bluetooth profile changes can settle after setActive returns.
    private func waitForInputRoute(_ engine: AVAudioEngine, speech: AVAudioPlayerNode,
                                   completion: @escaping (Error?) -> Void) {
        let timer = DispatchSource.makeTimerSource(queue: .main)
        let deadline = DispatchTime.now() + .seconds(3)
        routeTimer = timer
        timer.schedule(deadline: .now(), repeating: .milliseconds(100))
        timer.setEventHandler { [weak self, weak engine] in
            guard let self, let engine, self.engine === engine, self.awaitingInputRoute else { return }
            let coordinator = NativeAudioSessionCoordinator.shared
            let session = coordinator.session
            do {
                if self.route == .discoveringBluetooth { self.route = try coordinator.resolveBluetoothInput() }
                if case .headphones(_, let uid) = self.route { try coordinator.preferMediaInput(uid) }
                if self.route.matches(session) {
                    if !engine.isRunning { engine.prepare(); try engine.start() }
                    if engine.isRunning && self.route.matches(session) {
                        self.routeTimer?.cancel()
                        self.routeTimer = nil
                        self.publishGraph(engine, speech)
                        completion(nil)
                        return
                    }
                }
                if DispatchTime.now() >= deadline {
                    throw voiceAudioFailure(self.inputDeviceId == nil ? "audio_route_changed" : "voice_input_unavailable")
                }
            } catch {
                self.destroy()
                completion(error)
            }
        }
        timer.resume()
    }

    func setInputMuted(_ muted: Bool) {
        dispatchPrecondition(condition: .onQueue(.main))
        if route.usesVoiceProcessing { engine?.inputNode.isVoiceProcessingInputMuted = muted }
    }

    func destroy(releaseSession: Bool = true) {
        dispatchPrecondition(condition: .onQueue(.main))
        routeTimer?.cancel()
        routeTimer = nil
        observations.forEach { NotificationCenter.default.removeObserver($0) }
        observations.removeAll()
        onPlayer?(nil)
        speechPlayer = nil
        engine?.stop()
        engine?.reset()
        engine = nil
        route = .playback
        inputDeviceId = nil
        if releaseSession { NativeAudioSessionCoordinator.shared.releaseVoice(token) }
    }

    private func installObservers(_ engine: AVAudioEngine) {
        let originalOutput = NativeAudioSessionCoordinator.shared.session.currentRoute.outputs.first?.uid
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
                if case .communication(let uid) = route, uid != nil,
                   session.currentRoute.outputs.first?.uid == originalOutput { return nil }
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
