package com.vcp.mobile.service

import android.app.Application
import android.content.Context
import androidx.test.core.app.ApplicationProvider
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28])
class ForegroundGuardianTest {
    private val context: Context = ApplicationProvider.getApplicationContext()

    @After
    fun tearDown() {
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
}
