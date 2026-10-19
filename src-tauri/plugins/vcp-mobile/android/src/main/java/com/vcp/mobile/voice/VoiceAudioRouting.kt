package com.vcp.mobile.voice

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.media.AudioAttributes
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.media.AudioRecordingConfiguration
import android.os.Build
import android.os.Handler
import android.os.Looper
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/** Main-thread route/focus lease. Capture never assumes that an accepted route is already active. */
internal class VoiceAudioRouting(
    context: Context,
    private val changed: (String) -> Unit,
    private val interrupted: (String) -> Unit,
) : AutoCloseable {
    private val context = context.applicationContext
    private val manager = context.getSystemService(Context.AUDIO_SERVICE) as AudioManager
    private val main = Handler(Looper.getMainLooper())
    private val closed = AtomicBoolean(false)
    private val ready = CountDownLatch(1)
    private var communication: CommunicationRoute? = null
    private var deviceRegistered = false
    private var recordingRegistered = false
    private var scoRegistered = false
    private var scoRequested = false
    private var modeRequested = false
    private var focusGranted = false
    private var lastRouteIdentity: String? = null
    @Volatile private var captureSession = -1
    private val focus = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT)
        .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
            .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build())
        .setOnAudioFocusChangeListener({ value ->
            if (!closed.get() && value != AudioManager.AUDIOFOCUS_GAIN) interrupted("audio_focus_lost")
        }, main).build()

    private val devices = object : AudioDeviceCallback() {
        override fun onAudioDevicesAdded(addedDevices: Array<out AudioDeviceInfo>) = safelyReconcileDevices()
        override fun onAudioDevicesRemoved(removedDevices: Array<out AudioDeviceInfo>) = safelyReconcileDevices()
    }
    private val recording = object : AudioManager.AudioRecordingCallback() {
        override fun onRecordingConfigChanged(configs: MutableList<AudioRecordingConfiguration>) {
            if (closed.get() || Build.VERSION.SDK_INT < 29) return
            if (configs.any { it.clientAudioSessionId == captureSession && it.isClientSilenced }) {
                interrupted("microphone_silenced")
            }
        }
    }
    private val scoReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
            if (closed.get()) return
            when (intent?.getIntExtra(AudioManager.EXTRA_SCO_AUDIO_STATE, AudioManager.SCO_AUDIO_STATE_ERROR)) {
                AudioManager.SCO_AUDIO_STATE_CONNECTED -> { reportRoute("sco", "bluetooth"); ready.countDown() }
                AudioManager.SCO_AUDIO_STATE_DISCONNECTED, AudioManager.SCO_AUDIO_STATE_ERROR -> {
                    if (ready.count == 0L && scoRequested) interrupted("bluetooth_disconnected")
                }
            }
        }
    }

    fun open() {
        check(Looper.myLooper() == Looper.getMainLooper())
        check(manager.requestAudioFocus(focus) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED) { "audio_focus_unavailable" }
        focusGranted = true
        manager.mode = AudioManager.MODE_IN_COMMUNICATION
        modeRequested = true
        manager.registerAudioDeviceCallback(devices, main)
        deviceRegistered = true
        manager.registerAudioRecordingCallback(recording, main)
        recordingRegistered = true
        if (Build.VERSION.SDK_INT >= 31) {
            communication = Api31CommunicationRoute(context, manager, ::reportRoute, ready::countDown)
            communication?.open()
        } else {
            @Suppress("DEPRECATION")
            val hasSco = manager.getDevices(AudioManager.GET_DEVICES_ALL).any { it.type == AudioDeviceInfo.TYPE_BLUETOOTH_SCO }
            if (hasSco && manager.isBluetoothScoAvailableOffCall) {
                val sticky = context.registerReceiver(scoReceiver, IntentFilter(AudioManager.ACTION_SCO_AUDIO_STATE_UPDATED))
                scoRegistered = true
                scoRequested = true
                @Suppress("DEPRECATION")
                manager.startBluetoothSco()
                if (sticky?.getIntExtra(AudioManager.EXTRA_SCO_AUDIO_STATE, -1) == AudioManager.SCO_AUDIO_STATE_CONNECTED) {
                    reportRoute("sco", "bluetooth")
                    ready.countDown()
                }
            } else {
                val external = manager.getDevices(AudioManager.GET_DEVICES_OUTPUTS).firstOrNull(::isExternal)
                @Suppress("DEPRECATION")
                manager.isSpeakerphoneOn = external == null
                reportRoute(external?.id?.toString() ?: "builtin", external?.let(::routeName) ?: "speaker")
                ready.countDown()
            }
        }
    }

    fun awaitReady(stopped: () -> Boolean) {
        val deadline = android.os.SystemClock.elapsedRealtime() + 10_000
        while (!ready.await(50, TimeUnit.MILLISECONDS)) {
            if (stopped() || closed.get()) throw InterruptedException("cancelled")
            check(android.os.SystemClock.elapsedRealtime() < deadline) { "communication_route_timeout" }
        }
        if (stopped() || closed.get()) throw InterruptedException("cancelled")
    }

    fun bindRecordingSession(id: Int) { captureSession = id }

    private fun reportRoute(identity: String, route: String) {
        if (identity != lastRouteIdentity) {
            lastRouteIdentity = identity
            changed(route)
        }
    }

    private fun safelyReconcileDevices() {
        runCatching { reconcileDevices() }.onFailure { if (!closed.get()) interrupted("audio_route_unavailable") }
    }

    private fun reconcileDevices() {
        if (closed.get()) return
        if (communication != null) {
            communication?.reconcile()
        } else {
            // Old SCO can change the supported recording format. A new user
            // activation starts a new recorder rather than retaining old PCM.
            val external = manager.getDevices(AudioManager.GET_DEVICES_OUTPUTS).firstOrNull(::isExternal)
            val identity = if (scoRequested) "sco" else external?.id?.toString() ?: "builtin"
            if (captureSession >= 0 && identity != lastRouteIdentity) interrupted("audio_route_changed")
            else refreshLegacyRoute()
        }
    }

    private fun refreshLegacyRoute() {
        if (closed.get()) return
        if (!scoRequested) {
            val external = manager.getDevices(AudioManager.GET_DEVICES_OUTPUTS).firstOrNull(::isExternal)
            reportRoute(external?.id?.toString() ?: "builtin", external?.let(::routeName) ?: "speaker")
        }
    }

    override fun close() {
        if (!closed.compareAndSet(false, true)) return
        ready.countDown()
        if (deviceRegistered) runCatching { manager.unregisterAudioDeviceCallback(devices) }
        if (recordingRegistered) runCatching { manager.unregisterAudioRecordingCallback(recording) }
        runCatching { communication?.close() }
        if (Build.VERSION.SDK_INT < 31) {
            if (scoRequested) runCatching { @Suppress("DEPRECATION") manager.stopBluetoothSco() }
            if (scoRegistered) runCatching { context.unregisterReceiver(scoReceiver) }
            if (modeRequested) runCatching { @Suppress("DEPRECATION") manager.isSpeakerphoneOn = false }
        }
        if (modeRequested) runCatching { manager.mode = AudioManager.MODE_NORMAL }
        if (focusGranted) runCatching { manager.abandonAudioFocusRequest(focus) }
    }

    private interface CommunicationRoute : AutoCloseable {
        fun open()
        fun reconcile()
    }

    /** API 31 symbols are isolated from the class loaded on minSdk 26. */
    @androidx.annotation.RequiresApi(31)
    private class Api31CommunicationRoute(
        private val context: Context,
        private val manager: AudioManager,
        private val report: (String, String) -> Unit,
        private val ready: () -> Unit,
    ) : CommunicationRoute {
        private var selectedId: Int? = null
        private var closed = false
        private var registered = false
        private val listener = AudioManager.OnCommunicationDeviceChangedListener { device ->
            if (!closed) {
                report(device?.id?.toString() ?: "none", routeName(device))
                if (device?.id == selectedId) ready()
            }
        }

        override fun open() {
            manager.addOnCommunicationDeviceChangedListener(context.mainExecutor, listener)
            registered = true
            reconcile()
        }

        override fun reconcile() {
            if (closed) return
            val available = manager.availableCommunicationDevices
            val current = manager.communicationDevice
            val selected = current?.takeIf { isExternal(it) && available.any { candidate -> candidate.id == it.id } }
                ?: available.firstOrNull(::isExternal)
                ?: available.firstOrNull { it.type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER }
                ?: available.firstOrNull()
                ?: throw IllegalStateException("communication_device_unavailable")
            if (selected.id != selectedId) {
                selectedId = selected.id
                check(manager.setCommunicationDevice(selected)) { "communication_route_rejected" }
            }
            val actual = manager.communicationDevice
            report(actual?.id?.toString() ?: "none", routeName(actual))
            if (actual?.id == selectedId) ready()
        }

        override fun close() {
            if (closed) return
            closed = true
            if (registered) runCatching { manager.removeOnCommunicationDeviceChangedListener(listener) }
            if (selectedId != null) runCatching { manager.clearCommunicationDevice() }
        }
    }

    companion object {
        private fun isExternal(device: AudioDeviceInfo): Boolean = device.type in setOf(
            AudioDeviceInfo.TYPE_BLUETOOTH_SCO, AudioDeviceInfo.TYPE_BLE_HEADSET,
            AudioDeviceInfo.TYPE_WIRED_HEADSET, AudioDeviceInfo.TYPE_WIRED_HEADPHONES,
            AudioDeviceInfo.TYPE_USB_HEADSET, AudioDeviceInfo.TYPE_USB_DEVICE, AudioDeviceInfo.TYPE_HEARING_AID,
        )

        fun routeName(device: AudioDeviceInfo?): String = when (device?.type) {
            AudioDeviceInfo.TYPE_BLUETOOTH_SCO, AudioDeviceInfo.TYPE_BLE_HEADSET, AudioDeviceInfo.TYPE_HEARING_AID -> "bluetooth"
            AudioDeviceInfo.TYPE_WIRED_HEADSET, AudioDeviceInfo.TYPE_WIRED_HEADPHONES -> "wired"
            AudioDeviceInfo.TYPE_USB_HEADSET, AudioDeviceInfo.TYPE_USB_DEVICE -> "usb"
            AudioDeviceInfo.TYPE_BUILTIN_EARPIECE -> "earpiece"
            AudioDeviceInfo.TYPE_BUILTIN_SPEAKER, AudioDeviceInfo.TYPE_BUILTIN_MIC -> "speaker"
            else -> "unknown"
        }
    }
}
