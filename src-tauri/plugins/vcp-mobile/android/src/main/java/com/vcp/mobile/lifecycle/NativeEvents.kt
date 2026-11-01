package com.vcp.mobile.lifecycle

import android.app.Activity
import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.Channel
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import com.vcp.mobile.runtime.PluginTaskRuntime

/** One native Channel and one default-network observer, serialized with lifecycle snapshots on main. */
internal class NativeEvents(
    private val activity: Activity,
    private val runtime: PluginTaskRuntime,
    private val lifecycleSnapshot: () -> JSObject,
) {
    private companion object { const val TAG = "VcpMobilePlugin" }
    private data class NetworkStatus(val network: Network?, val connected: Boolean)
    private var networkCallback: ConnectivityManager.NetworkCallback? = null
    private var defaultNetwork: Network? = null
    private var networkStatus = NetworkStatus(null, false)
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
                    emitNetworkStatus()
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

    private fun emitNetworkStatus() {
        emitNativeEvent("network", JSObject().apply { put("connected", networkStatus.connected) })
    }

    private fun updateNetworkStatus(network: Network?, connected: Boolean) {
        val next = NetworkStatus(network, connected)
        if (next == networkStatus) return
        networkStatus = next
        emitNetworkStatus()
    }

    private fun startNetworkMonitoringInternal() {
        if (networkCallback != null) return
        val cm = activity.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        val callback = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                defaultNetwork = network
                // On Q+ Android immediately supplies the UID's initial blocked state.
                if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) updateNetworkStatus(network, true)
            }
            override fun onBlockedStatusChanged(network: Network, blocked: Boolean) {
                if (network == defaultNetwork) updateNetworkStatus(network, !blocked)
            }
            override fun onLost(network: Network) {
                if (network != defaultNetwork) return
                defaultNetwork = null
                updateNetworkStatus(null, false)
            }
        }
        cm.registerDefaultNetworkCallback(callback, Handler(Looper.getMainLooper()))
        networkCallback = callback
        // Subscribe first. A disconnected device may receive no initial callback;
        // activeNetwork is also null when the default network is blocked for this UID.
        defaultNetwork = cm.activeNetwork
        networkStatus = NetworkStatus(defaultNetwork, defaultNetwork != null)
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
                val cm = activity.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
                cm.unregisterNetworkCallback(callback)
                networkCallback = null
            }
        } catch (_: Exception) {}
    }
}

@InvokeArg
class RegisterNativeEventsArgs {
    lateinit var channel: Channel
}
