package com.vcp.mobile.runtime

import android.util.Log
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger

/** Response delivery can fail after the WebView disappears. It is never a second task outcome. */
internal fun deliverInvokeResponse(operation: String, response: () -> Unit): Boolean = try {
    response()
    true
} catch (error: Throwable) {
    runCatching { Log.e("VcpMobilePlugin", "$operation response delivery failed", error) }
    false
}

internal fun Invoke.deliverError(message: String): Boolean = deliverInvokeResponse(command) { reject(message) }
internal fun Invoke.deliverResult(): Boolean = deliverInvokeResponse(command) { resolve() }
internal fun Invoke.deliverResult(value: JSObject?): Boolean = deliverInvokeResponse(command) { resolve(value) }

internal class PluginExecutorDomains(
    fileThreadCount: Int = 1,
    rootQueueCapacity: Int = 16,
    fileQueueCapacity: Int = 64,
) {
    private val threadSequence = AtomicInteger(0)
    private fun threadFactory(prefix: String) = java.util.concurrent.ThreadFactory { runnable ->
        Thread(runnable, "$prefix-${threadSequence.incrementAndGet()}")
    }

    val oomScheduler = Executors.newSingleThreadScheduledExecutor(threadFactory("vcp-oom-guard"))
    val rootExecutor: ExecutorService = ThreadPoolExecutor(
        1,
        1,
        0L,
        TimeUnit.MILLISECONDS,
        ArrayBlockingQueue(rootQueueCapacity),
        threadFactory("vcp-root"),
        ThreadPoolExecutor.AbortPolicy(),
    )
    val fileIoExecutor: ExecutorService = ThreadPoolExecutor(
        fileThreadCount,
        fileThreadCount,
        0L,
        TimeUnit.MILLISECONDS,
        ArrayBlockingQueue(fileQueueCapacity),
        threadFactory("vcp-file-io"),
        ThreadPoolExecutor.AbortPolicy(),
    )

    fun shutdownNow(): List<Runnable> {
        oomScheduler.shutdownNow()
        return rootExecutor.shutdownNow() + fileIoExecutor.shutdownNow()
    }
}

internal class PendingInvokeTask(
    private val invoke: Invoke,
    private val operation: String,
    private val task: () -> Unit,
    private val reject: (String) -> Unit = { invoke.reject(it) },
) : Runnable {
    private val claimed = AtomicBoolean(false)

    override fun run() {
        if (!claimed.compareAndSet(false, true)) return
        try {
            task()
        } catch (error: Throwable) {
            Log.e("VcpMobilePlugin", "$operation failed", error)
            deliverInvokeResponse(operation) { reject("$operation failed: ${error.message ?: error.javaClass.simpleName}") }
        }
    }

    fun rejectBeforeStart(reason: String) {
        if (claimed.compareAndSet(false, true)) {
            deliverInvokeResponse(operation) { reject("$operation unavailable: $reason") }
        }
    }
}

/** Plugin-scoped work admission. Long-lived CLI/service owners keep their own lifetimes. */
internal class PluginTaskRuntime {
    val domains = PluginExecutorDomains()
    private val destroying = AtomicBoolean(false)
    val isDestroying: Boolean get() = destroying.get()

    fun execute(
        executor: ExecutorService,
        invoke: Invoke,
        operation: String,
        task: () -> Unit,
    ) {
        val pendingTask = PendingInvokeTask(invoke, operation, task)
        if (isDestroying) {
            pendingTask.rejectBeforeStart("plugin is shutting down")
            return
        }
        try {
            executor.execute(pendingTask)
        } catch (_: RejectedExecutionException) {
            pendingTask.rejectBeforeStart("executor queue is full or shutting down")
        }
    }

    fun beginDestroy() { destroying.set(true) }

    fun shutdown() {
        domains.shutdownNow().forEach { task ->
            (task as? PendingInvokeTask)?.rejectBeforeStart("plugin was destroyed")
        }
    }
}
