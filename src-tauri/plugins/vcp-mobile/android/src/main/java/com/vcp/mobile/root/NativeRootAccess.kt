package com.vcp.mobile.root

import android.app.Activity
import android.util.Log
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import com.topjohnwu.superuser.Shell
import com.vcp.mobile.runtime.PluginTaskRuntime
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit

/** Root requests and OOM adjustment use separate runtime domains so neither can starve file I/O. */
internal class NativeRootAccess(private val activity: Activity, private val runtime: PluginTaskRuntime) {
    private companion object { const val TAG = "VcpMobilePlugin" }
    @Volatile private var oomGuardFuture: ScheduledFuture<*>? = null

    fun startOomScoreGuard() {
        oomGuardFuture = runtime.domains.oomScheduler.schedule({
            if (runtime.isDestroying) return@schedule
            try {
                if (!Shell.getShell().isRoot) {
                    Log.i(TAG, "OomScoreGuard disabled: root access is unavailable.")
                    return@schedule
                }

                val pid = android.os.Process.myPid()
                Shell.cmd("echo -900 > /proc/$pid/oom_score_adj").exec()
                if (!runtime.isDestroying) {
                    oomGuardFuture = runtime.domains.oomScheduler.scheduleWithFixedDelay({
                        if (runtime.isDestroying) return@scheduleWithFixedDelay
                        try {
                            Shell.cmd("echo -900 > /proc/$pid/oom_score_adj").exec()
                        } catch (e: Exception) {
                            Log.e(TAG, "OomScoreGuard refresh failed", e)
                        }
                    }, 20L, 20L, TimeUnit.SECONDS)
                }
            } catch (e: Exception) {
                Log.e(TAG, "OomScoreGuard initialization failed", e)
            }
        }, 0L, TimeUnit.MILLISECONDS)
    }

    fun checkRootAccess(invoke: Invoke) {
        runtime.execute(runtime.domains.rootExecutor, invoke, "checkRootAccess") {
            try {
                val isRoot = Shell.getShell().isRoot
                val result = JSObject()
                result.put("isRoot", isRoot)
                invoke.resolve(result)
            } catch (e: Exception) {
                val result = JSObject()
                result.put("isRoot", false)
                invoke.resolve(result)
            }
        }
    }

    fun runRootCommand(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(RunRootCommandArgs::class.java)
            runtime.execute(runtime.domains.rootExecutor, invoke, "runRootCommand") {
                try {
                    val output = Shell.cmd(args.command).exec().out
                    val result = JSObject().apply {
                        put("success", true)
                        put("output", output.joinToString("\n"))
                    }
                    invoke.resolve(result)
                } catch (e: Exception) {
                    val result = JSObject().apply {
                        put("success", false)
                        put("output", e.message ?: "Unknown Shell execution error")
                    }
                    invoke.resolve(result)
                }
            }
        } catch (e: Exception) {
            invoke.reject(e.message ?: "Args parsing error")
        }
    }

    fun launchRootManager(invoke: Invoke) {
        try {
            val managers = listOf(
                "com.topjohnwu.magisk" to "Magisk",
                "me.weishu.kernelsu" to "KernelSU",
                "me.tool.apatch" to "APatch"
            )
            var launched = false
            for ((pkg, name) in managers) {
                try {
                    val intent = activity.packageManager.getLaunchIntentForPackage(pkg)
                    if (intent != null) {
                        intent.addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
                        activity.startActivity(intent)
                        launched = true
                        val result = JSObject().apply {
                            put("success", true)
                            put("manager", name)
                        }
                        invoke.resolve(result)
                        break
                    }
                } catch (e: Exception) {
                    // Continue checking next package
                }
            }
            if (!launched) {
                val result = JSObject().apply {
                    put("success", false)
                    put("message", "未找到支持的 Root 管理器 (Magisk, KernelSU, APatch)。")
                }
                invoke.resolve(result)
            }
        } catch (e: Exception) {
            invoke.reject(e.message ?: "启动 Root 管理器失败")
        }
    }

    fun destroy() {
        oomGuardFuture?.cancel(true)
        oomGuardFuture = null
    }
}

@InvokeArg
class RunRootCommandArgs {
    lateinit var command: String
}
