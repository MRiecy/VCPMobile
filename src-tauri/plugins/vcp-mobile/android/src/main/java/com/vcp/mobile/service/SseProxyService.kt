package com.vcp.mobile.service

import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.util.Log
import com.vcp.mobile.sse.SseNotifications
import com.vcp.mobile.sse.SseNotifications.Companion.NOTIFICATION_ID_SERVICE
import com.vcp.mobile.sse.SseSessionCoordinator

/**
 * 隔离进程网络助手 (SseProxyService)
 * 
 * 【职责】：
 * 1. 运行在独立的 ":helper" 进程中，通过本地 TCP 套接字与主进程通信，彻底避免 Binder 限制与高频 IPC 开销。
 * 2. 下载正文保留在内存；终态未在 5 秒内确认提交时原子发布离线结果。
 * 3. WifiLock 覆盖网络接收，WakeLock 另外覆盖有界终态交接，闲置后退出。
 */
class SseProxyService : Service() {

    private companion object {
        const val TAG = "VcpSseProxy"
    }

    private val notifications by lazy { SseNotifications(this) }
    private val sessions by lazy {
        SseSessionCoordinator(applicationContext, stopService = {
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
        })
    }

    override fun onCreate() {
        super.onCreate()
        sessions.activate()
        notifications.createChannels()
        
        // 启动为前台服务，获得后台守护资格
        val serviceNotification = notifications.serviceNotification()
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                startForeground(
                    NOTIFICATION_ID_SERVICE,
                    serviceNotification,
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_REMOTE_MESSAGING
                )
            } else {
                startForeground(NOTIFICATION_ID_SERVICE, serviceNotification)
            }
        } catch (e: Exception) {
            Log.e(TAG, "startForeground failed: ", e)
            sessions.destroy()
            stopSelf()
            return
        }

        // 启动本地 TCP 服务端
        sessions.startListening()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // Discovery is published once with the listener. Rewriting an unchanged
        // port used to expose an empty truncate/write window to control calls.
        sessions.onStart()
        return START_NOT_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? {
        // 本地 TCP 架构下，主进程不再绑定此服务，直接通过 TCP 连接通信
        return null
    }

    override fun onDestroy() {
        sessions.destroy()
        super.onDestroy()
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        super.onTaskRemoved(rootIntent)
        sessions.onTaskRemoved()
    }
}
