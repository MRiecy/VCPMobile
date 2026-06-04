package com.vcp.mobile

import android.content.Context
import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import android.location.Location
import android.location.LocationListener
import android.location.LocationManager
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log
import androidx.core.content.ContextCompat
import app.tauri.plugin.JSObject
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.TimeUnit
import java.util.Locale
import kotlin.math.sqrt

class SensorStatusManager(private val context: Context) {
    companion object {
        private const val TAG = "SensorStatusManager"
        private const val BURST_ACTIVE_DURATION = 2000L // 2s sampling
        private const val BURST_SLEEP_DURATION = 28000L // 28s sleep
        private const val SAMPLING_PERIOD_US = 100000 // 100ms = 10Hz
    }

    private val sensorManager = context.getSystemService(Context.SENSOR_SERVICE) as SensorManager
    private val locationManager = context.getSystemService(Context.LOCATION_SERVICE) as LocationManager

    // Cached values (thread-safe updates)
    @Volatile private var latestLocationStr = "位置信息: 等待数据采集..."
    @Volatile private var latestMotionStr = "运动状态: 静止"
    @Volatile private var latestAmbientStr = "环境传感器: 设备不支持或权限未授予"

    private var isRunning = false
    private val scheduler: ScheduledExecutorService = Executors.newSingleThreadScheduledExecutor()
    private val mainHandler = Handler(Looper.getMainLooper())

    // Sensor instances
    private val accelerometer = sensorManager.getDefaultSensor(Sensor.TYPE_ACCELEROMETER)
    private val lightSensor = sensorManager.getDefaultSensor(Sensor.TYPE_LIGHT)
    private val pressureSensor = sensorManager.getDefaultSensor(Sensor.TYPE_PRESSURE)

    // Temporary storage for burst sampling
    private val burstSamples = ArrayList<Double>()

    // Motion Sensor Listener for Burst
    private val motionListener = object : SensorEventListener {
        private var lastSampleTime = 0L
        override fun onSensorChanged(event: SensorEvent?) {
            if (event == null || event.sensor.type != Sensor.TYPE_ACCELEROMETER) return
            val now = System.currentTimeMillis()
            if (now - lastSampleTime < 100) return // Limit to ~10Hz
            lastSampleTime = now

            val x = event.values[0]
            val y = event.values[1]
            val z = event.values[2]
            val magnitude = sqrt((x * x + y * y + z * z).toDouble())
            synchronized(burstSamples) {
                burstSamples.add(magnitude)
            }
        }
        override fun onAccuracyChanged(sensor: Sensor?, accuracy: Int) {}
    }

    // Ambient sensors (Light and Pressure) listener
    private var lastLux = -1.0
    private var lastPressure = -1.0

    private val ambientListener = object : SensorEventListener {
        override fun onSensorChanged(event: SensorEvent?) {
            if (event == null) return
            if (event.sensor.type == Sensor.TYPE_LIGHT) {
                lastLux = event.values[0].toDouble()
                updateAmbientString()
            } else if (event.sensor.type == Sensor.TYPE_PRESSURE) {
                lastPressure = event.values[0].toDouble()
                updateAmbientString()
            }
        }
        override fun onAccuracyChanged(sensor: Sensor?, accuracy: Int) {}
    }

    // Location Listener
    private val locationListener = object : LocationListener {
        override fun onLocationChanged(location: Location) {
            updateLocationString(location)
        }
        @Deprecated("Deprecated in Java")
        override fun onStatusChanged(provider: String?, status: Int, extras: Bundle?) {}
        override fun onProviderEnabled(provider: String) {}
        override fun onProviderDisabled(provider: String) {}
    }

    @Synchronized
    fun start() {
        if (isRunning) return
        isRunning = true
        Log.i(TAG, "Starting SensorStatusManager collection services")

        // 1. Start Location Listening
        startLocationListening()

        // 2. Start Ambient Listening (continuous, low frequency)
        if (lightSensor != null) {
            sensorManager.registerListener(ambientListener, lightSensor, SensorManager.SENSOR_DELAY_NORMAL)
        }
        if (pressureSensor != null) {
            sensorManager.registerListener(ambientListener, pressureSensor, SensorManager.SENSOR_DELAY_NORMAL)
        }

        // 3. Start Burst Motion Sensing
        scheduleNextMotionBurst()
    }

    @Synchronized
    fun stop() {
        if (!isRunning) return
        isRunning = false
        Log.i(TAG, "Stopping SensorStatusManager collection services")

        // Unregister location
        try {
            locationManager.removeUpdates(locationListener)
        } catch (e: SecurityException) {
            Log.e(TAG, "Failed to remove location updates", e)
        }

        // Unregister all sensors
        sensorManager.unregisterListener(ambientListener)
        sensorManager.unregisterListener(motionListener)
        
        // Cancel all scheduler tasks
        mainHandler.removeCallbacksAndMessages(null)
    }

    fun getSensorData(type: String): JSObject {
        val obj = JSObject()
        when (type) {
            "location" -> obj.put("value", latestLocationStr)
            "motion" -> obj.put("value", latestMotionStr)
            "ambient" -> obj.put("value", latestAmbientStr)
            "all" -> {
                obj.put("location", latestLocationStr)
                obj.put("motion", latestMotionStr)
                obj.put("ambient", latestAmbientStr)
            }
        }
        return obj
    }

    // ==================================================================
    // Location Helpers
    // ==================================================================
    private fun startLocationListening() {
        val hasFine = ContextCompat.checkSelfPermission(context, android.Manifest.permission.ACCESS_FINE_LOCATION) == android.content.pm.PackageManager.PERMISSION_GRANTED
        val hasCoarse = ContextCompat.checkSelfPermission(context, android.Manifest.permission.ACCESS_COARSE_LOCATION) == android.content.pm.PackageManager.PERMISSION_GRANTED

        if (!hasFine && !hasCoarse) {
            latestLocationStr = "位置信息: 未获得定位权限"
            Log.w(TAG, "Location permissions not granted.")
            return
        }

        try {
            // Register for network provider
            if (locationManager.isProviderEnabled(LocationManager.NETWORK_PROVIDER)) {
                locationManager.requestLocationUpdates(
                    LocationManager.NETWORK_PROVIDER,
                    120000L, // 120s
                    10f,     // 10m
                    locationListener,
                    Looper.getMainLooper()
                )
                val lastKnown = locationManager.getLastKnownLocation(LocationManager.NETWORK_PROVIDER)
                if (lastKnown != null) {
                    updateLocationString(lastKnown)
                }
            }
            
            // Register for GPS provider
            if (locationManager.isProviderEnabled(LocationManager.GPS_PROVIDER)) {
                locationManager.requestLocationUpdates(
                    LocationManager.GPS_PROVIDER,
                    120000L, // 120s
                    10f,     // 10m
                    locationListener,
                    Looper.getMainLooper()
                )
                val lastKnown = locationManager.getLastKnownLocation(LocationManager.GPS_PROVIDER)
                if (lastKnown != null) {
                    updateLocationString(lastKnown)
                }
            }
        } catch (e: SecurityException) {
            latestLocationStr = "位置信息: 获取异常 (${e.message})"
            Log.e(TAG, "SecurityException registering location updates", e)
        } catch (e: Exception) {
            latestLocationStr = "位置信息: 未开启定位服务"
            Log.e(TAG, "Exception registering location updates", e)
        }
    }

    private fun updateLocationString(loc: Location) {
        val latitude = loc.latitude
        val longitude = loc.longitude
        val accuracy = loc.accuracy
        val altitude = loc.altitude
        
        val latDir = if (latitude >= 0) "N" else "S"
        val lonDir = if (longitude >= 0) "E" else "W"
        val lat = Math.abs(latitude)
        val lon = Math.abs(longitude)
        
        val accStr = if (accuracy > 0) "${Math.round(accuracy)}m" else "N/A"
        val altStr = if (loc.hasAltitude()) "${Math.round(altitude)}m" else "N/A"
        
        latestLocationStr = String.format(
            Locale.US,
            "坐标: %.4f°%s, %.4f°%s | 精度: %s | 海拔: %s",
            lat, latDir, lon, lonDir, accStr, altStr
        )
    }

    // ==================================================================
    // Motion Burst Sampling Helpers
    // ==================================================================
    private fun scheduleNextMotionBurst() {
        if (!isRunning) return
        
        mainHandler.post {
            if (!isRunning) return@post
            startMotionBurst()
        }
    }

    private fun startMotionBurst() {
        if (accelerometer == null) {
            latestMotionStr = "运动状态: 设备无重力传感器"
            return
        }
        
        synchronized(burstSamples) {
            burstSamples.clear()
        }
        
        sensorManager.registerListener(motionListener, accelerometer, SAMPLING_PERIOD_US)
        
        // Stop burst sampling after 2 seconds
        mainHandler.postDelayed({
            sensorManager.unregisterListener(motionListener, accelerometer)
            processMotionBurstData()
            
            // Schedule next burst after 28 seconds sleep
            if (isRunning) {
                mainHandler.postDelayed({
                    scheduleNextMotionBurst()
                }, BURST_SLEEP_DURATION)
            }
        }, BURST_ACTIVE_DURATION)
    }

    private fun processMotionBurstData() {
        val samples = synchronized(burstSamples) {
            ArrayList(burstSamples)
        }

        if (samples.isEmpty()) return

        val avg = samples.average()
        val max = samples.maxOrNull() ?: 0.0

        var state = "静止"
        if (avg > 12.0) {
            state = "运动中"
        } else if (avg > 10.5) {
            state = "步行中"
        } else if (avg > 9.5) {
            state = "轻微移动"
        }

        latestMotionStr = String.format(
            Locale.US,
            "状态: %s | 平均加速度: %.2fm/s² | 峰值: %.2fm/s²",
            state, avg, max
        )
    }

    // ==================================================================
    // Ambient Helpers
    // ==================================================================
    private fun updateAmbientString() {
        if (lastLux < 0.0 && lastPressure < 0.0) {
            latestAmbientStr = "环境传感器: 设备不支持或权限未授予"
            return
        }

        val parts = ArrayList<String>()
        if (lastLux >= 0.0) {
            var desc = "未知"
            if (lastLux < 50.0) desc = "暗"
            else if (lastLux < 200.0) desc = "室内"
            else if (lastLux < 1000.0) desc = "明亮"
            else desc = "户外"
            parts.add(String.format(Locale.US, "环境光: %.0f lux (%s)", lastLux, desc))
        }
        
        if (lastPressure >= 0.0) {
            parts.add(String.format(Locale.US, "气压: %.0f hPa", lastPressure))
        }

        latestAmbientStr = parts.joinToString(" | ")
    }
}
