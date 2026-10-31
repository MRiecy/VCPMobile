package com.vcp.mobile.voice

import android.media.AudioDeviceInfo
import android.media.AudioManager
import android.os.Build

/** Input and output port IDs differ even on the same headset. For Bluetooth the
 * selectable port is the communication sink; AudioRecord reports its paired source. */
internal data class VoiceInputDevice(
    val id: String,
    val label: String,
    val kind: String,
    val source: AudioDeviceInfo? = null,
    val bluetoothSink: AudioDeviceInfo? = null,
) {
    fun matches(manager: AudioManager, actual: AudioDeviceInfo?): Boolean {
        if (actual == null) return false
        val sink = bluetoothSink ?: return if (kind == "builtin") actual.type == AudioDeviceInfo.TYPE_BUILTIN_MIC
            else source?.id == actual.id
        if (actual.type != sink.type) return false
        if (Build.VERSION.SDK_INT >= 31 && manager.communicationDevice?.id != sink.id) return false
        val sinks = bluetoothOutputs(manager).filter { it.type == sink.type }
        if (sinks.none { it.id == sink.id }) return false
        val sinkAddress = endpointAddress(sink)
        val inputAddress = endpointAddress(actual)
        if (sinkAddress != null && inputAddress != null) return sinkAddress.equals(inputAddress, ignoreCase = true)
        // A hidden address cannot identify two same-profile headsets.
        return sinks.size == 1 && manager.getDevices(AudioManager.GET_DEVICES_INPUTS)
            .none { it.type == sink.type && it.id != actual.id }
    }

    companion object {
        private fun endpointAddress(device: AudioDeviceInfo): String? = if (Build.VERSION.SDK_INT >= 28)
            device.address.takeIf { it.isNotBlank() && it != "02:00:00:00:00:00" && it != "00:00:00:00:00:00" } else null

        @Suppress("DEPRECATION")
        private fun bluetoothOutputs(manager: AudioManager): List<AudioDeviceInfo> {
            if (Build.VERSION.SDK_INT >= 31) return manager.availableCommunicationDevices.filter {
                it.type == AudioDeviceInfo.TYPE_BLUETOOTH_SCO || it.type == AudioDeviceInfo.TYPE_BLE_HEADSET
            }
            if (!manager.isBluetoothScoAvailableOffCall) return emptyList()
            return manager.getDevices(AudioManager.GET_DEVICES_OUTPUTS).filter { it.type == AudioDeviceInfo.TYPE_BLUETOOTH_SCO }
        }

        fun available(manager: AudioManager): List<VoiceInputDevice> {
            val inputs = manager.getDevices(AudioManager.GET_DEVICES_INPUTS)
            val devices = mutableListOf<VoiceInputDevice>()
            inputs.firstOrNull { it.type == AudioDeviceInfo.TYPE_BUILTIN_MIC }?.let {
                devices.add(VoiceInputDevice("builtin", "内置麦克风", "builtin", source = it))
            }
            for (input in inputs) {
                val kind = when (input.type) {
                    AudioDeviceInfo.TYPE_WIRED_HEADSET -> "wired"
                    AudioDeviceInfo.TYPE_USB_HEADSET, AudioDeviceInfo.TYPE_USB_DEVICE, AudioDeviceInfo.TYPE_USB_ACCESSORY -> "usb"
                    AudioDeviceInfo.TYPE_LINE_ANALOG, AudioDeviceInfo.TYPE_LINE_DIGITAL -> "other"
                    else -> continue
                }
                devices.add(VoiceInputDevice("input:${input.id}", input.productName.toString(), kind, source = input))
            }
            val sinks = bluetoothOutputs(manager)
            for (sink in sinks) {
                val sameProfile = sinks.filter { it.type == sink.type }
                if (sameProfile.size > 1 || inputs.count { it.type == sink.type } > 1) {
                    if (Build.VERSION.SDK_INT < 31) continue // startBluetoothSco cannot select an individual headset.
                    val address = endpointAddress(sink) ?: continue
                    // Some devices publish the input only after HFP/LE has been selected.
                    if (sameProfile.count { address.equals(endpointAddress(it), ignoreCase = true) } != 1) continue
                }
                devices.add(VoiceInputDevice("bluetooth:${sink.id}", sink.productName.toString(), "bluetooth", bluetoothSink = sink))
            }
            return devices
        }
    }
}
