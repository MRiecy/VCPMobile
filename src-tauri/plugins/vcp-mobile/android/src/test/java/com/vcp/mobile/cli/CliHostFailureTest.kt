package com.vcp.mobile.cli

import android.content.Context
import android.os.Process as AndroidProcess
import android.system.Os
import android.system.OsConstants
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
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.util.concurrent.ConcurrentHashMap
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

    @Test fun groupDisappearingDuringIdentityCheckIsTerminalWithoutSendingASignal() {
        val identity = ProcIdentity(4242, 4242, 4242, 1, AndroidProcess.myUid())
        mockkObject(OwnedProcessGroup, recordPrivateCalls = true)
        mockkStatic(Os::class)
        every { Os.kill(any(), any()) } just Runs
        every { OwnedProcessGroup.isAlive(identity.pgid) } returnsMany listOf(true, false)
        every { OwnedProcessGroup.readIdentity(identity.pid) } returns null
        every { OwnedProcessGroup["groupHasOnlyVisibleAppUidMembers"](identity.pgid) } returns false

        val result = OwnedProcessGroup.terminate(identity, 0)

        assertTrue(result.groupGone)
        assertFalse(result.termSent)
        assertFalse(result.killSent)
        verify(exactly = 0) { Os.kill(any(), any()) }
    }

    @Test fun liveReplacementIdentityCannotBeTerminated() {
        val identity = ProcIdentity(4242, 4242, 4242, 1, AndroidProcess.myUid())
        mockkObject(OwnedProcessGroup)
        mockkStatic(Os::class)
        every { Os.kill(any(), any()) } just Runs
        every { OwnedProcessGroup.isAlive(identity.pgid) } returns true
        every { OwnedProcessGroup.readIdentity(identity.pid) } returns identity.copy(startTimeTicks = 2)

        assertThrows(IllegalStateException::class.java) { OwnedProcessGroup.terminate(identity, 0) }

        verify(exactly = 0) { Os.kill(any(), any()) }
    }

    @Test fun groupDisappearingBeforeKillDoesNotTurnItsExitIntoAnOwnershipFailure() {
        val identity = ProcIdentity(4242, 4242, 4242, 1, AndroidProcess.myUid())
        mockkObject(OwnedProcessGroup, recordPrivateCalls = true)
        mockkStatic(Os::class)
        every { Os.kill(any(), any()) } just Runs
        every { OwnedProcessGroup.isAlive(identity.pgid) } returnsMany listOf(true, true, true, false)
        every { OwnedProcessGroup.readIdentity(identity.pid) } returnsMany listOf(identity, null, null)
        every { OwnedProcessGroup["groupHasOnlyVisibleAppUidMembers"](identity.pgid) } returns false

        val result = OwnedProcessGroup.terminate(identity, 0)

        assertTrue(result.groupGone)
        assertTrue(result.termSent)
        assertFalse(result.killSent)
        verify(exactly = 1) { Os.kill(-identity.pgid, OsConstants.SIGTERM) }
        // The initial TERM is the only signal: disappearance must not issue another one.
        // Count the OS boundary calls because host Android stubs can alias signal constants.
        verify(exactly = 1) { Os.kill(any(), any()) }
    }

    @Test fun cancellationCompletesAfterTheOnlyWaiterFailed() = assertTerminalConvergence(waiterFails = true)

    @Test fun naturalExitAndConcurrentCancellationsDrainAndFinalizeOnce() = assertTerminalConvergence(waiterFails = false)

    private fun assertTerminalConvergence(waiterFails: Boolean) {
        val identity = ProcIdentity(4242, 4242, 4242, 1, AndroidProcess.myUid())
        val readerGate = CountDownLatch(1)
        val readersStarted = CountDownLatch(2)
        val waiterAttempted = CountDownLatch(1)
        val terminationCalls = AtomicInteger()
        val stdout = File(prepared.output, "terminal.stdout")
        val stderr = File(prepared.output, "terminal.stderr")
        val budget = ArtifactBudget(1024, stdout, stderr)
        fun delayedInput(value: String) = object : ByteArrayInputStream(value.toByteArray()) {
            override fun read(buffer: ByteArray, offset: Int, length: Int): Int {
                readersStarted.countDown()
                readerGate.await()
                return super.read(buffer, offset, length)
            }
        }
        val stdoutReader = budget.startDrainThread("fixture-stdout", delayedInput("last stdout"), ArtifactStream.STDOUT)
        val stderrReader = budget.startDrainThread("fixture-stderr", delayedInput("last stderr"), ArtifactStream.STDERR)
        val process = mockk<Process>()
        every { process.waitFor() } returns 0
        every { process.waitFor(any(), any()) } returns true
        every { process.exitValue() } returns 0
        every { process.isAlive } returns false
        every { process.destroyForcibly() } returns process
        every { process.inputStream } returns ByteArrayInputStream(byteArrayOf())
        every { process.errorStream } returns ByteArrayInputStream(byteArrayOf())
        every { process.outputStream } returns ByteArrayOutputStream()
        mockkObject(OwnedProcessGroup)
        every { OwnedProcessGroup.isAlive(identity.pgid) } returns false
        every { OwnedProcessGroup.terminate(identity, any()) } answers {
            val first = terminationCalls.incrementAndGet() == 1
            waiterAttempted.countDown()
            if (first && waiterFails) throw IllegalStateException("identity read interrupted before terminal publication")
            GroupTermination(false, false, true)
        }
        mockkObject(ForegroundGuardian)
        every { ForegroundGuardian.release(any(), any(), any()) } just Runs

        val keyClass = Class.forName("com.vcp.mobile.cli.ProcessKey")
        val key = keyClass.getDeclaredConstructor(String::class.java, String::class.java, Long::class.javaPrimitiveType)
            .apply { isAccessible = true }.newInstance("job", "attempt", 1L)
        val leaseClass = Class.forName("com.vcp.mobile.cli.CliForegroundLease")
        val lease = leaseClass.getDeclaredConstructor(String::class.java, Long::class.javaPrimitiveType)
            .apply { isAccessible = true }.newInstance("cli:job:attempt", 31L)
        val handleClass = Class.forName("com.vcp.mobile.cli.CliProcessHandle")
        val handle = handleClass.declaredConstructors.single().apply { isAccessible = true }.newInstance(
            key, "fingerprint", identity, process, stdout, stderr, budget,
            stdoutReader, stderrReader, File(prepared.prootTmpParent, "terminal").apply { mkdirs() }, lease,
        )
        @Suppress("UNCHECKED_CAST")
        val handles = CliProcessHost::class.java.getDeclaredField("handles").apply { isAccessible = true }
            .get(host) as ConcurrentHashMap<Any, Any>
        handles[key] = handle
        val finished = handleClass.getDeclaredMethod("getFinished").apply { isAccessible = true }
            .invoke(handle) as CountDownLatch
        val waiter = CliProcessHost::class.java.getDeclaredMethod("startWaiter", handleClass)
            .apply { isAccessible = true }.invoke(host, handle) as Thread
        handleClass.getDeclaredMethod("setWaiter", Thread::class.java).apply { isAccessible = true }
            .invoke(handle, waiter)
        try {
            assertTrue(readersStarted.await(2, TimeUnit.SECONDS))
            assertTrue(waiterAttempted.await(2, TimeUnit.SECONDS))
            if (waiterFails) {
                waiter.join(2_000)
                assertFalse(waiter.isAlive)
            }
            val responses = CountDownLatch(2)
            val failures = AtomicReference<String?>(null)
            repeat(2) { index ->
                host.cancel(CancelCliProcessArgs().apply {
                    operationId = "cancel-$index"
                    jobId = "job"
                    attemptId = "attempt"
                    runtimeGeneration = 1
                    graceMs = 0
                }, { response ->
                    if (!response.getBoolean("groupGone") || response.getInt("exitCode") != 0) {
                        failures.set("cancel did not report the completed process")
                    }
                    responses.countDown()
                }, { failures.set(it); responses.countDown() })
            }
            assertEquals(1L, finished.count)
            verify(exactly = 0) { ForegroundGuardian.release(any(), any(), any()) }
            readerGate.countDown()
            assertTrue(responses.await(5, TimeUnit.SECONDS))
            assertNull(failures.get())
            assertEquals(0L, finished.count)
            assertEquals("last stdout", stdout.readText())
            assertEquals("last stderr", stderr.readText())
            verify(exactly = 1) { ForegroundGuardian.release(context, "cli:job:attempt", 31L) }
        } finally {
            readerGate.countDown()
            waiter.join(2_000)
            stdoutReader.join(2_000)
            stderrReader.join(2_000)
        }
    }
}
