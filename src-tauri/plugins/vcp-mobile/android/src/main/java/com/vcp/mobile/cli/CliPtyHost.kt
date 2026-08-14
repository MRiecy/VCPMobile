package com.vcp.mobile.cli

import android.util.Base64
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.JSObject
import java.nio.charset.StandardCharsets
import java.util.UUID
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong

private const val PTY_MAX_READ_BYTES = 64 * 1024
private const val PTY_MAX_WRITE_BYTES = 16 * 1024
private const val PTY_MAX_WAIT_MS = 1_000
// Must stay above ~2.1 GiB: bionic's dynamic linker reserves a ~2 GiB CFI shadow
// (MAP_NORESERVE, no committed RAM) when exec'ing the host PRoot PIE. A smaller
// RLIMIT_AS makes the linker abort with linker_cfi.cpp "MapShadow" failure (code 134).
private const val PTY_ADDRESS_SPACE_KIB = 4L * 1024L * 1024L
private const val PTY_RUNNING_EXIT_CODE = Int.MIN_VALUE
private const val PTY_REPLAY_BYTES = 128 * 1024

@InvokeArg
class OpenCliPtyArgs {
    lateinit var operationId: String
    var runtimeGeneration: Long = 0
    lateinit var rootfsPath: String
    lateinit var cwd: String
    var rows: Int = 0
    var cols: Int = 0
}

@InvokeArg
class ReadCliPtyArgs {
    lateinit var operationId: String
    lateinit var sessionId: String
    var sessionGeneration: Long = 0
    var cursor: Long = 0
    var maxBytes: Int = 0
    var waitMs: Int = 0
}

@InvokeArg
class WriteCliPtyArgs {
    lateinit var operationId: String
    lateinit var sessionId: String
    var sessionGeneration: Long = 0
    lateinit var dataBase64: String
}

@InvokeArg
class ResizeCliPtyArgs {
    lateinit var operationId: String
    lateinit var sessionId: String
    var sessionGeneration: Long = 0
    var rows: Int = 0
    var cols: Int = 0
}

@InvokeArg
class CloseCliPtyArgs {
    lateinit var operationId: String
    lateinit var sessionId: String
    var sessionGeneration: Long = 0
}

internal object CliPtyNative {
    init {
        System.loadLibrary("vcp_pty")
    }

    external fun spawn(
        argv: Array<String>,
        environment: Array<String>,
        rows: Int,
        cols: Int,
        addressSpaceKib: Long,
    ): LongArray
    external fun read(handle: Long, maxBytes: Int, waitMs: Int): ByteArray?
    external fun write(handle: Long, bytes: ByteArray): Int
    external fun resize(handle: Long, rows: Int, cols: Int)
    external fun exitCode(handle: Long): Int
    external fun close(handle: Long)
}

private data class CliPtySession(
    val id: String,
    val generation: Long,
    val runtimeGeneration: Long,
    val nativeHandle: Long,
    val pid: Int,
    val cwd: String,
    val prootTmpDir: java.io.File,
    var cursor: Long = 0,
    var replay: ByteArray = ByteArray(0),
    var eof: Boolean = false,
    var exitCode: Int? = null,
)

internal class CliPtyHost(private val processHost: CliProcessHost) {
    private val executor = ThreadPoolExecutor(
        1,
        1,
        0L,
        TimeUnit.MILLISECONDS,
        ArrayBlockingQueue(64),
        { runnable -> Thread(runnable, "vcp-cli-pty") },
        ThreadPoolExecutor.AbortPolicy(),
    )
    private val generation = AtomicLong(0)
    private val lock = Any()
    @Volatile private var active: CliPtySession? = null

    private fun validateIdentity(sessionId: String, sessionGeneration: Long): CliPtySession {
        validateIdentifier(sessionId, "sessionId")
        val session = active ?: error("terminal session is not active")
        require(session.id == sessionId && session.generation == sessionGeneration) {
            "terminal session identity is stale"
        }
        return session
    }

    fun open(args: OpenCliPtyArgs, success: (JSObject) -> Unit, failure: (String) -> Unit) = submit(success, failure) {
        validateIdentifier(args.operationId, "operationId")
        require(args.runtimeGeneration > 0) { "runtimeGeneration must be positive" }
        require(args.rows in 1..1000 && args.cols in 1..1000) { "terminal dimensions are invalid" }
        synchronized(lock) {
            active?.let { session ->
                require(session.runtimeGeneration == args.runtimeGeneration) {
                    "an older terminal session must be ended before runtime replacement"
                }
                require(session.cwd == validateGuestCwd(args.cwd)) {
                    "an active terminal session cannot change cwd"
                }
                CliPtyNative.resize(session.nativeHandle, args.rows, args.cols)
                return@synchronized sessionResponse(args.operationId, session)
            }
            val sessionGeneration = generation.incrementAndGet()
            val sessionId = "pty-${UUID.randomUUID()}"
            val stem = sessionId.removePrefix("pty-")
            val launch = processHost.terminalLaunchSpec(
                args.runtimeGeneration,
                args.rootfsPath,
                args.cwd,
                stem,
            )
            try {
                val native = CliPtyNative.spawn(
                    launch.argv.toTypedArray(),
                    launch.environment.map { (key, value) -> "$key=$value" }.toTypedArray(),
                    args.rows,
                    args.cols,
                    PTY_ADDRESS_SPACE_KIB,
                )
                require(native.size == 2 && native[0] > 0 && native[1] > 1) {
                    "native PTY returned an invalid identity"
                }
                val session = CliPtySession(
                    id = sessionId,
                    generation = sessionGeneration,
                    runtimeGeneration = args.runtimeGeneration,
                    nativeHandle = native[0],
                    pid = native[1].toInt(),
                    cwd = validateGuestCwd(args.cwd),
                    prootTmpDir = launch.prootTmpDir,
                )
                active = session
                sessionResponse(args.operationId, session)
            } catch (error: Throwable) {
                launch.prootTmpDir.deleteRecursively()
                throw error
            }
        }
    }

    fun read(args: ReadCliPtyArgs, success: (JSObject) -> Unit, failure: (String) -> Unit) = submit(success, failure) {
        validateIdentifier(args.operationId, "operationId")
        require(args.maxBytes in 1..PTY_MAX_READ_BYTES) { "maxBytes is outside the PTY boundary" }
        require(args.waitMs in 0..PTY_MAX_WAIT_MS) { "waitMs is outside the PTY boundary" }
        synchronized(lock) {
            val session = validateIdentity(args.sessionId, args.sessionGeneration)
            require(args.cursor == session.cursor) { "terminal cursor is stale" }
            val bytes = if (session.eof) ByteArray(0) else CliPtyNative.read(
                session.nativeHandle,
                args.maxBytes,
                args.waitMs,
            )
            if (bytes != null && bytes.isEmpty()) session.eof = true
            if (bytes != null && bytes.isNotEmpty()) {
                val combined = session.replay + bytes
                session.replay = if (combined.size <= PTY_REPLAY_BYTES) {
                    combined
                } else {
                    combined.copyOfRange(combined.size - PTY_REPLAY_BYTES, combined.size)
                }
            }
            if (session.exitCode == null) {
                val exitCode = CliPtyNative.exitCode(session.nativeHandle)
                if (exitCode != PTY_RUNNING_EXIT_CODE) session.exitCode = exitCode
            }
            if (bytes != null) session.cursor += bytes.size
            JSObject().apply {
                put("operationId", args.operationId)
                put("sessionId", session.id)
                put("sessionGeneration", session.generation)
                put("cursor", session.cursor)
                put("dataBase64", Base64.encodeToString(bytes ?: ByteArray(0), Base64.NO_WRAP))
                put("timedOut", bytes == null)
                put("eof", session.eof)
                put("exitCode", session.exitCode ?: org.json.JSONObject.NULL)
            }
        }
    }

    fun write(args: WriteCliPtyArgs, success: (JSObject) -> Unit, failure: (String) -> Unit) = submit(success, failure) {
        validateIdentifier(args.operationId, "operationId")
        val bytes = Base64.decode(args.dataBase64, Base64.NO_WRAP)
        require(bytes.isNotEmpty() && bytes.size <= PTY_MAX_WRITE_BYTES) { "terminal write is outside the PTY boundary" }
        synchronized(lock) {
            val session = validateIdentity(args.sessionId, args.sessionGeneration)
            require(!session.eof && session.exitCode == null) { "terminal session has exited" }
            val written = CliPtyNative.write(session.nativeHandle, bytes)
            require(written == bytes.size) { "terminal write was partial" }
            JSObject().apply {
                put("operationId", args.operationId)
                put("sessionId", session.id)
                put("sessionGeneration", session.generation)
                put("writtenBytes", written)
            }
        }
    }

    fun resize(args: ResizeCliPtyArgs, success: (JSObject) -> Unit, failure: (String) -> Unit) = submit(success, failure) {
        validateIdentifier(args.operationId, "operationId")
        require(args.rows in 1..1000 && args.cols in 1..1000) { "terminal dimensions are invalid" }
        synchronized(lock) {
            val session = validateIdentity(args.sessionId, args.sessionGeneration)
            CliPtyNative.resize(session.nativeHandle, args.rows, args.cols)
            JSObject().apply {
                put("operationId", args.operationId)
                put("sessionId", session.id)
                put("sessionGeneration", session.generation)
                put("rows", args.rows)
                put("cols", args.cols)
            }
        }
    }

    fun close(args: CloseCliPtyArgs, success: (JSObject) -> Unit, failure: (String) -> Unit) = submit(success, failure) {
        validateIdentifier(args.operationId, "operationId")
        synchronized(lock) {
            val session = validateIdentity(args.sessionId, args.sessionGeneration)
            CliPtyNative.close(session.nativeHandle)
            session.prootTmpDir.deleteRecursively()
            active = null
            JSObject().apply {
                put("operationId", args.operationId)
                put("sessionId", session.id)
                put("sessionGeneration", session.generation)
                put("closed", true)
            }
        }
    }

    private fun sessionResponse(operationId: String, session: CliPtySession) = JSObject().apply {
        put("operationId", operationId)
        put("sessionId", session.id)
        put("sessionGeneration", session.generation)
        put("runtimeGeneration", session.runtimeGeneration)
        put("pid", session.pid)
        put("cwd", session.cwd)
        put("shell", "/bin/bash")
        put("state", if (session.exitCode == null) "running" else "exited")
        put("exitCode", session.exitCode ?: org.json.JSONObject.NULL)
        put("cursor", session.cursor)
        put("replayBase64", Base64.encodeToString(session.replay, Base64.NO_WRAP))
    }

    private fun submit(
        success: (JSObject) -> Unit,
        failure: (String) -> Unit,
        action: () -> JSObject,
    ) {
        try {
            executor.execute {
                try {
                    success(action())
                } catch (error: Throwable) {
                    failure(error.message ?: error.javaClass.simpleName)
                }
            }
        } catch (error: Throwable) {
            failure(error.message ?: "terminal executor is unavailable")
        }
    }
}

internal object CliPtyHostOwner {
    @Volatile private var instance: CliPtyHost? = null

    fun get(processHost: CliProcessHost): CliPtyHost = instance ?: synchronized(this) {
        instance ?: CliPtyHost(processHost).also { instance = it }
    }
}
