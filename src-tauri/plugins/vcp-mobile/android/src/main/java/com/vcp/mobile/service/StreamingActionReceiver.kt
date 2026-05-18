package com.vcp.mobile.service

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * 流式通知动作接收器
 *
 * 处理前台服务通知栏的 Action 按钮点击事件，
 * 目前仅支持「停止生成」——向应用内广播中断信号。
 */
class StreamingActionReceiver : BroadcastReceiver() {

    companion object {
        const val STREAM_INTERRUPT_ACTION = "com.vcp.avatar.STREAM_INTERRUPT"
    }

    override fun onReceive(context: Context, intent: Intent) {
        when (intent.action) {
            StreamKeepaliveService.ACTION_STOP_STREAMING -> {
                // 向应用内部发送中断广播
                val interruptIntent = Intent(STREAM_INTERRUPT_ACTION).apply {
                    setPackage(context.packageName)
                }
                context.sendBroadcast(interruptIntent)
            }
        }
    }
}
