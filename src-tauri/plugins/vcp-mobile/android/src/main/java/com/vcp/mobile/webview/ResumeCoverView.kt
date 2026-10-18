package com.vcp.mobile.webview

import android.app.Activity
import android.graphics.Color
import android.graphics.drawable.GradientDrawable
import android.util.TypedValue
import android.view.Gravity
import android.view.View
import android.widget.Button
import android.widget.FrameLayout
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.TextView

/** Presentation only; NativeWebViewResume owns the snapshot and recovery handshake. */
internal class ResumeCoverView(activity: Activity, retry: () -> Unit, reload: () -> Unit) : FrameLayout(activity) {
    val image = ImageView(activity).apply {
        scaleType = ImageView.ScaleType.FIT_XY
        importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_NO
    }
    private val panel = LinearLayout(activity)
    private val label = TextView(activity)
    private val actions = LinearLayout(activity)
    private var recoveryShown = false

    init {
        isClickable = true
        isFocusable = true
        val color = TypedValue().let {
            if (activity.theme.resolveAttribute(android.R.attr.colorBackground, it, true)) it.data else Color.DKGRAY
        }
        addView(image, LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT))
        val padding = (16 * resources.displayMetrics.density).toInt()
        panel.orientation = LinearLayout.VERTICAL
        panel.gravity = Gravity.CENTER
        panel.setPadding(padding, padding, padding, padding)
        panel.background = GradientDrawable().apply {
            setColor(color)
            cornerRadius = 8 * resources.displayMetrics.density
        }
        label.text = "正在恢复界面"
        label.gravity = Gravity.CENTER
        label.textSize = 15f
        panel.addView(label)
        actions.orientation = LinearLayout.HORIZONTAL
        actions.addView(Button(activity).apply { text = "重试显示"; isAllCaps = false; setOnClickListener { retry() } })
        actions.addView(Button(activity).apply { text = "重新加载界面"; isAllCaps = false; setOnClickListener { reload() } })
        panel.addView(actions)
        addView(panel, LayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT, Gravity.CENTER))
        showWaiting(false)
    }

    fun showWaiting(hasSnapshot: Boolean) {
        recoveryShown = false
        label.text = "正在恢复界面"
        actions.visibility = View.GONE
        panel.visibility = if (hasSnapshot) View.GONE else View.VISIBLE
    }

    fun showRecovery() {
        recoveryShown = true
        label.text = "界面仍在恢复"
        actions.visibility = View.VISIBLE
        panel.visibility = View.VISIBLE
    }

    fun snapshotInstalled() {
        if (!recoveryShown) panel.visibility = View.GONE
    }
}
