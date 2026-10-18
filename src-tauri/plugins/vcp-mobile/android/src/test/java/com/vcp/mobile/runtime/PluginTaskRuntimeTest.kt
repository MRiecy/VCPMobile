package com.vcp.mobile.runtime

import app.tauri.plugin.Invoke
import io.mockk.mockk
import io.mockk.verify
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

class PluginTaskRuntimeTest {
    @Test
    fun shutdownRejectsQueuedInvokeOnceWithoutExecutingItsWork() {
        val runtime = PluginTaskRuntime()
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val executed = AtomicBoolean(false)
        val invoke = mockk<Invoke>(relaxed = true)
        try {
            runtime.domains.fileIoExecutor.execute {
                entered.countDown()
                try { release.await(5, TimeUnit.SECONDS) } catch (_: InterruptedException) {}
            }
            assertTrue(entered.await(5, TimeUnit.SECONDS))
            runtime.execute(runtime.domains.fileIoExecutor, invoke, "export") { executed.set(true) }
            runtime.beginDestroy()
            runtime.shutdown()
            runtime.shutdown()
            verify(exactly = 1) { invoke.reject("export unavailable: plugin was destroyed") }
            assertFalse(executed.get())
        } finally {
            release.countDown()
            runtime.beginDestroy()
            runtime.shutdown()
        }
    }

    @Test
    fun admissionAfterDestroyRejectsWithoutSchedulingWork() {
        val runtime = PluginTaskRuntime()
        val invoke = mockk<Invoke>(relaxed = true)
        val executed = AtomicBoolean(false)
        try {
            runtime.beginDestroy()
            runtime.execute(runtime.domains.fileIoExecutor, invoke, "export") { executed.set(true) }
            verify(exactly = 1) { invoke.reject("export unavailable: plugin is shutting down") }
            assertFalse(executed.get())
        } finally {
            runtime.shutdown()
        }
    }
}
