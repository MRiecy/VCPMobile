package com.vcp.mobile

import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.BatteryManager
import android.os.Build
import android.os.PowerManager
import app.tauri.plugin.JSObject

/**
 * 独立的设备电量与系统省电状态管理器 (高解耦、模块化设计)
 */
class BatteryStatusManager(private val context: Context) {

    /**
     * 获取设备当前的电量百分比 (0-100)，获取失败时返回 -1
     */
    fun getBatteryLevel(): Int {
        return try {
            val filter = IntentFilter(Intent.ACTION_BATTERY_CHANGED)
            val batteryStatus: Intent? = context.registerReceiver(null, filter)
            
            val level = batteryStatus?.getIntExtra(BatteryManager.EXTRA_LEVEL, -1) ?: -1
            val scale = batteryStatus?.getIntExtra(BatteryManager.EXTRA_SCALE, -1) ?: -1
            
            if (level >= 0 && scale > 0) {
                ((level.toFloat() / scale.toFloat()) * 100).toInt()
            } else {
                -1
            }
        } catch (e: Exception) {
            -1
        }
    }

    /**
     * 检测系统是否处于省电模式 (Power Save Mode)
     */
    fun isPowerSaveMode(): Boolean {
        return try {
            val powerManager = context.getSystemService(Context.POWER_SERVICE) as PowerManager
            powerManager.isPowerSaveMode
        } catch (e: Exception) {
            false
        }
    }

    /**
     * 导出电量百分比与省电模式状态的 JSObject
     */
    fun getStatusJson(): JSObject {
        val result = JSObject()
        result.put("level", getBatteryLevel())
        result.put("isPowerSaveMode", isPowerSaveMode())
        return result
    }
}
