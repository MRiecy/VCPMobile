package com.vcp.mobile.cli

import app.tauri.plugin.JSObject
import java.util.concurrent.atomic.AtomicBoolean

/** A queued request is either claimed by its worker or rejected by host shutdown. */
internal class CliPendingRequest(
    private val success: (JSObject) -> Unit,
    private val failure: (String) -> Unit,
    private val action: () -> JSObject,
) : Runnable {
    private val claimed = AtomicBoolean(false)

    override fun run() {
        if (!claimed.compareAndSet(false, true)) return
        val result = try {
            action()
        } catch (error: Throwable) {
            failure(error.message ?: error.javaClass.simpleName)
            return
        }
        // A delivery failure must not invoke the other terminal callback.
        success(result)
    }

    fun rejectBeforeStart(reason: String) {
        if (claimed.compareAndSet(false, true)) failure(reason)
    }
}
