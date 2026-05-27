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
     * 检测系统是否处于省电模式 (Power Save Mode) - 三合一补强防线，全面兼容国产定制 ROM
     */
    fun isPowerSaveMode(): Boolean {
        return try {
            // 1. 原生 PowerManager 状态检测
            val powerManager = context.getSystemService(Context.POWER_SERVICE) as PowerManager
            if (powerManager.isPowerSaveMode) {
                return true
            }

            val resolver = context.contentResolver

            // 2. 原生 Android 广播全局低电量/省电标记检测
            val lowPowerGlobal = android.provider.Settings.Global.getInt(resolver, "low_power", 0)
            if (lowPowerGlobal == 1) {
                return true
            }

            // 3. 部分国产厂商 (如小米/华为等) 常用系统设置省电标记检测
            val powerSaveSystem = android.provider.Settings.System.getInt(resolver, "power_save_mode", 0)
            if (powerSaveSystem == 1) {
                return true
            }

            false
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
