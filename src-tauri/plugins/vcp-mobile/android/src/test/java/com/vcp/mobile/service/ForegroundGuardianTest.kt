package com.vcp.mobile.service

import android.app.Activity
import android.app.Application
import android.content.Context
import android.os.Looper
import android.view.WindowManager
import androidx.test.core.app.ApplicationProvider
import com.vcp.mobile.ScreenKeepOnArbiter
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import java.util.concurrent.TimeUnit

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28])
class ForegroundGuardianTest {
    private val context: Context = ApplicationProvider.getApplicationContext()

    @After
    fun tearDown() {
        ForegroundGuardian.setScreenStateListener(null)
        ForegroundGuardian.setCliLeaseLossListener(null)
        ForegroundGuardian.releaseAllLocks()
    }

    @Test
    fun acquireFirstConsumerActivatesGuardianAndStartsService() {
        ForegroundGuardian.acquire(
            context = context,
            tag = "stream:Nova",
            priority = ForegroundGuardian.PRIORITY_STREAM,
            label = "Nova",
            screenKeepOn = false,
            timeoutMs = 0,
        )

        assertTrue(ForegroundGuardian.isActive)
        assertFalse(ForegroundGuardian.isScreenKeepOnRequired)
        assertEquals("Nova", ForegroundGuardian.getNotificationLabel())

        val nextStartedService = shadowOf(context.applicationContext as Application).nextStartedService
        assertEquals(StreamKeepaliveService::class.java.name, nextStartedService.component?.className)
    }

    @Test
    fun acquireMultipleConsumersUsesHighestPriorityLabelAndScreenFlag() {
        ForegroundGuardian.acquire(
            context,
            "distributed",
            ForegroundGuardian.PRIORITY_DISTRIBUTED,
            "distributed",
            false,
            0,
        )
        ForegroundGuardian.acquire(
            context,
            "sync",
            ForegroundGuardian.PRIORITY_SYNC,
            "[数据同步]",
            true,
            0,
        )

        assertTrue(ForegroundGuardian.isActive)
        assertTrue(ForegroundGuardian.isScreenKeepOnRequired)
        assertEquals("[数据同步]", ForegroundGuardian.getNotificationLabel())
    }

    @Test
    fun releaseUnknownTagIsNoopAndReleaseLastConsumerDeactivates() {
        ForegroundGuardian.acquire(
            context,
            "stream:Nova",
            ForegroundGuardian.PRIORITY_STREAM,
            "Nova",
            false,
            0,
        )

        ForegroundGuardian.release(context, "missing")
        assertTrue(ForegroundGuardian.isActive)

        ForegroundGuardian.release(context, "stream:Nova")
        assertFalse(ForegroundGuardian.isActive)
        assertEquals("VCP 正在后台运行", ForegroundGuardian.getNotificationLabel())
    }

    @Test
    fun cliLeaseIsGenerationFencedAndSurvivesStoppingStreamingConsumers() {
        val target = ForegroundGuardian.CliNotificationTarget(
            jobId = "job-1",
            attemptId = "attempt-1",
            runtimeGeneration = 7,
            displayLabel = "bounded job",
        )
        val cliGeneration = ForegroundGuardian.acquire(
            context = context,
            tag = "cli:job-1:attempt-1",
            priority = ForegroundGuardian.PRIORITY_CLI,
            label = target.displayLabel,
            screenKeepOn = false,
            timeoutMs = 60_000,
            needsCpu = true,
            needsNetwork = false,
            kind = ForegroundGuardian.ConsumerKind.CLI_JOB,
            cliTarget = target,
        )
        ForegroundGuardian.acquire(
            context,
            "stream:Nova",
            ForegroundGuardian.PRIORITY_STREAM,
            "Nova",
            false,
            60_000,
        )

        ForegroundGuardian.release(context, "cli:job-1:attempt-1", cliGeneration + 1)
        assertEquals(listOf("cli:job-1:attempt-1" to target), ForegroundGuardian.activeCliTargets())

        ForegroundGuardian.releaseNonCliConsumers(context)
        assertTrue(ForegroundGuardian.isActive)
        assertEquals(listOf("cli:job-1:attempt-1" to target), ForegroundGuardian.activeCliTargets())

        ForegroundGuardian.release(context, "cli:job-1:attempt-1", cliGeneration)
        assertFalse(ForegroundGuardian.isActive)
    }

    @Test
    fun cliNotificationIdsProbeWithinTheirDedicatedNamespaceOnCollision() {
        val target = ForegroundGuardian.CliNotificationTarget("job-1", "attempt-1", 7, "job")
        val base = cliNotificationId(target)
        val allocated = allocateCliNotificationId(target, setOf(base))

        assertTrue(allocated != base)
        assertEquals(0x43000000, allocated and 0xff000000.toInt())
    }

    @Test
    @Config(sdk = [34])
    fun activeConsumerKindsSelectOnlyTheirDeclaredForegroundServiceTypes() {
        val target = ForegroundGuardian.CliNotificationTarget("job-1", "attempt-1", 1, "job")
        ForegroundGuardian.acquire(
            context,
            "cli:job-1:attempt-1",
            ForegroundGuardian.PRIORITY_CLI,
            "job",
            false,
            60_000,
            true,
            false,
            ForegroundGuardian.ConsumerKind.CLI_JOB,
            target,
        )

        val typeMask = ForegroundGuardian.foregroundServiceTypeMask()
        assertTrue(typeMask != 0)
    }

    @Test
    fun unexpectedForegroundServiceLossReportsOnlyExactCliTargets() {
        val target = ForegroundGuardian.CliNotificationTarget("job-1", "attempt-1", 9, "job")
        var affected: List<ForegroundGuardian.CliNotificationTarget>? = null
        ForegroundGuardian.setCliLeaseLossListener { affected = it }
        val generation = ForegroundGuardian.acquire(
            context,
            "cli:job-1:attempt-1",
            ForegroundGuardian.PRIORITY_CLI,
            "job",
            false,
            60_000,
            true,
            false,
            ForegroundGuardian.ConsumerKind.CLI_JOB,
            target,
        )

        ForegroundGuardian.onServiceDestroyed(context, generation)
        shadowOf(Looper.getMainLooper()).idle()

        assertEquals(listOf(target), affected)
        assertFalse(ForegroundGuardian.isActive)
    }

    @Test
    fun acquireSameTagUpdatesEntryInsteadOfDuplicating() {
        ForegroundGuardian.acquire(
            context,
            "stream:Nova",
            ForegroundGuardian.PRIORITY_STREAM,
            "Nova",
            false,
            0,
        )
        ForegroundGuardian.acquire(
            context,
            "stream:Nova",
            ForegroundGuardian.PRIORITY_PRERENDER,
            "[预渲染重建]",
            true,
            0,
        )

        assertTrue(ForegroundGuardian.isActive)
        assertTrue(ForegroundGuardian.isScreenKeepOnRequired)
        assertEquals("[预渲染重建]", ForegroundGuardian.getNotificationLabel())

        ForegroundGuardian.release(context, "stream:Nova")
        assertFalse(ForegroundGuardian.isActive)
    }

    @Test
    fun staleServiceDestroyCannotClearNewGeneration() {
        val firstGeneration = ForegroundGuardian.acquire(
            context, "stream:first", ForegroundGuardian.PRIORITY_STREAM, "first", false, 0,
        )
        ForegroundGuardian.acquire(
            context, "stream:second", ForegroundGuardian.PRIORITY_STREAM, "second", false, 0,
        )

        ForegroundGuardian.onServiceDestroyed(context, firstGeneration)

        assertTrue(ForegroundGuardian.isActive)
        assertFalse(ForegroundGuardian.isScreenKeepOnRequired)
    }

    @Test
    fun timeoutReleaseAlsoNotifiesScreenOwner() {
        var keepScreenOn: Boolean? = null
        ForegroundGuardian.setScreenStateListener { keepScreenOn = it }
        ForegroundGuardian.acquire(
            context, "sync", ForegroundGuardian.PRIORITY_SYNC, "[数据同步]", true, 1,
        )
        assertEquals(true, keepScreenOn)

        shadowOf(Looper.getMainLooper()).idleFor(2, TimeUnit.MILLISECONDS)

        assertFalse(ForegroundGuardian.isActive)
        assertEquals(false, keepScreenOn)
    }

    @Test
    fun foregroundStartFailureRollsBackOnlyFailedGeneration() {
        val originalGeneration = ForegroundGuardian.acquire(
            context, "stream:Nova", ForegroundGuardian.PRIORITY_STREAM, "old", false, 0,
        )
        ForegroundGuardian.onServiceReady(originalGeneration)
        val failedGeneration = ForegroundGuardian.acquire(
            context, "stream:Nova", ForegroundGuardian.PRIORITY_STREAM, "new", true, 0,
        )
        val applicationShadow = shadowOf(context.applicationContext as Application)
        while (applicationShadow.nextStartedService != null) {
            // Drain starts issued by the two acquire calls.
        }

        ForegroundGuardian.onServiceStartFailed(context, failedGeneration, "denied")

        assertTrue(ForegroundGuardian.isActive)
        assertFalse(ForegroundGuardian.isScreenKeepOnRequired)
        assertEquals("old", ForegroundGuardian.getNotificationLabel())

        ForegroundGuardian.onServiceDestroyed(context, failedGeneration)

        val recoveryIntent = applicationShadow.nextStartedService
        assertEquals(StreamKeepaliveService::class.java.name, recoveryIntent.component?.className)
        assertTrue(
            recoveryIntent.getLongExtra(StreamKeepaliveService.EXTRA_GENERATION, 0L) > failedGeneration,
        )
        assertTrue(ForegroundGuardian.isActive)
    }

    @Test
    fun readinessCallbackCompletesOnlyAfterServiceAck() {
        val generation = ForegroundGuardian.acquire(
            context, "stream:Nova", ForegroundGuardian.PRIORITY_STREAM, "Nova", false, 0,
        )
        var ready: Boolean? = null
        ForegroundGuardian.awaitServiceReadiness(context, generation) { success, _ ->
            ready = success
        }
        assertEquals(null, ready)

        ForegroundGuardian.onServiceReady(generation)
        shadowOf(Looper.getMainLooper()).idle()

        assertEquals(true, ready)
    }

    @Test
    fun screenKeepOnFlagUsesManualOrGuardianOwnership() {
        val controller = Robolectric.buildActivity(Activity::class.java).setup()
        val testActivity = controller.get()
        val keepScreenOn = WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON

        try {
            ScreenKeepOnArbiter.setAppInForeground(testActivity, true)
            ScreenKeepOnArbiter.setManualRequested(testActivity, true)
            ScreenKeepOnArbiter.setGuardianRequested(testActivity, false)
            shadowOf(Looper.getMainLooper()).idle()
            assertTrue(testActivity.window.attributes.flags and keepScreenOn != 0)

            ScreenKeepOnArbiter.setGuardianRequested(testActivity, true)
            ScreenKeepOnArbiter.setManualRequested(testActivity, false)
            shadowOf(Looper.getMainLooper()).idle()
            assertTrue(testActivity.window.attributes.flags and keepScreenOn != 0)

            ScreenKeepOnArbiter.setGuardianRequested(testActivity, false)
            shadowOf(Looper.getMainLooper()).idle()
            assertFalse(testActivity.window.attributes.flags and keepScreenOn != 0)
        } finally {
            ScreenKeepOnArbiter.setManualRequested(testActivity, false)
            ScreenKeepOnArbiter.setGuardianRequested(testActivity, false)
            ScreenKeepOnArbiter.detach(testActivity)
            controller.pause().stop().destroy()
        }
    }
}
