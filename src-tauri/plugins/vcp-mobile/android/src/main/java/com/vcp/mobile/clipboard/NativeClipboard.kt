package com.vcp.mobile.clipboard

import android.app.Activity
import android.content.Context
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject

internal class NativeClipboard(private val activity: Activity) {
    fun writeClipboard(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(WriteClipboardArgs::class.java)
            activity.runOnUiThread {
                try {
                    val clipboard = activity.getSystemService(Context.CLIPBOARD_SERVICE) as android.content.ClipboardManager
                    val clip = android.content.ClipData.newPlainText("VCP Distributed Copy", args.content)
                    clipboard.setPrimaryClip(clip)
                    invoke.resolve()
                } catch (e: Exception) {
                    invoke.reject(e.message ?: "Failed to write clipboard on UI thread")
                }
            }
        } catch (e: Exception) {
            invoke.reject(e.message ?: "Failed to parse arguments")
        }
    }

    fun readClipboard(invoke: Invoke) {
        try {
            activity.runOnUiThread {
                try {
                    val clipboard = activity.getSystemService(Context.CLIPBOARD_SERVICE) as android.content.ClipboardManager
                    val clipData = clipboard.primaryClip
                    val content = if (clipData != null && clipData.itemCount > 0) {
                        clipData.getItemAt(0).text?.toString() ?: ""
                    } else {
                        ""
                    }
                    val result = JSObject().apply {
                        put("content", content)
                    }
                    invoke.resolve(result)
                } catch (e: Exception) {
                    invoke.reject(e.message ?: "Failed to read clipboard on UI thread")
                }
            }
        } catch (e: Exception) {
            invoke.reject(e.message ?: "Failed to execute readClipboard")
        }
    }
}

@InvokeArg
class WriteClipboardArgs {
    lateinit var content: String
}
