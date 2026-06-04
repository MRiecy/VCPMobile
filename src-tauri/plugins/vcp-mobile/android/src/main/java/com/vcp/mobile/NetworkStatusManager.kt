package com.vcp.mobile

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import app.tauri.plugin.JSObject
import java.net.Inet4Address
import java.net.NetworkInterface

/**
 * 独立的网络连接与带宽状态管理器 (高解耦、模块化设计)
 */
class NetworkStatusManager(private val context: Context) {

    fun getNetworkStatus(): JSObject {
        val result = JSObject()
        try {
            val cm = context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
            val activeNetwork = cm.activeNetwork
            if (activeNetwork == null) {
                result.put("connected", false)
                result.put("type", "未连接")
                result.put("downSpeedKbps", 0)
                result.put("upSpeedKbps", 0)
                result.put("ip", "未分配")
                return result
            }

            val capabilities = cm.getNetworkCapabilities(activeNetwork)
            if (capabilities == null) {
                result.put("connected", false)
                result.put("type", "未连接")
                result.put("downSpeedKbps", 0)
                result.put("upSpeedKbps", 0)
                result.put("ip", "未分配")
                return result
            }

            result.put("connected", true)
            val type = when {
                capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> "WiFi"
                capabilities.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> "移动数据"
                capabilities.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> "以太网"
                else -> "未知"
            }
            result.put("type", type)

            // 下行与上行带宽估计值 (单位: Kbps)
            val downSpeed = capabilities.linkDownstreamBandwidthKbps
            val upSpeed = capabilities.linkUpstreamBandwidthKbps
            result.put("downSpeedKbps", downSpeed)
            result.put("upSpeedKbps", upSpeed)

            // 获取本地 IPv4 地址
            val ip = getLocalIpAddress()
            result.put("ip", ip ?: "未分配")

        } catch (e: Exception) {
            result.put("connected", false)
            result.put("type", "未连接")
            result.put("downSpeedKbps", 0)
            result.put("upSpeedKbps", 0)
            result.put("ip", "未分配")
        }
        return result
    }

    private fun getLocalIpAddress(): String? {
        try {
            val en = NetworkInterface.getNetworkInterfaces()
            while (en.hasMoreElements()) {
                val intf = en.nextElement()
                val enumIpAddr = intf.inetAddresses
                while (enumIpAddr.hasMoreElements()) {
                    val inetAddress = enumIpAddr.nextElement()
                    if (!inetAddress.isLoopbackAddress && inetAddress is Inet4Address) {
                        return inetAddress.hostAddress
                    }
                }
            }
        } catch (ex: Exception) {
            // ignore
        }
        return null
    }
}
