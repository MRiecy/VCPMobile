package com.vcp.mobile.cli

import java.io.File
import java.nio.charset.StandardCharsets
import java.security.MessageDigest

// Pure launch construction shared by background jobs and the interactive PTY.

private const val GUEST_PATH = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

// The command is never interpolated into this host script. It is passed after the
// handshake path as an argv element and reaches Bash as the argument following -lc.
internal const val HOST_HANDSHAKE_SCRIPT =
    "umask 077; printf '%s\\n' \"\$\$\" > \"\$1\"; " +
        "memory_kib=\$2; shift 2; " +
        "ulimit -v \"\$memory_kib\" || exit 126; " +
        "IFS= read -r ready; [ \"\$ready\" = GO ] || exit 125; exec \"\$@\""

internal data class CliPtyLaunchSpec(
    val runtimeGeneration: Long,
    val argv: List<String>,
    val environment: Map<String, String>,
    val prootTmpDir: File,
)

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
    sessionId: String? = null,
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
    add("VCP_CLI_WORKSPACE=/workspace")
    if (sessionId != null) add("VCP_CLI_SESSION_ID=$sessionId")
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
    add("VCP_CLI_WORKSPACE=/workspace")
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
    MessageDigest.getInstance("SHA-256").digest(bytes).toCliHex()

internal fun ByteArray.toCliHex(): String = joinToString(separator = "") { byte -> "%02x".format(byte) }

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
        args.sessionId ?: "",
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
    return digest.digest().toCliHex()
}
