package com.vcp.mobile

import android.app.Activity

// Keep this class and static method in com.vcp.mobile: Rust calls its fixed JNI name.
object ScreenKeepOnArbiter {
    private val stateLock = Any()
    private var activityRef: java.lang.ref.WeakReference<Activity>? = null
    private var manualRequested = false
    private var guardianRequested = false
    private var appInForeground = true

    @JvmStatic
    fun setManualRequested(activity: Activity, requested: Boolean) {
        synchronized(stateLock) {
            activityRef = java.lang.ref.WeakReference(activity)
            manualRequested = requested
        }
        apply(activity)
    }

    fun setGuardianRequested(activity: Activity, requested: Boolean) {
        synchronized(stateLock) {
            activityRef = java.lang.ref.WeakReference(activity)
            guardianRequested = requested
        }
        apply(activity)
    }

    fun setAppInForeground(activity: Activity, foreground: Boolean) {
        synchronized(stateLock) {
            activityRef = java.lang.ref.WeakReference(activity)
            appInForeground = foreground
        }
        apply(activity)
    }

    fun detach(activity: Activity) {
        val ownsActivity = synchronized(stateLock) {
            if (activityRef?.get() !== activity) {
                false
            } else {
                activityRef?.clear()
                activityRef = null
                manualRequested = false
                appInForeground = false
                true
            }
        }
        if (ownsActivity) {
            activity.runOnUiThread {
                activity.window.clearFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            }
        }
    }

    private fun apply(activity: Activity) {
        activity.runOnUiThread {
            val shouldKeepScreenOn = synchronized(stateLock) {
                if (activityRef?.get() !== activity) {
                    null
                } else {
                    appInForeground && (manualRequested || guardianRequested)
                }
            } ?: return@runOnUiThread

            if (shouldKeepScreenOn) {
                activity.window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            } else {
                activity.window.clearFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            }
        }
    }
}
