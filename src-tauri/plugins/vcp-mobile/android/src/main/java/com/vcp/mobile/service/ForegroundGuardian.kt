package com.vcp.mobile.service

import android.content.Context
import android.content.Intent
import android.net.wifi.WifiManager
import android.os.Build
import android.os.PowerManager
import android.util.Log
import java.util.concurrent.ConcurrentHashMap

/**
 * 前台守护者 (ForegroundGuardian)
 * 
 * 进程级单例，统一负责双锁 (WakeLock + WifiLock) 与前台服务 (FGS) 的生命周期协同。
 * 采用引用计数机制，支持多模块并发申请锁，按优先级动态校准通知栏文案。
 */
object ForegroundGuardian {
    private const val TAG = "ForegroundGuardian"

    // 优先级常量定义
    const val PRIORITY_SYNC = 40
    const val PRIORITY_PRERENDER = 30
    const val PRIORITY_STREAM = 20
    const val PRIORITY_DISTRIBUTED = 10

    // 消费者注册表：唯一业务 Tag -> 消费者配置
    private val consumers = ConcurrentHashMap<String, ConsumerEntry>()

    // 全局物理锁实例
    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: WifiManager.WifiLock? = null

    data class ConsumerEntry(
        val priority: Int,
        val displayLabel: String,
        val screenKeepOn: Boolean
    )

    /**
     * 当前是否有任何活动消费者
     */
    val isActive: Boolean
        get() = consumers.isNotEmpty()

    /**
     * 当前是否需要保持屏幕常亮
     */
    val isScreenKeepOnRequired: Boolean
        get() = consumers.values.any { it.screenKeepOn }

    /**
     * 获取当前处于活动状态的消费者中，优先级最高者的通知文案
     */
    fun getNotificationLabel(): String {
        return consumers.values.maxByOrNull { it.priority }?.displayLabel ?: "VCP 正在后台运行"
    }

    /**
     * 申请持有前台锁（幂等）
     */
    @Synchronized
    fun acquire(context: Context, tag: String, priority: Int, label: String, screenKeepOn: Boolean = false) {
        Log.i(TAG, "acquire: tag=$tag, priority=$priority, label=$label, screenKeepOn=$screenKeepOn")
        val wasEmpty = consumers.isEmpty()
        
        // 更新/插入消费者
        consumers[tag] = ConsumerEntry(priority, label, screenKeepOn)

        if (wasEmpty) {
            // 首次消费者进入：物理获取系统双锁，并拉起前台服务
            acquireLocks(context)
            startFgs(context)
        } else {
            // 已有消费者在运行：仅触发 Service 更新通知文案与屏幕状态
            updateFgs(context)
        }
    }

    /**
     * 释放前台锁（幂等）
     */
    @Synchronized
    fun release(context: Context, tag: String) {
        Log.i(TAG, "release: tag=$tag")
        if (!consumers.containsKey(tag)) {
            Log.d(TAG, "release: tag=$tag is not registered, ignore.")
            return
        }

        consumers.remove(tag)

        if (consumers.isEmpty()) {
            // 最后一个消费者退出：物理释放系统双锁，并停用前台服务
            releaseLocks()
            stopFgs(context)
        } else {
            // 仍有消费者在运行：更新通知文案与屏幕状态
            updateFgs(context)
        }
    }

    /**
     * 进程毁灭或前台服务销毁时的自我了断，强行释放全部物理锁，防止锁泄露
     */
    @Synchronized
    fun releaseAllLocks() {
        Log.w(TAG, "releaseAllLocks: Force clearing all locks and consumers.")
        consumers.clear()
        releaseLocks()
    }

    /**
     * 物理获取 WakeLock 和 WifiLock
     */
    private fun acquireLocks(context: Context) {
        val appContext = context.applicationContext

        // 1. 获取 WakeLock (保持 CPU 运转)
        if (wakeLock == null) {
            val powerManager = appContext.getSystemService(Context.POWER_SERVICE) as? PowerManager
            if (powerManager != null) {
                wakeLock = powerManager.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "VCP:ForegroundGuardian")
            }
        }
        wakeLock?.let {
            if (!it.isHeld) {
                it.acquire()
                Log.d(TAG, "acquireLocks: WakeLock acquired.")
            }
        }

        // 2. 获取 WifiLock (防止后台 Wi-Fi 睡眠/降速)
        if (wifiLock == null) {
            val wifiManager = appContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
            if (wifiManager != null) {
                wifiLock = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    wifiManager.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "VCP:ForegroundGuardianWifi")
                } else {
                    @Suppress("DEPRECATION")
                    wifiManager.createWifiLock(WifiManager.WIFI_MODE_FULL, "VCP:ForegroundGuardianWifi")
                }
            }
        }
        wifiLock?.let {
            if (!it.isHeld) {
                it.acquire()
                Log.d(TAG, "acquireLocks: WifiLock acquired.")
            }
        }
    }

    /**
     * 物理释放 WakeLock 和 WifiLock
     */
    private fun releaseLocks() {
        wakeLock?.let {
            if (it.isHeld) {
                it.release()
                Log.d(TAG, "releaseLocks: WakeLock released.")
            }
        }
        wakeLock = null

        wifiLock?.let {
            if (it.isHeld) {
                it.release()
                Log.d(TAG, "releaseLocks: WifiLock released.")
            }
        }
        wifiLock = null
    }

    private fun startFgs(context: Context) {
        Log.i(TAG, "startFgs: Starting StreamKeepaliveService...")
        val intent = Intent(context.applicationContext, StreamKeepaliveService::class.java)
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                context.applicationContext.startForegroundService(intent)
            } else {
                context.applicationContext.startService(intent)
            }
        } catch (e: Exception) {
            Log.e(TAG, "startFgs failed: ", e)
        }
    }

    private fun updateFgs(context: Context) {
        Log.d(TAG, "updateFgs: Updating StreamKeepaliveService notification...")
        val intent = Intent(context.applicationContext, StreamKeepaliveService::class.java)
        // 重复调用 startForegroundService 会触发 onStartCommand，轻量更新通知文案
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                context.applicationContext.startForegroundService(intent)
            } else {
                context.applicationContext.startService(intent)
            }
        } catch (e: Exception) {
            Log.e(TAG, "updateFgs failed: ", e)
        }
    }

    private fun stopFgs(context: Context) {
        Log.i(TAG, "stopFgs: Stopping StreamKeepaliveService...")
        val intent = Intent(context.applicationContext, StreamKeepaliveService::class.java)
        try {
            context.applicationContext.stopService(intent)
        } catch (e: Exception) {
            Log.e(TAG, "stopFgs failed: ", e)
        }
    }
}
