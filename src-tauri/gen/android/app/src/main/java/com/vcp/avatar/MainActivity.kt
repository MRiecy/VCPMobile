package com.vcp.avatar

import android.os.Bundle
import androidx.activity.enableEdgeToEdge

/**
 * VCP Mobile Android 主 Activity
 *
 * 精简为默认 Tauri Activity，所有自定义原生能力已迁移到 tauri-plugin-vcp-mobile。
 */
class MainActivity : TauriActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
    }
}
