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
    fun distributedLabelFallsBackToAppName() {
        val copy = resolveNotificationCopy("distributed", hasCliJobs = false)
        assertEquals("VCP Mobile", copy.title)
        assertEquals("分布式后台连接维系中...", copy.contentText)
    }

    @Test
    fun legacyBracketedDistributedLabelFallsBackToAppName() {
        // 历史版本 Rust 侧传入的内部标签，不得泄漏为标题
        val copy = resolveNotificationCopy("[分布式连接]", hasCliJobs = false)
        assertEquals("VCP Mobile", copy.title)
        assertEquals("分布式后台连接维系中...", copy.contentText)
    }

    @Test
    fun vcpLogLingerLabelFallsBackToAppName() {
        val copy = resolveNotificationCopy("VCP Log Linger", hasCliJobs = false)
        assertEquals("VCP Mobile", copy.title)
        assertEquals("正在保持后台连接...", copy.contentText)
    }

    @Test
    fun syncLabelStripsDomainTagFromTitle() {
        val copy = resolveNotificationCopy("[数据同步] 增量", hasCliJobs = false)
        assertEquals("增量", copy.title)
        assertEquals("正在与云端服务器进行高精度同步...", copy.contentText)
    }

    @Test
    fun prerenderLabelStripsDomainTagAndFallsBackWhenEmpty() {
        val copy = resolveNotificationCopy("[预渲染重建]", hasCliJobs = false)
        assertEquals("VCP Mobile", copy.title)
        assertEquals("正在优化与加速本地响应缓存...", copy.contentText)
    }

    @Test
    fun agentNameBecomesTitleWithThinkingText() {
        val copy = resolveNotificationCopy("Nova", hasCliJobs = false)
        assertEquals("Nova", copy.title)
        assertEquals("思考中……", copy.contentText)
    }

    @Test
    fun emptyLabelFallsBackToAppNameAndConnectedText() {
        val copy = resolveNotificationCopy("", hasCliJobs = false)
        assertEquals("VCP Mobile", copy.title)
        assertEquals("已连接", copy.contentText)
    }

    @Test
    fun manualKeepaliveBracketTagFallsBackToAppName() {
        // manual_keepalive（旧 acquireWakeLock 兼容 API）的内部域标签同样不得泄漏
        val copy = resolveNotificationCopy("[后台保活]", hasCliJobs = false)
        assertEquals("VCP Mobile", copy.title)
        assertEquals("正在保持后台连接...", copy.contentText)
    }

    @Test
    fun cliJobsTakePrecedenceInContentTextButNotTitle() {
        val copy = resolveNotificationCopy("Nova", hasCliJobs = true)
        assertEquals("Nova", copy.title)
        assertEquals("CLI 任务正在运行；系统仍可能中断", copy.contentText)
    }
}
