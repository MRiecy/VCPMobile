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
    var playbackId: String?
    var reason: String?
}

struct NativeVoicePlaybackEvent: Encodable {
    let playbackId: String
    let kind = "started"
}

private struct VoiceCaptureStopReceipt: Encodable {
    let audioEpoch: UInt64
    let sampleIndex: UInt64
}

private struct VoiceStartArgs: Decodable { let sessionId: String; let callId: String }
private struct VoiceSessionArgs: Decodable { let sessionId: String }
private struct VoiceCallArgs: Decodable { let callId: String }
private struct VoiceSegmentArgs: Decodable {
    let sessionId: String; let audioEpoch: UInt64; let startSample: UInt64; let endSample: UInt64
}
private struct VoicePlaybackArgs: Decodable { let playbackId: String }
private struct VoicePlaybackStartArgs: Decodable { let playbackId: String; let startupFrames: Int }
private struct VoicePlaybackAppendArgs: Decodable { let playbackId: String; let pcmBase64: String }

private final class VoiceWebViewLifetime {
    let onDestroy: () -> Void
    init(_ onDestroy: @escaping () -> Void) { self.onDestroy = onDestroy }
    deinit { onDestroy() }
}

/// Capture scopes, a call reservation and independent playback share one
/// physical renderer. Main owns control; inference and file I/O are isolated.
/// Network ASR/TTS and speech-boundary policy remain outside this class.
final class NativeVoiceAudio {
    private final class Run {
        let sessionId: String
        let epoch: UInt64
        let deliverySlots = DispatchSemaphore(value: 8)
        let cancelled = Atomic<Bool>(false)
        let released = Atomic<Bool>(false)
        let playbackCaptureState = VoicePlaybackCaptureState()
        var startInvoke: Invoke?
        var engine: AVAudioEngine?
        var pipe: VoiceAudioPipe?
        var tapped = false
        var aecState = "unavailable"
        var route = "unknown"
        var exportPending = false
        var releaseRequested = false
        var finalSampleIndex: UInt64?
        var stopReplies: [Invoke] = []
        let segmentFiles: VoiceSegmentFiles
        var ttsPlaybackId: String?
        var ttsPlaying = false
        var nextPlaybackMarker: UInt64 = 0
        var ttsPlaybackMarker: UInt64 = 0
        var playbackIdentities: [UInt64: String] = [:]
        var observations: [NSObjectProtocol] = []
        init(sessionId: String, epoch: UInt64, invoke: Invoke, fileQueue: DispatchQueue) {
            self.sessionId = sessionId
            self.epoch = epoch
            startInvoke = invoke
            segmentFiles = VoiceSegmentFiles(queue: fileQueue)
        }
    }

    private static var nextEpoch: UInt64 = 0
    private static var lifetimeKey: UInt8 = 0
    private weak var webView: WKWebView?
    private var attachment = UUID()
    private var active: Run?
    private var callId: String?
    private var closedCalls: [String] = []
    private let renderer = NativeVoiceRenderer()
    private var playback: NativeVoicePlayback?
    private var outputId: String?
    private var outputPlaying = false
    private var sealedCaptures: [String: Run] = [:]
    private var disposed = false
    private var cancelledBeforeStart: [String] = []
    private var releasedBeforeStart: [String] = []
    private let worker = DispatchQueue(label: "com.vcp.avatar.voice-inference", qos: .userInitiated)
    private let fileWorker = DispatchQueue(label: "com.vcp.avatar.voice-segment-files", qos: .utility)
    var eventsReady: (() -> Bool)?
    var onEvent: ((NativeVoiceAudioEvent) -> Bool)?
    var onPlaybackEvent: ((NativeVoicePlaybackEvent) -> Bool)?

    init() {
        renderer.onPlayer = { [weak self] player in self?.playback?.bind(player) }
        renderer.onFault = { [weak self] reason in
            guard let self = self else { return }
            self.finish(reason: reason, error: true)
            self.playback?.cancelCurrent(reason: reason)
            self.renderer.destroy()
        }
    }

    func beginCall(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(VoiceCallArgs.self)
            DispatchQueue.main.async { [self] in
                guard validIdentity(args.callId) else { invoke.reject("invalid_voice_call_id"); return }
                guard !disposed, !closedCalls.contains(args.callId) else { invoke.reject("cancelled:voice_call"); return }
                guard callId == nil || callId == args.callId else { invoke.reject("voice_call_busy"); return }
                callId = args.callId // logical reservation only: no mic or engine access
                invoke.resolve()
            }
        } catch { invoke.reject("invalid_voice_call_args") }
    }

    func endCall(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(VoiceCallArgs.self)
            DispatchQueue.main.async { [self] in
                guard validIdentity(args.callId) else { invoke.reject("invalid_voice_call_id"); return }
                if !closedCalls.contains(args.callId) {
                    closedCalls.append(args.callId)
                    if closedCalls.count > 32 { closedCalls.removeFirst() }
                }
                if callId == args.callId { closeCall(reason: "call_ended") }
                invoke.resolve()
            }
        } catch { invoke.reject("invalid_voice_call_args") }
    }

    private func closeCall(reason: String) {
        finish(reason: reason, error: false)
        if let ended = callId, !closedCalls.contains(ended) {
            closedCalls.append(ended)
            if closedCalls.count > 32 { closedCalls.removeFirst() }
        }
        callId = nil
        // Rust separately stops the playback belonging to this call. An
        // independent output owner keeps its queued PCM across graph demotion.
        if playback?.ownsPlayback == true {
            do { try renderer.ensure(voice: false) }
            catch { playback?.cancelCurrent(reason: voiceAudioReason(error, fallback: "voice_playback_failed")); renderer.destroy() }
        } else { renderer.destroy() }
    }

    func attach(_ webView: WKWebView) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard !disposed else { return }
        guard self.webView !== webView else { return }
        closeCall(reason: "webview_replaced")
        self.webView = webView
        attachment = UUID()
        let currentAttachment = attachment
        objc_setAssociatedObject(webView, &NativeVoiceAudio.lifetimeKey, VoiceWebViewLifetime { [weak self] in
            DispatchQueue.main.async { [weak self] in
                guard let self = self, self.attachment == currentAttachment else { return }
                self.closeCall(reason: "webview_destroyed")
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
                guard !disposed, !cancelledBeforeStart.contains(args.sessionId), !releasedBeforeStart.contains(args.sessionId) else {
                    invoke.reject("voice_start_cancelled"); return
                }
                guard callId == args.callId else { invoke.reject("voice_call_not_active"); return }
                guard active == nil else { invoke.reject("voice_audio_busy"); return }
                // Reserve the eventual frozen-cache slot before opening input;
                // stopping hardware must never fail because retention is full.
                guard sealedCaptures.count < 4 else { invoke.reject("voice_segment_retention_busy"); return }
                guard webView != nil, UIApplication.shared.applicationState == .active else {
                    invoke.reject("voice_requires_foreground"); return
                }
                guard eventsReady?() == true else { invoke.reject("voice_events_unavailable"); return }
                NativeVoiceAudio.nextEpoch += 1
                let run = Run(sessionId: args.sessionId, epoch: NativeVoiceAudio.nextEpoch, invoke: invoke, fileQueue: fileWorker)
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
                        self.prepareSegmentStorage(run)
                    }
                }
            }
        } catch { invoke.reject("invalid_voice_start_args: \(error.localizedDescription)") }
    }

    private func prepareSegmentStorage(_ run: Run) {
        dispatchPrecondition(condition: .onQueue(.main))
        fileWorker.async { [weak self, weak run] in
            guard let self = self, let run = run, !run.cancelled.load(ordering: .acquiring) else { return }
            let result = Result { _ = try VoiceSegmentFiles.prepareStorage() }
            DispatchQueue.main.async { [weak self, weak run] in
                guard let self = self, let run = run, self.active === run,
                      !self.disposed, !run.cancelled.load(ordering: .acquiring) else { return }
                guard UIApplication.shared.applicationState != .background,
                      self.webView?.window?.windowScene?.activationState != .background else {
                    self.finish(reason: "background", error: false); return
                }
                switch result {
                case .success: self.prepare(run)
                case .failure(let error):
                    self.finish(reason: voiceAudioReason(error, fallback: "voice_segment_cache_cleanup_failed"), error: true)
                }
            }
        }
    }

    private func prepare(_ run: Run) {
        dispatchPrecondition(condition: .onQueue(.main))
        do {
            try renderer.ensure(voice: true)
            guard let engine = renderer.engine else { throw voiceAudioFailure("audio_engine_not_running") }
            run.engine = engine
            run.aecState = "enabled"
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
                    let model = try SileroVoiceModel()
                    guard !run.cancelled.load(ordering: .acquiring) else { return }
                    stage = "audio_processing_failed"
                    let pipe = try VoiceAudioPipe(inputRate: sampleRate, model: model, playbackState: run.playbackCaptureState, queue: self.worker,
                        onFrame: { [weak self, weak run] frame in
                            guard let self = self, let run = run else { return false }
                            return self.post(frame, run: run)
                        }, onError: { [weak self, weak run] reason in
                            DispatchQueue.main.async {
                                guard let self = self, let run = run, self.active === run else { return }
                                self.finish(reason: reason, error: true)
                            }
                        })
                    let processingQueue = self.worker
                    DispatchQueue.main.async { [weak self, weak run] in
                        guard let self = self, let run = run, self.active === run else {
                            processingQueue.async { pipe.stop() }
                            return
                        }
                        self.startEngine(run, pipe: pipe)
                    }
                } catch {
                    let reason = voiceAudioReason(error, fallback: stage)
                    DispatchQueue.main.async { [weak self, weak run] in
                        guard let self = self, let run = run, self.active === run else { return }
                        self.finish(reason: reason, error: true)
                    }
                }
            }
        } catch {
            let reason = voiceAudioReason(error, fallback: "audio_start_failed")
            finish(reason: reason, error: true)
            if renderer.engine == nil { playback?.cancelCurrent(reason: reason) }
        }
    }

    private func startEngine(_ run: Run, pipe: VoiceAudioPipe) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard let engine = run.engine, UIApplication.shared.applicationState != .background else {
            pipe.stop()
            finish(reason: "background", error: false)
            return
        }
        run.pipe = pipe
        publishOutputStatus(outputId, playing: outputPlaying)
        // The input is tapped after Voice Processing. It is never connected to
        // the output mixer: only independent TTS is rendered.
        engine.inputNode.installTap(onBus: 0, bufferSize: 1024, format: nil) { [pipe] buffer, _ in
            pipe.accept(buffer)
        }
        run.tapped = true
        engine.inputNode.isVoiceProcessingInputMuted = false
        guard engine.isRunning else { finish(reason: "audio_engine_not_running", error: true); return }
        let invoke = run.startInvoke
        run.startInvoke = nil
        invoke?.resolve()
    }

    private func post(_ frame: VoiceAnalysisFrame, run: Run) -> Bool {
        // A stalled UIKit queue cannot accumulate unbounded per-frame messages.
        // Failure stops capture rather than inventing continuous sample history.
        guard run.deliverySlots.wait(timeout: .now()) == .success else { return false }
        DispatchQueue.main.async { [weak self, weak run] in
            defer { run?.deliverySlots.signal() }
            guard let self = self, let run = run, self.active === run, run.engine?.isRunning == true else { return }
            let playbackId = run.playbackIdentities[frame.playbackMarker]
            let playbackKnown = frame.playbackMarker == 0 || playbackId != nil
            let delivered = self.onEvent?(NativeVoiceAudioEvent(
                kind: "frame", sessionId: run.sessionId, audioEpoch: run.epoch,
                sampleIndex: frame.sampleIndex, probability: frame.probability,
                level: frame.level, bands: frame.bands, aecState: run.aecState,
                route: run.route, playbackActive: frame.playbackActive && playbackKnown, playbackId: playbackId
            )) ?? false
            if !delivered { self.finish(reason: "voice_events_unavailable", error: true) }
        }
        return true
    }

    func stop(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(VoiceSessionArgs.self)
            DispatchQueue.main.async { [self] in
                guard !args.sessionId.isEmpty, args.sessionId.utf8.count <= 128 else {
                    invoke.reject("invalid_voice_session_id"); return
                }
                rememberCancellation(args.sessionId)
                if active?.sessionId == args.sessionId {
                    active?.stopReplies.append(invoke)
                    finish(reason: "user_stopped", error: false)
                } else if let captured = sealedCaptures[args.sessionId] {
                    if let sampleIndex = captured.finalSampleIndex {
                        invoke.resolve(VoiceCaptureStopReceipt(audioEpoch: captured.epoch, sampleIndex: sampleIndex))
                    } else if captured.stopReplies.count < 8 { captured.stopReplies.append(invoke) }
                    else { invoke.reject("voice_stop_busy") }
                } else { invoke.resolve(VoiceCaptureStopReceipt(audioEpoch: 0, sampleIndex: 0)) }
            }
        } catch { invoke.reject("invalid_voice_stop_args: \(error.localizedDescription)") }
    }

    private func finish(reason: String, error: Bool) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard let run = active else { return }
        active = nil // revoke all pending permission, inference and event deliveries
        sealedCaptures[run.sessionId] = run
        rememberCancellation(run.sessionId)
        run.cancelled.store(true, ordering: .releasing)
        run.pipe?.stopAccepting()
        renderer.mute()
        run.playbackCaptureState.playing.store(false, ordering: .releasing)
        if run.tapped { run.engine?.inputNode.removeTap(onBus: 0); run.tapped = false }
        for token in run.observations { NotificationCenter.default.removeObserver(token) }
        run.observations.removeAll()
        run.engine = nil
        let pipe = run.pipe
        worker.async { [self, pipe, run] in
            let completed: (UInt64, String?) -> Void = { [self, run] sampleIndex, sealFailure in
                DispatchQueue.main.async { [self, run] in
                    run.finalSampleIndex = sampleIndex
                    let replies = run.stopReplies
                    run.stopReplies.removeAll()
                    let receipt = VoiceCaptureStopReceipt(audioEpoch: run.epoch, sampleIndex: sampleIndex)
                    for reply in replies { reply.resolve(receipt) }
                    _ = self.onEvent?(NativeVoiceAudioEvent(kind: error || sealFailure != nil ? "error" : "stopped",
                        sessionId: run.sessionId, audioEpoch: run.epoch, sampleIndex: sampleIndex,
                        aecState: run.aecState, reason: sealFailure ?? reason))
                    self.releaseIfReady(run)
                }
            }
            if let pipe = pipe { pipe.seal(on: worker, completion: completed) }
            else { completed(0, nil) }
        }
        let pending = run.startInvoke
        run.startInvoke = nil
        pending?.reject("voice_start_ended: \(reason)")
    }

    private func updatePlaybackSnapshot(_ run: Run) {
        run.playbackCaptureState.playing.store(false, ordering: .releasing)
        run.playbackCaptureState.playbackMarker.store(run.ttsPlaybackId == nil ? 0 : run.ttsPlaybackMarker, ordering: .releasing)
        run.playbackCaptureState.playing.store(run.ttsPlaying, ordering: .releasing)
    }

    func exportSegment(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(VoiceSegmentArgs.self)
            DispatchQueue.main.async { [self] in
                guard let run = capture(args.sessionId), run.epoch == args.audioEpoch, !run.releaseRequested, !run.released.load(ordering: .acquiring),
                      let pipe = run.pipe else { invoke.reject("cancelled:voice_segment"); return }
                guard !run.exportPending else { invoke.reject("voice_segment_export_busy"); return }
                run.exportPending = true
                let fileQueue = fileWorker
                worker.async { [self, run, pipe] in
                    let snapshot = Result { try pipe.snapshotSegment(start: args.startSample, end: args.endSample) }
                    fileQueue.async { [self, run] in
                        let result: Result<VoiceSegmentExport, Error>
                        if run.released.load(ordering: .acquiring) {
                            result = .failure(voiceAudioFailure("cancelled:voice_segment"))
                        } else { result = snapshot.flatMap { value in Result { try run.segmentFiles.write(value) } } }
                        DispatchQueue.main.async { [self, run] in
                            run.exportPending = false
                            defer { self.releaseIfReady(run) }
                            guard self.capture(args.sessionId) === run, !run.releaseRequested, !run.released.load(ordering: .acquiring) else {
                                invoke.reject("cancelled:voice_segment"); return
                            }
                            switch result {
                            case .success(let exported): invoke.resolve(exported)
                            case .failure(let error): invoke.reject(voiceAudioReason(error, fallback: "voice_segment_export_failed"))
                            }
                        }
                    }
                }
            }
        } catch { invoke.reject("invalid_voice_segment_args") }
    }

    private func capture(_ sessionId: String) -> Run? {
        active?.sessionId == sessionId ? active : sealedCaptures[sessionId]
    }

    func releaseSegments(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(VoiceSessionArgs.self)
            DispatchQueue.main.async { [self] in
                guard validIdentity(args.sessionId) else { invoke.reject("invalid_voice_session_id"); return }
                if !releasedBeforeStart.contains(args.sessionId) {
                    releasedBeforeStart.append(args.sessionId)
                    if releasedBeforeStart.count > 32 { releasedBeforeStart.removeFirst() }
                }
                if let run = capture(args.sessionId) {
                    run.releaseRequested = true
                    releaseIfReady(run)
                }
                invoke.resolve() // acknowledges intent; seal/export still owns its resources
            }
        } catch { invoke.reject("invalid_voice_segment_args") }
    }

    private func releaseIfReady(_ run: Run) {
        guard run.releaseRequested, run.finalSampleIndex != nil, !run.exportPending,
              !run.released.load(ordering: .acquiring) else { return }
        sealedCaptures.removeValue(forKey: run.sessionId)
        releaseSegments(run)
    }

    private func releaseSegments(_ run: Run) {
        run.released.store(true, ordering: .releasing)
        let fileQueue = fileWorker
        let pipe = run.pipe
        run.pipe = nil
        worker.async { [pipe, run] in
            pipe?.clearSegments()
            fileQueue.async {
                run.segmentFiles.clear()
            }
        }
    }

    func startPlayback(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(VoicePlaybackStartArgs.self)
            DispatchQueue.main.async { [self] in
                guard validIdentity(args.playbackId) else { invoke.reject("invalid_voice_playback_id"); return }
                guard (1...5_760).contains(args.startupFrames) else { invoke.reject("voice_playback_invalid_startup"); return }
                guard !disposed else { invoke.reject("cancelled:voice_playback"); return }
                do {
                    let playback = try ensurePlayback()
                    guard playback.canStart(args.playbackId) else { invoke.reject("cancelled:voice_playback"); return }
                    if let engine = renderer.engine, !engine.isRunning {
                        throw voiceAudioFailure("audio_engine_not_running")
                    }
                    // A reserved but never authorized call is still output-only.
                    try renderer.ensure(voice: renderer.isVoice)
                    playback.bind(renderer.speechPlayer)
                    playback.start(id: args.playbackId, startupFrames: args.startupFrames, invoke: invoke)
                } catch {
                    let reason = voiceAudioReason(error, fallback: "voice_playback_failed")
                    playback?.cancelCurrent(reason: reason)
                    invoke.reject(reason)
                }
            }
        } catch { invoke.reject("invalid_voice_playback_args") }
    }

    func appendPlayback(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(VoicePlaybackAppendArgs.self)
            DispatchQueue.main.async { [self] in
                guard let playback = playback, renderer.engine?.isRunning == true else { invoke.reject("cancelled:voice_playback"); return }
                playback.append(id: args.playbackId, pcmBase64: args.pcmBase64, invoke: invoke)
            }
        } catch { invoke.reject("invalid_voice_playback_args") }
    }

    func finishPlayback(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(VoicePlaybackArgs.self)
            DispatchQueue.main.async { [self] in
                guard let playback = playback else { invoke.reject("cancelled:voice_playback"); return }
                guard renderer.engine?.isRunning == true else {
                    playback.cancelCurrent(reason: "audio_engine_not_running")
                    invoke.reject("audio_engine_not_running"); return
                }
                playback.finish(id: args.playbackId, invoke: invoke)
            }
        } catch { invoke.reject("invalid_voice_playback_args") }
    }

    func endPlaybackSegment(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(VoicePlaybackArgs.self)
            DispatchQueue.main.async { [self] in
                guard let playback = playback, renderer.engine?.isRunning == true else {
                    invoke.reject("cancelled:voice_playback"); return
                }
                playback.endSegment(id: args.playbackId, invoke: invoke)
            }
        } catch { invoke.reject("invalid_voice_playback_args") }
    }

    func stopPlayback(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(VoicePlaybackArgs.self)
            DispatchQueue.main.async { [self] in
                guard validIdentity(args.playbackId) else { invoke.reject("invalid_voice_playback_id"); return }
                do { try ensurePlayback().stop(id: args.playbackId); invoke.resolve() }
                catch { invoke.reject(voiceAudioReason(error, fallback: "voice_playback_failed")) }
            }
        } catch { invoke.reject("invalid_voice_playback_args") }
    }

    private func validIdentity(_ id: String) -> Bool {
        !id.isEmpty && id.utf8.count <= 128
    }

    private func ensurePlayback() throws -> NativeVoicePlayback {
        if let playback = playback { return playback }
        guard let format = AVAudioFormat(standardFormatWithSampleRate: 24_000, channels: 1) else {
            throw voiceAudioFailure("voice_playback_format_unavailable")
        }
        let output = NativeVoicePlayback(format: format)
        playback = output
        output.onStarted = { [weak self] id in
            _ = self?.onPlaybackEvent?(NativeVoicePlaybackEvent(playbackId: id))
        }
        output.onStatus = { [weak self] id, playing in
            guard let self = self else { return }
            self.publishOutputStatus(id, playing: playing)
            // A new output may be admitted immediately after a completed stop.
            // Recheck ownership on the next control turn before releasing I/O.
            if id == nil {
                DispatchQueue.main.async { [weak self] in
                    guard let self = self, self.playback?.ownsPlayback != true, !self.renderer.isVoice else { return }
                    self.renderer.destroy()
                }
            }
        }
        output.bind(renderer.speechPlayer)
        return output
    }

    private func publishOutputStatus(_ id: String?, playing: Bool) {
        outputId = id
        outputPlaying = playing
        guard let run = active else { return }
        if let id = id, run.ttsPlaybackId != id {
            run.nextPlaybackMarker += 1
            run.ttsPlaybackMarker = run.nextPlaybackMarker
            run.playbackIdentities[run.ttsPlaybackMarker] = id
            if run.playbackIdentities.count > 32, let oldest = run.playbackIdentities.keys.min() {
                run.playbackIdentities.removeValue(forKey: oldest)
            }
        }
        run.ttsPlaybackId = id
        run.ttsPlaying = playing
        updatePlaybackSnapshot(run)
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

    func shutdown() {
        DispatchQueue.main.async { [self] in
            disposed = true
            finish(reason: "plugin_destroyed", error: false)
            callId = nil
            playback?.shutdown()
            renderer.destroy()
            for capture in Array(sealedCaptures.values) {
                capture.releaseRequested = true
                releaseIfReady(capture)
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
    @objc public func beginVoiceCall(_ invoke: Invoke) { voice.beginCall(invoke) }
    @objc public func endVoiceCall(_ invoke: Invoke) { voice.endCall(invoke) }
    @objc public func startVoiceAudio(_ invoke: Invoke) { voice.start(invoke) }
    @objc public func stopVoiceAudio(_ invoke: Invoke) { voice.stop(invoke) }
    @objc public func exportVoiceSegment(_ invoke: Invoke) { voice.exportSegment(invoke) }
    @objc public func releaseVoiceSegments(_ invoke: Invoke) { voice.releaseSegments(invoke) }
    @objc public func startVoicePlayback(_ invoke: Invoke) { voice.startPlayback(invoke) }
    @objc public func appendVoicePlayback(_ invoke: Invoke) { voice.appendPlayback(invoke) }
    @objc public func endVoicePlaybackSegment(_ invoke: Invoke) { voice.endPlaybackSegment(invoke) }
    @objc public func finishVoicePlayback(_ invoke: Invoke) { voice.finishPlayback(invoke) }
    @objc public func stopVoicePlayback(_ invoke: Invoke) { voice.stopPlayback(invoke) }
}
