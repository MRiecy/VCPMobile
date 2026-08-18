package com.vcp.mobile.service

import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.util.Log
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong

/**
 * 前台守护者 (ForegroundGuardian)
 * 
 * 进程级单例，统一负责按消费者声明的 WakeLock / WifiLock 与前台服务 (FGS) 生命周期协同。
 * 采用引用计数机制，支持多模块并发申请锁，按优先级动态校准通知栏文案。
 */
object ForegroundGuardian {
    private const val TAG = "ForegroundGuardian"

    // 优先级常量定义
    const val PRIORITY_OTA = 50
    const val PRIORITY_SYNC = 40
    const val PRIORITY_CLI = 35
    const val PRIORITY_PRERENDER = 30
    const val PRIORITY_STREAM = 20
    const val PRIORITY_DISTRIBUTED = 10

    enum class ConsumerKind {
        REMOTE_MESSAGING,
        CLI_JOB,
    }

    data class CliNotificationTarget(
        val jobId: String,
        val attemptId: String,
        val runtimeGeneration: Long,
        val displayLabel: String,
    )

    // 消费者注册表：唯一业务 Tag -> 消费者配置
    private val consumers = ConcurrentHashMap<String, ConsumerEntry>()

    // 全局物理锁实例
    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: WifiManager.WifiLock? = null

    // 超时自动释放任务调度器
    private val handler = Handler(Looper.getMainLooper())
    private val timeoutRunnables = ConcurrentHashMap<String, Runnable>()
    private val generationCounter = AtomicLong(0)
    private val pendingAcquires = ConcurrentHashMap<Long, PendingAcquire>()
    private val pendingGenerations = ConcurrentHashMap.newKeySet<Long>()
    private val restartAfterDestroy = ConcurrentHashMap<Long, Long>()
    private val readinessWaiters = ConcurrentHashMap<Long, (Boolean, String?) -> Unit>()
    private val readinessOutcomes = ConcurrentHashMap<Long, Pair<Boolean, String?>>()
    private val readinessTimeouts = ConcurrentHashMap<Long, Runnable>()
    @Volatile private var desiredGeneration = 0L
    @Volatile private var expectedStopGeneration = 0L
    @Volatile private var screenStateListener: ((Boolean) -> Unit)? = null
    @Volatile private var cliLeaseLossListener: ((List<CliNotificationTarget>) -> Unit)? = null

    data class ConsumerEntry(
        val priority: Int,
        val displayLabel: String,
        val screenKeepOn: Boolean,
        val generation: Long,
        val needsCpu: Boolean,
        val needsNetwork: Boolean,
        val kind: ConsumerKind,
        val cliTarget: CliNotificationTarget?,
    )

    private data class PendingAcquire(
        val previousEntry: ConsumerEntry?,
        val previousDesiredGeneration: Long,
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
    fun acquire(
        context: Context,
        tag: String,
        priority: Int,
        label: String,
        screenKeepOn: Boolean = false,
        timeoutMs: Long = -1,
        needsCpu: Boolean = true,
        needsNetwork: Boolean = true,
        kind: ConsumerKind = ConsumerKind.REMOTE_MESSAGING,
        cliTarget: CliNotificationTarget? = null,
    ): Long {
        Log.i(TAG, "acquire: tag=$tag, priority=$priority, label=$label, screenKeepOn=$screenKeepOn, timeoutMs=$timeoutMs")
        
        // 1. 取消该 tag 已有的超时任务
        timeoutRunnables.remove(tag)?.let {
            handler.removeCallbacks(it)
        }

        val wasEmpty = consumers.isEmpty()
        val previous = consumers[tag]
        val pendingAcquire = PendingAcquire(previous, desiredGeneration)
        val generation = generationCounter.incrementAndGet()
        desiredGeneration = generation
        pendingGenerations.add(generation)
        pendingAcquires[generation] = pendingAcquire
        
        // 更新/插入消费者
        require((kind == ConsumerKind.CLI_JOB) == (cliTarget != null)) {
            "CLI foreground consumers require an exact notification target"
        }
        consumers[tag] = ConsumerEntry(
            priority,
            label,
            screenKeepOn,
            generation,
            needsCpu,
            needsNetwork,
            kind,
            cliTarget,
        )

        try {
            if (wasEmpty) {
                // 首次消费者进入：物理获取系统双锁，并拉起前台服务
                reconcileLocks(context)
                startFgs(context, generation)
            } else {
                // 已有消费者在运行：仅触发 Service 更新通知文案与屏幕状态
                reconcileLocks(context)
                updateFgs(context, generation)
            }
        } catch (error: Exception) {
            rollbackAcquire(context, tag, generation, pendingAcquire, false)
            throw error
        }
        notifyScreenState()

        // 2. 调度超时自动释放任务
        val actualTimeout = if (timeoutMs >= 0) {
            timeoutMs
        } else {
            // 根据不同业务 Tag/优先级 赋予对应的安全超时限制
            when {
                tag.startsWith("stream:") -> 10 * 60 * 1000L // 对话流生成：10 分钟
                tag == "sync" -> 30 * 60 * 1000L        // 增量数据同步：30 分钟
                tag == "ota" -> 60 * 60 * 1000L         // OTA 大文件下载：60 分钟
                tag == "prerender" -> 30 * 60 * 1000L   // 预渲染重建：30 分钟
                tag == "distributed" || tag == "manual_keepalive" -> 2 * 60 * 60 * 1000L // 分布式/手动锁：2 小时
                else -> 15 * 60 * 1000L                 // 默认兜底：15 分钟
            }
        }

        if (actualTimeout > 0) {
            val runnable = Runnable {
                Log.w(TAG, "Timeout reached for tag: $tag. Force releasing to prevent lock leak.")
                releaseIfCurrent(context, tag, generation)
            }
            timeoutRunnables[tag] = runnable
            handler.postDelayed(runnable, actualTimeout)
            Log.d(TAG, "Scheduled timeout for tag: $tag in $actualTimeout ms")
        }
        return generation
    }

    /**
     * 释放前台锁（幂等）
     */
    @Synchronized
    fun release(context: Context, tag: String, expectedGeneration: Long? = null) {
        Log.i(TAG, "release: tag=$tag")
        
        val current = consumers[tag]
        if (current == null) {
            Log.d(TAG, "release: tag=$tag is not registered, ignore.")
            return
        }
        if (expectedGeneration != null && current.generation != expectedGeneration) {
            Log.d(
                TAG,
                "release: stale generation for tag=$tag expected=$expectedGeneration actual=${current.generation}",
            )
            return
        }

        // 只有通过代次 fence 后才能移除当前 lease 的超时任务。
        timeoutRunnables.remove(tag)?.let {
            handler.removeCallbacks(it)
        }

        val removedEntry = consumers.remove(tag, current).let { removed ->
            if (!removed) return
            current
        }
        if (pendingGenerations.remove(removedEntry.generation)) {
            pendingAcquires.remove(removedEntry.generation)
            completeReadiness(
                removedEntry.generation,
                false,
                "Foreground lease released before service readiness",
            )
        }
        notifyScreenState()

        if (consumers.isEmpty()) {
            // 最后一个消费者退出：物理释放系统双锁，并停用前台服务
            releaseLocks()
            stopFgs(context)
        } else {
            reconcileLocks(context)
            // 仍有消费者在运行：更新通知文案与屏幕状态
            try {
                updateFgs(context, generationCounter.incrementAndGet())
            } catch (error: Exception) {
                Log.e(TAG, "Failed to refresh foreground service after release", error)
                releaseAll(context)
                throw error
            }
        }
    }

    @Synchronized
    private fun releaseIfCurrent(context: Context, tag: String, generation: Long) {
        if (consumers[tag]?.generation != generation) {
            Log.d(TAG, "Ignoring stale timeout for tag=$tag generation=$generation")
            return
        }
        release(context, tag, generation)
    }

    @Synchronized
    fun activeCliTargets(): List<Pair<String, CliNotificationTarget>> = consumers.entries
        .mapNotNull { entry -> entry.value.cliTarget?.let { entry.key to it } }
        .sortedBy { it.first }

    @Synchronized
    fun foregroundServiceTypeMask(): Int {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE) return 0
        var mask = 0
        if (consumers.values.any { it.kind == ConsumerKind.REMOTE_MESSAGING }) {
            mask = mask or ServiceInfo.FOREGROUND_SERVICE_TYPE_REMOTE_MESSAGING
        }
        if (consumers.values.any { it.kind == ConsumerKind.CLI_JOB }) {
            mask = mask or ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE
        }
        return mask
    }

    @Synchronized
    fun releaseNonCliConsumers(context: Context) {
        consumers.entries
            .filter { it.value.kind != ConsumerKind.CLI_JOB }
            .map { it.key to it.value.generation }
            .forEach { (tag, generation) -> release(context, tag, generation) }
    }

    /**
     * 进程毁灭或前台服务销毁时的自我了断，强行释放全部物理锁，防止锁泄露
     */
    @Synchronized
    fun releaseAllLocks() {
        Log.w(TAG, "releaseAllLocks: Force clearing all locks and consumers.")
        // 取消所有待执行的超时任务
        for (runnable in timeoutRunnables.values) {
            handler.removeCallbacks(runnable)
        }
        timeoutRunnables.clear()
        for (runnable in readinessTimeouts.values) handler.removeCallbacks(runnable)
        readinessTimeouts.clear()
        val pendingWaiters = readinessWaiters.values.toList()
        readinessWaiters.clear()
        readinessOutcomes.clear()
        pendingAcquires.clear()
        pendingGenerations.clear()
        restartAfterDestroy.clear()
        consumers.clear()
        desiredGeneration = 0L
        expectedStopGeneration = 0L
        releaseLocks()
        notifyScreenState()
        for (waiter in pendingWaiters) {
            handler.post { waiter(false, "Foreground Guardian released before readiness") }
        }
    }

    @Synchronized
    fun releaseAll(context: Context) {
        releaseAllLocks()
        stopFgs(context)
    }

    @Synchronized
    fun setScreenStateListener(listener: ((Boolean) -> Unit)?) {
        screenStateListener = listener
        notifyScreenState()
    }

    @Synchronized
    fun setCliLeaseLossListener(listener: ((List<CliNotificationTarget>) -> Unit)?) {
        cliLeaseLossListener = listener
    }

    @Synchronized
    fun onServiceReady(generation: Long) {
        pendingGenerations.remove(generation)
        pendingAcquires.remove(generation)
        restartAfterDestroy.entries
            .filter { it.value <= generation }
            .forEach { restartAfterDestroy.remove(it.key, it.value) }
        completeReadiness(generation, true, null)
        if (generation < desiredGeneration) {
            Log.d(TAG, "Ignoring stale service ready generation=$generation desired=$desiredGeneration")
            return
        }
        Log.i(TAG, "Foreground service ready generation=$generation")
    }

    @Synchronized
    fun onServiceStartFailed(context: Context, generation: Long, message: String) {
        pendingGenerations.remove(generation)
        val pendingAcquire = pendingAcquires.remove(generation)
        Log.e(TAG, "Foreground service failed generation=$generation: $message")
        if (pendingAcquire != null) {
            consumers.entries.firstOrNull { it.value.generation == generation }?.let { entry ->
                rollbackAcquire(context, entry.key, generation, pendingAcquire, true)
            }
        }
        completeReadiness(generation, false, message)
    }

    @Synchronized
    fun awaitServiceReadiness(
        context: Context,
        generation: Long,
        callback: (Boolean, String?) -> Unit,
    ) {
        val outcome = readinessOutcomes.remove(generation)
        if (outcome != null) {
            handler.post { callback(outcome.first, outcome.second) }
            return
        }
        readinessWaiters[generation] = callback
        val timeout = Runnable {
            onServiceStartFailed(context, generation, "Foreground service readiness timed out")
        }
        readinessTimeouts[generation] = timeout
        handler.postDelayed(timeout, 5_000L)
    }

    @Synchronized
    fun onServiceDestroyed(context: Context, generation: Long) {
        if (generation <= 0L) {
            Log.w(TAG, "Ignoring service destroy without a generation; readiness timeout owns rollback")
            return
        }
        val recoveryGeneration = restartAfterDestroy.remove(generation)
        if (recoveryGeneration != null) {
            if (expectedStopGeneration == generation) expectedStopGeneration = 0L
            if (consumers.isNotEmpty() && desiredGeneration == recoveryGeneration) {
                Log.i(
                    TAG,
                    "Restarting foreground service after failed generation=$generation as generation=$recoveryGeneration",
                )
                try {
                    startFgs(context, recoveryGeneration)
                } catch (error: Exception) {
                    Log.e(TAG, "Foreground service recovery start failed", error)
                    releaseAllLocks()
                }
            } else if (consumers.isEmpty() && desiredGeneration == recoveryGeneration) {
                desiredGeneration = 0L
            }
            return
        }
        if (generation == expectedStopGeneration) {
            expectedStopGeneration = 0L
            if (consumers.isEmpty()) desiredGeneration = 0L
            return
        }
        if (generation < desiredGeneration) {
            Log.d(TAG, "Ignoring stale service destroy generation=$generation desired=$desiredGeneration")
            return
        }
        if (consumers.isNotEmpty()) {
            Log.e(TAG, "Foreground service destroyed unexpectedly at generation=$generation")
            val affectedCliTargets = activeCliTargets().map { it.second }
            releaseAllLocks()
            if (affectedCliTargets.isNotEmpty()) {
                handler.post { cliLeaseLossListener?.invoke(affectedCliTargets) }
            }
        }
    }

    private fun notifyScreenState() {
        screenStateListener?.invoke(isScreenKeepOnRequired)
    }

    private fun completeReadiness(generation: Long, success: Boolean, message: String?) {
        readinessTimeouts.remove(generation)?.let(handler::removeCallbacks)
        val waiter = readinessWaiters.remove(generation)
        if (waiter != null) {
            handler.post { waiter(success, message) }
        } else {
            readinessOutcomes[generation] = Pair(success, message)
            handler.postDelayed({ readinessOutcomes.remove(generation) }, 10_000L)
        }
    }

    private fun rollbackAcquire(
        context: Context,
        tag: String,
        generation: Long,
        pendingAcquire: PendingAcquire,
        failedServiceWillStop: Boolean,
    ) {
        if (consumers[tag]?.generation != generation) return
        val isDesiredFailure = desiredGeneration == generation
        pendingGenerations.remove(generation)
        pendingAcquires.remove(generation)
        if (pendingAcquire.previousEntry == null) {
            consumers.remove(tag)
        } else {
            consumers[tag] = pendingAcquire.previousEntry
        }
        timeoutRunnables.remove(tag)?.let(handler::removeCallbacks)
        if (isDesiredFailure) {
            desiredGeneration = pendingAcquire.previousDesiredGeneration
        }
        if (consumers.isEmpty()) {
            releaseLocks()
            if (failedServiceWillStop && isDesiredFailure) {
                stopFgs(context, generation)
            }
        } else {
            reconcileLocks(context)
            if (failedServiceWillStop && isDesiredFailure) {
                val recoveryGeneration = generationCounter.incrementAndGet()
                desiredGeneration = recoveryGeneration
                restartAfterDestroy[generation] = recoveryGeneration
                stopFgs(context, generation)
            }
        }
        notifyScreenState()
    }

    /**
     * 物理获取 WakeLock 和 WifiLock
     */
    private fun reconcileLocks(context: Context) {
        val appContext = context.applicationContext

        val needCpu = consumers.values.any { it.needsCpu }
        val needNetwork = consumers.values.any { it.needsNetwork }

        if (needCpu && wakeLock == null) {
            val powerManager = appContext.getSystemService(Context.POWER_SERVICE) as? PowerManager
            if (powerManager != null) {
                wakeLock = powerManager.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "VCP:ForegroundGuardian")
            }
        }
        wakeLock?.let {
            if (!needCpu && it.isHeld) it.release()
            if (!it.isHeld) {
                if (needCpu) {
                    it.acquire()
                    Log.d(TAG, "reconcileLocks: WakeLock acquired.")
                }
            }
        }
        if (!needCpu) wakeLock = null

        if (needNetwork && wifiLock == null) {
            val wifiManager = appContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
            if (wifiManager != null) {
                @Suppress("DEPRECATION")
                wifiLock = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    wifiManager.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "VCP:ForegroundGuardianWifi")
                } else {
                    wifiManager.createWifiLock(WifiManager.WIFI_MODE_FULL, "VCP:ForegroundGuardianWifi")
                }
            }
        }
        wifiLock?.let {
            if (!needNetwork && it.isHeld) it.release()
            if (!it.isHeld) {
                if (needNetwork) {
                    it.acquire()
                    Log.d(TAG, "reconcileLocks: WifiLock acquired.")
                }
            }
        }
        if (!needNetwork) wifiLock = null
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

    private fun startFgs(context: Context, generation: Long) {
        Log.i(TAG, "startFgs: Starting StreamKeepaliveService...")
        val intent = Intent(context.applicationContext, StreamKeepaliveService::class.java).apply {
            putExtra(StreamKeepaliveService.EXTRA_GENERATION, generation)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            context.applicationContext.startForegroundService(intent)
        } else {
            context.applicationContext.startService(intent)
        }
    }

    private fun updateFgs(context: Context, generation: Long) {
        Log.d(TAG, "updateFgs: Updating StreamKeepaliveService notification...")
        desiredGeneration = generation
        val intent = Intent(context.applicationContext, StreamKeepaliveService::class.java).apply {
            putExtra(StreamKeepaliveService.EXTRA_GENERATION, generation)
        }
        // 重复调用 startForegroundService 会触发 onStartCommand，轻量更新通知文案
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            context.applicationContext.startForegroundService(intent)
        } else {
            context.applicationContext.startService(intent)
        }
    }

    private fun stopFgs(context: Context, expectedGeneration: Long = desiredGeneration) {
        Log.i(TAG, "stopFgs: Stopping StreamKeepaliveService...")
        expectedStopGeneration = expectedGeneration
        val intent = Intent(context.applicationContext, StreamKeepaliveService::class.java)
        try {
            context.applicationContext.stopService(intent)
        } catch (e: Exception) {
            Log.e(TAG, "stopFgs failed: ", e)
        }
    }
}
