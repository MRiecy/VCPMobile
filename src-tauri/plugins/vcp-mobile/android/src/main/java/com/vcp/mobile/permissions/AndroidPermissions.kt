package com.vcp.mobile.permissions

import android.app.Activity
import android.app.ActivityManager
import android.app.NotificationManager
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.PowerManager
import android.os.UserManager
import android.provider.Settings
import android.util.Log
import androidx.activity.result.ActivityResult
import androidx.core.content.ContextCompat
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import com.vcp.mobile.runtime.deliverError
import com.vcp.mobile.runtime.deliverResult
import com.vcp.mobile.service.VcpNotificationListenerService
import org.json.JSONObject
import java.util.Locale

/** Shared permission snapshot and Android/OEM settings navigation. Operation waits stay with their owners. */
internal class AndroidPermissions(
    private val activity: Activity,
    private val requestPermission: (String, Invoke, String) -> Unit,
    private val launchActivity: (Invoke, Intent, String) -> Unit,
    private val emitEvent: (String, JSObject) -> Unit,
) {
    private companion object { const val TAG = "VcpMobilePlugin" }

    fun checkAllPermissions(invoke: Invoke) { invoke.deliverResult(permissionSnapshot()) }

    private fun JSObject.putPermission(name: String, query: () -> Boolean?) {
        val granted = try {
            query()
        } catch (e: RuntimeException) {
            Log.w(TAG, "Could not read $name on this device", e)
            null
        }
        put(name, granted ?: JSONObject.NULL)
    }

    private fun permissionSnapshot(): JSObject {
        val storageGranted = if (Build.VERSION.SDK_INT >= 34) {
            val hasAll = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_MEDIA_IMAGES) == PackageManager.PERMISSION_GRANTED
            val hasVisualSelected = ContextCompat.checkSelfPermission(activity, "android.permission.READ_MEDIA_VISUAL_USER_SELECTED") == PackageManager.PERMISSION_GRANTED
            hasAll || hasVisualSelected
        } else if (Build.VERSION.SDK_INT >= 33) {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_MEDIA_IMAGES) == PackageManager.PERMISSION_GRANTED
        } else if (Build.VERSION.SDK_INT >= 29) {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
        } else {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED &&
                ContextCompat.checkSelfPermission(activity, android.Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
        }

        val microphoneGranted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
        val cameraGranted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED
        val locationGranted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED ||
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.ACCESS_COARSE_LOCATION) == PackageManager.PERMISSION_GRANTED

        val result = JSObject()
        result.putPermission("notification") {
            activity.getSystemService(NotificationManager::class.java)?.areNotificationsEnabled()
        }
        result.putPermission("battery") {
            activity.getSystemService(PowerManager::class.java)?.isIgnoringBatteryOptimizations(activity.packageName)
        }
        result.putPermission("backgroundAllowed") {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                activity.getSystemService(ActivityManager::class.java)?.isBackgroundRestricted?.not()
            } else null
        }
        result.putPermission("notificationListener", ::notificationListenerGranted)
        result.put("storage", storageGranted)
        result.put("microphone", microphoneGranted)
        result.put("camera", cameraGranted)
        result.put("location", locationGranted)

        return result
    }

    fun requestAndroidPermission(invoke: Invoke) {
        val args = invoke.parseArgs(RequestPermissionArgs::class.java)
        when (args.type) {
            "notification" -> {
                if (Build.VERSION.SDK_INT >= 33) {
                    requestPermission("notification", invoke, "onPermissionResult")
                } else {
                    val snapshot = permissionSnapshot()
                    if (snapshot.optBoolean("notification")) {
                        emitEvent("vcp-permission-change", snapshot)
                        invoke.deliverResult()
                    } else {
                        openNotificationSettings(invoke)
                    }
                }
            }
            "notificationSettings" -> openNotificationSettings(invoke)
            "storage" -> {
                if (Build.VERSION.SDK_INT >= 33) {
                    requestPermission("storage", invoke, "onPermissionResult")
                } else {
                    requestPermission("storageLegacy", invoke, "onPermissionResult")
                }
            }
            "camera" -> {
                requestPermission("camera", invoke, "onPermissionResult")
            }
            "location" -> {
                requestPermission("location", invoke, "onPermissionResult")
            }
            "battery" -> {
                try {
                    val intent = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS).apply {
                        data = Uri.parse("package:${activity.packageName}")
                    }
                    launchActivity(invoke, intent, "onPermissionSettingsResult")
                } catch (e: Exception) {
                    val intent = Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
                    openPermissionSettings(invoke, intent)
                }
            }
        }
    }

    private fun openNotificationSettings(invoke: Invoke) {
        val intent = Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).apply {
            putExtra(Settings.EXTRA_APP_PACKAGE, activity.packageName)
        }
        openPermissionSettings(invoke, intent)
    }

    private fun openPermissionSettings(invoke: Invoke, intent: Intent) {
        try {
            launchActivity(invoke, intent, "onPermissionSettingsResult")
        } catch (e: RuntimeException) {
            invoke.deliverError("Could not open ${intent.action}: ${e.message}")
        }
    }

    private fun notificationListenerGranted(): Boolean? {
        val userManager = activity.getSystemService(UserManager::class.java) ?: return null
        if (userManager.isManagedProfile) return null
        if (Build.VERSION.SDK_INT <= Build.VERSION_CODES.Q) {
            val activityManager = activity.getSystemService(ActivityManager::class.java) ?: return null
            if (activityManager.isLowRamDevice) return null
        }
        val listener = ComponentName(activity, VcpNotificationListenerService::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
            return activity.getSystemService(NotificationManager::class.java)?.isNotificationListenerAccessGranted(listener)
        }
        // API 26 stores the list of granted components in Secure settings. An empty list is a denial.
        val granted = Settings.Secure.getString(activity.contentResolver, "enabled_notification_listeners")
        return granted.orEmpty().split(':').any { ComponentName.unflattenFromString(it) == listener }
    }

    fun check_notification_listener_permission(invoke: Invoke) {
        invoke.deliverResult(JSObject().apply { putPermission("enabled", ::notificationListenerGranted) })
    }

    fun request_notification_listener_permission(invoke: Invoke) {
        openPermissionSettings(invoke, Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS))
    }

    private fun checkAutoStartStatus(): String {
        val manufacturer = Build.MANUFACTURER.lowercase(Locale.ROOT)
        if (manufacturer.contains("xiaomi") || manufacturer.contains("redmi") ||
            manufacturer.contains("vivo") || manufacturer.contains("meizu")) {
            val ops = activity.getSystemService(Context.APP_OPS_SERVICE) as? android.app.AppOpsManager
            if (ops != null) {
                try {
                    val method = ops.javaClass.getMethod(
                        "checkOpNoThrow",
                        Int::class.javaPrimitiveType,
                        Int::class.javaPrimitiveType,
                        String::class.java
                    )
                    // 10008 is OP_AUTO_START in MIUI / HyperOS / Flyme / OriginOS
                    val mode = method.invoke(
                        ops,
                        10008,
                        activity.applicationInfo.uid,
                        activity.packageName
                    ) as Int
                    return if (mode == android.app.AppOpsManager.MODE_ALLOWED) "true" else "false"
                } catch (e: Exception) {
                    Log.e(TAG, "checkAutoStartStatus: reflection failed", e)
                }
            }
        }
        return "unsupported"
    }

    fun checkAutoStartPermission(invoke: Invoke) {
        val status = checkAutoStartStatus()
        val result = JSObject()
        result.put("status", status)
        invoke.deliverResult(result)
    }

    fun requestAutoStartPermission(invoke: Invoke) {
        val manufacturer = Build.MANUFACTURER.lowercase(Locale.ROOT)
        var success = false
        val intents = mutableListOf<Intent>()

        if (manufacturer.contains("xiaomi") || manufacturer.contains("redmi")) {
            // 小米 / HyperOS
            intents.add(Intent().setComponent(ComponentName("com.miui.securitycenter", "com.miui.permcenter.autostart.AutoStartManagementActivity")))
        } else if (manufacturer.contains("huawei") || manufacturer.contains("honor")) {
            // 华为 / 荣耀
            intents.add(Intent().setComponent(ComponentName("com.huawei.systemmanager", "com.huawei.systemmanager.startupmgr.ui.StartupNormalAppListActivity")))
            intents.add(Intent().setComponent(ComponentName("com.huawei.systemmanager", "com.huawei.systemmanager.optimize.bootstart.BootStartActivity")))
        } else if (manufacturer.contains("oppo") || manufacturer.contains("oneplus") || manufacturer.contains("realme")) {
            // OPPO / 一加 / 真我
            // 针对自启动跳错，我们优先拉起系统应用管理主页，或直接拉起应用详情页，保障在 OPPO/ColorOS 上的准确性
            intents.add(Intent(Settings.ACTION_MANAGE_APPLICATIONS_SETTINGS))
        } else if (manufacturer.contains("vivo")) {
            // VIVO
            intents.add(Intent().setComponent(ComponentName("com.iqoo.secure", "com.iqoo.secure.ui.phoneoptimize.BgStartUpManager")))
            intents.add(Intent().setComponent(ComponentName("com.vivo.permissionmanager", "com.vivo.permissionmanager.activity.BgStartUpManagerActivity")))
            intents.add(Intent().setComponent(ComponentName("com.iqoo.secure", "com.iqoo.secure.MainActivity")))
        } else if (manufacturer.contains("meizu")) {
            // 魅族
            intents.add(Intent().setComponent(ComponentName("com.meizu.safe", "com.meizu.safe.permission.SmartBGActivity")))
            intents.add(Intent().setComponent(ComponentName("com.meizu.safe", "com.meizu.safe.MainActivity")))
        }

        // 尝试打开厂商特定的 Activity
        for (intent in intents) {
            try {
                intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                activity.startActivity(intent)
                success = true
                break
            } catch (e: Exception) {
                // Try next
            }
        }

        // 兜底退避
        if (!success) {
            try {
                val intent = Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS).apply {
                    data = Uri.parse("package:${activity.packageName}")
                    addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                }
                activity.startActivity(intent)
                success = true
            } catch (e: Exception) {
                try {
                    val intent = Intent(Settings.ACTION_SETTINGS).apply {
                        addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    }
                    activity.startActivity(intent)
                    success = true
                } catch (e2: Exception) {}
            }
        }

        val result = JSObject()
        result.put("success", success)
        invoke.deliverResult(result)
    }

    fun requestPowerManagementPermission(invoke: Invoke) {
        try {
            val packageUri = Uri.fromParts("package", activity.packageName, null)
            launchActivity(invoke, Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, packageUri), "onPermissionSettingsResult")
        } catch (e: RuntimeException) {
            Log.w(TAG, "Could not open app settings for background restrictions", e)
            invoke.deliverResult(JSObject().apply { put("success", false) })
        }
    }

    fun onPermissionResult(invoke: Invoke) {
        emitPermissionsToWebView()
        invoke.deliverResult()
    }

    fun onPermissionSettingsResult(invoke: Invoke, @Suppress("UNUSED_PARAMETER") result: ActivityResult) {
        emitPermissionsToWebView()
        // Success means the settings page opened. Only the permission snapshot records a grant.
        invoke.deliverResult(JSObject().apply { put("success", true) })
    }

    private fun emitPermissionsToWebView() {
        emitEvent("vcp-permission-change", permissionSnapshot())
    }
}

@InvokeArg
class RequestPermissionArgs {
    lateinit var type: String
}
