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

            // 3. 通用系统省电标记检测
            val powerSaveSystem = android.provider.Settings.System.getInt(resolver, "power_save_mode", 0)
            if (powerSaveSystem == 1) {
                return true
            }

            // 4. 小米 (MIUI / HyperOS) 专用省电标记
            val miuiPowerSave = android.provider.Settings.System.getInt(resolver, "POWER_SAVE_MODE_OPEN", 0)
            if (miuiPowerSave == 1) {
                return true
            }

            // 5. 华为 (EMUI / HarmonyOS) 专用省电标记
            // SmartModeStatus: 1=正常, 2=省电, 3=超级省电, 4=性能
            val hwPowerSave = android.provider.Settings.System.getInt(resolver, "SmartModeStatus", 1)
            if (hwPowerSave == 2 || hwPowerSave == 3) {
                return true
            }

            // 6. Vivo / Oppo 等其他定制 ROM 可能存放在 Global 或 Secure 中
            val globalPowerSave = android.provider.Settings.Global.getInt(resolver, "power_save_mode", 0)
            if (globalPowerSave == 1) {
                return true
            }
            
            false
        } catch (e: Exception) {
            false
        }
    }

    /**
     * 导出电量百分比、充电状态、电池温度与省电模式状态的 JSObject
     */
    fun getStatusJson(): JSObject {
        val result = JSObject()
        try {
            val filter = IntentFilter(Intent.ACTION_BATTERY_CHANGED)
            val batteryStatus: Intent? = context.registerReceiver(null, filter)
            
            // 1. 电量百分比
            val level = batteryStatus?.getIntExtra(BatteryManager.EXTRA_LEVEL, -1) ?: -1
            val scale = batteryStatus?.getIntExtra(BatteryManager.EXTRA_SCALE, -1) ?: -1
            val pct = if (level >= 0 && scale > 0) {
                ((level.toFloat() / scale.toFloat()) * 100).toInt()
            } else {
                -1
            }
            result.put("level", pct)
            
            // 2. 充电状态
            val status = batteryStatus?.getIntExtra(BatteryManager.EXTRA_STATUS, -1) ?: -1
            val statusStr = when (status) {
                BatteryManager.BATTERY_STATUS_CHARGING -> "充电中"
                BatteryManager.BATTERY_STATUS_DISCHARGING -> "放电中"
                BatteryManager.BATTERY_STATUS_FULL -> "已充满"
                BatteryManager.BATTERY_STATUS_NOT_CHARGING -> "未充电"
                else -> "未知"
            }
            result.put("status", statusStr)
            
            // 3. 电池温度 (单位为 0.1 摄氏度，例如 320 代表 32.0°C)
            val temp = batteryStatus?.getIntExtra(BatteryManager.EXTRA_TEMPERATURE, -1) ?: -1
            val tempDouble = if (temp >= 0) temp.toDouble() / 10.0 else -1.0
            result.put("temperature", tempDouble)
            
            // 4. 是否省电模式
            result.put("isPowerSaveMode", isPowerSaveMode())
        } catch (e: Exception) {
            result.put("level", getBatteryLevel())
            result.put("status", "未知")
            result.put("temperature", -1.0)
            result.put("isPowerSaveMode", isPowerSaveMode())
        }
        return result
    }
}
