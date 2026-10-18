package com.vcp.mobile.cli

import android.content.Context
import androidx.test.core.app.ApplicationProvider
import app.tauri.plugin.JSObject
import com.vcp.mobile.service.ForegroundGuardian
import io.mockk.*
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.io.File
import java.io.IOException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28])
class CliHostFailureTest {
    @get:Rule val temporary = TemporaryFolder(File("build").apply { mkdirs() })
    private val context: Context = ApplicationProvider.getApplicationContext()
    private lateinit var host: CliProcessHost
    private lateinit var prepared: PreparedRuntime
    private lateinit var rootfs: File

    @Before fun setUp() {
        val root = temporary.newFolder("runtime").canonicalFile
        fun directory(name: String) = File(root, name).apply { mkdirs() }
        val rootfsParent = directory("rootfs")
        rootfs = File(rootfsParent, "base").apply { mkdirs() }
        prepared = PreparedRuntime(
            profileId = "fixture",
            runtimeGeneration = 1,
            archive = File(root, "archive"),
            rootfsParent = rootfsParent,
            workspace = directory("workspace"),
            skills = directory("skills"),
            output = directory("output"),
            projectionRoot = directory("projections"),
            proot = File(root, "proot"),
            prootLoader = File(root, "loader"),
            prootTmpParent = directory("proot-tmp"),
            handshakeParent = directory("handshakes"),
        )
        mockkConstructor(CliRuntimeInstaller::class)
        every { anyConstructed<CliRuntimeInstaller>().prepare(any()) } returns prepared
        host = CliProcessHost(context)
    }

    @After fun tearDown() {
        host.close()
        unmockkAll()
    }

    private fun prepareHost() {
        val completed = CountDownLatch(1)
        val failure = AtomicReference<String?>(null)
        host.prepare(PrepareCliRuntimeArgs().apply { operationId = "prepare" },
            { completed.countDown() }, { failure.set(it); completed.countDown() })
        assertTrue(completed.await(5, TimeUnit.SECONDS))
        assertNull(failure.get())
    }

    private fun launchArgs(background: Boolean = false) = StartCliProcessArgs().apply {
        operationId = "start"
        jobId = "job"
        attemptId = "attempt"
        runtimeGeneration = 1
        command = "true"
        rootfsPath = rootfs.absolutePath
        cwd = "/workspace"
        artifactMaxBytes = 1024
        backgroundLease = background
        timeoutMs = 30_000
        displayLabel = "fixture"
    }

    private fun failedStart(): String? {
        val completed = CountDownLatch(1)
        val failure = AtomicReference<String?>(null)
        host.start(launchArgs(), { completed.countDown() }, { failure.set(it); completed.countDown() })
        assertTrue(completed.await(5, TimeUnit.SECONDS))
        return failure.get()
    }

    @Test fun stdoutCollisionDoesNotDeleteEarlierAttemptFiles() {
        prepareHost()
        val stem = sha256Hex("1\u0000job\u0000attempt".toByteArray())
        val stdout = File(prepared.output, "$stem.stdout").apply { writeText("earlier stdout") }
        val stderr = File(prepared.output, "$stem.stderr").apply { writeText("earlier stderr") }
        val handshake = File(prepared.handshakeParent, "$stem.pid").apply { writeText("earlier handshake") }

        assertEquals("stdout artifact identity already exists", failedStart())
        assertEquals("earlier stdout", stdout.readText())
        assertEquals("earlier stderr", stderr.readText())
        assertEquals("earlier handshake", handshake.readText())
    }

    @Test fun stderrCollisionRemovesOnlyTheNewlyReservedStdout() {
        prepareHost()
        val stem = sha256Hex("1\u0000job\u0000attempt".toByteArray())
        val stdout = File(prepared.output, "$stem.stdout")
        val stderr = File(prepared.output, "$stem.stderr").apply { writeText("earlier stderr") }
        val handshake = File(prepared.handshakeParent, "$stem.pid").apply { writeText("earlier handshake") }

        assertEquals("stderr artifact identity already exists", failedStart())
        assertFalse(stdout.exists())
        assertEquals("earlier stderr", stderr.readText())
        assertEquals("earlier handshake", handshake.readText())
    }

    @Test fun closeSettlesBothRunningAndQueuedPrepareCalls() {
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val runningFailure = AtomicInteger()
        val queuedFailure = AtomicInteger()
        val successes = AtomicInteger()
        every { anyConstructed<CliRuntimeInstaller>().prepare(any()) } answers {
            entered.countDown()
            release.await(5, TimeUnit.SECONDS)
            prepared
        }
        try {
            host.prepare(PrepareCliRuntimeArgs().apply { operationId = "running" },
                { successes.incrementAndGet() }, { runningFailure.incrementAndGet() })
            assertTrue(entered.await(5, TimeUnit.SECONDS))
            host.prepare(PrepareCliRuntimeArgs().apply { operationId = "queued" },
                { successes.incrementAndGet() }, { queuedFailure.incrementAndGet() })
            host.close()
            assertEquals(1, runningFailure.get())
            assertEquals(1, queuedFailure.get())
            assertEquals(0, successes.get())
            verify(exactly = 1) { anyConstructed<CliRuntimeInstaller>().prepare(any()) }
        } finally {
            release.countDown()
        }
    }

    @Test fun closeDuringForegroundReadinessReleasesTheUnreturnedLease() {
        prepareHost()
        val waiting = CountDownLatch(1)
        val failures = AtomicInteger()
        val successes = AtomicInteger()
        mockkObject(ForegroundGuardian)
        every {
            ForegroundGuardian.acquire(any(), any(), any(), any(), any(), any(), any(), any(), any(), any())
        } returns 17L
        every { ForegroundGuardian.awaitServiceReadiness(any(), 17L, any()) } answers {
            waiting.countDown()
        }
        every { ForegroundGuardian.release(any(), any(), any()) } just Runs

        host.start(launchArgs(background = true), { successes.incrementAndGet() }, { failures.incrementAndGet() })
        assertTrue(waiting.await(5, TimeUnit.SECONDS))
        host.close()

        verify(exactly = 1) { ForegroundGuardian.release(context, "cli:job:attempt", 17L) }
        assertEquals(1, failures.get())
        assertEquals(0, successes.get())
        assertTrue(prepared.output.listFiles().orEmpty().isEmpty())
    }

    @Test fun leaseReleaseFailureDoesNotSkipFailedStartArtifactCleanup() {
        prepareHost()
        mockkObject(ForegroundGuardian)
        every {
            ForegroundGuardian.acquire(any(), any(), any(), any(), any(), any(), any(), any(), any(), any())
        } returns 23L
        every { ForegroundGuardian.awaitServiceReadiness(any(), 23L, any()) } answers {
            thirdArg<(Boolean, String?) -> Unit>().invoke(true, null)
        }
        every { ForegroundGuardian.release(any(), any(), any()) } throws IllegalStateException("lease refresh failed")
        val completed = CountDownLatch(1)
        val failure = AtomicReference<String?>(null)
        host.start(launchArgs(background = true),
            { completed.countDown() }, { failure.set(it); completed.countDown() })

        assertTrue(completed.await(5, TimeUnit.SECONDS))
        assertNotNull(failure.get())
        assertNotEquals("lease refresh failed", failure.get())
        verify(exactly = 1) { ForegroundGuardian.release(context, "cli:job:attempt", 23L) }
        assertTrue(prepared.output.listFiles().orEmpty().isEmpty())
        assertTrue(prepared.prootTmpParent.listFiles().orEmpty().isEmpty())
    }

    @Test fun terminalCleanupFailuresStillReleaseTheCompletionWaiter() {
        // Reach the already-terminal cleanup boundary without inventing a native process.
        // Its callers' group/reader checks remain in production, before finalizeHandle.
        val keyClass = Class.forName("com.vcp.mobile.cli.ProcessKey")
        val key = keyClass.getDeclaredConstructor(String::class.java, String::class.java, Long::class.javaPrimitiveType)
            .apply { isAccessible = true }.newInstance("job", "attempt", 1L)
        val leaseClass = Class.forName("com.vcp.mobile.cli.CliForegroundLease")
        val lease = leaseClass.getDeclaredConstructor(String::class.java, Long::class.javaPrimitiveType)
            .apply { isAccessible = true }.newInstance("cli:job:attempt", 29L)
        val budget = mockk<ArtifactBudget>()
        every { budget.closeAll() } throws IOException("output close failed")
        val directory = mockk<File>()
        every { directory.isDirectory } throws SecurityException("temporary directory unavailable")
        mockkObject(ForegroundGuardian)
        every { ForegroundGuardian.release(any(), any(), any()) } throws IllegalStateException("lease refresh failed")
        val handleClass = Class.forName("com.vcp.mobile.cli.CliProcessHandle")
        val handle = handleClass.declaredConstructors.single().apply { isAccessible = true }.newInstance(
            key, "fingerprint", ProcIdentity(4242, 4242, 4242, 1, 10000),
            mockk<Process>(), File(prepared.output, "stdout"), File(prepared.output, "stderr"),
            budget, Thread(), Thread(), directory, lease,
        )
        val finalize = CliProcessHost::class.java.getDeclaredMethod("finalizeHandle", handleClass)
            .apply { isAccessible = true }

        finalize.invoke(host, handle)
        finalize.invoke(host, handle)

        val finished = handleClass.getDeclaredMethod("getFinished").apply { isAccessible = true }
            .invoke(handle) as CountDownLatch
        assertEquals(0L, finished.count)
        verify(exactly = 1) { budget.closeAll() }
        verify(exactly = 1) { ForegroundGuardian.release(context, "cli:job:attempt", 29L) }
    }

    @Test fun failedSuccessDeliveryDoesNotInvokeFailureAsASecondTerminal() {
        val failures = AtomicInteger()
        val request = CliPendingRequest(
            success = { throw IllegalStateException("receiver is gone") },
            failure = { failures.incrementAndGet() },
            action = { JSObject() },
        )
        assertThrows(IllegalStateException::class.java) { request.run() }
        request.rejectBeforeStart("closed")
        assertEquals(0, failures.get())
    }
}
