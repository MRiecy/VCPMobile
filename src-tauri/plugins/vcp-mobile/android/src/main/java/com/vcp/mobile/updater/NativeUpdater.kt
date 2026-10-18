package com.vcp.mobile.updater

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import android.util.Log
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import com.vcp.mobile.files.isSafeLocalPath
import com.vcp.mobile.runtime.PluginTaskRuntime

internal class NativeUpdater(private val activity: Activity, private val runtime: PluginTaskRuntime) {
    private companion object { const val TAG = "VcpMobilePlugin" }

    fun verifyApkSignature(invoke: Invoke) {
        val args = invoke.parseArgs(VerifyApkSignatureArgs::class.java)
        val path = args.path
        if (path.isEmpty()) {
            invoke.reject("Path is empty")
            return
        }
        runtime.execute(runtime.domains.fileIoExecutor, invoke, "verifyApkSignature", task@{
            try {
                if (!isSafeLocalPath(activity, path)) {
                    invoke.reject("安全拒绝：禁止校验沙箱外部的文件")
                    return@task
                }
                val file = java.io.File(path)
                if (!file.exists() || !file.isFile) {
                    invoke.reject("APK 文件不存在: $path")
                    return@task
                }
                val apkSha256 = ApkSignatureInspector.apkSignatureSha256(activity, path)
                val selfSha256 = ApkSignatureInspector.selfSignatureSha256(activity)
                val result = JSObject().apply {
                    put("apkSha256", apkSha256)
                    put("selfSha256", selfSha256)
                    put("matched", ApkSignatureInspector.digestsMatch(apkSha256, selfSha256))
                }
                invoke.resolve(result)
            } catch (e: Throwable) {
                Log.e(TAG, "[verifyApkSignature] failed", e)
                invoke.reject("签名校验失败: ${e.message}")
            }
        })
    }

    fun canInstallPackages(invoke: Invoke) {
        try {
            val allowed = activity.packageManager.canRequestPackageInstalls()
            invoke.resolve(JSObject().apply { put("allowed", allowed) })
        } catch (e: Throwable) {
            Log.e(TAG, "[canInstallPackages] failed", e)
            invoke.reject("检查安装权限失败: ${e.message}")
        }
    }

    fun openUnknownSourcesSettings(invoke: Invoke) {
        try {
            val intent = Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES).apply {
                data = Uri.parse("package:${activity.packageName}")
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }
            activity.startActivity(intent)
            invoke.resolve()
        } catch (e: Throwable) {
            Log.e(TAG, "[openUnknownSourcesSettings] failed", e)
            invoke.reject("打开安装权限设置失败: ${e.message}")
        }
    }
}

@InvokeArg
class VerifyApkSignatureArgs {
    lateinit var path: String
}
