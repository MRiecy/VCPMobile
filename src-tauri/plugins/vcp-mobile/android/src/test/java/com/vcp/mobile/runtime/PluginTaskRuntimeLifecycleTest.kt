package com.vcp.mobile.runtime

import app.tauri.plugin.Invoke
import com.fasterxml.jackson.databind.ObjectMapper
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28])
class PluginTaskRuntimeLifecycleTest {
    @Test
    fun failedFirstResponseDoesNotStrandLaterQueuedInvokesDuringShutdown() {
        val runtime = PluginTaskRuntime()
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val delivered = mutableListOf<Long>()
        runtime.domains.fileIoExecutor.execute {
            entered.countDown()
            try { release.await() } catch (_: InterruptedException) { }
        }
        try {
            assertTrue(entered.await(5, TimeUnit.SECONDS))
            val first = Invoke(1, "first", 1, 11, { callback, _ ->
                delivered += callback
                throw IllegalStateException("WebView response channel is closed")
            }, "{}", ObjectMapper())
            val second = Invoke(2, "second", 2, 12, { callback, _ -> delivered += callback }, "{}", ObjectMapper())
            runtime.execute(runtime.domains.fileIoExecutor, first, "first") { error("Queued work must not run") }
            runtime.execute(runtime.domains.fileIoExecutor, second, "second") { error("Queued work must not run") }

            runtime.beginDestroy()
            runtime.shutdown()
            assertEquals(listOf(11L, 12L), delivered)
            runtime.shutdown()
            assertEquals(listOf(11L, 12L), delivered)
        } finally {
            release.countDown()
            runtime.shutdown()
        }
    }
}
