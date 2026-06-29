package com.vcp.mobile.service

import android.app.NotificationManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.SharedPreferences
import android.util.Log
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import org.json.JSONObject
import java.util.concurrent.TimeUnit

/**
 * 隔离进程通知栏动作接收器 (PushActionReceiver)
 * 
 * 【设计初衷】：
 * 1. 运行在独立的 ":push" 进程中，避免唤醒耗电的 Tauri/WebView 主进程。
 * 2. 用户在通知栏直接点击【允许】或【拒绝】时，触发此广播。
 * 3. 极速建立临时 WebSocket 连接，向 VCPToolBox 发送 `tool_approval_response` 决策包，发送完毕后立即断开连接。
 */
class PushActionReceiver : BroadcastReceiver() {

    companion object {
        private const val TAG = "PushActionReceiver"
        private const val PREFS_NAME = "vcp_push_prefs"
        
        const val ACTION_APPROVE = "com.vcp.mobile.action.APPROVE"
        const val ACTION_DENY = "com.vcp.mobile.action.DENY"
    }

    override fun onReceive(context: Context, intent: Intent?) {
        val action = intent?.action
        val approvalId = intent?.getStringExtra("approval_id") ?: ""
        val notificationId = intent?.getIntExtra("notification_id", -1) ?: -1

        Log.i(TAG, "onReceive: action=$action, approvalId=$approvalId, notificationId=$notificationId")

        if (approvalId.isEmpty()) {
            Log.w(TAG, "Approval ID is empty, ignoring action.")
            return
        }

        val approved = action == ACTION_APPROVE
        
        // 1. 立即清除手机通知栏对应的通知（给用户即时的交互反馈）
        if (notificationId != -1) {
            val notificationManager = context.getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager
            notificationManager?.cancel(notificationId)
        }

        // 2. 异步执行向 VCPToolBox 反馈决策
        sendApprovalResponse(context, approvalId, approved)
    }

    /**
     * 建立临时 WebSocket 发送决策包
     */
    private fun sendApprovalResponse(context: Context, approvalId: String, approved: Boolean) {
        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        val vcpUrl = prefs.getString("vcp_url", "") ?: ""
        val vcpKey = prefs.getString("vcp_key", "") ?: ""

        if (vcpUrl.isEmpty() || vcpKey.isEmpty()) {
            Log.e(TAG, "Credentials missing in SharedPreferences. Cannot send approval response.")
            return
        }

        // 将 HTTP(S) 地址转换为 WS(S) 地址
        var wsUrl = vcpUrl.trimEnd('/')
        wsUrl = when {
            wsUrl.startsWith("https://") -> wsUrl.replaceFirst("https://", "wss://")
            wsUrl.startsWith("http://") -> wsUrl.replaceFirst("http://", "ws://")
            else -> "ws://$wsUrl"
        }

        // 拼接成 VCPLog/VCPInfo 的认证 Websocket 路径
        if (!wsUrl.contains("/VCPlog")) {
            wsUrl = "$wsUrl/VCPlog"
        }
        val finalWsUrl = "$wsUrl/VCP_Key=$vcpKey"

        Log.i(TAG, "Connecting temporary WebSocket to send approval: url=$wsUrl")

        val client = OkHttpClient.Builder()
            .connectTimeout(10, TimeUnit.SECONDS)
            .writeTimeout(10, TimeUnit.SECONDS)
            .readTimeout(10, TimeUnit.SECONDS)
            .build()

        val request = Request.Builder()
            .url(finalWsUrl)
            .build()

        client.newWebSocket(request, object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                Log.i(TAG, "Temporary WebSocket opened. Sending approval response...")
                
                // 构造标准 VCP 协议的决策包
                val dataObj = JSONObject().apply {
                    put("requestId", approvalId)
                    put("approved", approved)
                    put("reason", "Approved via VCPMobile Notification Action")
                }
                val messageObj = JSONObject().apply {
                    put("type", "tool_approval_response")
                    put("data", dataObj)
                }

                val success = webSocket.send(messageObj.toString())
                Log.i(TAG, "Approval payload sent, success=$success")
                
                // 延迟 500ms 确保数据完全刷入 TCP 缓冲区，然后关闭连接
                try {
                    Thread.sleep(500)
                } catch (_: InterruptedException) {}
                webSocket.close(1000, "Approval sent")
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                Log.e(TAG, "Temporary WebSocket failed: ${t.message}", t)
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                Log.i(TAG, "Temporary WebSocket closed. Code=$code, Reason=$reason")
                // 彻底释放 OkHttp 线程池资源
                client.dispatcher.executorService.shutdown()
            }
        })
    }
}
