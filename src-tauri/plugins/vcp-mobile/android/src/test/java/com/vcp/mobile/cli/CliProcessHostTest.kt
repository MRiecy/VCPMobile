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
    fun invokeArgsDecodeSemanticAssetContract() {
        val hash = "a".repeat(64)
        val args = ObjectMapper().readValue(
            """{
                "operationId":"op","modelId":"model-r2",
                "modelBytes":24471328,"modelSha256":"$hash",
                "tokenizerBytes":10437027,"tokenizerSha256":"$hash"
            }""".trimIndent(),
            PrepareCliSemanticAssetsArgs::class.java,
        )
        assertEquals("model-r2", args.modelId)
        assertEquals(24_471_328L, args.modelBytes)
        assertEquals(10_437_027L, args.tokenizerBytes)
    }

    @Test
    fun invokeArgsDecodeNestedRiverArtifactArray() {
        val hash = "a".repeat(64)
        val args = ObjectMapper().readValue(
            """{
                "operationId":"op","jobId":"job","attemptId":"attempt",
                "runtimeGeneration":1,"command":"true","rootfsPath":"/rootfs",
                "cwd":"/workspace","artifactMaxBytes":1024,
                "riverContextProjection":{
                    "hostPath":"/private/river-context.json","sizeBytes":12,"sha256":"$hash",
                    "artifacts":[{
                        "hostPath":"/private/river-artifact-00-aaaaaaaaaaaa.png",
                        "guestPath":"/run/river-artifact-00-aaaaaaaaaaaa.png",
                        "sizeBytes":7,"sha256":"$hash"
                    }]
                }
            }""".trimIndent(),
            StartCliProcessArgs::class.java,
        )

        assertEquals(1, args.riverContextProjection?.artifacts?.size)
        assertEquals(
            "/run/river-artifact-00-aaaaaaaaaaaa.png",
            args.riverContextProjection?.artifacts?.single()?.guestPath,
        )
    }

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
    fun riverProjectionAddsOnlyFencedFileBindsAndFixedGuestEnvironment() {
        val hostProjection =
            "/private/no-backup/vcp-cli/projections/${"a".repeat(64)}/river-context.json"
        val hostArtifact =
            "/private/no-backup/vcp-cli/projections/${"a".repeat(64)}/river-artifact-00-bbbbbbbbbbbb.png"
        val userCommand = "test -r \"${'$'}VCP_RIVER_CONTEXT_FILE\""
        val argv = buildProotArguments(
            prootPath = "/native/libvcp_proot.so",
            rootfsPath = "/private/rootfs/profile",
            workspacePath = "/private/workspace",
            cwd = "/workspace",
            command = userCommand,
            riverContextHostPath = hostProjection,
            riverArtifactBinds = listOf(
                hostArtifact to "/run/river-artifact-00-bbbbbbbbbbbb.png",
            ),
        )

        assertEquals(
            listOf(
                "/dev",
                "/proc",
                "/private/workspace:/workspace",
                "$hostProjection:/run/vcp-river-context.json",
                "$hostArtifact:/run/river-artifact-00-bbbbbbbbbbbb.png",
            ),
            argv.windowed(2).filter { it.first() == "-b" }.map { it.last() },
        )
        assertTrue(argv.contains("VCP_RIVER_CONTEXT_FILE=/run/vcp-river-context.json"))
        assertFalse(argv.any { it == "/private/no-backup/vcp-cli/projections:/run" })
        assertFalse(argv.any { it.contains(":/output") || it.contains(":/skills") })
        assertEquals(listOf("/bin/bash", "-lc", userCommand), argv.takeLast(3))
        assertEquals(1, argv.count { it == userCommand })
    }

    @Test
    fun riverProjectionVerifierRejectsHashSizeTypeSymlinkAndContainmentSubstitution() {
        val root = Files.createTempDirectory("vcp-cli-river-projection").toFile().canonicalFile
        val sibling = Files.createTempDirectory("vcp-cli-river-sibling").toFile().canonicalFile
        val stem = "a".repeat(64)
        val bytes = "{\"request\":\"frozen\"}".toByteArray()
        val hash = sha256Hex(bytes)

        fun projection(path: File, size: Long = bytes.size.toLong(), sha256: String = hash) =
            RiverContextProjectionArgs().apply {
                hostPath = path.absolutePath
                sizeBytes = size
                this.sha256 = sha256
                artifacts = emptyArray()
            }

        fun freshSource(parent: File = File(root, stem)): File {
            parent.deleteRecursively()
            assertTrue(parent.mkdirs())
            return File(parent, "river-context.json").apply { writeBytes(bytes) }
        }

        try {
            var source = freshSource()
            assertEquals(
                source.canonicalFile,
                verifyRiverContextProjection(root, stem, projection(source)).contextFile,
            )
            assertThrows(IllegalArgumentException::class.java) {
                verifyRiverContextProjection(root, stem, projection(source, sha256 = "0".repeat(64)))
            }
            assertThrows(IllegalArgumentException::class.java) {
                verifyRiverContextProjection(root, stem, projection(source, sha256 = "a".repeat(65)))
            }
            val oversizedPath = projection(source).apply { hostPath = "/${"x".repeat(4097)}" }
            assertThrows(IllegalArgumentException::class.java) {
                verifyRiverContextProjection(root, stem, oversizedPath)
            }
            assertThrows(IllegalArgumentException::class.java) {
                verifyRiverContextProjection(root, stem, projection(source, size = bytes.size + 1L))
            }
            assertThrows(IllegalArgumentException::class.java) {
                verifyRiverContextProjection(root, stem, projection(source, size = 128L * 1024L + 1L))
            }

            source.delete()
            assertTrue(source.mkdir())
            assertThrows(IllegalArgumentException::class.java) {
                verifyRiverContextProjection(root, stem, projection(source, size = 1L))
            }

            source.deleteRecursively()
            val siblingSource = File(sibling, "source.json").apply { writeBytes(bytes) }
            Files.createSymbolicLink(source.toPath(), siblingSource.toPath())
            assertThrows(IllegalArgumentException::class.java) {
                verifyRiverContextProjection(root, stem, projection(source))
            }

            File(root, stem).deleteRecursively()
            val siblingParent = File(sibling, "real-parent").apply { mkdirs() }
            File(siblingParent, "river-context.json").writeBytes(bytes)
            Files.createSymbolicLink(File(root, stem).toPath(), siblingParent.toPath())
            source = File(root, "$stem/river-context.json")
            assertThrows(IllegalArgumentException::class.java) {
                verifyRiverContextProjection(root, stem, projection(source))
            }

            source = freshSource()
            val rootLink = File(sibling, "projection-root-link")
            Files.createSymbolicLink(rootLink.toPath(), root.toPath())
            val sourceThroughRootLink = File(rootLink, "$stem/river-context.json")
            assertThrows(IllegalArgumentException::class.java) {
                verifyRiverContextProjection(rootLink, stem, projection(sourceThroughRootLink))
            }

            val siblingAttempt = File(sibling, stem).apply { mkdirs() }
            source = File(siblingAttempt, "river-context.json").apply { writeBytes(bytes) }
            assertThrows(IllegalArgumentException::class.java) {
                verifyRiverContextProjection(root, stem, projection(source))
            }

            val wrongStemParent = File(root, "b".repeat(64))
            source = freshSource(wrongStemParent)
            assertThrows(IllegalArgumentException::class.java) {
                verifyRiverContextProjection(root, stem, projection(source))
            }

            source = File(wrongStemParent, "../${wrongStemParent.name}/river-context.json")
            assertThrows(IllegalArgumentException::class.java) {
                verifyRiverContextProjection(root, stem, projection(source))
            }
        } finally {
            root.deleteRecursively()
            sibling.deleteRecursively()
        }
    }

    @Test
    fun riverArtifactVerifierFreezesAttemptCopyHashAndGuestLeaf() {
        val root = Files.createTempDirectory("vcp-cli-river-artifact").toFile().canonicalFile
        val stem = "c".repeat(64)
        val parent = File(root, stem).apply { mkdirs() }
        val contextBytes = "{\"schema\":\"vcp.mobile.attempt-projection.v1\"}".toByteArray()
        val context = File(parent, "river-context.json").apply { writeBytes(contextBytes) }
        val artifactBytes = "attempt copy".toByteArray()
        val artifactHash = sha256Hex(artifactBytes)
        val artifactLeaf = "river-artifact-00-${artifactHash.take(12)}.png"
        val artifact = File(parent, artifactLeaf).apply { writeBytes(artifactBytes) }
        val artifactArgs = RiverArtifactProjectionArgs().apply {
            hostPath = artifact.absolutePath
            guestPath = "/run/$artifactLeaf"
            sizeBytes = artifactBytes.size.toLong()
            sha256 = artifactHash
        }
        val projection = RiverContextProjectionArgs().apply {
            hostPath = context.absolutePath
            sizeBytes = contextBytes.size.toLong()
            sha256 = sha256Hex(contextBytes)
            artifacts = arrayOf(artifactArgs)
        }

        try {
            val verified = verifyRiverContextProjection(root, stem, projection)
            assertEquals(context.canonicalFile, verified.contextFile)
            assertEquals(artifact.canonicalFile, verified.artifacts.single().hostFile)
            assertEquals("/run/$artifactLeaf", verified.artifacts.single().guestPath)

            artifactArgs.guestPath = "/run/renamed.png"
            assertThrows(IllegalArgumentException::class.java) {
                verifyRiverContextProjection(root, stem, projection)
            }
            artifactArgs.guestPath = "/run/$artifactLeaf"
            artifact.delete()
            val sibling = File(parent, "sibling.bin").apply { writeBytes(artifactBytes) }
            Files.createSymbolicLink(artifact.toPath(), sibling.toPath())
            assertThrows(IllegalArgumentException::class.java) {
                verifyRiverContextProjection(root, stem, projection)
            }
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun processFingerprintIncludesOptionalRiverProjectionIdentity() {
        fun request(projection: RiverContextProjectionArgs? = null) = StartCliProcessArgs().apply {
            operationId = "operation-1"
            jobId = "job-1"
            attemptId = "attempt-1"
            runtimeGeneration = 7
            command = "true"
            rootfsPath = "/private/rootfs/profile"
            cwd = "/workspace"
            artifactMaxBytes = 1024
            riverContextProjection = projection
        }

        val projection = RiverContextProjectionArgs().apply {
            hostPath = "/private/projections/${"a".repeat(64)}/river-context.json"
            sizeBytes = 17
            sha256 = "b".repeat(64)
            artifacts = arrayOf(
                RiverArtifactProjectionArgs().apply {
                    hostPath = "/private/projections/${"a".repeat(64)}/river-artifact-00-cccccccccccc.png"
                    guestPath = "/run/river-artifact-00-cccccccccccc.png"
                    sizeBytes = 23
                    sha256 = "c".repeat(64)
                },
            )
        }
        val withoutProjection = fingerprint(request())
        val withProjection = fingerprint(request(projection))
        assertTrue(withProjection != withoutProjection)

        projection.sha256 = projection.sha256.uppercase()
        assertEquals(withProjection, fingerprint(request(projection)))
        projection.artifacts.single().sizeBytes++
        assertTrue(withProjection != fingerprint(request(projection)))
        projection.artifacts.single().sizeBytes--
        projection.sizeBytes++
        assertTrue(withProjection != fingerprint(request(projection)))
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
        val host = buildHostCommand("/private/handshake.pid", proot)

        assertEquals(
            listOf(
                "/system/bin/toybox",
                "setsid",
                "/system/bin/sh",
                "-c",
                HOST_HANDSHAKE_SCRIPT,
                "vcp-cli-host",
                "/private/handshake.pid",
            ),
            host.take(7),
        )
        assertFalse(HOST_HANDSHAKE_SCRIPT.contains(userCommand))
        assertEquals(userCommand, host.last())
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
            val host = buildHostCommand("/private/handshake.pid", proot)

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

    @Test
    fun semanticAssetStagingIsLazyDirectAndIdentityBound() {
        val root = Files.createTempDirectory("vcp-cli-semantic-asset-test").toFile()
        val bytes = "frozen-semantic-model".toByteArray()
        val digest = MessageDigest.getInstance("SHA-256").digest(bytes)
            .joinToString("") { "%02x".format(it) }
        try {
            val staged = stageVerifiedSemanticAsset(
                assets = root,
                assetName = "vcp-semantic-model-r2.safetensors",
                expectedBytes = bytes.size.toLong(),
                expectedHash = digest,
                openInput = { ByteArrayInputStream(bytes) },
            )
            assertEquals(root.canonicalFile, staged.parentFile)
            assertArrayEquals(bytes, staged.readBytes())
            var warmAssetOpens = 0
            val warm = stageVerifiedSemanticAsset(
                assets = root,
                assetName = staged.name,
                expectedBytes = bytes.size.toLong(),
                expectedHash = digest,
                openInput = {
                    warmAssetOpens += 1
                    ByteArrayInputStream(bytes)
                },
            )
            assertEquals(staged.canonicalFile, warm)
            assertEquals("verified warm asset must not reopen AssetManager", 0, warmAssetOpens)

            val replacement = "repaired-semantic-model".toByteArray()
            val replacementDigest = MessageDigest.getInstance("SHA-256").digest(replacement)
                .joinToString("") { "%02x".format(it) }
            staged.writeBytes("damaged".toByteArray())
            val repaired = stageVerifiedSemanticAsset(
                assets = root,
                assetName = staged.name,
                expectedBytes = replacement.size.toLong(),
                expectedHash = replacementDigest,
                openInput = { ByteArrayInputStream(replacement) },
            )
            assertArrayEquals(replacement, repaired.readBytes())

            assertThrows(IllegalArgumentException::class.java) {
                stageVerifiedSemanticAsset(
                    assets = root,
                    assetName = "../escape.safetensors",
                    expectedBytes = bytes.size.toLong(),
                    expectedHash = digest,
                    openInput = { ByteArrayInputStream(bytes) },
                )
            }

            repaired.delete()
            val sibling = File(root.parentFile, "semantic-sibling").apply { writeBytes(bytes) }
            Files.createSymbolicLink(repaired.toPath(), sibling.toPath())
            assertThrows(IllegalArgumentException::class.java) {
                stageVerifiedSemanticAsset(
                    assets = root,
                    assetName = repaired.name,
                    expectedBytes = bytes.size.toLong(),
                    expectedHash = digest,
                    openInput = { ByteArrayInputStream(bytes) },
                )
            }
            repaired.delete()
            sibling.delete()

            File(root, "vcp-semantic-model-r2.safetensors").mkdir()
            assertThrows(IllegalArgumentException::class.java) {
                stageVerifiedSemanticAsset(
                    assets = root,
                    assetName = "vcp-semantic-model-r2.safetensors",
                    expectedBytes = bytes.size.toLong(),
                    expectedHash = digest,
                    openInput = { ByteArrayInputStream(bytes) },
                )
            }
        } finally {
            root.deleteRecursively()
        }
    }
}
