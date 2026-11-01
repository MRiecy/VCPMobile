package com.vcp.mobile.permissions

import android.app.Activity
import android.app.ActivityManager
import android.app.NotificationManager
import android.content.ComponentName
import android.os.PowerManager
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import com.vcp.mobile.service.VcpNotificationListenerService
import io.mockk.Runs
import io.mockk.every
import io.mockk.just
import io.mockk.mockk
import io.mockk.slot
import org.json.JSONObject
import org.junit.Assert.assertFalse
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28])
class AndroidPermissionsTest {
    private val activity = Robolectric.buildActivity(Activity::class.java).setup().get()
    private val permissions = AndroidPermissions(activity, { _, _, _ -> }, { _, _, _ -> }, { _, _ -> })

    private fun read(command: (Invoke) -> Unit): JSObject {
        val invoke = mockk<Invoke>(relaxed = true)
        val result = slot<JSObject>()
        every { invoke.resolve(capture(result)) } just Runs
        command(invoke)
        return result.captured
    }

    @Test fun batteryExemptionAndBackgroundRestrictionRemainIndependent() {
        shadowOf(activity.getSystemService(PowerManager::class.java))
            .setIgnoringBatteryOptimizations(activity.packageName, true)
        shadowOf(activity.getSystemService(ActivityManager::class.java)).setBackgroundRestricted(true)
        val notifications = shadowOf(activity.getSystemService(NotificationManager::class.java))
        notifications.setNotificationsEnabled(false)
        val listener = ComponentName(activity, VcpNotificationListenerService::class.java)
        notifications.setNotificationListenerAccessGranted(listener, false)

        val snapshot = read(permissions::checkAllPermissions)
        assertTrue(snapshot.getBoolean("battery"))
        assertFalse(snapshot.getBoolean("backgroundAllowed"))
        assertFalse(snapshot.getBoolean("notification"))
        assertFalse(snapshot.getBoolean("notificationListener"))

        notifications.setNotificationListenerAccessGranted(listener, true)
        assertTrue(read(permissions::checkAllPermissions).getBoolean("notificationListener"))
        assertTrue(read(permissions::check_notification_listener_permission).getBoolean("enabled"))
    }

    @Test fun lowRamDeviceOmitsOnlyTheUnusableListenerFromGateDecisions() {
        shadowOf(activity.getSystemService(ActivityManager::class.java)).setIsLowRamDevice(true)
        shadowOf(activity.getSystemService(NotificationManager::class.java)).setNotificationsEnabled(false)
        shadowOf(activity.getSystemService(PowerManager::class.java))
            .setIgnoringBatteryOptimizations(activity.packageName, false)

        val snapshot = read(permissions::checkAllPermissions)
        assertSame(JSONObject.NULL, snapshot.get("notificationListener"))
        assertFalse(snapshot.getBoolean("notification"))
        assertFalse(snapshot.getBoolean("battery"))
        assertSame(JSONObject.NULL, read(permissions::check_notification_listener_permission).get("enabled"))
    }
}
