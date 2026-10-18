package com.vcp.mobile.cli

import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.Process as AndroidProcess
import android.util.Log
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import app.tauri.plugin.JSObject
import com.vcp.mobile.service.ForegroundGuardian
import java.io.File
import java.nio.charset.StandardCharsets
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.CountDownLatch
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference

private const val TAG = "CliProcessHost"
private const val MAX_COMMAND_BYTES = 64 * 1024
private const val MAX_ACTIVE_PROCESSES = 4
private const val MAX_REMEMBERED_PROCESSES = 1024
private const val MAX_CANCEL_GRACE_MS = 5_000L
private const val HANDSHAKE_TIMEOUT_MS = 3_000L
private const val ORPHAN_GRACE_MS = 250L
private const val THREAD_JOIN_MS = 2_000L
private const val TERMINAL_DRAIN_WAIT_MS = 10_000L
private const val FOREGROUND_READINESS_WAIT_MS = 6_000L
// Must stay above ~2.1 GiB: bionic's dynamic linker reserves a ~2 GiB CFI shadow
// (MAP_NORESERVE, no committed RAM) when exec'ing the host PRoot PIE. A smaller
// RLIMIT_AS makes the linker abort with linker_cfi.cpp "MapShadow" failure (code 134).
private const val PROCESS_ADDRESS_SPACE_KIB = 4L * 1024L * 1024L

/** A cleanup failure must not skip another resource or replace the operation's original error. */
private inline fun cleanupCliResource(
    resource: String,
    primaryError: Throwable? = null,
    cleanup: () -> Unit,
) {
    try {
        cleanup()
    } catch (error: Throwable) {
        if (primaryError != null && error !== primaryError) primaryError.addSuppressed(error)
        Log.w(TAG, "Unable to clean up CLI $resource", error)
    }
}

private data class ProcessKey(
    val jobId: String,
    val attemptId: String,
    val runtimeGeneration: Long,
)

private data class CliForegroundLease(
    val tag: String,
    val generation: Long,
)

private data class PrepareResult(
    val operationId: String,
    val prepared: PreparedRuntime,
) {
    fun toJsObject() = JSObject().apply {
        put("operationId", operationId)
        put("profileId", prepared.profileId)
        put("runtimeGeneration", prepared.runtimeGeneration)
        put("archivePath", prepared.archive.absolutePath)
        put("rootfsParentPath", prepared.rootfsParent.absolutePath)
        put("workspacePath", prepared.workspace.absolutePath)
        put("skillsPath", prepared.skills.absolutePath)
        put("outputPath", prepared.output.absolutePath)
        put("projectionRootPath", prepared.projectionRoot.absolutePath)
        put("prootPath", prepared.proot.absolutePath)
        put("prootLoaderPath", prepared.prootLoader.absolutePath)
    }
}

private data class StartResult(
    val operationId: String,
    val handle: CliProcessHandle,
) {
    fun toJsObject() = JSObject().apply {
        put("operationId", operationId)
        put("jobId", handle.key.jobId)
        put("attemptId", handle.key.attemptId)
        put("runtimeGeneration", handle.key.runtimeGeneration)
        put("pid", handle.identity.pid)
        put("pgid", handle.identity.pgid)
        put("sessionId", handle.identity.sessionId)
        put("startTimeTicks", handle.identity.startTimeTicks)
        put("stdoutPath", handle.stdoutFile.absolutePath)
        put("stderrPath", handle.stderrFile.absolutePath)
    }
}

private class CliProcessHandle(
    val key: ProcessKey,
    val fingerprint: String,
    val identity: ProcIdentity,
    val process: Process,
    val stdoutFile: File,
    val stderrFile: File,
    val artifactBudget: ArtifactBudget,
    val stdoutReader: Thread,
    val stderrReader: Thread,
    val prootTmpDir: File,
    val foregroundLease: CliForegroundLease?,
) {
    val exitCode = AtomicReference<Int?>(null)
    val finished = CountDownLatch(1)
    val finalized = AtomicBoolean(false)
    val backgroundLeaseLost = AtomicBoolean(false)
    val terminationLock = Any()
    lateinit var waiter: Thread

    fun isCompleted(): Boolean = finished.count == 0L
}

private fun safeFileStem(key: ProcessKey): String = sha256Hex(
    "${key.runtimeGeneration}\u0000${key.jobId}\u0000${key.attemptId}"
        .toByteArray(StandardCharsets.UTF_8),
)

/**
 * Android-only transient process owner. Durable state, terminal decisions,
 * output cursors and retry fencing remain owned by the Rust MobileCliRuntime.
 */
internal class CliProcessHost(private val context: Context) : AutoCloseable {
    private val threadSequence = AtomicInteger(0)
    private val controlExecutor = ThreadPoolExecutor(
        1,
        1,
        0L,
        TimeUnit.MILLISECONDS,
        ArrayBlockingQueue(64),
        { runnable -> Thread(runnable, "vcp-cli-control-${threadSequence.incrementAndGet()}") },
        ThreadPoolExecutor.AbortPolicy(),
    )
    private val inspectExecutor = ThreadPoolExecutor(
        2,
        2,
        0L,
        TimeUnit.MILLISECONDS,
        ArrayBlockingQueue(64),
        { runnable -> Thread(runnable, "vcp-cli-inspect-${threadSequence.incrementAndGet()}") },
        ThreadPoolExecutor.AbortPolicy(),
    )
    private val cancelExecutor = ThreadPoolExecutor(
        2,
        2,
        0L,
        TimeUnit.MILLISECONDS,
        ArrayBlockingQueue(32),
        { runnable -> Thread(runnable, "vcp-cli-cancel-${threadSequence.incrementAndGet()}") },
        ThreadPoolExecutor.AbortPolicy(),
    )
    private val closed = AtomicBoolean(false)
    private val lifecycleLock = Any()
    private val handles = ConcurrentHashMap<ProcessKey, CliProcessHandle>()
    private val completedKeys = ConcurrentLinkedQueue<ProcessKey>()
    private val installer = CliRuntimeInstaller(context.applicationContext)
    @Volatile private var preparedRuntime: PreparedRuntime? = null

    companion object {
        fun foregroundTag(jobId: String, attemptId: String): String = "cli:$jobId:$attemptId"
    }

    fun terminalLaunchSpec(
        runtimeGeneration: Long,
        rootfsPath: String,
        cwd: String,
        sessionStem: String,
    ): CliPtyLaunchSpec = synchronized(lifecycleLock) {
        ensureOpen()
        val prepared = preparedRuntime ?: error("CLI runtime has not been prepared")
        require(prepared.runtimeGeneration == runtimeGeneration) {
            "runtimeGeneration does not match the prepared runtime"
        }
        validateIdentifier(sessionStem, "sessionStem")
        val rootfs = File(rootfsPath).canonicalFile
        require(rootfs.isDirectory && rootfs.toPath().startsWith(prepared.rootfsParent.toPath()) &&
            rootfs != prepared.rootfsParent
        ) { "rootfsPath is outside the prepared private root" }
        val prootTmpDir = File(prepared.prootTmpParent, "pty-$sessionStem").ensureDirectory(mode = 0x1c0)
        CliPtyLaunchSpec(
            runtimeGeneration = runtimeGeneration,
            argv = buildProotTerminalArguments(
                prootPath = prepared.proot.absolutePath,
                rootfsPath = rootfs.absolutePath,
                workspacePath = prepared.workspace.absolutePath,
                cwd = cwd,
            ),
            environment = buildHostEnvironment(
                prootTmpPath = prootTmpDir.absolutePath,
                prootLoaderPath = prepared.prootLoader.absolutePath,
            ),
            prootTmpDir = prootTmpDir,
        )
    }

    fun prepare(
        args: PrepareCliRuntimeArgs,
        success: (JSObject) -> Unit,
        failure: (String) -> Unit,
    ) = submit(success, failure) {
        validateIdentifier(args.operationId, "operationId")
        synchronized(lifecycleLock) {
            ensureOpen()
            val prepared = installer.prepare(args)
            val previous = preparedRuntime
            if (previous != null &&
                (previous.runtimeGeneration != prepared.runtimeGeneration || previous.profileId != prepared.profileId)
            ) {
                require(handles.values.none { !it.isCompleted() }) {
                    "cannot replace the runtime while CLI processes are active"
                }
                handles.clear()
                completedKeys.clear()
            }
            preparedRuntime = prepared
            PrepareResult(args.operationId, prepared).toJsObject()
        }
    }

    fun start(
        args: StartCliProcessArgs,
        success: (JSObject) -> Unit,
        failure: (String) -> Unit,
    ) = submit(success, failure) {
        synchronized(lifecycleLock) {
            ensureOpen()
            startBlocking(args).toJsObject()
        }
    }

    fun inspect(
        args: InspectCliProcessArgs,
        success: (JSObject) -> Unit,
        failure: (String) -> Unit,
    ) = submitOn(inspectExecutor, "inspect", success, failure) { inspectBlocking(args) }

    fun cancel(
        args: CancelCliProcessArgs,
        success: (JSObject) -> Unit,
        failure: (String) -> Unit,
    ) = submitOn(cancelExecutor, "cancel", success, failure) { cancelBlocking(args) }

    private fun submit(
        success: (JSObject) -> Unit,
        failure: (String) -> Unit,
        action: () -> JSObject,
    ) = submitOn(controlExecutor, "control", success, failure, action)

    private fun submitOn(
        executor: ThreadPoolExecutor,
        queueName: String,
        success: (JSObject) -> Unit,
        failure: (String) -> Unit,
        action: () -> JSObject,
    ) {
        val pending = CliPendingRequest(success, failure, action)
        if (closed.get()) {
            pending.rejectBeforeStart("CLI ProcessHost is closed")
            return
        }
        try {
            executor.execute(pending)
        } catch (_: RejectedExecutionException) {
            pending.rejectBeforeStart("CLI ProcessHost $queueName queue is unavailable")
        }
    }

    private fun startBlocking(args: StartCliProcessArgs): StartResult {
        validateIdentifier(args.operationId, "operationId")
        validateIdentifier(args.jobId, "jobId")
        validateIdentifier(args.attemptId, "attemptId")
        require(args.runtimeGeneration > 0) { "runtimeGeneration must be positive" }
        require(!args.command.contains('\u0000') &&
            args.command.toByteArray(StandardCharsets.UTF_8).size <= MAX_COMMAND_BYTES
        ) { "command exceeds the non-interactive Bash boundary" }
        require(args.rootfsPath.length <= 4096 && !args.rootfsPath.contains('\u0000')) {
            "rootfsPath is invalid"
        }
        require(args.artifactMaxBytes in 0..MAX_ARTIFACT_BYTES) {
            "artifactMaxBytes is outside the supported range"
        }
        require(args.timeoutMs > 0) { "timeoutMs must be positive" }
        require(args.displayLabel.toByteArray(StandardCharsets.UTF_8).size in 1..256 &&
            !args.displayLabel.contains('\u0000') &&
            !args.displayLabel.contains('\n') &&
            !args.displayLabel.contains('\r')
        ) { "displayLabel must be a bounded single line" }
        args.sessionId?.let { validateIdentifier(it, "sessionId") }
        val cwd = validateGuestCwd(args.cwd)
        val prepared = preparedRuntime ?: error("CLI runtime has not been prepared")
        require(prepared.runtimeGeneration == args.runtimeGeneration) {
            "runtimeGeneration does not match the prepared runtime"
        }

        val rootfs = File(args.rootfsPath).canonicalFile
        require(rootfs.isDirectory && rootfs.toPath().startsWith(prepared.rootfsParent.toPath()) &&
            rootfs != prepared.rootfsParent
        ) { "rootfsPath is outside the prepared private root" }

        val key = ProcessKey(args.jobId, args.attemptId, args.runtimeGeneration)
        val requestFingerprint = fingerprint(args)
        handles[key]?.let { existing ->
            require(existing.fingerprint == requestFingerprint) {
                "process identity was reused with different launch parameters"
            }
            return StartResult(args.operationId, existing)
        }
        evictCompletedHandlesIfNeeded()
        require(handles.size < MAX_REMEMBERED_PROCESSES) { "CLI ProcessHost identity ledger is full" }
        require(handles.values.count { !it.isCompleted() } < MAX_ACTIVE_PROCESSES) {
            "CLI ProcessHost active process limit reached"
        }

        val stem = safeFileStem(key)
        val stdoutFile = File(prepared.output, "$stem.stdout")
        val stderrFile = File(prepared.output, "$stem.stderr")
        val handshakeFile = File(prepared.handshakeParent, "$stem.pid")
        var prootTmpDir: File? = null
        var budget: ArtifactBudget? = null
        var process: Process? = null
        var stdoutReader: Thread? = null
        var stderrReader: Thread? = null
        var verifiedIdentity: ProcIdentity? = null
        var foregroundLease: CliForegroundLease? = null
        var ownsStdout = false
        var ownsStderr = false
        var ownsHandshake = false
        try {
            require(stdoutFile.createNewFile()) { "stdout artifact identity already exists" }
            ownsStdout = true
            require(stderrFile.createNewFile()) { "stderr artifact identity already exists" }
            ownsStderr = true
            if (handshakeFile.exists()) require(handshakeFile.delete()) { "stale handshake cannot be removed" }
            ownsHandshake = true
            val activeProotTmpDir = File(prepared.prootTmpParent, stem).ensureDirectory(mode = 0x1c0)
            prootTmpDir = activeProotTmpDir
            val activeBudget = ArtifactBudget(args.artifactMaxBytes, stdoutFile, stderrFile)
            budget = activeBudget
            foregroundLease = acquireForegroundLease(args)

            val prootArguments = buildProotArguments(
                prootPath = prepared.proot.absolutePath,
                rootfsPath = rootfs.absolutePath,
                workspacePath = prepared.workspace.absolutePath,
                cwd = cwd,
                command = args.command,
                sessionId = args.sessionId,
            )
            val processBuilder = ProcessBuilder(
                buildHostCommand(
                    handshakeFile.absolutePath,
                    PROCESS_ADDRESS_SPACE_KIB,
                    prootArguments,
                ),
            )
            processBuilder.redirectErrorStream(false)
            processBuilder.environment().apply {
                clear()
                putAll(
                    buildHostEnvironment(
                        prootTmpPath = activeProotTmpDir.absolutePath,
                        prootLoaderPath = prepared.prootLoader.absolutePath,
                    ),
                )
            }

            process = processBuilder.start()
            stdoutReader = activeBudget.startDrainThread(
                name = "vcp-cli-stdout-${stem.take(12)}",
                input = process.inputStream,
                stream = ArtifactStream.STDOUT,
            )
            stderrReader = activeBudget.startDrainThread(
                name = "vcp-cli-stderr-${stem.take(12)}",
                input = process.errorStream,
                stream = ArtifactStream.STDERR,
            )

            val pid = waitForHandshakePid(handshakeFile, process)
            val identity = OwnedProcessGroup.readIdentity(pid) ?: error("process leader disappeared during identity handshake")
            require(identity.pid == pid && identity.pgid == pid && identity.sessionId == pid) {
                "setsid did not establish a dedicated process group and session"
            }
            require(identity.uid == AndroidProcess.myUid()) { "CLI process is not owned by the app UID" }
            verifiedIdentity = identity

            process.outputStream.use { stdin ->
                stdin.write("GO\n".toByteArray(StandardCharsets.US_ASCII))
                stdin.flush()
            }
            handshakeFile.delete()

            val handle = CliProcessHandle(
                key = key,
                fingerprint = requestFingerprint,
                identity = identity,
                process = process,
                stdoutFile = stdoutFile.canonicalFile,
                stderrFile = stderrFile.canonicalFile,
                artifactBudget = activeBudget,
                stdoutReader = stdoutReader,
                stderrReader = stderrReader,
                prootTmpDir = activeProotTmpDir,
                foregroundLease = foregroundLease,
            )
            handle.waiter = startWaiter(handle)
            handles[key] = handle
            return StartResult(args.operationId, handle)
        } catch (error: Throwable) {
            foregroundLease?.let { lease ->
                cleanupCliResource("foreground lease after start failure", error) { releaseForegroundLease(lease) }
            }
            abortUnregisteredProcess(process, verifiedIdentity, stdoutReader, stderrReader, budget, error)
            if (ownsStdout) cleanupCliResource("stdout after start failure", error) {
                check(stdoutFile.delete() || !stdoutFile.exists()) { "Unable to remove stdout artifact" }
            }
            if (ownsStderr) cleanupCliResource("stderr after start failure", error) {
                check(stderrFile.delete() || !stderrFile.exists()) { "Unable to remove stderr artifact" }
            }
            if (ownsHandshake) cleanupCliResource("handshake after start failure", error) {
                check(handshakeFile.delete() || !handshakeFile.exists()) { "Unable to remove handshake" }
            }
            prootTmpDir?.let { directory ->
                cleanupCliResource("temporary directory after start failure", error) {
                    check(directory.deleteRecursively() || !directory.exists()) { "Unable to remove PRoot temporary directory" }
                }
            }
            throw error
        }
    }

    private fun acquireForegroundLease(args: StartCliProcessArgs): CliForegroundLease? {
        if (!args.backgroundLease) return null
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            ContextCompat.checkSelfPermission(
                context,
                android.Manifest.permission.POST_NOTIFICATIONS,
            ) != PackageManager.PERMISSION_GRANTED
        ) {
            Log.w(TAG, "CLI background lease skipped because notification permission is denied")
            return null
        }
        if (!NotificationManagerCompat.from(context).areNotificationsEnabled()) {
            Log.w(TAG, "CLI background lease skipped because notifications are disabled")
            return null
        }
        val target = ForegroundGuardian.CliNotificationTarget(
            jobId = args.jobId,
            attemptId = args.attemptId,
            runtimeGeneration = args.runtimeGeneration,
            displayLabel = args.displayLabel,
        )
        val tag = foregroundTag(args.jobId, args.attemptId)
        val generation = ForegroundGuardian.acquire(
            context = context,
            tag = tag,
            priority = ForegroundGuardian.PRIORITY_CLI,
            label = "VCP CLI",
            screenKeepOn = false,
            timeoutMs = args.timeoutMs,
            needsCpu = true,
            needsNetwork = false,
            kind = ForegroundGuardian.ConsumerKind.CLI_JOB,
            cliTarget = target,
        )
        val readiness = CountDownLatch(1)
        val accepted = AtomicBoolean(false)
        val failure = AtomicReference<String?>(null)
        try {
            ForegroundGuardian.awaitServiceReadiness(context, generation) { success, reason ->
                accepted.set(success)
                failure.set(reason)
                readiness.countDown()
            }
            val completed = readiness.await(FOREGROUND_READINESS_WAIT_MS, TimeUnit.MILLISECONDS)
            check(completed && accepted.get()) {
                failure.get() ?: "CLI foreground service readiness timed out"
            }
            return CliForegroundLease(tag, generation)
        } catch (error: Throwable) {
            // The caller does not own the lease until readiness succeeds, including when
            // host shutdown interrupts this wait or readiness registration itself fails.
            cleanupCliResource("foreground lease before readiness", error) {
                ForegroundGuardian.release(context, tag, generation)
            }
            if (error is InterruptedException) Thread.currentThread().interrupt()
            throw error
        }
    }

    private fun releaseForegroundLease(lease: CliForegroundLease) {
        ForegroundGuardian.release(context, lease.tag, lease.generation)
    }

    private fun inspectBlocking(args: InspectCliProcessArgs): JSObject {
        validateIdentifier(args.operationId, "operationId")
        validateIdentifier(args.jobId, "jobId")
        validateIdentifier(args.attemptId, "attemptId")
        val key = ProcessKey(args.jobId, args.attemptId, args.runtimeGeneration)
        val handle = handles[key]
        return JSObject().apply {
            put("operationId", args.operationId)
            put("jobId", args.jobId)
            put("attemptId", args.attemptId)
            put("runtimeGeneration", args.runtimeGeneration)
            if (handle == null) {
                put("state", "missing")
                put("exitCode", org.json.JSONObject.NULL)
                put("stdoutBytes", 0L)
                put("stderrBytes", 0L)
                put("stdoutTruncated", false)
                put("stderrTruncated", false)
                put("backgroundLeaseLost", false)
            } else {
                put("state", if (handle.isCompleted()) "exited" else "running")
                put("exitCode", handle.exitCode.get() ?: org.json.JSONObject.NULL)
                put("stdoutBytes", handle.artifactBudget.stdoutBytes.get())
                put("stderrBytes", handle.artifactBudget.stderrBytes.get())
                put("stdoutTruncated", handle.artifactBudget.stdoutTruncated.get())
                put("stderrTruncated", handle.artifactBudget.stderrTruncated.get())
                put("backgroundLeaseLost", handle.backgroundLeaseLost.get())
            }
        }
    }

    fun handleForegroundLeaseLoss(targets: List<ForegroundGuardian.CliNotificationTarget>) {
        // Termination decisions belong to the Rust Runtime. Lease loss is published as a
        // process fact through inspectCliProcess; the Runtime drives containment via cancel.
        targets.forEach { target ->
            val key = ProcessKey(target.jobId, target.attemptId, target.runtimeGeneration)
            val handle = handles[key] ?: return@forEach
            handle.backgroundLeaseLost.set(true)
        }
    }

    private fun cancelBlocking(args: CancelCliProcessArgs): JSObject {
        validateIdentifier(args.operationId, "operationId")
        validateIdentifier(args.jobId, "jobId")
        validateIdentifier(args.attemptId, "attemptId")
        require(args.graceMs in 0..MAX_CANCEL_GRACE_MS) { "graceMs is outside the supported range" }
        val key = ProcessKey(args.jobId, args.attemptId, args.runtimeGeneration)
        val handle = handles[key]
        if (handle == null) {
            return cancelResponse(args, found = false, termSent = false, killSent = false, groupGone = true, exitCode = null)
        }
        if (handle.isCompleted() && !OwnedProcessGroup.isAlive(handle.identity.pgid)) {
            return cancelResponse(
                args,
                found = true,
                termSent = false,
                killSent = false,
                groupGone = true,
                exitCode = handle.exitCode.get(),
            )
        }

        synchronized(handle.terminationLock) {
            if (handle.isCompleted() && !OwnedProcessGroup.isAlive(handle.identity.pgid)) {
                return cancelResponse(
                    args,
                    found = true,
                    termSent = false,
                    killSent = false,
                    groupGone = true,
                    exitCode = handle.exitCode.get(),
                )
            }
            val termination = OwnedProcessGroup.terminate(handle.identity, args.graceMs)
            if (!termination.groupGone) {
                throw IllegalStateException("CLI process group remained alive after SIGKILL")
            }
            completeHandleAfterGroupGone(handle, THREAD_JOIN_MS)
            return cancelResponse(
                args,
                found = true,
                termSent = termination.termSent,
                killSent = termination.killSent,
                groupGone = true,
                exitCode = handle.exitCode.get(),
            )
        }
    }

    private fun cancelResponse(
        args: CancelCliProcessArgs,
        found: Boolean,
        termSent: Boolean,
        killSent: Boolean,
        groupGone: Boolean,
        exitCode: Int?,
    ) = JSObject().apply {
        put("operationId", args.operationId)
        put("jobId", args.jobId)
        put("attemptId", args.attemptId)
        put("runtimeGeneration", args.runtimeGeneration)
        put("found", found)
        put("termSent", termSent)
        put("killSent", killSent)
        put("groupGone", groupGone)
        put("exitCode", exitCode ?: org.json.JSONObject.NULL)
    }

    private fun startWaiter(handle: CliProcessHandle): Thread = Thread({
        val code = try {
            handle.process.waitFor()
        } catch (_: InterruptedException) {
            null
        }
        if (code != null) handle.exitCode.compareAndSet(null, code)
        val termination = runCatching { OwnedProcessGroup.terminate(handle.identity, ORPHAN_GRACE_MS) }.getOrNull()
        if (termination?.groupGone == true && drainReadersForTerminal(handle)) {
            finalizeHandle(handle)
        }
    }, "vcp-cli-wait-${safeFileStem(handle.key).take(12)}").apply {
        isDaemon = true
        start()
    }

    private fun waitForHandshakePid(handshake: File, process: Process): Int {
        val deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(HANDSHAKE_TIMEOUT_MS)
        while (System.nanoTime() < deadline) {
            if (handshake.isFile) {
                val pid = parseHandshakePidSnapshot(handshake.readBytes())
                if (pid != null) return pid
            }
            if (!process.isAlive) error("process exited before identity handshake")
            Thread.sleep(5)
        }
        error("process identity handshake timed out")
    }

    private fun finalizeHandle(handle: CliProcessHandle) {
        if (!handle.finalized.compareAndSet(false, true)) return
        // Callers have already established group disappearance and reader completion.
        // Ancillary resource failures cannot turn that terminal fact into an endless wait.
        try {
            cleanupCliResource("terminal output descriptors") { handle.artifactBudget.closeAll() }
            cleanupCliResource("terminal temporary directory") {
                check(handle.prootTmpDir.deleteRecursively() || !handle.prootTmpDir.exists()) {
                    "Unable to remove PRoot temporary directory"
                }
            }
            handle.foregroundLease?.let { lease ->
                cleanupCliResource("terminal foreground lease") { releaseForegroundLease(lease) }
            }
        } finally {
            handle.finished.countDown()
            completedKeys.offer(handle.key)
        }
    }

    private fun completeHandleAfterGroupGone(handle: CliProcessHandle, waitMs: Long) {
        if (handle.isCompleted()) return
        try {
            if (handle.process.waitFor(waitMs, TimeUnit.MILLISECONDS)) {
                handle.exitCode.compareAndSet(null, handle.process.exitValue())
            }
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
        }
        if (handle.process.isAlive) {
            handle.process.destroyForcibly()
            try {
                handle.process.waitFor(waitMs, TimeUnit.MILLISECONDS)
            } catch (_: InterruptedException) {
                Thread.currentThread().interrupt()
            }
        }
        try {
            if (!handle.finished.await(maxOf(waitMs, TERMINAL_DRAIN_WAIT_MS), TimeUnit.MILLISECONDS)) {
                throw IllegalStateException("CLI output drain did not reach terminal state")
            }
        } catch (error: InterruptedException) {
            Thread.currentThread().interrupt()
            throw IllegalStateException("interrupted while waiting for CLI output drain", error)
        }
    }

    private fun abortUnregisteredProcess(
        process: Process?,
        identity: ProcIdentity?,
        stdoutReader: Thread?,
        stderrReader: Thread?,
        budget: ArtifactBudget?,
        primaryError: Throwable,
    ) {
        if (identity != null) cleanupCliResource("unregistered process group", primaryError) {
            OwnedProcessGroup.abort(identity)
        }
        if (process != null) {
            cleanupCliResource("unregistered direct child", primaryError) { process.destroyForcibly() }
            cleanupCliResource("unregistered direct child wait", primaryError) {
                try {
                    process.waitFor(THREAD_JOIN_MS, TimeUnit.MILLISECONDS)
                } catch (error: InterruptedException) {
                    Thread.currentThread().interrupt()
                    throw error
                }
            }
            closeProcessStreams(process, primaryError)
        }
        if (stdoutReader != null) cleanupCliResource("unregistered stdout reader", primaryError) {
            joinThread(stdoutReader, THREAD_JOIN_MS)
        }
        if (stderrReader != null) cleanupCliResource("unregistered stderr reader", primaryError) {
            joinThread(stderrReader, THREAD_JOIN_MS)
        }
        if (budget != null) cleanupCliResource("unregistered output descriptors", primaryError) { budget.closeAll() }
    }

    private fun evictCompletedHandlesIfNeeded() {
        while (handles.size >= MAX_REMEMBERED_PROCESSES) {
            val key = completedKeys.poll() ?: return
            val completed = handles[key] ?: continue
            if (completed.isCompleted()) handles.remove(key, completed)
        }
    }

    private fun closeProcessStreams(process: Process, primaryError: Throwable? = null) {
        cleanupCliResource("stdin", primaryError) { process.outputStream.close() }
        cleanupCliResource("stdout pipe", primaryError) { process.inputStream.close() }
        cleanupCliResource("stderr pipe", primaryError) { process.errorStream.close() }
    }

    private fun joinThread(thread: Thread, waitMs: Long) {
        if (thread === Thread.currentThread()) return
        try {
            thread.join(waitMs)
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
        }
    }

    private fun drainReadersForTerminal(handle: CliProcessHandle): Boolean {
        joinThread(handle.stdoutReader, THREAD_JOIN_MS)
        joinThread(handle.stderrReader, THREAD_JOIN_MS)
        if (!handle.stdoutReader.isAlive && !handle.stderrReader.isAlive) return true

        // A tracee that retained a pipe must never make terminal publication
        // unbounded. Fail closed, mark the affected artifact truncated, close
        // our read end, and still require both drain threads to join.
        if (handle.stdoutReader.isAlive) handle.artifactBudget.abandonStream(ArtifactStream.STDOUT)
        if (handle.stderrReader.isAlive) handle.artifactBudget.abandonStream(ArtifactStream.STDERR)
        closeProcessStreams(handle.process)
        joinThread(handle.stdoutReader, THREAD_JOIN_MS)
        joinThread(handle.stderrReader, THREAD_JOIN_MS)
        return !handle.stdoutReader.isAlive && !handle.stderrReader.isAlive
    }

    private fun ensureOpen() {
        check(!closed.get()) { "CLI ProcessHost is closed" }
    }

    override fun close() {
        if (!closed.compareAndSet(false, true)) return
        val abandoned = controlExecutor.shutdownNow() +
            inspectExecutor.shutdownNow() +
            cancelExecutor.shutdownNow()
        abandoned.forEach { request ->
            try {
                (request as? CliPendingRequest)?.rejectBeforeStart("CLI ProcessHost is closed")
            } catch (error: Throwable) {
                Log.w(TAG, "Unable to deliver queued CLI request shutdown", error)
            }
        }
        synchronized(lifecycleLock) {
            handles.values.forEach { handle ->
                cleanupCliResource("process group during host close") {
                    val termination = OwnedProcessGroup.terminate(handle.identity, graceMs = 0)
                    if (termination.groupGone) completeHandleAfterGroupGone(handle, THREAD_JOIN_MS)
                }
                cleanupCliResource("direct child during host close") { handle.process.destroyForcibly() }
                closeProcessStreams(handle.process)
                cleanupCliResource("stdout reader during host close") { joinThread(handle.stdoutReader, THREAD_JOIN_MS) }
                cleanupCliResource("stderr reader during host close") { joinThread(handle.stderrReader, THREAD_JOIN_MS) }
                cleanupCliResource("waiter during host close") { joinThread(handle.waiter, THREAD_JOIN_MS) }
                cleanupCliResource("output descriptors during host close") { handle.artifactBudget.closeAll() }
                handle.foregroundLease?.let { lease ->
                    cleanupCliResource("foreground lease during host close") { releaseForegroundLease(lease) }
                }
            }
            handles.clear()
            completedKeys.clear()
        }
        try {
            controlExecutor.awaitTermination(THREAD_JOIN_MS, TimeUnit.MILLISECONDS)
            inspectExecutor.awaitTermination(THREAD_JOIN_MS, TimeUnit.MILLISECONDS)
            cancelExecutor.awaitTermination(THREAD_JOIN_MS, TimeUnit.MILLISECONDS)
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
        }
    }
}

internal object CliProcessHostOwner {
    @Volatile
    private var instance: CliProcessHost? = null

    fun get(context: Context): CliProcessHost = instance ?: synchronized(this) {
        instance ?: CliProcessHost(context.applicationContext).also { host ->
            ForegroundGuardian.setCliLeaseLossListener(host::handleForegroundLeaseLoss)
            instance = host
        }
    }
}
