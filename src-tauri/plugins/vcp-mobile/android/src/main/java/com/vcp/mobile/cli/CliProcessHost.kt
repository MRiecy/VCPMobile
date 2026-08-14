package com.vcp.mobile.cli

import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.Process as AndroidProcess
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import android.util.Log
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.JSObject
import com.vcp.mobile.service.ForegroundGuardian
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.InputStream
import java.io.OutputStream
import java.nio.charset.StandardCharsets
import java.nio.file.LinkOption
import java.nio.file.StandardCopyOption
import java.nio.file.attribute.BasicFileAttributes
import java.security.MessageDigest
import java.util.UUID
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.CountDownLatch
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference
import kotlin.math.min

private const val ROOTFS_ASSET_NAME = "vcp-cli-rootfs-3.24.1-aarch64.tar.zst"
private const val TAG = "CliProcessHost"
private const val PROOT_LIBRARY_NAME = "libvcp_proot.so"
private const val PROOT_LOADER_LIBRARY_NAME = "libvcp_proot_loader.so"
private const val MAX_ROOTFS_ARCHIVE_BYTES = 512L * 1024L * 1024L
private const val MAX_PROOT_BYTES = 64L * 1024L * 1024L
private const val MAX_PROOT_LOADER_BYTES = 4L * 1024L * 1024L
private const val MAX_ARTIFACT_BYTES = 256L * 1024L * 1024L
private const val MAX_COMMAND_BYTES = 64 * 1024
private const val MAX_ACTIVE_PROCESSES = 4
private const val MAX_REMEMBERED_PROCESSES = 1024
private const val MAX_CANCEL_GRACE_MS = 5_000L
private const val HANDSHAKE_TIMEOUT_MS = 3_000L
private const val GROUP_KILL_WAIT_MS = 2_000L
private const val PROOT_TRACEES_QUIT_GRACE_MS = 1_000L
private const val ORPHAN_GRACE_MS = 250L
private const val THREAD_JOIN_MS = 2_000L
private const val TERMINAL_DRAIN_WAIT_MS = 10_000L
private const val FOREGROUND_READINESS_WAIT_MS = 6_000L
// Must stay above ~2.1 GiB: bionic's dynamic linker reserves a ~2 GiB CFI shadow
// (MAP_NORESERVE, no committed RAM) when exec'ing the host PRoot PIE. A smaller
// RLIMIT_AS makes the linker abort with linker_cfi.cpp "MapShadow" failure (code 134).
private const val PROCESS_ADDRESS_SPACE_KIB = 4L * 1024L * 1024L
private const val GUEST_PATH = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
private val LEGACY_SEMANTIC_ASSET_NAMES = listOf(
    "vcp-semantic-model-r2.safetensors",
    "vcp-semantic-tokenizer-r2.vcpbpe",
)

// The command is never interpolated into this host script. It is passed after the
// handshake path as an argv element and reaches Bash as the argument following -lc.
internal const val HOST_HANDSHAKE_SCRIPT =
    "umask 077; printf '%s\\n' \"\$\$\" > \"\$1\"; " +
        "memory_kib=\$2; shift 2; " +
        "ulimit -v \"\$memory_kib\" || exit 126; " +
        "IFS= read -r ready; [ \"\$ready\" = GO ] || exit 125; exec \"\$@\""

@InvokeArg
class PrepareCliRuntimeArgs {
    lateinit var operationId: String
    lateinit var profileId: String
    var runtimeGeneration: Long = 0
    var rootfsArchiveBytes: Long = 0
    lateinit var rootfsArchiveSha256: String
    var prootBytes: Long = 0
    lateinit var prootSha256: String
    var prootLoaderBytes: Long = 0
    lateinit var prootLoaderSha256: String
}

@InvokeArg
class StartCliProcessArgs {
    lateinit var operationId: String
    lateinit var jobId: String
    lateinit var attemptId: String
    var runtimeGeneration: Long = 0
    lateinit var command: String
    lateinit var rootfsPath: String
    lateinit var cwd: String
    var artifactMaxBytes: Long = 0
    var backgroundLease: Boolean = false
    var timeoutMs: Long = 0
    lateinit var displayLabel: String
}

@InvokeArg
class InspectCliProcessArgs {
    lateinit var operationId: String
    lateinit var jobId: String
    lateinit var attemptId: String
    var runtimeGeneration: Long = 0
}

@InvokeArg
class CancelCliProcessArgs {
    lateinit var operationId: String
    lateinit var jobId: String
    lateinit var attemptId: String
    var runtimeGeneration: Long = 0
    var graceMs: Long = 0
}

private data class ProcessKey(
    val jobId: String,
    val attemptId: String,
    val runtimeGeneration: Long,
)

private data class GroupTermination(
    val termSent: Boolean,
    val killSent: Boolean,
    val groupGone: Boolean,
)

private data class CliForegroundLease(
    val tag: String,
    val generation: Long,
)

internal data class ProcIdentity(
    val pid: Int,
    val pgid: Int,
    val sessionId: Int,
    val startTimeTicks: Long,
    val uid: Int,
)

private data class PreparedRuntime(
    val profileId: String,
    val runtimeGeneration: Long,
    val archive: File,
    val rootfsParent: File,
    val workspace: File,
    val skills: File,
    val output: File,
    val projectionRoot: File,
    val proot: File,
    val prootLoader: File,
    val prootTmpParent: File,
    val handshakeParent: File,
)

internal data class CliPtyLaunchSpec(
    val runtimeGeneration: Long,
    val argv: List<String>,
    val environment: Map<String, String>,
    val prootTmpDir: File,
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
    val backgroundLeaseLossPending = AtomicBoolean(false)
    val backgroundLeaseLost = AtomicBoolean(false)
    val terminationLock = Any()
    lateinit var waiter: Thread

    fun isCompleted(): Boolean = finished.count == 0L
}

internal enum class ArtifactStream {
    STDOUT,
    STDERR,
}

/**
 * A shared cap across stdout and stderr. Writes stop at the cap, but the caller
 * keeps draining the child pipe so a verbose process cannot deadlock or grow RAM.
 */
internal class ArtifactBudget(
    private val maxBytes: Long,
    stdoutFile: File,
    stderrFile: File,
) {
    private val lock = Any()
    private val stdout: FileOutputStream
    private val stderr: FileOutputStream
    private var stdoutOpen = true
    private var stderrOpen = true
    private var totalBytes = 0L

    val stdoutBytes = AtomicLong(0)
    val stderrBytes = AtomicLong(0)
    val stdoutTruncated = AtomicBoolean(false)
    val stderrTruncated = AtomicBoolean(false)

    init {
        require(maxBytes in 0..MAX_ARTIFACT_BYTES) { "artifactMaxBytes is outside the supported range" }
        val stdoutStream = FileOutputStream(stdoutFile, false)
        try {
            val stderrStream = FileOutputStream(stderrFile, false)
            stdout = stdoutStream
            stderr = stderrStream
        } catch (error: Throwable) {
            try {
                stdoutStream.close()
            } catch (_: Exception) {}
            throw error
        }
    }

    fun write(stream: ArtifactStream, bytes: ByteArray, count: Int) {
        if (count <= 0) return
        synchronized(lock) {
            val output = outputFor(stream)
            if (output == null) {
                markTruncated(stream)
                return
            }

            val allowed = min(count.toLong(), maxBytes - totalBytes).coerceAtLeast(0).toInt()
            if (allowed > 0) {
                try {
                    output.write(bytes, 0, allowed)
                    totalBytes += allowed
                    storedCounter(stream).addAndGet(allowed.toLong())
                } catch (_: Exception) {
                    markTruncated(stream)
                    closeStreamLocked(stream, sync = false)
                    return
                }
            }
            if (allowed < count) markTruncated(stream)
        }
    }

    fun closeStream(stream: ArtifactStream) {
        synchronized(lock) {
            closeStreamLocked(stream, sync = true)
        }
    }

    fun closeAll() {
        synchronized(lock) {
            closeStreamLocked(ArtifactStream.STDOUT, sync = true)
            closeStreamLocked(ArtifactStream.STDERR, sync = true)
        }
    }

    fun abandonStream(stream: ArtifactStream) {
        synchronized(lock) {
            markTruncated(stream)
            closeStreamLocked(stream, sync = true)
        }
    }

    private fun outputFor(stream: ArtifactStream): FileOutputStream? = when (stream) {
        ArtifactStream.STDOUT -> if (stdoutOpen) stdout else null
        ArtifactStream.STDERR -> if (stderrOpen) stderr else null
    }

    private fun storedCounter(stream: ArtifactStream) = when (stream) {
        ArtifactStream.STDOUT -> stdoutBytes
        ArtifactStream.STDERR -> stderrBytes
    }

    private fun markTruncated(stream: ArtifactStream) {
        when (stream) {
            ArtifactStream.STDOUT -> stdoutTruncated.set(true)
            ArtifactStream.STDERR -> stderrTruncated.set(true)
        }
    }

    private fun closeStreamLocked(stream: ArtifactStream, sync: Boolean) {
        val output = outputFor(stream) ?: return
        try {
            output.flush()
            if (sync) output.fd.sync()
        } catch (_: Exception) {
            markTruncated(stream)
        } finally {
            try {
                output.close()
            } catch (_: Exception) {
                markTruncated(stream)
            }
            when (stream) {
                ArtifactStream.STDOUT -> stdoutOpen = false
                ArtifactStream.STDERR -> stderrOpen = false
            }
        }
    }
}

internal fun parseProcStat(stat: String, uid: Int): ProcIdentity {
    val openParen = stat.indexOf('(')
    val closeParen = stat.lastIndexOf(')')
    require(openParen > 0 && closeParen > openParen) { "malformed /proc stat comm field" }
    val pid = stat.substring(0, openParen).trim().toInt()
    val fields = stat.substring(closeParen + 1).trim().split(Regex("\\s+"))
    require(fields.size > 19) { "incomplete /proc stat" }
    return ProcIdentity(
        pid = pid,
        pgid = fields[2].toInt(),
        sessionId = fields[3].toInt(),
        startTimeTicks = fields[19].toLong(),
        uid = uid,
    )
}

/**
 * Parses the PID only after the shell has published a complete newline-terminated
 * snapshot. Redirection creates the handshake file before `printf` writes into it,
 * so an empty or partial snapshot is a normal in-progress state rather than an
 * invalid identity.
 */
internal fun parseHandshakePidSnapshot(snapshot: ByteArray): Int? {
    if (snapshot.isEmpty() || snapshot.last() != '\n'.code.toByte()) return null

    val value = snapshot.toString(StandardCharsets.US_ASCII).dropLast(1)
    require(value.isNotEmpty() && value.all { it in '0'..'9' }) {
        "invalid process identity handshake"
    }
    val pid = value.toIntOrNull()
    require(pid != null && pid > 1) { "invalid process identity handshake" }
    return pid
}

internal fun validateGuestCwd(cwd: String): String {
    require(cwd.length <= 4096 && cwd.startsWith('/')) { "cwd must be a bounded guest absolute path" }
    require(!cwd.contains('\u0000') && !cwd.contains("//")) { "cwd contains an invalid path component" }
    val components = cwd.split('/').drop(1)
    require(components.none { it.isEmpty() || it == "." || it == ".." }) {
        "cwd contains an invalid path component"
    }
    require(cwd == "/workspace" || cwd.startsWith("/workspace/")) {
        "cwd must remain inside /workspace"
    }
    return cwd
}

internal fun buildProotArguments(
    prootPath: String,
    rootfsPath: String,
    workspacePath: String,
    cwd: String,
    command: String,
): List<String> = buildList {
    add(prootPath)
    add("-0")
    add("--kill-on-exit")
    add("--link2symlink")
    add("-r")
    add(rootfsPath)
    add("-b")
    add("/dev")
    add("-b")
    add("/proc")
    add("-b")
    add("$workspacePath:/workspace")
    add("-w")
    add(validateGuestCwd(cwd))
    add("/usr/bin/env")
    add("-i")
    add("HOME=/root")
    add("USER=root")
    add("LOGNAME=root")
    add("SHELL=/bin/bash")
    add("PATH=$GUEST_PATH")
    add("TMPDIR=/tmp")
    add("TERM=dumb")
    add("/bin/bash")
    add("-lc")
    add(command)
}

internal fun buildProotTerminalArguments(
    prootPath: String,
    rootfsPath: String,
    workspacePath: String,
    cwd: String,
): List<String> = buildList {
    add(prootPath)
    add("-0")
    add("--kill-on-exit")
    add("--link2symlink")
    add("-r")
    add(rootfsPath)
    add("-b")
    add("/dev")
    add("-b")
    add("/proc")
    add("-b")
    add("$workspacePath:/workspace")
    add("-w")
    add(validateGuestCwd(cwd))
    add("/usr/bin/env")
    add("-i")
    add("HOME=/root")
    add("USER=root")
    add("LOGNAME=root")
    add("SHELL=/bin/bash")
    add("PATH=$GUEST_PATH")
    add("TMPDIR=/tmp")
    add("TERM=xterm-256color")
    add("COLORTERM=truecolor")
    add("/bin/bash")
    add("-l")
}

internal fun buildHostEnvironment(prootTmpPath: String, prootLoaderPath: String): Map<String, String> {
    require(prootTmpPath.startsWith('/') && !prootTmpPath.contains('\u0000')) {
        "PROOT_TMP_DIR must be an absolute private path"
    }
    require(prootLoaderPath.startsWith('/') && !prootLoaderPath.contains('\u0000')) {
        "PROOT_LOADER must be an absolute APK-native path"
    }
    return linkedMapOf(
        "PATH" to "/system/bin:/system/xbin",
        "TMPDIR" to prootTmpPath,
        "PROOT_TMP_DIR" to prootTmpPath,
        "PROOT_LOADER" to prootLoaderPath,
    )
}

internal fun buildHostCommand(
    handshakePath: String,
    processAddressSpaceKib: Long,
    prootArguments: List<String>,
): List<String> =
    listOf(
        "/system/bin/toybox",
        "setsid",
        "/system/bin/sh",
        "-c",
        HOST_HANDSHAKE_SCRIPT,
        "vcp-cli-host",
        handshakePath,
        processAddressSpaceKib.toString(),
    ) + prootArguments

internal fun sha256Hex(bytes: ByteArray): String =
    MessageDigest.getInstance("SHA-256").digest(bytes).toHex()

private fun ByteArray.toHex(): String = joinToString(separator = "") { byte -> "%02x".format(byte) }

private fun validateSha256(value: String, field: String): String {
    require(value.length == 64 && value.all { it in '0'..'9' || it in 'a'..'f' || it in 'A'..'F' }) {
        "$field must be a SHA-256 hex digest"
    }
    return value.lowercase()
}

private fun sha256File(file: File, maxBytes: Long): Pair<Long, String> {
    require(file.isFile) { "runtime file is missing: ${file.name}" }
    val digest = MessageDigest.getInstance("SHA-256")
    var total = 0L
    FileInputStream(file).use { input ->
        val buffer = ByteArray(64 * 1024)
        while (true) {
            val read = input.read(buffer)
            if (read < 0) break
            total += read
            require(total <= maxBytes) { "runtime file exceeds its size budget: ${file.name}" }
            digest.update(buffer, 0, read)
        }
    }
    return total to digest.digest().toHex()
}

internal fun verifyNativeExecutable(
    nativeLibraryDirectory: File,
    libraryName: String,
    expectedBytes: Long,
    expectedSha256: String,
): File {
    require(libraryName.isNotEmpty() && libraryName == File(libraryName).name) {
        "native executable name must be a leaf"
    }
    val directory = nativeLibraryDirectory.canonicalFile
    val candidatePath = File(nativeLibraryDirectory, libraryName).toPath()
    val attributes = java.nio.file.Files.readAttributes(
        candidatePath,
        BasicFileAttributes::class.java,
        LinkOption.NOFOLLOW_LINKS,
    )
    require(attributes.isRegularFile && !attributes.isSymbolicLink) {
        "native executable must be a real regular file: $libraryName"
    }
    val candidate = candidatePath.toFile().canonicalFile
    require(candidate.parentFile == directory) {
        "native executable escaped nativeLibraryDir: $libraryName"
    }
    require(attributes.size() == expectedBytes) {
        "native executable size does not match the frozen profile: $libraryName"
    }
    val (actualBytes, actualHash) = sha256File(candidate, expectedBytes)
    require(actualBytes == expectedBytes && actualHash == validateSha256(expectedSha256, libraryName)) {
        "native executable SHA-256 does not match the frozen profile: $libraryName"
    }
    require(candidate.canExecute()) { "native executable is not executable: $libraryName" }
    return candidate
}

internal fun copyVerifiedStreamAtomically(
    openInput: () -> InputStream,
    destination: File,
    expectedBytes: Long,
    expectedSha256: String,
    syncParent: (File) -> Unit = {},
) {
    require(expectedBytes in 1..MAX_ROOTFS_ARCHIVE_BYTES) { "rootfsArchiveBytes is outside the supported range" }
    val expectedHash = validateSha256(expectedSha256, "rootfsArchiveSha256")
    val parent = destination.parentFile ?: error("archive destination has no parent")
    require(parent.isDirectory || parent.mkdirs()) { "failed to create archive directory" }
    val staging = File(parent, ".${destination.name}.install-${UUID.randomUUID()}")
    try {
        val digest = MessageDigest.getInstance("SHA-256")
        var total = 0L
        openInput().use { input ->
            FileOutputStream(staging, false).use { output ->
                val buffer = ByteArray(64 * 1024)
                while (true) {
                    val read = input.read(buffer)
                    if (read < 0) break
                    total += read
                    require(total <= expectedBytes) { "rootfs archive is larger than the frozen profile" }
                    digest.update(buffer, 0, read)
                    output.write(buffer, 0, read)
                }
                output.flush()
                output.fd.sync()
            }
        }
        require(total == expectedBytes) { "rootfs archive size does not match the frozen profile" }
        require(digest.digest().toHex() == expectedHash) {
            "rootfs archive SHA-256 does not match the frozen profile"
        }
        java.nio.file.Files.move(
            staging.toPath(),
            destination.toPath(),
            StandardCopyOption.ATOMIC_MOVE,
            StandardCopyOption.REPLACE_EXISTING,
        )
        syncParent(parent)
    } finally {
        if (staging.exists()) staging.delete()
    }
}

private class CliRuntimeInstaller(private val context: Context) {
    fun prepare(args: PrepareCliRuntimeArgs): PreparedRuntime {
        validateIdentifier(args.profileId, "profileId")
        require(args.runtimeGeneration > 0) { "runtimeGeneration must be positive" }
        require(args.rootfsArchiveBytes in 1..MAX_ROOTFS_ARCHIVE_BYTES) {
            "rootfsArchiveBytes is outside the supported range"
        }
        require(args.prootBytes in 1..MAX_PROOT_BYTES) { "prootBytes is outside the supported range" }
        require(args.prootLoaderBytes in 1..MAX_PROOT_LOADER_BYTES) {
            "prootLoaderBytes is outside the supported range"
        }
        val archiveHash = validateSha256(args.rootfsArchiveSha256, "rootfsArchiveSha256")
        val prootHash = validateSha256(args.prootSha256, "prootSha256")
        val prootLoaderHash = validateSha256(args.prootLoaderSha256, "prootLoaderSha256")

        val privateRoot = File(context.noBackupFilesDir, "vcp-cli").ensureDirectory()
        val archiveDirectory = File(privateRoot, "assets").ensureDirectory()
        cleanupLegacySemanticAssets(archiveDirectory)
        val archive = File(archiveDirectory, ROOTFS_ASSET_NAME)
        val currentArchive = if (archive.exists()) {
            runCatching { sha256File(archive, args.rootfsArchiveBytes) }.getOrNull()
        } else {
            null
        }
        if (currentArchive?.first != args.rootfsArchiveBytes || currentArchive.secondOrNull() != archiveHash) {
            copyVerifiedStreamAtomically(
                openInput = { context.assets.open(ROOTFS_ASSET_NAME) },
                destination = archive,
                expectedBytes = args.rootfsArchiveBytes,
                expectedSha256 = archiveHash,
                syncParent = ::fsyncDirectory,
            )
        }

        val nativeLibraryDirectory = File(context.applicationInfo.nativeLibraryDir)
        val proot = verifyNativeExecutable(
            nativeLibraryDirectory,
            PROOT_LIBRARY_NAME,
            args.prootBytes,
            prootHash,
        )
        val prootLoader = verifyNativeExecutable(
            nativeLibraryDirectory,
            PROOT_LOADER_LIBRARY_NAME,
            args.prootLoaderBytes,
            prootLoaderHash,
        )
        require(prootLoader != proot) { "PRoot loader must be a separate APK-native executable" }

        val rootfsParent = File(privateRoot, "rootfs").ensureDirectory()
        val output = File(privateRoot, "output").ensureDirectory()
        val projectionRoot = File(privateRoot, "projections").ensureDirectory(mode = 0x1c0)
        val prootTmpParent = File(privateRoot, "proot-tmp").ensureDirectory(mode = 0x1c0)
        val handshakeParent = File(privateRoot, "handshakes").ensureDirectory(mode = 0x1c0)
        val workspace = File(context.filesDir, "vcp-cli/workspace").ensureDirectory()
        val skills = File(context.filesDir, "vcp-cli/skills").ensureDirectory()

        return PreparedRuntime(
            profileId = args.profileId,
            runtimeGeneration = args.runtimeGeneration,
            archive = archive.canonicalFile,
            rootfsParent = rootfsParent.canonicalFile,
            workspace = workspace.canonicalFile,
            skills = skills.canonicalFile,
            output = output.canonicalFile,
            projectionRoot = projectionRoot.canonicalFile,
            proot = proot,
            prootLoader = prootLoader,
            prootTmpParent = prootTmpParent.canonicalFile,
            handshakeParent = handshakeParent.canonicalFile,
        )
    }

}

private fun fsyncDirectory(directory: File) {
    val descriptor = Os.open(
        directory.absolutePath,
        OsConstants.O_RDONLY or OsConstants.O_CLOEXEC,
        0,
    )
    try {
        Os.fsync(descriptor)
    } finally {
        Os.close(descriptor)
    }
}

private fun Pair<Long, String>?.secondOrNull(): String? = this?.second

private fun File.ensureDirectory(mode: Int? = null): File {
    require(isDirectory || mkdirs()) { "failed to create private runtime directory: $name" }
    if (mode != null) Os.chmod(absolutePath, mode)
    require(isDirectory && !java.nio.file.Files.isSymbolicLink(toPath())) {
        "private runtime directory is not a real directory: $name"
    }
    return this
}

private fun cleanupLegacySemanticAssets(assets: File) {
    runCatching {
        if (!assets.isDirectory || java.nio.file.Files.isSymbolicLink(assets.toPath())) return
        LEGACY_SEMANTIC_ASSET_NAMES.forEach { name ->
            java.nio.file.Files.deleteIfExists(File(assets, name).toPath())
        }
    }
}

internal fun validateIdentifier(value: String, field: String) {
    require(value.isNotBlank() && value.toByteArray(StandardCharsets.UTF_8).size <= 256) {
        "$field must be a non-empty bounded identifier"
    }
    require(!value.contains('\u0000')) { "$field contains NUL" }
}

internal fun fingerprint(args: StartCliProcessArgs): String {
    val digest = MessageDigest.getInstance("SHA-256")
    val fields = mutableListOf(
        args.jobId,
        args.attemptId,
        args.runtimeGeneration.toString(),
        args.command,
        args.rootfsPath,
        args.cwd,
        args.artifactMaxBytes.toString(),
        args.backgroundLease.toString(),
        args.timeoutMs.toString(),
        args.displayLabel,
    )
    fields.forEach { value ->
        val bytes = value.toByteArray(StandardCharsets.UTF_8)
        digest.update(byteArrayOf(
            (bytes.size ushr 24).toByte(),
            (bytes.size ushr 16).toByte(),
            (bytes.size ushr 8).toByte(),
            bytes.size.toByte(),
        ))
        digest.update(bytes)
    }
    return digest.digest().toHex()
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
        if (closed.get()) {
            failure("CLI ProcessHost is closed")
            return
        }
        try {
            executor.execute {
                try {
                    success(action())
                } catch (error: Throwable) {
                    failure(error.message ?: error.javaClass.simpleName)
                }
            }
        } catch (_: RejectedExecutionException) {
            failure("CLI ProcessHost $queueName queue is unavailable")
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
        try {
            require(stdoutFile.createNewFile()) { "stdout artifact identity already exists" }
            require(stderrFile.createNewFile()) { "stderr artifact identity already exists" }
            if (handshakeFile.exists()) require(handshakeFile.delete()) { "stale handshake cannot be removed" }
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
            stdoutReader = startDrainThread(
                name = "vcp-cli-stdout-${stem.take(12)}",
                input = process.inputStream,
                stream = ArtifactStream.STDOUT,
                budget = activeBudget,
            )
            stderrReader = startDrainThread(
                name = "vcp-cli-stderr-${stem.take(12)}",
                input = process.errorStream,
                stream = ArtifactStream.STDERR,
                budget = activeBudget,
            )

            val pid = waitForHandshakePid(handshakeFile, process)
            val identity = readProcessIdentity(pid) ?: error("process leader disappeared during identity handshake")
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
            foregroundLease?.let(::releaseForegroundLease)
            abortUnregisteredProcess(process, verifiedIdentity, stdoutReader, stderrReader, budget)
            stdoutFile.delete()
            stderrFile.delete()
            handshakeFile.delete()
            prootTmpDir?.deleteRecursively()
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
        ForegroundGuardian.awaitServiceReadiness(context, generation) { success, reason ->
            accepted.set(success)
            failure.set(reason)
            readiness.countDown()
        }
        val completed = readiness.await(FOREGROUND_READINESS_WAIT_MS, TimeUnit.MILLISECONDS)
        if (!completed || !accepted.get()) {
            ForegroundGuardian.release(context, tag, generation)
            error(failure.get() ?: "CLI foreground service readiness timed out")
        }
        return CliForegroundLease(tag, generation)
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
        targets.forEach { target ->
            val key = ProcessKey(target.jobId, target.attemptId, target.runtimeGeneration)
            val handle = handles[key] ?: return@forEach
            if (!handle.backgroundLeaseLossPending.compareAndSet(false, true)) return@forEach
            try {
                cancelExecutor.execute {
                    synchronized(handle.terminationLock) {
                        try {
                            val termination = terminateOwnedGroup(handle, graceMs = 0)
                            if (!termination.groupGone) {
                                throw IllegalStateException(
                                    "CLI process group remained alive after foreground lease loss",
                                )
                            }
                            completeHandleAfterGroupGone(handle, THREAD_JOIN_MS)
                            handle.backgroundLeaseLost.set(true)
                        } catch (error: Throwable) {
                            Log.e(TAG, "Failed to contain CLI Job after foreground lease loss", error)
                            handle.process.destroyForcibly()
                        }
                    }
                }
            } catch (error: RejectedExecutionException) {
                Log.e(TAG, "CLI cancel queue rejected foreground lease loss containment", error)
                handle.process.destroyForcibly()
            }
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
        if (handle.isCompleted() && !isGroupAlive(handle.identity.pgid)) {
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
            if (handle.isCompleted() && !isGroupAlive(handle.identity.pgid)) {
                return cancelResponse(
                    args,
                    found = true,
                    termSent = false,
                    killSent = false,
                    groupGone = true,
                    exitCode = handle.exitCode.get(),
                )
            }
            val termination = terminateOwnedGroup(handle, args.graceMs)
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

    private fun startDrainThread(
        name: String,
        input: InputStream,
        stream: ArtifactStream,
        budget: ArtifactBudget,
    ): Thread = Thread({
        try {
            input.use {
                val buffer = ByteArray(16 * 1024)
                while (true) {
                    val read = it.read(buffer)
                    if (read < 0) break
                    budget.write(stream, buffer, read)
                }
            }
        } catch (_: Exception) {
            // A close during cancellation is expected. The waiter still fences terminal state.
        } finally {
            budget.closeStream(stream)
        }
    }, name).apply {
        isDaemon = true
        start()
    }

    private fun startWaiter(handle: CliProcessHandle): Thread = Thread({
        val code = try {
            handle.process.waitFor()
        } catch (_: InterruptedException) {
            null
        }
        if (code != null) handle.exitCode.compareAndSet(null, code)
        val termination = runCatching { terminateOwnedGroup(handle, ORPHAN_GRACE_MS) }.getOrNull()
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

    private fun readProcessIdentity(pid: Int): ProcIdentity? = try {
        val procDirectory = File("/proc/$pid")
        val uid = Os.stat(procDirectory.absolutePath).st_uid
        val stat = File(procDirectory, "stat").readText(Charsets.US_ASCII)
        parseProcStat(stat, uid)
    } catch (_: Exception) {
        null
    }

    private fun canSignalOwnedGroup(handle: CliProcessHandle): Boolean {
        val current = readProcessIdentity(handle.identity.pid)
        if (current != null) {
            return current.pid == handle.identity.pid &&
                current.pgid == handle.identity.pgid &&
                current.sessionId == handle.identity.sessionId &&
                current.startTimeTicks == handle.identity.startTimeTicks &&
                current.uid == handle.identity.uid &&
                current.uid == AndroidProcess.myUid()
        }
        return groupHasOnlyVisibleAppUidMembers(handle.identity.pgid)
    }

    private fun groupHasOnlyVisibleAppUidMembers(pgid: Int): Boolean {
        var found = false
        val entries = File("/proc").listFiles() ?: return false
        for (entry in entries) {
            val pid = entry.name.toIntOrNull() ?: continue
            val identity = readProcessIdentity(pid) ?: continue
            if (identity.pgid == pgid) {
                if (identity.uid != AndroidProcess.myUid()) return false
                found = true
            }
        }
        return found
    }

    private fun signalGroup(pgid: Int, signal: Int): Boolean = try {
        Os.kill(-pgid, signal)
        true
    } catch (error: ErrnoException) {
        if (error.errno == OsConstants.ESRCH) false else throw error
    }

    private fun isGroupAlive(pgid: Int): Boolean = try {
        Os.kill(-pgid, 0)
        true
    } catch (error: ErrnoException) {
        when (error.errno) {
            OsConstants.ESRCH -> false
            OsConstants.EPERM -> true
            else -> throw error
        }
    }

    private fun signalOwnedLeader(handle: CliProcessHandle, signal: Int): Boolean {
        val current = readProcessIdentity(handle.identity.pid) ?: return false
        require(current == handle.identity && current.uid == AndroidProcess.myUid()) {
            "CLI process identity changed before signaling its PRoot leader"
        }
        return try {
            Os.kill(current.pid, signal)
            true
        } catch (error: ErrnoException) {
            if (error.errno == OsConstants.ESRCH) false else throw error
        }
    }

    private fun waitForGroupGone(pgid: Int, waitMs: Long): Boolean {
        val deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(waitMs)
        do {
            if (!isGroupAlive(pgid)) return true
            if (System.nanoTime() >= deadline) return false
            Thread.sleep(20)
        } while (true)
    }

    private fun terminateOwnedGroup(handle: CliProcessHandle, graceMs: Long): GroupTermination {
        if (!isGroupAlive(handle.identity.pgid)) {
            return GroupTermination(termSent = false, killSent = false, groupGone = true)
        }
        require(canSignalOwnedGroup(handle)) { "CLI process identity no longer matches its owned process group" }
        val termSent = signalGroup(handle.identity.pgid, OsConstants.SIGTERM)
        if (waitForGroupGone(handle.identity.pgid, graceMs)) {
            return GroupTermination(termSent = termSent, killSent = false, groupGone = true)
        }

        // PRoot handles SIGQUIT by stopping its remaining tracees. Give that
        // --kill-on-exit path a bounded chance to reap guest processes that
        // created a new session before the outer process group is force-killed.
        if (signalOwnedLeader(handle, OsConstants.SIGQUIT) &&
            waitForGroupGone(handle.identity.pgid, PROOT_TRACEES_QUIT_GRACE_MS)
        ) {
            return GroupTermination(termSent = termSent, killSent = false, groupGone = true)
        }
        require(canSignalOwnedGroup(handle)) { "CLI process group ownership changed before SIGKILL" }
        val killSent = signalGroup(handle.identity.pgid, OsConstants.SIGKILL)
        return GroupTermination(
            termSent = termSent,
            killSent = killSent,
            groupGone = waitForGroupGone(handle.identity.pgid, GROUP_KILL_WAIT_MS),
        )
    }

    private fun finalizeHandle(handle: CliProcessHandle) {
        if (!handle.finalized.compareAndSet(false, true)) return
        handle.artifactBudget.closeAll()
        handle.prootTmpDir.deleteRecursively()
        handle.foregroundLease?.let(::releaseForegroundLease)
        handle.finished.countDown()
        completedKeys.offer(handle.key)
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
    ) {
        if (identity != null && isGroupAlive(identity.pgid) && canSignalOwnedIdentity(identity)) {
            signalGroup(identity.pgid, OsConstants.SIGKILL)
            waitForGroupGone(identity.pgid, GROUP_KILL_WAIT_MS)
        }
        if (process != null) {
            process.destroyForcibly()
            try {
                process.waitFor(THREAD_JOIN_MS, TimeUnit.MILLISECONDS)
            } catch (_: InterruptedException) {
                Thread.currentThread().interrupt()
            }
            closeProcessStreams(process)
        }
        if (stdoutReader != null) joinThread(stdoutReader, THREAD_JOIN_MS)
        if (stderrReader != null) joinThread(stderrReader, THREAD_JOIN_MS)
        budget?.closeAll()
    }

    private fun canSignalOwnedIdentity(identity: ProcIdentity): Boolean {
        val current = readProcessIdentity(identity.pid)
        if (current != null) {
            return current.pid == identity.pid &&
                current.pgid == identity.pgid &&
                current.sessionId == identity.sessionId &&
                current.startTimeTicks == identity.startTimeTicks &&
                current.uid == identity.uid &&
                current.uid == AndroidProcess.myUid()
        }
        return groupHasOnlyVisibleAppUidMembers(identity.pgid)
    }

    private fun evictCompletedHandlesIfNeeded() {
        while (handles.size >= MAX_REMEMBERED_PROCESSES) {
            val key = completedKeys.poll() ?: return
            val completed = handles[key] ?: continue
            if (completed.isCompleted()) handles.remove(key, completed)
        }
    }

    private fun closeProcessStreams(process: Process) {
        try {
            process.outputStream.close()
        } catch (_: Exception) {}
        try {
            process.inputStream.close()
        } catch (_: Exception) {}
        try {
            process.errorStream.close()
        } catch (_: Exception) {}
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
        controlExecutor.shutdownNow()
        inspectExecutor.shutdownNow()
        cancelExecutor.shutdownNow()
        synchronized(lifecycleLock) {
            handles.values.forEach { handle ->
                try {
                    val termination = terminateOwnedGroup(handle, graceMs = 0)
                    if (termination.groupGone) completeHandleAfterGroupGone(handle, THREAD_JOIN_MS)
                } catch (_: Exception) {
                    // Direct-child cleanup below is still mandatory during teardown.
                }
                handle.process.destroyForcibly()
                closeProcessStreams(handle.process)
                joinThread(handle.stdoutReader, THREAD_JOIN_MS)
                joinThread(handle.stderrReader, THREAD_JOIN_MS)
                joinThread(handle.waiter, THREAD_JOIN_MS)
                handle.artifactBudget.closeAll()
                handle.foregroundLease?.let(::releaseForegroundLease)
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
