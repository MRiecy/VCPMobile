package com.vcp.mobile.service

import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification
import android.util.Log

class VcpNotificationListenerService : NotificationListenerService() {
    companion object {
        private const val TAG = "VcpNotificationListener"
    }

    override fun onListenerConnected() {
        super.onListenerConnected()
        Log.i(TAG, "Notification Listener connected.")
    }

    override fun onNotificationPosted(sbn: StatusBarNotification?) {
        // Do nothing to protect user privacy
    }

    override fun onNotificationRemoved(sbn: StatusBarNotification?) {
        // Do nothing
    }
}
