package com.vcp.mobile.voice

import android.content.res.AssetManager
import ai.onnxruntime.OnnxTensor
import ai.onnxruntime.OrtEnvironment
import ai.onnxruntime.OrtSession
import java.nio.ByteBuffer
import java.nio.ByteOrder

/** The session, context and recurrent state belong exclusively to the capture worker. */
internal class SileroVoiceDetector(assets: AssetManager) : AutoCloseable {
    private val environment = OrtEnvironment.getEnvironment()
    private val options = OrtSession.SessionOptions()
    private var session: OrtSession? = null
    private val input = ByteBuffer.allocateDirect(576 * 4).order(ByteOrder.nativeOrder()).asFloatBuffer()
    private val state = ByteBuffer.allocateDirect(256 * 4).order(ByteOrder.nativeOrder()).asFloatBuffer()
    private val sampleRate = ByteBuffer.allocateDirect(8).order(ByteOrder.nativeOrder()).asLongBuffer()
        .apply { put(16_000); rewind() }
    private val context = FloatArray(64)
    private var inputTensor: OnnxTensor? = null
    private var stateTensor: OnnxTensor? = null
    private var rateTensor: OnnxTensor? = null
    private lateinit var feeds: Map<String, OnnxTensor>

    init {
        try {
            options.setOptimizationLevel(OrtSession.SessionOptions.OptLevel.NO_OPT)
            options.setExecutionMode(OrtSession.SessionOptions.ExecutionMode.SEQUENTIAL)
            options.setIntraOpNumThreads(1)
            options.setInterOpNumThreads(1)
            options.addConfigEntry("session.intra_op.allow_spinning", "0")
            options.addConfigEntry("session.inter_op.allow_spinning", "0")
            val model = assets.open("voice/silero_vad.ort").use { readBoundedVoiceAsset(it, 8 * 1024 * 1024) }
            session = environment.createSession(model, options)
            val audio = OnnxTensor.createTensor(environment, input, longArrayOf(1, 576)).also { inputTensor = it }
            val recurrent = OnnxTensor.createTensor(environment, state, longArrayOf(2, 1, 128)).also { stateTensor = it }
            val rate = OnnxTensor.createTensor(environment, sampleRate, longArrayOf()).also { rateTensor = it }
            feeds = mapOf("input" to audio, "state" to recurrent, "sr" to rate)
        } catch (error: Throwable) {
            close()
            throw error
        }
    }

    fun reset() {
        context.fill(0f)
        state.clear()
        repeat(state.capacity()) { state.put(0f) }
        state.rewind()
    }

    fun process(samples: FloatArray): Double {
        require(samples.size == 512)
        val activeSession = checkNotNull(session) { "Silero session is closed" }
        input.clear()
        input.put(context).put(samples).rewind()
        val probability = activeSession.run(feeds).use { result ->
            val output = result.get("output").orElseThrow { IllegalStateException("Silero output missing") } as OnnxTensor
            val nextState = result.get("stateN").orElseThrow { IllegalStateException("Silero state missing") } as OnnxTensor
            val probabilityBuffer = checkNotNull(output.floatBuffer) { "Silero probability is not float" }
            val stateBuffer = checkNotNull(nextState.floatBuffer) { "Silero state is not float" }
            check(probabilityBuffer.remaining() == 1 && stateBuffer.remaining() == state.capacity()) { "Unexpected Silero output shape" }
            state.clear()
            state.put(stateBuffer).rewind()
            probabilityBuffer.get().toDouble()
        }
        check(probability.isFinite() && probability in 0.0..1.0) { "Silero probability is invalid" }
        samples.copyInto(context, 0, 448, 512)
        return probability
    }

    override fun close() {
        inputTensor?.let { runCatching { it.close() } }; inputTensor = null
        stateTensor?.let { runCatching { it.close() } }; stateTensor = null
        rateTensor?.let { runCatching { it.close() } }; rateTensor = null
        val previous = session
        session = null
        try { previous?.close() } finally { options.close() }
    }
}
