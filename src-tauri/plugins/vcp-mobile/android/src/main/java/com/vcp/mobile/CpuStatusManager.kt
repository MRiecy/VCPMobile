package com.vcp.mobile

import android.content.Context
import android.os.Build
import android.os.PowerManager
import app.tauri.plugin.JSObject

/**
 * 独立的系统 CPU 热状态分级管理器 (高解耦、模块化设计)
 */
class CpuStatusManager(private val context: Context) {

    fun getThermalStatus(): JSObject {
        val result = JSObject()
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                val powerManager = context.getSystemService(Context.POWER_SERVICE) as PowerManager
                val status = powerManager.currentThermalStatus
                val statusStr = when (status) {
                    PowerManager.THERMAL_STATUS_NONE -> "正常"
                    PowerManager.THERMAL_STATUS_LIGHT -> "轻微发热"
                    PowerManager.THERMAL_STATUS_MODERATE -> "中等发热"
                    PowerManager.THERMAL_STATUS_SEVERE -> "严重发热"
                    PowerManager.THERMAL_STATUS_CRITICAL -> "极热(限频)"
                    PowerManager.THERMAL_STATUS_EMERGENCY -> "紧急(限流)"
                    PowerManager.THERMAL_STATUS_SHUTDOWN -> "即将关机"
                    else -> "未知"
                }
                result.put("status", statusStr)
            } else {
                result.put("status", "不支持(API<29)")
            }
        } catch (e: Exception) {
            result.put("status", "未知")
        }
        return result
    }
}
