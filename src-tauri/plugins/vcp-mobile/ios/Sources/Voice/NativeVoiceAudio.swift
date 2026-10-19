import AVFoundation
import Foundation
import ObjectiveC
import Synchronization
import Tauri
import UIKit
import WebKit

struct NativeVoiceAudioEvent: Encodable {
    let kind: String
    let sessionId: String
    let audioEpoch: UInt64
    var sampleIndex: UInt64?
    var probability: Float?
    var level: Float?
    var bands: [Float]?
    var aecState: String?
    var route: String?
    var playbackActive: Bool?
    var probeRevision: UInt64?
    var reason: String?
}

private struct VoiceStartArgs: Decodable { let sessionId: String }
private struct VoiceProbeArgs: Decodable { let sessionId: String; let enabled: Bool; let probeRevision: UInt64 }

private final class VoiceWebViewLifetime {
    let onDestroy: () -> Void
    init(_ onDestroy: @escaping () -> Void) { self.onDestroy = onDestroy }
    deinit { onDestroy() }
}

/// One native input/output owner. UIKit/session/engine operations are serialized
/// on main; ORT, conversion and spectrum work stay on a separate serial queue.
/// No ASR, TTS, audio recording, network or speech-boundary policy lives here.
final class NativeVoiceAudio {
    private final class Run {
        let token = UUID()
        let sessionId: String
        let epoch: UInt64
        let deliverySlots = DispatchSemaphore(value: 8)
        let cancelled = Atomic<Bool>(false)
        let probeCaptureState = VoiceProbeCaptureState()
        var startInvoke: Invoke?
        var engine: AVAudioEngine?
        var pipe: VoiceAudioPipe?
        var player: AVAudioPlayerNode?
        var probe: AVAudioPCMBuffer?
        var tapped = false
        var aecState = "unavailable"
        var route = "unknown"
        var playbackActive = false
        var probeGeneration: UInt64 = 0
        var probeRevision: UInt64 = 0
        var observations: [NSObjectProtocol] = []
        init(sessionId: String, epoch: UInt64, invoke: Invoke) {
            self.sessionId = sessionId
            self.epoch = epoch
            startInvoke = invoke
        }
    }

    private static var nextEpoch: UInt64 = 0
    private static var lifetimeKey: UInt8 = 0
    private weak var webView: WKWebView?
    private var attachment = UUID()
    private var active: Run?
    private var disposed = false
    private var cancelledBeforeStart: [String] = []
    private let worker = DispatchQueue(label: "com.vcp.avatar.voice-inference", qos: .userInitiated)
    // This property is read/written exclusively on worker, including resets.
    private var model: SileroVoiceModel?
    private var modelEpoch: UInt64?
    var eventsReady: (() -> Bool)?
    var onEvent: ((NativeVoiceAudioEvent) -> Bool)?

    func attach(_ webView: WKWebView) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard !disposed else { return }
        guard self.webView !== webView else { return }
        finish(reason: "webview_replaced", error: false)
        self.webView = webView
        attachment = UUID()
        let currentAttachment = attachment
        objc_setAssociatedObject(webView, &NativeVoiceAudio.lifetimeKey, VoiceWebViewLifetime { [weak self] in
            DispatchQueue.main.async { [weak self] in
                guard let self = self, self.attachment == currentAttachment else { return }
                self.finish(reason: "webview_destroyed", error: false)
            }
        }, .OBJC_ASSOCIATION_RETAIN_NONATOMIC)
    }

    func start(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(VoiceStartArgs.self)
            DispatchQueue.main.async { [self] in
                guard !args.sessionId.isEmpty, args.sessionId.utf8.count <= 128 else {
                    invoke.reject("invalid_voice_session_id"); return
                }
                guard !disposed, !cancelledBeforeStart.contains(args.sessionId) else {
                    invoke.reject("voice_start_cancelled"); return
                }
                guard active == nil else { invoke.reject("voice_audio_busy"); return }
                guard webView != nil, UIApplication.shared.applicationState == .active else {
                    invoke.reject("voice_requires_foreground"); return
                }
                guard eventsReady?() == true else { invoke.reject("voice_events_unavailable"); return }
                NativeVoiceAudio.nextEpoch += 1
                let run = Run(sessionId: args.sessionId, epoch: NativeVoiceAudio.nextEpoch, invoke: invoke)
                active = run
                installLifetimeObservers(run)
                AVAudioApplication.requestRecordPermission { [weak self, weak run] allowed in
                    DispatchQueue.main.async {
                        guard let self = self, let run = run, self.active === run else { return }
                        guard allowed else { self.finish(reason: "microphone_permission_denied", error: true); return }
                        // The permission sheet may still be completing its
                        // inactive -> active transition. Only background revokes
                        // this already explicit start intent.
                        guard UIApplication.shared.applicationState != .background else {
                            self.finish(reason: "background", error: false); return
                        }
                        self.prepare(run)
                    }
                }
            }
        } catch { invoke.reject("invalid_voice_start_args: \(error.localizedDescription)") }
    }

    private func prepare(_ run: Run) {
        dispatchPrecondition(condition: .onQueue(.main))
        do {
            try NativeAudioSessionCoordinator.shared.acquireVoice(run.token)
            let engine = AVAudioEngine()
            run.engine = engine
            do { try engine.inputNode.setVoiceProcessingEnabled(true) }
            catch {
                run.aecState = "failed"
                throw voiceAudioFailure(voiceAudioReason(error, fallback: "voice_processing_failed"))
            }
            guard engine.inputNode.isVoiceProcessingEnabled, engine.outputNode.isVoiceProcessingEnabled else {
                run.aecState = "failed"
                throw voiceAudioFailure("voice_processing_unavailable")
            }
            run.aecState = "enabled"
            engine.inputNode.isVoiceProcessingBypassed = false
            engine.inputNode.voiceProcessingOtherAudioDuckingConfiguration = .init(
                enableAdvancedDucking: false, duckingLevel: .min
            )
            let format = engine.inputNode.outputFormat(forBus: 0)
            guard format.sampleRate > 0, format.channelCount > 0 else {
                throw voiceAudioFailure("microphone_format_unavailable")
            }
            run.route = routeName(NativeAudioSessionCoordinator.shared.session.currentRoute)
            let sampleRate = format.sampleRate
            worker.async { [weak self, weak run] in
                guard let self = self, let run = run else { return }
                guard !run.cancelled.load(ordering: .acquiring) else { return }
                var stage = "vad_initialization_failed"
                do {
                    let model: SileroVoiceModel
                    if let existing = self.model { model = existing }
                    else {
                        model = try SileroVoiceModel()
                        self.model = model
                    }
                    self.modelEpoch = run.epoch
                    guard !run.cancelled.load(ordering: .acquiring) else { return }
                    stage = "voice_probe_invalid"
                    let probe = try self.loadProbe()
                    stage = "audio_processing_failed"
                    let pipe = try VoiceAudioPipe(inputRate: sampleRate, model: model, probeState: run.probeCaptureState, queue: self.worker,
                        onFrame: { [weak self, weak run] frame in
                            guard let self = self, let run = run else { return false }
                            return self.post(frame, run: run)
                        }, onError: { [weak self, weak run] reason in
                            DispatchQueue.main.async {
                                guard let self = self, let run = run, self.active === run else { return }
                                self.finish(reason: reason, error: true)
                            }
                        })
                    DispatchQueue.main.async { [weak self, weak run] in
                        guard let self = self, let run = run, self.active === run else { pipe.stop(); return }
                        self.startEngine(run, pipe: pipe, probe: probe)
                    }
                } catch {
                    let reason = voiceAudioReason(error, fallback: stage)
                    DispatchQueue.main.async { [weak self, weak run] in
                        guard let self = self, let run = run, self.active === run else { return }
                        self.finish(reason: reason, error: true)
                    }
                }
            }
        } catch { finish(reason: voiceAudioReason(error, fallback: "audio_start_failed"), error: true) }
    }

    private func startEngine(_ run: Run, pipe: VoiceAudioPipe, probe: AVAudioPCMBuffer) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard let engine = run.engine, UIApplication.shared.applicationState != .background else {
            pipe.stop()
            finish(reason: "background", error: false)
            return
        }
        run.pipe = pipe
        run.probe = probe
        let player = AVAudioPlayerNode()
        run.player = player
        engine.attach(player)
        engine.connect(player, to: engine.mainMixerNode, format: probe.format)
        // The input is tapped after Voice Processing. It is never connected to
        // the output mixer: only the explicitly enabled fixture is rendered.
        engine.inputNode.installTap(onBus: 0, bufferSize: 1024, format: nil) { [pipe] buffer, _ in
            pipe.accept(buffer)
        }
        run.tapped = true
        do {
            engine.prepare()
            try engine.start()
            guard engine.isRunning else { throw voiceAudioFailure("audio_engine_not_running") }
            installAudioObservers(run, engine: engine)
            let invoke = run.startInvoke
            run.startInvoke = nil
            invoke?.resolve()
        } catch { finish(reason: voiceAudioReason(error, fallback: "audio_start_failed"), error: true) }
    }

    private func post(_ frame: VoiceAnalysisFrame, run: Run) -> Bool {
        // A stalled UIKit queue cannot accumulate unbounded per-frame messages.
        // Failure stops capture rather than inventing continuous sample history.
        guard run.deliverySlots.wait(timeout: .now()) == .success else { return false }
        DispatchQueue.main.async { [weak self, weak run] in
            defer { run?.deliverySlots.signal() }
            guard let self = self, let run = run, self.active === run, run.engine?.isRunning == true else { return }
            let delivered = self.onEvent?(NativeVoiceAudioEvent(
                kind: "frame", sessionId: run.sessionId, audioEpoch: run.epoch,
                sampleIndex: frame.sampleIndex, probability: frame.probability,
                level: frame.level, bands: frame.bands, aecState: run.aecState,
                route: run.route, playbackActive: frame.playbackActive, probeRevision: frame.probeRevision
            )) ?? false
            if !delivered { self.finish(reason: "voice_events_unavailable", error: true) }
        }
        return true
    }

    func stop(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(VoiceStartArgs.self)
            DispatchQueue.main.async { [self] in
                guard !args.sessionId.isEmpty, args.sessionId.utf8.count <= 128 else {
                    invoke.reject("invalid_voice_session_id"); return
                }
                rememberCancellation(args.sessionId)
                if active?.sessionId == args.sessionId { finish(reason: "user_stopped", error: false) }
                invoke.resolve()
            }
        } catch { invoke.reject("invalid_voice_stop_args: \(error.localizedDescription)") }
    }

    func setProbe(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(VoiceProbeArgs.self)
            DispatchQueue.main.async { [self] in
                guard let run = active, run.sessionId == args.sessionId, run.engine?.isRunning == true,
                      let player = run.player, let probe = run.probe else {
                    invoke.reject("voice_audio_not_running"); return
                }
                guard args.probeRevision >= run.probeRevision else {
                    invoke.reject("cancelled:stale_probe_revision"); return
                }
                if args.probeRevision == run.probeRevision { invoke.resolve(); return }
                run.probeRevision = args.probeRevision
                run.probeCaptureState.replaceIntent(args.probeRevision)
                run.probeGeneration += 1
                let probeGeneration = run.probeGeneration
                player.stop() // stop clears every scheduled buffer immediately
                if args.enabled {
                    player.scheduleBuffer(probe, at: nil, options: [], completionCallbackType: .dataPlayedBack) {
                        [weak self, weak run] _ in
                        DispatchQueue.main.async {
                            guard let self = self, let run = run, self.active === run,
                                  run.probeGeneration == probeGeneration else { return }
                            guard run.probeRevision == args.probeRevision else { return }
                            run.playbackActive = false
                            run.probeCaptureState.playing.store(false, ordering: .releasing)
                        }
                    }
                    player.play()
                }
                run.playbackActive = args.enabled && player.isPlaying
                run.probeCaptureState.playing.store(run.playbackActive, ordering: .releasing)
                invoke.resolve()
            }
        } catch { invoke.reject("invalid_voice_probe_args: \(error.localizedDescription)") }
    }

    private func finish(reason: String, error: Bool) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard let run = active else { return }
        active = nil // revoke all pending permission, inference and event deliveries
        rememberCancellation(run.sessionId)
        run.cancelled.store(true, ordering: .releasing)
        run.pipe?.stop()
        run.probeGeneration += 1
        run.probeCaptureState.playing.store(false, ordering: .releasing)
        run.player?.stop()
        run.playbackActive = false
        run.engine?.stop()
        if run.tapped { run.engine?.inputNode.removeTap(onBus: 0); run.tapped = false }
        for token in run.observations { NotificationCenter.default.removeObserver(token) }
        run.observations.removeAll()
        run.engine?.reset()
        run.engine = nil
        run.player = nil
        run.probe = nil
        run.pipe = nil
        let epoch = run.epoch
        worker.async { [weak self] in
            guard let self = self, self.modelEpoch == epoch else { return }
            self.model?.reset()
            self.model = nil
            self.modelEpoch = nil
        }
        NativeAudioSessionCoordinator.shared.releaseVoice(run.token)
        let pending = run.startInvoke
        run.startInvoke = nil
        pending?.reject("voice_start_ended: \(reason)")
        _ = onEvent?(NativeVoiceAudioEvent(kind: error ? "error" : "stopped",
            sessionId: run.sessionId, audioEpoch: run.epoch, aecState: run.aecState, reason: reason))
    }

    private func installLifetimeObservers(_ run: Run) {
        observe(UIApplication.didEnterBackgroundNotification, run: run) { [weak self] _ in
            self?.finish(reason: "background", error: false)
        }
        observe(UIScene.didEnterBackgroundNotification, run: run) { [weak self] notification in
            guard let self = self, let scene = notification.object as? UIScene,
                  self.webView?.window?.windowScene === scene else { return }
            self.finish(reason: "background", error: false)
        }
        observe(UIApplication.willTerminateNotification, run: run) { [weak self] _ in
            self?.finish(reason: "application_terminated", error: false)
        }
    }

    private func installAudioObservers(_ run: Run, engine: AVAudioEngine) {
        observe(AVAudioSession.interruptionNotification, run: run) { [weak self] notification in
            guard let raw = notification.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
                  AVAudioSession.InterruptionType(rawValue: raw) == .began else { return }
            self?.finish(reason: "audio_interrupted", error: true)
        }
        observe(AVAudioSession.mediaServicesWereLostNotification, run: run) { [weak self] _ in
            self?.finish(reason: "audio_services_lost", error: true)
        }
        observe(AVAudioSession.mediaServicesWereResetNotification, run: run) { [weak self] _ in
            self?.finish(reason: "audio_services_reset", error: true)
        }
        observe(AVAudioSession.routeChangeNotification, run: run) { [weak self] notification in
            let session = NativeAudioSessionCoordinator.shared.session
            let raw = notification.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt
            if raw.flatMap(AVAudioSession.RouteChangeReason.init(rawValue:)) == .categoryChange,
               session.category == .playAndRecord, session.mode == .voiceChat { return }
            self?.finish(reason: "audio_route_changed", error: true)
        }
        observe(.AVAudioEngineConfigurationChange, run: run, object: engine) { [weak self] _ in
            self?.finish(reason: "audio_configuration_changed", error: true)
        }
    }

    private func observe(_ name: Notification.Name, run: Run, object: Any? = nil,
                         handler: @escaping (Notification) -> Void) {
        // Never tear down AVAudioEngine on its configuration notification queue.
        // Always hop asynchronously to the sole native control owner.
        run.observations.append(NotificationCenter.default.addObserver(forName: name, object: object, queue: nil) {
            [weak self, weak run] notification in
            DispatchQueue.main.async {
                guard let self = self, let run = run, self.active === run else { return }
                handler(notification)
            }
        })
    }

    private func routeName(_ route: AVAudioSessionRouteDescription) -> String {
        guard let port = route.outputs.first?.portType else { return "unknown" }
        switch port {
        case .builtInSpeaker: return "speaker"
        case .builtInReceiver: return "receiver"
        case .bluetoothHFP, .bluetoothA2DP, .bluetoothLE: return "bluetooth"
        case .headphones, .headsetMic: return "headset"
        case .usbAudio: return "usb"
        default: return "other"
        }
    }

    private func loadProbe() throws -> AVAudioPCMBuffer {
        guard let url = Bundle.main.url(forResource: "voice-probe", withExtension: "wav", subdirectory: "voice") else {
            throw voiceAudioFailure("voice_probe_missing")
        }
        let file = try AVAudioFile(forReading: url)
        let byteCount = Double(file.length) * Double(file.processingFormat.channelCount) * 4
        guard file.length > 0, file.length <= Int64(UInt32.max), byteCount <= 4 * 1024 * 1024,
              let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: AVAudioFrameCount(file.length)) else {
            throw voiceAudioFailure("voice_probe_invalid")
        }
        try file.read(into: buffer)
        return buffer
    }

    func shutdown() {
        DispatchQueue.main.async { [self] in
            disposed = true
            finish(reason: "plugin_destroyed", error: false)
            worker.async { [weak self] in
                self?.model = nil
                self?.modelEpoch = nil
            }
        }
    }

    private func rememberCancellation(_ sessionId: String) {
        if cancelledBeforeStart.contains(sessionId) { return }
        cancelledBeforeStart.append(sessionId)
        if cancelledBeforeStart.count > 32 { cancelledBeforeStart.removeFirst() }
    }
}

extension VcpMobilePlugin {
    @objc public func startVoiceAudio(_ invoke: Invoke) { voice.start(invoke) }
    @objc public func stopVoiceAudio(_ invoke: Invoke) { voice.stop(invoke) }
    @objc public func setVoiceProbe(_ invoke: Invoke) { voice.setProbe(invoke) }
}
