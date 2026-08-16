package com.vcp.mobile.cli

import com.fasterxml.jackson.databind.ObjectMapper
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
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
    fun prootArgvFreezesBashPathAndKeepsUserCommandAsOneArgument() {
        val userCommand = "printf '%s\\n' \"${'$'}API_KEY\"; echo pwned > /workspace/result"
        val argv = buildProotArguments(
            prootPath = "/native/libvcp_proot.so",
            rootfsPath = "/private/rootfs/profile",
            workspacePath = "/private/workspace",
            cwd = "/workspace/topic",
            command = userCommand,
        )

        assertEquals("/native/libvcp_proot.so", argv.first())
        assertTrue(argv.contains("--kill-on-exit"))
        assertTrue(argv.indexOf("--kill-on-exit") < argv.indexOf("--link2symlink"))
        assertTrue(argv.windowed(2).contains(listOf("-r", "/private/rootfs/profile")))
        assertTrue(argv.windowed(2).contains(listOf("-b", "/dev")))
        assertTrue(argv.windowed(2).contains(listOf("-b", "/proc")))
        assertEquals(
            listOf("/dev", "/proc", "/private/workspace:/workspace"),
            argv.windowed(2).filter { it.first() == "-b" }.map { it.last() },
        )
        assertFalse(argv.contains("/sys"))
        assertTrue(argv.windowed(2).contains(listOf("-b", "/private/workspace:/workspace")))
        assertFalse(argv.any { it == "/skills" || it.endsWith(":/skills") })
        assertTrue(argv.windowed(2).contains(listOf("-w", "/workspace/topic")))
        assertTrue(argv.contains("PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"))
        assertFalse(argv.any { it.contains("vcp-river-context") })
        assertFalse(argv.any { it.startsWith("VCP_RIVER_CONTEXT_FILE=") })
        assertFalse(argv.any { it.startsWith("PROOT_LOADER=") || it.startsWith("PROOT_TMP_DIR=") })
        assertEquals(listOf("/bin/bash", "-lc", userCommand), argv.takeLast(3))
        assertEquals(1, argv.count { it == userCommand })
        assertFalse(argv.any { it.contains("/output") || it.contains("host-output") })
    }

    @Test
    fun managedGuestEnvironmentCarriesVcpCliFacts() {
        val withSession = buildProotArguments(
            prootPath = "/native/libvcp_proot.so",
            rootfsPath = "/private/rootfs/profile",
            workspacePath = "/private/workspace",
            cwd = "/workspace",
            command = "true",
            sessionId = "dist-session:abc",
        )
        assertTrue(withSession.contains("VCP_CLI_WORKSPACE=/workspace"))
        assertTrue(withSession.contains("VCP_CLI_SESSION_ID=dist-session:abc"))
        assertEquals(1, withSession.count { it.startsWith("VCP_CLI_SESSION_ID=") })

        val withoutSession = buildProotArguments(
            prootPath = "/native/libvcp_proot.so",
            rootfsPath = "/private/rootfs/profile",
            workspacePath = "/private/workspace",
            cwd = "/workspace",
            command = "true",
        )
        assertTrue(withoutSession.contains("VCP_CLI_WORKSPACE=/workspace"))
        assertFalse(withoutSession.any { it.startsWith("VCP_CLI_SESSION_ID=") })

        val terminal = buildProotTerminalArguments(
            prootPath = "/native/libvcp_proot.so",
            rootfsPath = "/private/rootfs/profile",
            workspacePath = "/private/workspace",
            cwd = "/workspace",
        )
        assertTrue(terminal.contains("VCP_CLI_WORKSPACE=/workspace"))
        assertFalse(terminal.any { it.startsWith("VCP_CLI_SESSION_ID=") })
    }

    @Test
    fun hostEnvironmentIsClearedToFixedPathsAndApkNativeUnbundledLoader() {
        val environment = buildHostEnvironment(
            prootTmpPath = "/private/no-backup/proot-tmp/attempt",
            prootLoaderPath = "/data/app/package/lib/arm64/libvcp_proot_loader.so",
        )

        assertEquals(
            setOf("PATH", "TMPDIR", "PROOT_TMP_DIR", "PROOT_LOADER"),
            environment.keys,
        )
        assertEquals("/system/bin:/system/xbin", environment["PATH"])
        assertEquals("/private/no-backup/proot-tmp/attempt", environment["TMPDIR"])
        assertEquals("/private/no-backup/proot-tmp/attempt", environment["PROOT_TMP_DIR"])
        assertEquals(
            "/data/app/package/lib/arm64/libvcp_proot_loader.so",
            environment["PROOT_LOADER"],
        )
        assertFalse(environment.containsKey("LD_LIBRARY_PATH"))
        assertFalse(environment.containsKey("API_KEY"))
        assertThrows(IllegalArgumentException::class.java) {
            buildHostEnvironment("relative/tmp", "/native/libvcp_proot_loader.so")
        }
        assertThrows(IllegalArgumentException::class.java) {
            buildHostEnvironment("/private/tmp", "relative/libvcp_proot_loader.so")
        }
    }

    @Test
    fun hostWrapperIsFixedAndCommandRemainsPastTheShellBoundary() {
        val userCommand = "touch /workspace/a; echo '${'$'}HOME'"
        val proot = buildProotArguments(
            prootPath = "/native/libvcp_proot.so",
            rootfsPath = "/private/rootfs/profile",
            workspacePath = "/private/workspace",
            cwd = "/workspace",
            command = userCommand,
        )
        val host = buildHostCommand("/private/handshake.pid", 524_288, proot)

        assertEquals(
            listOf(
                "/system/bin/toybox",
                "setsid",
                "/system/bin/sh",
                "-c",
                HOST_HANDSHAKE_SCRIPT,
                "vcp-cli-host",
                "/private/handshake.pid",
                "524288",
            ),
            host.take(8),
        )
        assertFalse(HOST_HANDSHAKE_SCRIPT.contains(userCommand))
        assertEquals(userCommand, host.last())
    }

    @Test
    fun terminalLaunchUsesLoginBashAndNeverInjectsAnAutomaticCommand() {
        val arguments = buildProotTerminalArguments(
            prootPath = "/native/libvcp_proot.so",
            rootfsPath = "/private/rootfs/profile",
            workspacePath = "/private/workspace",
            cwd = "/workspace",
        )

        assertEquals(listOf("/bin/bash", "-l"), arguments.takeLast(2))
        assertTrue(arguments.contains("TERM=xterm-256color"))
        assertTrue(arguments.contains("COLORTERM=truecolor"))
        assertFalse(arguments.contains("-c"))
        assertFalse(arguments.contains("-lc"))
    }

    @Test
    fun detachedTraceeAttackKeepsKillOnExitAndCommandBoundary() {
        listOf(
            "setsid /bin/sleep 300 >/dev/null 2>&1 & exit 0",
            "nohup setsid /bin/sleep 300 >/dev/null 2>&1 & exit 0",
        ).forEach { userCommand ->
            val proot = buildProotArguments(
                prootPath = "/native/libvcp_proot.so",
                rootfsPath = "/private/rootfs/profile",
                workspacePath = "/private/workspace",
                cwd = "/workspace",
                command = userCommand,
            )
            val host = buildHostCommand("/private/handshake.pid", 524_288, proot)

            assertTrue(proot.contains("--kill-on-exit"))
            assertTrue(proot.indexOf("--kill-on-exit") < proot.indexOf("-r"))
            assertEquals(listOf("/bin/bash", "-lc", userCommand), proot.takeLast(3))
            assertEquals(1, host.count { it == userCommand })
            assertFalse(HOST_HANDSHAKE_SCRIPT.contains("setsid"))
            assertFalse(HOST_HANDSHAKE_SCRIPT.contains("sleep 300"))
        }
    }

    @Test
    fun guestCannotReachCanonicalSkillsProjection() {
        val writeAttack = "printf x >> /skills/vcp-mobile-cli-basics/SKILL.md"
        val proot = buildProotArguments(
            prootPath = "/native/libvcp_proot.so",
            rootfsPath = "/private/rootfs/profile",
            workspacePath = "/private/workspace",
            cwd = "/workspace",
            command = writeAttack,
        )

        assertFalse(proot.any { it == "/skills" || it.endsWith(":/skills") })
        assertFalse(proot.any { it.contains("/private/skills") })
        assertEquals(listOf("/bin/bash", "-lc", writeAttack), proot.takeLast(3))
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
