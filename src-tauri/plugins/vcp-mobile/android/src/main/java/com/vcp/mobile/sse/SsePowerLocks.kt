package com.vcp.mobile.sse

import android.content.Context
import android.net.wifi.WifiManager
import android.os.Build
import android.os.PowerManager
import android.os.SystemClock
import android.util.Log

/** Native locks are updated only while the coordinator's lifecycle monitor is held. */
internal class SsePowerLocks(private val applicationContext: Context) {
    private companion object { const val TAG = "VcpSseProxy" }
    private var wakeLock: PowerManager.WakeLock? = null
    private var wakeLockDeadlineMs: Long = 0
    private var wifiLock: WifiManager.WifiLock? = null

    fun update(hasRunning: Boolean, handoffDeadline: Long) {
        if (hasRunning) {
            acquireWakeLock(0)
            acquireWifiLock()
        } else {
            releaseWifiLock()
            if (handoffDeadline > SystemClock.elapsedRealtime()) acquireWakeLock(handoffDeadline)
            else releaseWakeLock()
        }
    }

    private fun acquireWakeLock(deadlineMs: Long) {
        val appContext = applicationContext
        if (wakeLock == null) {
            val powerManager = appContext.getSystemService(Context.POWER_SERVICE) as? PowerManager
            if (powerManager != null) {
                wakeLock = powerManager.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "VCP:SseProxyWakeLock")
                    .apply { setReferenceCounted(false) }
            }
        }
        wakeLock?.let {
            if (!it.isHeld || wakeLockDeadlineMs != deadlineMs) {
                if (it.isHeld) it.release()
                if (deadlineMs == 0L) it.acquire()
                else it.acquire(maxOf(1, deadlineMs - SystemClock.elapsedRealtime()))
                wakeLockDeadlineMs = deadlineMs
                Log.i(TAG, "SseProxy WakeLock ACQUIRED.")
            }
        }
    }

    private fun acquireWifiLock() {
        if (wifiLock == null) {
            val wifiManager = applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
            if (wifiManager != null) {
                @Suppress("DEPRECATION")
                wifiLock = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    wifiManager.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "VCP:SseProxyWifiLock")
                } else {
                    wifiManager.createWifiLock(WifiManager.WIFI_MODE_FULL, "VCP:SseProxyWifiLock")
                }
            }
        }
        wifiLock?.let {
            if (!it.isHeld) {
                it.acquire()
                Log.i(TAG, "SseProxy WifiLock ACQUIRED.")
            }
        }
    }

    fun release() {
        releaseWakeLock()
        releaseWifiLock()
    }

    private fun releaseWakeLock() {
        wakeLock?.let {
            if (it.isHeld) {
                it.release()
                Log.i(TAG, "SseProxy WakeLock RELEASED.")
            }
        }
        wakeLock = null
        wakeLockDeadlineMs = 0
    }

    private fun releaseWifiLock() {
        wifiLock?.let {
            if (it.isHeld) {
                it.release()
                Log.i(TAG, "SseProxy WifiLock RELEASED.")
            }
        }
        wifiLock = null
    }
}
