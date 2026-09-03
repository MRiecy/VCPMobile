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
import java.util.Locale
import kotlin.math.sqrt

class SensorStatusManager(private val context: Context) {
    companion object {
        private const val TAG = "SensorStatusManager"
        private const val SAMPLE_DURATION_MS = 2000L
        private const val SAMPLING_PERIOD_US = 100000 // 100ms = 10Hz
    }

    private val sensorManager = context.getSystemService(Context.SENSOR_SERVICE) as SensorManager
    private val locationManager = context.getSystemService(Context.LOCATION_SERVICE) as LocationManager

    // Cached values (thread-safe updates)
    @Volatile private var latestLocationStr = "位置信息: 等待数据采集..."
    @Volatile private var latestMotionStr = "运动状态: 静止"
    @Volatile private var latestAmbientStr = "环境传感器: 设备不支持或权限未授予"

    private val mainHandler = Handler(Looper.getMainLooper())
    private var sampleActive = false
    private var sampleLocation = false
    private var sampleMotion = false
    private var sampleAmbient = false
    private var sampleCompletion: ((JSObject) -> Unit)? = null
    private var sampleFinishRunnable: Runnable? = null

    // Sensor instances
    private val accelerometer = sensorManager.getDefaultSensor(Sensor.TYPE_ACCELEROMETER)
    private val gyroscope = sensorManager.getDefaultSensor(Sensor.TYPE_GYROSCOPE)
    private val magneticField = sensorManager.getDefaultSensor(Sensor.TYPE_MAGNETIC_FIELD)
    private val lightSensor = sensorManager.getDefaultSensor(Sensor.TYPE_LIGHT)
    private val pressureSensor = sensorManager.getDefaultSensor(Sensor.TYPE_PRESSURE)

    // Temporary storage for burst sampling
    private val burstAccelSamples = ArrayList<Double>()
    private val burstGyroSamples = ArrayList<Double>()
    private val burstMagSamples = ArrayList<Double>()

    // Motion Sensor Listener for Burst
    private val motionListener = object : SensorEventListener {
        private var lastAccelTime = 0L
        private var lastGyroTime = 0L
        private var lastMagTime = 0L

        override fun onSensorChanged(event: SensorEvent?) {
            if (event == null) return
            val now = System.currentTimeMillis()
            when (event.sensor.type) {
                Sensor.TYPE_ACCELEROMETER -> {
                    if (now - lastAccelTime < 100) return
                    lastAccelTime = now
                    val x = event.values[0]
                    val y = event.values[1]
                    val z = event.values[2]
                    val magnitude = sqrt((x * x + y * y + z * z).toDouble())
                    synchronized(burstAccelSamples) {
                        burstAccelSamples.add(magnitude)
                    }
                }
                Sensor.TYPE_GYROSCOPE -> {
                    if (now - lastGyroTime < 100) return
                    lastGyroTime = now
                    val x = event.values[0]
                    val y = event.values[1]
                    val z = event.values[2]
                    val magnitude = sqrt((x * x + y * y + z * z).toDouble())
                    synchronized(burstGyroSamples) {
                        burstGyroSamples.add(magnitude)
                    }
                }
                Sensor.TYPE_MAGNETIC_FIELD -> {
                    if (now - lastMagTime < 100) return
                    lastMagTime = now
                    val x = event.values[0]
                    val y = event.values[1]
                    val z = event.values[2]
                    val magnitude = sqrt((x * x + y * y + z * z).toDouble())
                    synchronized(burstMagSamples) {
                        burstMagSamples.add(magnitude)
                    }
                }
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

    /**
     * 执行一次有界、按需求的物理传感器采样。调用者保证批次串行；若防御性地收到重叠
     * 调用，先完整收尾并回传上一批，确保没有悬挂 Invoke 或遗留 listener。
     */
    @Synchronized
    fun sample(
        location: Boolean,
        motion: Boolean,
        ambient: Boolean,
        completion: (JSObject) -> Unit,
    ) {
        finishActiveSample()
        if (!location && !motion && !ambient) {
            completion(JSObject())
            return
        }

        sampleActive = true
        sampleLocation = location
        sampleMotion = motion
        sampleAmbient = ambient
        sampleCompletion = completion
        Log.i(
            TAG,
            "Starting bounded sensor sample: location=$location motion=$motion ambient=$ambient",
        )

        try {
            if (location) startLocationSample()
            if (ambient) startAmbientSample()
            if (motion) startMotionSample()
        } catch (error: Exception) {
            Log.e(TAG, "Failed to start bounded sensor sample", error)
            if (location) latestLocationStr = "位置信息: 采样异常 (${error.message})"
            if (motion) latestMotionStr = "运动传感器: 采样异常 (${error.message})"
            if (ambient) latestAmbientStr = "环境传感器: 采样异常 (${error.message})"
            finishActiveSample()
            return
        }

        val finish = Runnable { finishActiveSample() }
        sampleFinishRunnable = finish
        mainHandler.postDelayed(finish, SAMPLE_DURATION_MS)
    }

    @Synchronized
    fun shutdown() {
        finishActiveSample()
    }

    @Synchronized
    private fun finishActiveSample() {
        if (!sampleActive) return
        sampleFinishRunnable?.let(mainHandler::removeCallbacks)
        sampleFinishRunnable = null

        if (sampleLocation) {
            try {
                locationManager.removeUpdates(locationListener)
            } catch (error: Exception) {
                Log.w(TAG, "Failed to stop bounded location sample", error)
            }
        }
        if (sampleAmbient) {
            sensorManager.unregisterListener(ambientListener)
            updateAmbientString()
        }
        if (sampleMotion) {
            sensorManager.unregisterListener(motionListener)
            processMotionBurstData()
        }

        val result = JSObject()
        if (sampleLocation) result.put("location", latestLocationStr)
        if (sampleMotion) result.put("motion", latestMotionStr)
        if (sampleAmbient) result.put("ambient", latestAmbientStr)
        val completion = sampleCompletion

        sampleActive = false
        sampleLocation = false
        sampleMotion = false
        sampleAmbient = false
        sampleCompletion = null
        Log.i(TAG, "Bounded sensor sample finished; all listeners released")
        completion?.invoke(result)
    }

    // ==================================================================
    // Location Helpers
    // ==================================================================
    private fun startLocationSample() {
        val hasFine = ContextCompat.checkSelfPermission(context, android.Manifest.permission.ACCESS_FINE_LOCATION) == android.content.pm.PackageManager.PERMISSION_GRANTED
        val hasCoarse = ContextCompat.checkSelfPermission(context, android.Manifest.permission.ACCESS_COARSE_LOCATION) == android.content.pm.PackageManager.PERMISSION_GRANTED

        if (!hasFine && !hasCoarse) {
            latestLocationStr = "位置信息: 未获得定位权限"
            Log.w(TAG, "Location permissions not granted.")
            return
        }

        try {
            seedLastKnownLocation(LocationManager.NETWORK_PROVIDER)
            seedLastKnownLocation(LocationManager.GPS_PROVIDER)
            requestSingleUpdate(LocationManager.NETWORK_PROVIDER)
            requestSingleUpdate(LocationManager.GPS_PROVIDER)
        } catch (e: SecurityException) {
            latestLocationStr = "位置信息: 获取异常 (${e.message})"
            Log.e(TAG, "SecurityException registering location updates", e)
        } catch (e: Exception) {
            latestLocationStr = "位置信息: 未开启定位服务"
            Log.e(TAG, "Exception registering location updates", e)
        }
    }

    private fun seedLastKnownLocation(provider: String) {
        try {
            if (locationManager.isProviderEnabled(provider)) {
                val lastKnown = locationManager.getLastKnownLocation(provider)
                if (lastKnown != null) {
                    updateLocationString(lastKnown)
                }
            }
        } catch (e: SecurityException) {
            Log.w(TAG, "SecurityException getting last known location from $provider", e)
        } catch (e: Exception) {
            Log.w(TAG, "Exception getting last known location from $provider", e)
        }
    }

    private fun requestSingleUpdate(provider: String) {
        try {
            if (locationManager.isProviderEnabled(provider)) {
                locationManager.requestSingleUpdate(provider, locationListener, Looper.getMainLooper())
            }
        } catch (e: SecurityException) {
            Log.w(TAG, "SecurityException requesting single update from $provider", e)
        } catch (e: Exception) {
            Log.w(TAG, "Exception requesting single update from $provider", e)
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
    private fun startMotionSample() {
        if (accelerometer == null) {
            latestMotionStr = "运动状态: 设备无重力传感器"
            return
        }
        
        synchronized(burstAccelSamples) { burstAccelSamples.clear() }
        synchronized(burstGyroSamples) { burstGyroSamples.clear() }
        synchronized(burstMagSamples) { burstMagSamples.clear() }
        
        sensorManager.registerListener(motionListener, accelerometer, SAMPLING_PERIOD_US)
        if (gyroscope != null) {
            sensorManager.registerListener(motionListener, gyroscope, SAMPLING_PERIOD_US)
        }
        if (magneticField != null) {
            sensorManager.registerListener(motionListener, magneticField, SAMPLING_PERIOD_US)
        }
        
    }

    private fun processMotionBurstData() {
        val accelList = synchronized(burstAccelSamples) { ArrayList(burstAccelSamples) }
        val gyroList = synchronized(burstGyroSamples) { ArrayList(burstGyroSamples) }
        val magList = synchronized(burstMagSamples) { ArrayList(burstMagSamples) }

        if (accelList.isEmpty()) return

        val accelAvg = accelList.average()
        val accelMax = accelList.maxOrNull() ?: 0.0

        val gyroAvg = if (gyroList.isNotEmpty()) gyroList.average() else 0.0
        val gyroMax = if (gyroList.isNotEmpty()) gyroList.maxOrNull() ?: 0.0 else 0.0

        val magAvg = if (magList.isNotEmpty()) magList.average() else 0.0

        var state = "静止"
        if (accelAvg > 12.0 || gyroAvg > 1.5) {
            state = "运动中"
        } else if (accelAvg > 10.5 || gyroAvg > 0.5) {
            state = "步行中"
        } else if (accelAvg > 9.5 || gyroAvg > 0.1) {
            state = "轻微移动"
        }

        val gyroStr = if (gyroscope != null) {
            String.format(Locale.US, " | 旋转角速度: %.2frad/s (峰值: %.2f)", gyroAvg, gyroMax)
        } else {
            " | 旋转角速度: 设备不支持"
        }

        val magStr = if (magneticField != null) {
            String.format(Locale.US, " | 磁场强度: %.1fμT", magAvg)
        } else {
            " | 磁场强度: 设备不支持"
        }

        val briefStr = String.format(Locale.US, "状态: %s", state)
        val detailStr = String.format(
            Locale.US,
            "状态: %s | 平均加速度: %.2fm/s² (峰值: %.2fm/s²)%s%s",
            state, accelAvg, accelMax, gyroStr, magStr
        )
        latestMotionStr = String.format(
            Locale.US,
            "[===vcp_fold: 0.0 ::desc: 物理运动姿态粗略状态(静止、步行、步行中或剧烈移动)===]\n%s\n\n[===vcp_fold: 0.50 ::desc: 九轴高频遥测指标、旋转角速度、加速度峰值、三轴磁敏度物理强度===]\n%s",
            briefStr, detailStr
        )
    }

    // ==================================================================
    // Ambient Helpers
    // ==================================================================
    private fun startAmbientSample() {
        lastLux = -1.0
        lastPressure = -1.0
        if (lightSensor != null) {
            sensorManager.registerListener(
                ambientListener,
                lightSensor,
                SensorManager.SENSOR_DELAY_NORMAL,
            )
        }
        if (pressureSensor != null) {
            sensorManager.registerListener(
                ambientListener,
                pressureSensor,
                SensorManager.SENSOR_DELAY_NORMAL,
            )
        }
    }

    private fun updateAmbientString() {
        val lightStr = if (lightSensor != null) {
            if (lastLux >= 0.0) {
                var desc = "未知"
                if (lastLux < 50.0) desc = "暗"
                else if (lastLux < 200.0) desc = "室内"
                else if (lastLux < 1000.0) desc = "明亮"
                else desc = "户外"
                String.format(Locale.US, "环境光: %.0f lux (%s)", lastLux, desc)
            } else {
                "环境光: 采集中..."
            }
        } else {
            "环境光: 设备不支持"
        }
        
        val pressureStr = if (pressureSensor != null) {
            if (lastPressure >= 0.0) {
                String.format(Locale.US, "气压: %.0f hPa", lastPressure)
            } else {
                "气压: 采集中..."
            }
        } else {
            "气压: 设备不支持"
        }

        val briefStr = lightStr
        val detailStr = "$lightStr | $pressureStr"

        latestAmbientStr = String.format(
            Locale.US,
            "[===vcp_fold: 0.0 ::desc: 当前所处的物理环境光照度大体描述(如暗、室内、户外)===]\n%s\n\n[===vcp_fold: 0.45 ::desc: 物理环境大气压强、精确光照度数值与场景气压监测===]\n%s",
            briefStr, detailStr
        )
    }
}
