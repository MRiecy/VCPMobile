package com.vcp.mobile.cli

import android.content.Context
import android.system.Os
import android.system.OsConstants
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.InputStream
import java.nio.file.LinkOption
import java.nio.file.StandardCopyOption
import java.nio.file.attribute.BasicFileAttributes
import java.security.MessageDigest
import java.util.UUID

// Verifies immutable packaged inputs and creates the private runtime directories.
// The prepared runtime is a value; CliProcessHost alone installs/replaces the active value.

private const val ROOTFS_ASSET_NAME = "vcp-cli-rootfs-3.24.1-aarch64.tar.zst"
private const val PROOT_LIBRARY_NAME = "libvcp_proot.so"
private const val PROOT_LOADER_LIBRARY_NAME = "libvcp_proot_loader.so"
private const val MAX_ROOTFS_ARCHIVE_BYTES = 512L * 1024L * 1024L
private const val MAX_PROOT_BYTES = 64L * 1024L * 1024L
private const val MAX_PROOT_LOADER_BYTES = 4L * 1024L * 1024L

private val LEGACY_SEMANTIC_ASSET_NAMES = listOf(
    "vcp-semantic-model-r2.safetensors",
    "vcp-semantic-tokenizer-r2.vcpbpe",
)

internal data class PreparedRuntime(
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
    return total to digest.digest().toCliHex()
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
        require(digest.digest().toCliHex() == expectedHash) {
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

internal class CliRuntimeInstaller(private val context: Context) {
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

internal fun File.ensureDirectory(mode: Int? = null): File {
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
