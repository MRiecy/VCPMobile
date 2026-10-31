import AVFoundation
import Foundation

enum NativeVoiceRoute: Equatable {
    case playback, discoveringBluetooth, communication(inputUID: String?), headphones(outputUID: String, inputUID: String)

    var usesVoiceProcessing: Bool {
        switch self {
        case .communication, .discoveringBluetooth: return true
        default: return false
        }
    }

    func matches(_ session: AVAudioSession) -> Bool {
        switch self {
        case .playback: return session.category == .playback && session.mode == .default
        case .discoveringBluetooth: return false
        case .communication(let uid):
            return session.category == .playAndRecord && session.mode == .voiceChat
                && (uid == nil || session.currentRoute.inputs.first?.uid == uid)
        case .headphones(let outputUID, let inputUID):
            return session.category == .playAndRecord && session.mode == .default
                && session.currentRoute.outputs.first?.uid == outputUID
                && session.currentRoute.inputs.first?.uid == inputUID
        }
    }
}

struct VoiceInputDevice: Encodable {
    let id: String
    let label: String
    let kind: String

    init(_ port: AVAudioSessionPortDescription) {
        id = port.uid
        label = port.portName
        switch port.portType {
        case .builtInMic: kind = "builtin"
        case .bluetoothHFP, .bluetoothLE: kind = "bluetooth"
        case .headsetMic, .lineIn: kind = "wired"
        case .usbAudio: kind = "usb"
        default: kind = "other"
        }
    }

    // A2DP may expose a Bluetooth output without identifying any input.
    // This action asks iOS to negotiate an HFP input when the user opens capture.
    static let bluetoothCall = VoiceInputDevice(id: "ios:bluetooth-call-input",
        label: "蓝牙免提输入", kind: "bluetooth")

    private init(id: String, label: String, kind: String) {
        self.id = id
        self.label = label
        self.kind = kind
    }
}

struct VoiceInputInventory: Encodable {
    let devices: [VoiceInputDevice]
    let routeOptions: [VoiceInputDevice]
    let activeId: String?

    enum CodingKeys: String, CodingKey { case devices, routeOptions, activeId }
    func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        try values.encode(devices, forKey: .devices)
        try values.encode(routeOptions, forKey: .routeOptions)
        try values.encode(activeId, forKey: .activeId)
    }
}

/// Main-queue ownership of the app's shared audio session. The real renderer
/// (VP call or output-only speech) takes priority over silent keepalive, including
/// setup/teardown when engine.isRunning cannot identify the current owner.
final class NativeAudioSessionCoordinator {
    static let shared = NativeAudioSessionCoordinator()
    let session = AVAudioSession.sharedInstance()
    private(set) var voiceOwner: UUID?
    private var keepaliveActivated = false
    private var builtInInput: VoiceInputDevice?
    private var lastBluetoothMediaUID: String?

    func forgetCallMediaOutput() { lastBluetoothMediaUID = nil }

    /// Inspect the old graph before stopping it: stopping HFP may momentarily
    /// report the speaker while iOS reconnects the previously used A2DP output.
    func mediaUIDLeavingHFP() -> String? {
        let output = session.currentRoute.outputs.first
        if output?.portType == .bluetoothA2DP { lastBluetoothMediaUID = output?.uid }
        let usingHFP = output?.portType == .bluetoothHFP || session.currentRoute.inputs.first?.portType == .bluetoothHFP
        return usingHFP ? lastBluetoothMediaUID : nil
    }

    func inputInventory(capturing: Bool) -> VoiceInputInventory {
        dispatchPrecondition(condition: .onQueue(.main))
        var ports = session.availableInputs ?? []
        let activePort = capturing ? session.currentRoute.inputs.first : nil
        if let activePort, !ports.contains(where: { $0.uid == activePort.uid }) { ports.append(activePort) }
        var devices = ports.map(VoiceInputDevice.init)
        if let mic = ports.first(where: { $0.portType == .builtInMic }) {
            builtInInput = VoiceInputDevice(mic)
        } else if ports.isEmpty, !capturing, let builtInInput {
            // Unlike a detachable accessory the phone's own mic is still there
            // while playback-only sessions hide all input ports.
            devices.append(builtInInput)
        }
        let output = session.currentRoute.outputs.first?.portType
        let supportsBluetoothChoice = output == .bluetoothA2DP || output == .bluetoothHFP
            || ports.contains(where: { $0.portType == .bluetoothHFP })
        return VoiceInputInventory(devices: devices, routeOptions: supportsBluetoothChoice ? [.bluetoothCall] : [],
            activeId: activePort?.uid)
    }

    var ownsKeepaliveConfiguration: Bool {
        voiceOwner == nil && session.category == .playback && session.mode == .default
            && session.categoryOptions.contains(.mixWithOthers)
    }

    private var headphoneOutput: AVAudioSessionPortDescription? {
        guard let output = session.currentRoute.outputs.first else { return nil }
        switch output.portType {
        case .bluetoothA2DP, .bluetoothLE, .headphones: return output
        default: return nil
        }
    }

    private var usbOutput: AVAudioSessionPortDescription? {
        session.currentRoute.outputs.first(where: { $0.portType == .usbAudio })
    }

    private func clearInputPreferences() throws {
        guard session.category == .playAndRecord else { return }
        if session.preferredInput != nil { try session.setPreferredInput(nil) }
        if session.mode == .default, session.prefersEchoCancelledInput { try session.setPrefersEchoCancelledInput(false) }
    }

    private func activate(_ mode: AVAudioSession.Mode, _ options: AVAudioSession.CategoryOptions) throws {
        try session.setCategory(.playAndRecord, mode: mode, options: options)
        try session.setActive(true)
    }

    private func selectInput(_ uid: String) throws {
        guard let port = session.availableInputs?.first(where: { $0.uid == uid }) else {
            throw voiceAudioFailure("voice_input_unavailable")
        }
        try session.setPreferredInput(port)
    }

    func preferMediaInput(_ uid: String) throws {
        if session.preferredInput?.uid != uid,
           let port = session.availableInputs?.first(where: { $0.uid == uid }) {
            try session.setPreferredInput(port)
        }
        if session.isEchoCancelledInputAvailable, !session.prefersEchoCancelledInput {
            try session.setPrefersEchoCancelledInput(true)
        }
    }

    func resolveBluetoothInput() throws -> NativeVoiceRoute {
        if let active = session.currentRoute.inputs.first, active.portType == .bluetoothHFP {
            return .communication(inputUID: active.uid)
        }
        let ports = (session.availableInputs ?? []).filter { $0.portType == .bluetoothHFP }
        guard ports.count == 1, let port = ports.first else { return .discoveringBluetooth }
        try selectInput(port.uid)
        return .communication(inputUID: port.uid)
    }

    private func communicationInput(_ requested: String?) throws -> NativeVoiceRoute {
        try activate(.voiceChat, [.defaultToSpeaker, .allowBluetoothHFP])
        guard let requested else { return .communication(inputUID: nil) }
        if requested == VoiceInputDevice.bluetoothCall.id { return try resolveBluetoothInput() }
        try selectInput(requested)
        return .communication(inputUID: requested)
    }

    func acquireVoice(_ owner: UUID, recording: Bool, inputDeviceId: String?, returningMediaUID: String?) throws -> NativeVoiceRoute {
        dispatchPrecondition(condition: .onQueue(.main))
        guard voiceOwner == nil || voiceOwner == owner else { throw voiceAudioFailure("audio_session_busy") }
        voiceOwner = owner
        NativeBackgroundExecution.shared.yieldToVoice()
        keepaliveActivated = false
        do {
            let previousHeadphones = headphoneOutput
            try clearInputPreferences()
            if recording {
                // Find the media route before voiceChat can select a paired HFP microphone.
                if inputDeviceId == VoiceInputDevice.bluetoothCall.id || session.availableInputs?.contains(where: {
                    $0.uid == inputDeviceId && $0.portType == .bluetoothHFP
                }) == true {
                    return try communicationInput(inputDeviceId)
                }
                try activate(.default, [.allowBluetoothA2DP])
                let ports = session.availableInputs ?? []
                let mic = ports.first(where: { $0.portType == .builtInMic })
                if let mic { builtInInput = VoiceInputDevice(mic) }
                let output = headphoneOutput ?? previousHeadphones ?? (inputDeviceId != nil ? usbOutput : nil)
                if output?.portType == .bluetoothA2DP { lastBluetoothMediaUID = output?.uid }
                let returningToPhone = returningMediaUID != nil && (inputDeviceId == nil || inputDeviceId == builtInInput?.id)
                if let outputUID = output?.uid ?? (returningToPhone ? returningMediaUID : nil) {
                    let selected = inputDeviceId == nil ? mic : ports.first(where: { $0.uid == inputDeviceId })
                    if let selected {
                        try preferMediaInput(selected.uid)
                        // A wired/USB input may select its matching output. The built-in mic keeps the media route.
                        let expectedUID = selected.portType == .builtInMic ? outputUID : (headphoneOutput?.uid ?? usbOutput?.uid ?? outputUID)
                        return .headphones(outputUID: expectedUID, inputUID: selected.uid)
                    }
                    if returningToPhone, let uid = builtInInput?.id {
                        // HFP may still hide even the phone input while switching
                        // profiles. The renderer waits for this known physical UID.
                        return .headphones(outputUID: outputUID, inputUID: uid)
                    }
                    if inputDeviceId == nil { throw voiceAudioFailure("audio_route_unavailable") }
                    if returningToPhone { throw voiceAudioFailure("voice_input_unavailable") }
                }
                return try communicationInput(inputDeviceId)
            } else {
                try session.setCategory(.playback, mode: .default, options: [])
            }
            try session.setActive(true)
            return .playback
        } catch {
            releaseVoice(owner)
            throw error
        }
    }

    func releaseVoice(_ owner: UUID) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard voiceOwner == owner else { return }
        // The caller has already stopped input and output before relinquishing
        // its reservation. Never deactivate a session configured by another user.
        if session.category == .playAndRecord || session.category == .playback {
            do { try clearInputPreferences() }
            catch { NSLog("[VcpVoice] Reset input preferences: %@", error.localizedDescription) }
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
