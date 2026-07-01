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
}
