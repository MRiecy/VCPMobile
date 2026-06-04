package com.vcp.mobile

import android.content.Context
import android.opengl.GLES20
import app.tauri.plugin.JSObject
import javax.microedition.khronos.egl.EGL10
import javax.microedition.khronos.egl.EGLConfig
import javax.microedition.khronos.egl.EGLContext

/**
 * 独立的 GPU 硬件静态信息拉取管理器 (高解耦、模块化设计)
 */
class GpuStatusManager(private val context: Context) {

    @Volatile 
    private var cachedGpuRenderer: String? = null

    fun getGpuRenderer(): String {
        cachedGpuRenderer?.let { return it }
        synchronized(this) {
            cachedGpuRenderer?.let { return it }
            val renderer = fetchGpuRendererFromEgl()
            cachedGpuRenderer = renderer
            return renderer
        }
    }

    /**
     * 动态构建临时的、轻量级 1x1 像素 Pbuffer Surface 和 EGL 1.0 上下文，
     * 用于安全地在后台/主线程读取真实 GPU 型号并及时释放，防止内存及硬件资源泄露。
     */
    private fun fetchGpuRendererFromEgl(): String {
        return try {
            val egl = EGLContext.getEGL() as EGL10
            val dpy = egl.eglGetDisplay(EGL10.EGL_DEFAULT_DISPLAY)
            val vers = IntArray(2)
            egl.eglInitialize(dpy, vers)

            val configAttr = intArrayOf(
                EGL10.EGL_RED_SIZE, 8,
                EGL10.EGL_GREEN_SIZE, 8,
                EGL10.EGL_BLUE_SIZE, 8,
                EGL10.EGL_NONE
            )
            val configs = arrayOfNulls<EGLConfig>(1)
            val numConfig = IntArray(1)
            egl.eglChooseConfig(dpy, configAttr, configs, 1, numConfig)
            val config = configs[0] ?: return "Unknown GPU"

            val surfAttr = intArrayOf(
                EGL10.EGL_WIDTH, 1,
                EGL10.EGL_HEIGHT, 1,
                EGL10.EGL_NONE
            )
            val surf = egl.eglCreatePbufferSurface(dpy, config, surfAttr)
            val ctxAttr = intArrayOf(
                0x3098, 2, // EGL_CONTEXT_CLIENT_VERSION = 2 (GLES 2.0)
                EGL10.EGL_NONE
            )
            val ctx = egl.eglCreateContext(dpy, config, EGL10.EGL_NO_CONTEXT, ctxAttr)
            egl.eglMakeCurrent(dpy, surf, surf, ctx)
            
            // 读取 GPU 模型渲染器 (例如 "Adreno (TM) 740")
            val glRenderer = GLES20.glGetString(GLES20.GL_RENDERER)
            
            // 清理并完全释放 EGL 状态
            egl.eglMakeCurrent(dpy, EGL10.EGL_NO_SURFACE, EGL10.EGL_NO_SURFACE, EGL10.EGL_NO_CONTEXT)
            egl.eglDestroyContext(dpy, ctx)
            egl.eglDestroySurface(dpy, surf)
            egl.eglTerminate(dpy)
            
            glRenderer ?: "Unknown GPU"
        } catch (e: Exception) {
            "Unknown GPU"
        }
    }

    fun getGpuStatusJson(): JSObject {
        val result = JSObject()
        result.put("renderer", getGpuRenderer())
        result.put("restricted", true)
        return result
    }
}
