package com.vcp.mobile.cli

import com.fasterxml.jackson.databind.ObjectMapper
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.File
import java.nio.file.Files
import java.security.MessageDigest

class CliProcessHostTest {
    @Test
    fun handshakePidSnapshotWaitsForNewlineAndRejectsCompleteInvalidIdentity() {
        assertEquals(null, parseHandshakePidSnapshot(byteArrayOf()))
        assertEquals(null, parseHandshakePidSnapshot("4242".toByteArray()))
        assertEquals(4242, parseHandshakePidSnapshot("4242\n".toByteArray()))

        listOf("\n", "1\n", "not-a-pid\n", "4242\nextra\n", "999999999999999999999\n").forEach { snapshot ->
            val error = assertThrows(IllegalArgumentException::class.java) {
                parseHandshakePidSnapshot(snapshot.toByteArray())
            }
            assertEquals("invalid process identity handshake", error.message)
        }
    }

    @Test
    fun nativeExecutableVerifierRejectsIdentityAndPathSubstitution() {
        val nativeDirectory = Files.createTempDirectory("vcp-cli-native-test").toFile()
        val siblingDirectory = Files.createTempDirectory("vcp-cli-native-sibling").toFile()
        val bytes = "frozen-apk-native-elf".toByteArray()
        val expectedHash = sha256Hex(bytes)
        val candidate = File(nativeDirectory, "libvcp_proot_loader.so")
        try {
            candidate.writeBytes(bytes)
            candidate.setExecutable(true, true)
            assertEquals(
                candidate.canonicalFile,
                verifyNativeExecutable(
                    nativeDirectory,
                    candidate.name,
                    bytes.size.toLong(),
                    expectedHash,
                ),
            )

            assertThrows(IllegalArgumentException::class.java) {
                verifyNativeExecutable(nativeDirectory, candidate.name, bytes.size.toLong(), "0".repeat(64))
            }
            assertThrows(IllegalArgumentException::class.java) {
                verifyNativeExecutable(nativeDirectory, candidate.name, bytes.size.toLong() + 1, expectedHash)
            }

            candidate.delete()
            candidate.mkdir()
            assertThrows(IllegalArgumentException::class.java) {
                verifyNativeExecutable(nativeDirectory, candidate.name, 0, expectedHash)
            }
            candidate.delete()

            val sibling = File(siblingDirectory, candidate.name).apply {
                writeBytes(bytes)
                setExecutable(true, true)
            }
            Files.createSymbolicLink(candidate.toPath(), sibling.toPath())
            assertThrows(IllegalArgumentException::class.java) {
                verifyNativeExecutable(nativeDirectory, candidate.name, bytes.size.toLong(), expectedHash)
            }
            candidate.delete()

            assertThrows(IllegalArgumentException::class.java) {
                verifyNativeExecutable(
                    nativeDirectory,
                    "../${siblingDirectory.name}/${candidate.name}",
                    bytes.size.toLong(),
                    expectedHash,
                )
            }
        } finally {
            nativeDirectory.deleteRecursively()
            siblingDirectory.deleteRecursively()
        }
    }

    @Test
    fun procStatParserHandlesSpacesAndParenthesesInComm() {
        val tail = mutableListOf(
            "S", "1", "4242", "4242", "0", "0", "0", "0", "0", "0",
            "0", "0", "0", "0", "0", "0", "0", "0", "0", "987654321",
        )
        val identity = parseProcStat("4242 (bash worker (cli)) ${tail.joinToString(" ")}", uid = 10555)

        assertEquals(4242, identity.pid)
        assertEquals(4242, identity.pgid)
        assertEquals(4242, identity.sessionId)
        assertEquals(987654321L, identity.startTimeTicks)
        assertEquals(10555, identity.uid)
    }

    @Test
    fun guestCwdCannotEscapeWorkspace() {
        assertEquals("/workspace", validateGuestCwd("/workspace"))
        assertEquals("/workspace/topic", validateGuestCwd("/workspace/topic"))
        assertThrows(IllegalArgumentException::class.java) { validateGuestCwd("/") }
        assertThrows(IllegalArgumentException::class.java) { validateGuestCwd("/root") }
        assertThrows(IllegalArgumentException::class.java) { validateGuestCwd("/workspace/../root") }
        assertThrows(IllegalArgumentException::class.java) { validateGuestCwd("/workspace//topic") }
    }

    @Test
    fun artifactBudgetSharesCapButContinuesAcceptingDiscardedDrain() {
        val root = Files.createTempDirectory("vcp-cli-artifact-test").toFile()
        val stdout = File(root, "stdout")
        val stderr = File(root, "stderr")
        try {
            val budget = ArtifactBudget(5, stdout, stderr)
            budget.write(ArtifactStream.STDOUT, "abcd".toByteArray(), 4)
            budget.write(ArtifactStream.STDERR, "WXYZ".toByteArray(), 4)
            budget.write(ArtifactStream.STDOUT, "late".toByteArray(), 4)
            budget.closeAll()

            assertArrayEquals("abcd".toByteArray(), stdout.readBytes())
            assertArrayEquals("W".toByteArray(), stderr.readBytes())
            assertEquals(4L, budget.stdoutBytes.get())
            assertEquals(1L, budget.stderrBytes.get())
            assertTrue(budget.stdoutTruncated.get())
            assertTrue(budget.stderrTruncated.get())
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun verifiedAssetCopyIsAtomicAndFailClosed() {
        val root = Files.createTempDirectory("vcp-cli-asset-test").toFile()
        val destination = File(root, "rootfs.tar.zst")
        val old = "old-archive".toByteArray()
        val replacement = "new-runtime-archive".toByteArray()
        destination.writeBytes(old)
        val digest = MessageDigest.getInstance("SHA-256").digest(replacement)
            .joinToString("") { "%02x".format(it) }
        var syncedParent: File? = null
        try {
            assertThrows(IllegalArgumentException::class.java) {
                copyVerifiedStreamAtomically(
                    openInput = { ByteArrayInputStream(replacement) },
                    destination = destination,
                    expectedBytes = replacement.size.toLong(),
                    expectedSha256 = "0".repeat(64),
                )
            }
            assertArrayEquals(old, destination.readBytes())
            assertTrue(root.listFiles().orEmpty().none { it.name.contains(".install-") })

            copyVerifiedStreamAtomically(
                openInput = { ByteArrayInputStream(replacement) },
                destination = destination,
                expectedBytes = replacement.size.toLong(),
                expectedSha256 = digest,
                syncParent = { syncedParent = it },
            )
            assertArrayEquals(replacement, destination.readBytes())
            assertEquals(root.canonicalFile, syncedParent?.canonicalFile)
        } finally {
            root.deleteRecursively()
        }
    }

}
