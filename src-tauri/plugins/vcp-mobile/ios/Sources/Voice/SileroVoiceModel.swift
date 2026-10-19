import Foundation
import onnxruntime_objc

/// One CPU session, accessed only by NativeVoiceAudio's serial inference queue.
/// This owns model recurrent state, not the product's speech/turn decisions.
final class SileroVoiceModel {
    private let environment: ORTEnv
    private let session: ORTSession
    private let inputData: NSMutableData
    private let stateData: NSMutableData
    private let rateData: NSMutableData
    private let inputValue: ORTValue
    private let stateValue: ORTValue
    private let rateValue: ORTValue

    init() throws {
        // The app's Xcode resources phase owns this folder. swift-rs links our
        // static library but does not embed SwiftPM resource bundles in the app.
        guard let path = Bundle.main.url(forResource: "silero_vad", withExtension: "ort", subdirectory: "voice"),
              let input = NSMutableData(length: 576 * MemoryLayout<Float>.size),
              let state = NSMutableData(length: 256 * MemoryLayout<Float>.size),
              let rate = NSMutableData(length: MemoryLayout<Int64>.size) else {
            throw voiceAudioFailure("vad_resources_unavailable")
        }
        inputData = input
        stateData = state
        rateData = rate
        rate.mutableBytes.storeBytes(of: Int64(16_000), as: Int64.self)
        environment = try ORTEnv(loggingLevel: .warning)
        let options = try ORTSessionOptions()
        try options.setIntraOpNumThreads(1)
        // ORT's default execution mode is sequential. The shipped minimal ORT
        // model is already optimized; runtime graph transforms are not included.
        try options.setGraphOptimizationLevel(.none)
        try options.addConfigEntry(withKey: "session.intra_op.allow_spinning", value: "0")
        try options.addConfigEntry(withKey: "session.inter_op.allow_spinning", value: "0")
        session = try ORTSession(env: environment, modelPath: path.path, sessionOptions: options)
        inputValue = try ORTValue(tensorData: input, elementType: .float, shape: [1, 576])
        stateValue = try ORTValue(tensorData: state, elementType: .float, shape: [2, 1, 128])
        rateValue = try ORTValue(tensorData: rate, elementType: .int64, shape: [])
    }

    func reset() {
        memset(inputData.mutableBytes, 0, inputData.length)
        memset(stateData.mutableBytes, 0, stateData.length)
    }

    func probability(_ samples: UnsafePointer<Float>) throws -> Float {
        let input = inputData.mutableBytes.assumingMemoryBound(to: Float.self)
        memcpy(input.advanced(by: 64), samples, 512 * MemoryLayout<Float>.size)
        let outputs: [String: ORTValue]
        do {
            outputs = try session.run(
                withInputs: ["input": inputValue, "state": stateValue, "sr": rateValue],
                outputNames: ["output", "stateN"], runOptions: nil
            )
        } catch { throw voiceAudioFailure(voiceAudioReason(error, fallback: "vad_inference_failed")) }
        guard let output = outputs["output"], let nextState = outputs["stateN"] else {
            throw voiceAudioFailure("vad_output_missing")
        }
        let probabilityData = try output.tensorData()
        let nextStateData = try nextState.tensorData()
        guard probabilityData.length == MemoryLayout<Float>.size,
              nextStateData.length == stateData.length else {
            throw voiceAudioFailure("vad_output_shape_invalid")
        }
        let probability = probabilityData.bytes.assumingMemoryBound(to: Float.self).pointee
        guard probability.isFinite, probability >= 0, probability <= 1 else {
            throw voiceAudioFailure("vad_probability_invalid")
        }
        memcpy(stateData.mutableBytes, nextStateData.bytes, stateData.length)
        // The next inference receives the last 64 samples as context, in
        // addition to (never instead of) its 512 new samples.
        memcpy(input, samples.advanced(by: 448), 64 * MemoryLayout<Float>.size)
        return probability
    }
}
