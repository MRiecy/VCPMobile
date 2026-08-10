package com.vcp.mobile.contract

import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

class PluginContractTest {
    private val pluginRoot = findPluginRoot()

    private fun findPluginRoot(): File {
        var dir = File(System.getProperty("user.dir") ?: ".").canonicalFile
        repeat(8) {
            val direct = File(dir, "src-tauri/plugins/vcp-mobile")
            if (File(direct, "src/lib.rs").exists()) {
                return direct
            }

            val androidModule = File(dir, "src/lib.rs")
            if (androidModule.exists() && File(dir, "guest-js/index.ts").exists()) {
                return dir
            }

            dir = dir.parentFile ?: return@repeat
        }
        error("无法定位 tauri-plugin-vcp-mobile 根目录，user.dir=${System.getProperty("user.dir")}")
    }

    @Test
    fun defaultPermissionContainsAllRegisteredPluginCommands() {
        val libRs = File(pluginRoot, "src/lib.rs").readText()
        val defaultToml = File(pluginRoot, "permissions/default.toml").readText()

        val registeredCommands = Regex("(?:screen|stream|system)::([a-zA-Z0-9_]+)")
            .findAll(libRs)
            .map { it.groupValues[1] }
            .toSet()

        val missing = registeredCommands.filter { command ->
            !defaultToml.contains("\"$command\"")
        }

        assertTrue("default.toml 缺少插件命令授权: $missing", missing.isEmpty())
    }

    @Test
    fun guestJsStopStreamServicePassesAgentNameArgument() {
        val guestJs = File(pluginRoot, "guest-js/index.ts").readText()

        assertTrue(
            "stopStreamService 应接收 agentName 参数",
            Regex("function\\s+stopStreamService\\s*\\(\\s*agentName\\s*:\\s*string\\s*\\)").containsMatchIn(guestJs),
        )
        assertTrue(
            "stopStreamService invoke 应传递 { agentName }",
            guestJs.contains("plugin:vcp-mobile|stop_streaming_service") && guestJs.contains("{ agentName }"),
        )
    }

    @Test
    fun rustRunMobilePluginMethodNamesExistInKotlinPlugin() {
        val rustSources = listOf("src/system.rs", "src/stream.rs")
            .map { File(pluginRoot, it).readText() }
            .joinToString("\n")
        val kotlinPlugin = File(
            pluginRoot,
            "android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt",
        ).readText()

        val methodNames = Regex("run_mobile_plugin(?:::<[^>]+>)?\\(\\s*\"([A-Za-z0-9_]+)\"")
            .findAll(rustSources)
            .map { it.groupValues[1] }
            .toSet()

        val missing = methodNames.filter { method ->
            !Regex("fun\\s+$method\\s*\\(").containsMatchIn(kotlinPlugin)
        }

        assertTrue("Rust run_mobile_plugin 方法在 Kotlin 中不存在: $missing", missing.isEmpty())
    }

    @Test
    fun helperConnectionAndSessionCleanupUseIdentityChecks() {
        val helper = File(
            pluginRoot,
            "android/src/main/java/com/vcp/mobile/service/SseProxyService.kt",
        ).readText()

        assertTrue("resume 连接必须按引用同一性解绑", helper.contains("activeConnection !== connection"))
        assertTrue("重复 requestId 必须用 putIfAbsent 拒绝覆盖", helper.contains("activeSessions.putIfAbsent(requestId, session)"))
        assertTrue("session 删除必须携带实例身份", helper.contains("activeSessions.remove(requestId, session)"))
        assertTrue("EventSource 安装必须复核 session owner", helper.contains("isServiceRunning && isCurrentSession(session)"))
        assertTrue("失去 owner 的 EventSource 必须立即取消", helper.contains("source.cancel()"))
        assertTrue("旧 raw output 所有权字段不应残留", !helper.contains("activeSocketOutputStream"))
    }

    @Test
    fun pluginExecutorsAreIsolatedBoundedAndCancellable() {
        val kotlinPlugin = File(
            pluginRoot,
            "android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt",
        ).readText()

        assertTrue("OOM guard 必须使用 fixed-delay scheduler", kotlinPlugin.contains("scheduleWithFixedDelay"))
        assertTrue("Root 命令必须使用独立 executor", kotlinPlugin.contains("executorDomains.rootExecutor"))
        assertTrue("文件任务必须使用独立 executor", kotlinPlugin.contains("executorDomains.fileIoExecutor"))
        assertTrue("文件域默认保持串行语义", kotlinPlugin.contains("fileThreadCount: Int = 1"))
        assertTrue("执行队列必须有界", kotlinPlugin.contains("ArrayBlockingQueue"))
        assertTrue("非 Root 设备只探测一次后退出", kotlinPlugin.contains("if (!Shell.getShell().isRoot)"))
        assertTrue("销毁必须立即取消执行域", kotlinPlugin.contains("executorDomains.shutdownNow()"))
    }

    @Test
    fun rustMobilePluginCallsDoNotHoldHandleMutexWhileWaitingForKotlin() {
        val libRs = File(pluginRoot, "src/lib.rs").readText()
        val commandSources = listOf("src/system.rs", "src/stream.rs")
            .map { File(pluginRoot, it).readText() }
            .joinToString("\n")

        assertTrue("PluginHandle 必须在锁内 clone", libRs.contains(".as_ref()\n            .cloned()"))
        assertTrue("command 必须通过短锁 helper 获取句柄", commandSources.contains("mobile_plugin_handle()?"))
        assertTrue("command 不得直接持锁跨 run_mobile_plugin", !commandSources.contains("plugin_handle.lock()"))
    }
}
