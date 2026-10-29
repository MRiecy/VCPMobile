package com.vcp.mobile.lifecycle

import android.app.Activity
import android.content.Context
import android.os.Handler
import android.os.Looper
import android.util.Log
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.Channel
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import com.vcp.mobile.device.NetworkStatusManager
import com.vcp.mobile.runtime.PluginTaskRuntime

/** One native Channel and one default-network observer, serialized with lifecycle snapshots on main. */
internal class NativeEvents(
    private val activity: Activity,
    private val runtime: PluginTaskRuntime,
    private val networkStatusManager: NetworkStatusManager,
    private val lifecycleSnapshot: () -> JSObject,
) {
    private companion object { const val TAG = "VcpMobilePlugin" }
    private var networkCallback: android.net.ConnectivityManager.NetworkCallback? = null
    private var lastConnected: Boolean? = null
    private var lastDefaultNetwork: android.net.Network? = null
    private var isNetworkMonitoringStarted = false
    private var nativeEventChannel: Channel? = null

    internal fun isReady(): Boolean = !runtime.isDestroying && nativeEventChannel != null

    /** Voice probabilities must never be acknowledged while the native transport is absent. */
    internal fun tryEmitVoiceAudio(payload: JSObject): Boolean {
        return tryEmitVoiceEvent("voiceAudio", payload)
    }

    internal fun tryEmitVoicePlayback(payload: JSObject): Boolean {
        return tryEmitVoiceEvent("voicePlayback", payload)
    }

    private fun tryEmitVoiceEvent(event: String, payload: JSObject): Boolean {
        check(Looper.myLooper() == Looper.getMainLooper())
        if (runtime.isDestroying) return false
        val channel = nativeEventChannel ?: return false
        return try {
            channel.send(JSObject().apply { put("event", event); put("payload", payload) })
            true
        } catch (error: Exception) {
            Log.e(TAG, "Voice event channel failed", error)
            nativeEventChannel = null
            false
        }
    }

    internal fun emitNativeEvent(event: String, payload: JSObject) {
        activity.runOnUiThread {
            if (runtime.isDestroying) return@runOnUiThread
            nativeEventChannel?.send(JSObject().apply {
                put("event", event)
                put("payload", payload)
            })
        }
    }

    fun registerNativeEvents(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(RegisterNativeEventsArgs::class.java)
            activity.runOnUiThread {
                if (runtime.isDestroying) {
                    invoke.reject("plugin was destroyed")
                    return@runOnUiThread
                }
                try {
                    // 每次注册替换唯一订阅者；快照与实时事件不经过 WebView JS。
                    nativeEventChannel = args.channel
                    emitNativeEvent("lifecycle", lifecycleSnapshot())
                    startNetworkMonitoringInternal()
                    emitNetworkStatus(force = true)
                    invoke.resolve()
                } catch (error: Exception) {
                    Log.e(TAG, "Failed to register native events", error)
                    invoke.reject(error.message ?: "Failed to register native events")
                }
            }
        } catch (error: Exception) {
            invoke.reject(error.message ?: "Invalid native event registration")
        }
    }

    private fun emitNetworkStatus(force: Boolean = false) {
        if (runtime.isDestroying) return
        val cm = activity.getSystemService(Context.CONNECTIVITY_SERVICE) as android.net.ConnectivityManager
        val network = cm.activeNetwork
        val status = networkStatusManager.getNetworkStatus()
        val connected = status.optBoolean("connected", false)
        if (force || connected != lastConnected || network != lastDefaultNetwork) {
            lastConnected = connected
            lastDefaultNetwork = network
            emitNativeEvent("network", status)
        }
    }

    private fun startNetworkMonitoringInternal() {
        if (isNetworkMonitoringStarted) return
        val cm = activity.getSystemService(Context.CONNECTIVITY_SERVICE) as android.net.ConnectivityManager
        val callback = object : android.net.ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: android.net.Network) {
                emitNetworkStatus()
            }
            override fun onLost(network: android.net.Network) {
                emitNetworkStatus()
            }
            override fun onCapabilitiesChanged(network: android.net.Network, networkCapabilities: android.net.NetworkCapabilities) {
                emitNetworkStatus()
            }
        }
        cm.registerDefaultNetworkCallback(callback, Handler(Looper.getMainLooper()))
        networkCallback = callback
        isNetworkMonitoringStarted = true
        Log.i(TAG, "[Network] Native default network monitoring started.")
    }

    fun startNetworkMonitoring(invoke: Invoke) {
        activity.runOnUiThread {
            if (runtime.isDestroying) {
                invoke.reject("plugin was destroyed")
                return@runOnUiThread
            }
            try {
                startNetworkMonitoringInternal()
                invoke.resolve()
            } catch (error: Exception) {
                Log.e(TAG, "Failed to register network callback", error)
                invoke.reject(error.message ?: "Failed to register network callback")
            }
        }
    }

    fun destroy() {
        nativeEventChannel = null
        try {
            networkCallback?.let { callback ->
                val cm = activity.getSystemService(Context.CONNECTIVITY_SERVICE) as android.net.ConnectivityManager
                cm.unregisterNetworkCallback(callback)
                networkCallback = null
                isNetworkMonitoringStarted = false
            }
        } catch (_: Exception) {}
    }
}

@InvokeArg
class RegisterNativeEventsArgs {
    lateinit var channel: Channel
}
