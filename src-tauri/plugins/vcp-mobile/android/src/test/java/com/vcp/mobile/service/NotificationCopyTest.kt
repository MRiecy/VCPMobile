package com.vcp.mobile.service

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * resolveNotificationCopy 是纯函数，无需 Robolectric 运行时。
 *
 * 回归背景：commit d874802 将分布式保活 label 改为 "[分布式连接]" 后，
 * 内部标识未经清洗直接成为常驻通知标题。本测试锁定"内部标识一律回落为
 * 应用名 VCP Mobile"的映射契约。
 */
class NotificationCopyTest {

    @Test
    fun internalDomainLabelsNeverLeakIntoTheNotificationTitle() {
        val cases = listOf(
            Triple("distributed", "VCP Mobile", "分布式后台连接维系中..."),
            Triple("[分布式连接]", "VCP Mobile", "分布式后台连接维系中..."),
            Triple("VCP Log Linger", "VCP Mobile", "正在保持后台连接..."),
            Triple("[数据同步] 增量", "增量", "正在与云端服务器进行高精度同步..."),
            Triple("[预渲染重建]", "VCP Mobile", "正在优化与加速本地响应缓存..."),
            Triple("[后台保活]", "VCP Mobile", "正在保持后台连接..."),
            Triple("", "VCP Mobile", "已连接"),
        )

        cases.forEach { (label, expectedTitle, expectedContent) ->
            val copy = resolveNotificationCopy(label, hasCliJobs = false)
            assertEquals(label, expectedTitle, copy.title)
            assertEquals(label, expectedContent, copy.contentText)
        }
    }

    @Test
    fun agentNameBecomesTitleWithThinkingText() {
        val copy = resolveNotificationCopy("Nova", hasCliJobs = false)
        assertEquals("Nova", copy.title)
        assertEquals("思考中……", copy.contentText)
    }

    @Test
    fun cliJobsTakePrecedenceInContentTextButNotTitle() {
        val copy = resolveNotificationCopy("Nova", hasCliJobs = true)
        assertEquals("Nova", copy.title)
        assertEquals("CLI 任务正在运行；系统仍可能中断", copy.contentText)
    }
}
