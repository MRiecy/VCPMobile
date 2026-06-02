package com.vcp.mobile

import android.animation.Animator
import android.animation.AnimatorListenerAdapter
import android.animation.ValueAnimator
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.graphics.Color
import android.graphics.PixelFormat
import android.graphics.drawable.GradientDrawable
import android.net.Uri
import android.os.Build
import android.provider.Settings
import android.util.Log
import android.view.Gravity
import android.view.MotionEvent
import android.view.View
import android.view.WindowManager
import android.view.animation.DecelerateInterpolator
import android.webkit.JavascriptInterface
import android.webkit.WebSettings
import android.webkit.WebView
import android.widget.FrameLayout
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.TextView

class FloatingWindowManager(private val activity: Activity) {

    private val context: Context = activity.applicationContext
    private var floatingBallView: ImageView? = null
    private var assistantContainer: LinearLayout? = null
    private var assistantWebView: WebView? = null
    
    private var windowManager: WindowManager? = null
    private var ballLayoutParams: WindowManager.LayoutParams? = null

    companion object {
        private const val TAG = "FloatingWindowManager"
        private const val LOCAL_SERVER_PORT = 14202
    }

    fun hasOverlayPermission(): Boolean {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            Settings.canDrawOverlays(context)
        } else {
            true
        }
    }

    fun requestOverlayPermission() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            try {
                val intent = Intent(
                    Settings.ACTION_MANAGE_OVERLAY_PERMISSION,
                    Uri.parse("package:${activity.packageName}")
                ).apply {
                    addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                }
                activity.startActivity(intent)
            } catch (e: Exception) {
                Log.e(TAG, "Failed to start ACTION_MANAGE_OVERLAY_PERMISSION", e)
            }
        }
    }

    fun toggleFloatingBall(show: Boolean): Boolean {
        if (show) {
            if (!hasOverlayPermission()) return false
            activity.runOnUiThread { showFloatingBall() }
        } else {
            activity.runOnUiThread {
                hideAssistantWindow()
                hideFloatingBall()
            }
        }
        return true
    }

    private fun showFloatingBall() {
        if (floatingBallView != null) return
        try {
            windowManager = context.getSystemService(Context.WINDOW_SERVICE) as WindowManager
            val dp48 = dpToPx(48)
            val params = WindowManager.LayoutParams(
                dp48, dp48,
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY else WindowManager.LayoutParams.TYPE_PHONE,
                WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS or WindowManager.LayoutParams.FLAG_HARDWARE_ACCELERATED,
                PixelFormat.TRANSLUCENT
            ).apply {
                gravity = Gravity.TOP or Gravity.START
                val dm = context.resources.displayMetrics
                x = dm.widthPixels - dp48 - dpToPx(16)
                y = (dm.heightPixels * 0.7).toInt()
            }
            ballLayoutParams = params
            val imageView = ImageView(context).apply {
                try { setImageDrawable(context.packageManager.getApplicationIcon(context.packageName)) } catch (e: Exception) {}
                setPadding(dpToPx(4), dpToPx(4), dpToPx(4), dpToPx(4))
                background = GradientDrawable().apply {
                    shape = GradientDrawable.OVAL
                    setColor(0xFFF9FAFB.toInt())
                    setStroke(dpToPx(1), 0x1F000000)
                }
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.LOLLIPOP) elevation = dpToPx(6).toFloat()
                alpha = 0.85f
            }
            setupBallTouchListener(imageView)
            windowManager?.addView(imageView, params)
            floatingBallView = imageView
        } catch (e: Exception) { Log.e(TAG, "Failed to show ball", e) }
    }

    private fun hideFloatingBall() {
        floatingBallView?.let { view ->
            try { windowManager?.removeView(view) } catch (e: Exception) {}
            finally { floatingBallView = null; ballLayoutParams = null }
        }
    }

    private fun setupBallTouchListener(view: View) {
        var lastX = 0f; var lastY = 0f; var startX = 0f; var startY = 0f; var startTime = 0L
        view.setOnTouchListener { v, event ->
            val params = ballLayoutParams ?: return@setOnTouchListener false
            val wm = windowManager ?: return@setOnTouchListener false
            when (event.action) {
                MotionEvent.ACTION_DOWN -> {
                    lastX = event.rawX; lastY = event.rawY; startX = event.rawX; startY = event.rawY; startTime = System.currentTimeMillis()
                    view.alpha = 1.0f
                }
                MotionEvent.ACTION_MOVE -> {
                    params.x += (event.rawX - lastX).toInt(); params.y += (event.rawY - lastY).toInt()
                    val dm = context.resources.displayMetrics
                    params.x = params.x.coerceIn(0, dm.widthPixels - v.width); params.y = params.y.coerceIn(0, dm.heightPixels - v.height)
                    wm.updateViewLayout(v, params)
                    lastX = event.rawX; lastY = event.rawY
                }
                MotionEvent.ACTION_UP -> {
                    if (Math.hypot((event.rawX - startX).toDouble(), (event.rawY - startY).toDouble()) < dpToPx(6) && System.currentTimeMillis() - startTime < 300) {
                        onFloatingBallClick()
                    }
                    animateBallToEdge(v, params)
                }
            }
            true
        }
    }

    private fun animateBallToEdge(view: View, params: WindowManager.LayoutParams) {
        val wm = windowManager ?: return
        val dm = context.resources.displayMetrics
        val targetX = if (params.x + view.width / 2 < dm.widthPixels / 2) 0 else dm.widthPixels - view.width
        ValueAnimator.ofInt(params.x, targetX).apply {
            duration = 300
            interpolator = DecelerateInterpolator()
            addUpdateListener { animation ->
                if (view.parent != null) {
                    params.x = animation.animatedValue as Int
                    try { wm.updateViewLayout(view, params) } catch (_: Exception) {}
                }
            }
            addListener(object : AnimatorListenerAdapter() { override fun onAnimationEnd(a: Animator) { view.alpha = 0.5f } })
        }.start()
    }

    private fun onFloatingBallClick() {
        if (assistantContainer == null) showAssistantWindow() else hideAssistantWindow()
    }

    private fun showAssistantWindow() {
        if (assistantContainer != null) return
        try {
            val wm = context.getSystemService(Context.WINDOW_SERVICE) as WindowManager
            val dm = context.resources.displayMetrics
            val height = (dm.heightPixels * 0.75).toInt()

            val params = WindowManager.LayoutParams(
                WindowManager.LayoutParams.MATCH_PARENT, height,
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY else WindowManager.LayoutParams.TYPE_PHONE,
                WindowManager.LayoutParams.FLAG_HARDWARE_ACCELERATED,
                PixelFormat.TRANSLUCENT
            ).apply {
                gravity = Gravity.BOTTOM
                windowAnimations = android.R.style.Animation_InputMethod
            }

            // 💥 结构优化：线性布局，顶部增加原生操作栏
            val rootLayout = LinearLayout(context).apply {
                orientation = LinearLayout.VERTICAL
                // 只有显示内容的区域才拦截点击，点击顶部透明处关闭
                setBackgroundColor(Color.TRANSPARENT)
            }

            // 1. 顶部救生圈：原生关闭把手
            val handleBar = FrameLayout(context).apply {
                layoutParams = LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, dpToPx(32))
                background = GradientDrawable().apply {
                    setColor(0xAA000000.toInt()) // 半透明黑
                    val radius = dpToPx(12).toFloat()
                    setCornerRadii(floatArrayOf(radius, radius, radius, radius, 0f, 0f, 0f, 0f))
                }
                setOnClickListener { hideAssistantWindow() }
                
                val hint = TextView(context).apply {
                    text = "点击此条或外部收起助手"
                    setTextColor(Color.WHITE)
                    textSize = 10f
                    gravity = Gravity.CENTER
                }
                addView(hint, FrameLayout.LayoutParams(FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.MATCH_PARENT))
            }

            // 2. 主 WebView
            val webView = WebView(context).apply {
                setBackgroundColor(Color.TRANSPARENT)
                layoutParams = LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, 0, 1.0f)
                settings.apply {
                    javaScriptEnabled = true
                    domStorageEnabled = true
                    databaseEnabled = true
                    mixedContentMode = WebSettings.MIXED_CONTENT_ALWAYS_ALLOW
                }
                addJavascriptInterface(object {
                    @JavascriptInterface fun closeWindow() { activity.runOnUiThread { hideAssistantWindow() } }
                    @JavascriptInterface fun getClipboard(): String {
                        val cb = context.getSystemService(Context.CLIPBOARD_SERVICE) as android.content.ClipboardManager
                        return cb.primaryClip?.getItemAt(0)?.text?.toString() ?: ""
                    }
                }, "AndroidBridge")
                loadUrl("http://127.0.0.1:$LOCAL_SERVER_PORT/floating")
            }

            rootLayout.addView(handleBar)
            rootLayout.addView(webView)
            wm.addView(rootLayout, params)
            
            assistantContainer = rootLayout
            assistantWebView = webView
            floatingBallView?.visibility = View.GONE
        } catch (e: Exception) { Log.e(TAG, "Failed to show assistant", e) }
    }

    private fun hideAssistantWindow() {
        assistantContainer?.let { view ->
            // 先移除 WebView 和容器，再销毁 WebView，避免 "destroy() called while still attached" 警告
            assistantWebView?.let { wv ->
                (wv.parent as? android.view.ViewGroup)?.removeView(wv)
                wv.destroy()
            }
            try { windowManager?.removeView(view) } catch (e: Exception) {}
            assistantWebView = null
            assistantContainer = null
            floatingBallView?.visibility = View.VISIBLE
        }
    }

    private fun dpToPx(dp: Int): Int = (dp * context.resources.displayMetrics.density).toInt()
}
