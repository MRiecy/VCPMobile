package com.vcp.mobile.device

import android.content.Context
import android.hardware.Sensor
import android.hardware.SensorManager
import android.location.LocationManager
import android.os.Handler
import android.os.Looper
import io.mockk.every
import io.mockk.mockk
import io.mockk.verify
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import org.robolectric.annotation.LooperMode
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28])
@LooperMode(LooperMode.Mode.PAUSED)
class SensorStatusManagerTest {
    private fun sampleMotion(acceleration: List<Double>, angularSpeed: List<Double> = listOf(0.0)): String {
        val sensors = mockk<SensorManager>(relaxed = true)
        val locations = mockk<LocationManager>(relaxed = true)
        val accelerometer = mockk<Sensor>()
        val gyroscope = mockk<Sensor>()
        every { sensors.getDefaultSensor(any()) } returns null
        every { sensors.getDefaultSensor(Sensor.TYPE_ACCELEROMETER) } returns accelerometer
        every { sensors.getDefaultSensor(Sensor.TYPE_GYROSCOPE) } returns gyroscope
        val context = mockk<Context>()
        every { context.getSystemService(Context.SENSOR_SERVICE) } returns sensors
        every { context.getSystemService(Context.LOCATION_SERVICE) } returns locations
        val manager = SensorStatusManager(context)
        var result: String? = null
        manager.sample(location = false, motion = true, ambient = false) { result = it.getString("motion") }
        // Supply a two-second burst at the classifier's input boundary. No wall-clock
        // sleeps or invented sensor callbacks are needed to verify the reported physical state.
        fun fillBurst(field: String, values: List<Double>) {
            @Suppress("UNCHECKED_CAST")
            val burst = SensorStatusManager::class.java.getDeclaredField(field).apply { isAccessible = true }
                .get(manager) as ArrayList<Double>
            burst.addAll(values)
        }
        fillBurst("burstAccelSamples", acceleration)
        fillBurst("burstGyroSamples", angularSpeed)
        manager.shutdown()
        return requireNotNull(result)
    }

    @Test fun gravityAndSmallSensorNoiseRemainStationary() {
        val gravity = SensorManager.GRAVITY_EARTH.toDouble()
        val motion = sampleMotion(List(20) { gravity + if (it % 2 == 0) 0.03 else -0.03 })
        assertTrue(motion.contains("状态: 静止"))
    }

    @Test fun rotationIsDetectedEvenWhenAccelerationMagnitudeStaysAtGravity() {
        val gravity = SensorManager.GRAVITY_EARTH.toDouble()
        assertTrue(sampleMotion(List(20) { gravity }, List(20) { 0.3 }).contains("状态: 轻微移动"))
        assertTrue(sampleMotion(List(20) { gravity }, List(20) { 1.7 }).contains("状态: 运动中"))
    }

    @Test fun oppositeAccelerationExcursionsDoNotCancelIntoARestingAverage() {
        val gravity = SensorManager.GRAVITY_EARTH.toDouble()
        val walking = sampleMotion(List(20) { gravity + if (it % 2 == 0) 1.0 else -1.0 })
        val vigorous = sampleMotion(List(20) { gravity + if (it % 2 == 0) 3.0 else -3.0 })
        assertTrue(walking.contains("状态: 步行中"))
        assertTrue(vigorous.contains("状态: 运动中"))
    }

    @Test
    fun shutdownReleasesSamplingAndRejectsAnAlreadyQueuedStart() {
        val sensors = mockk<SensorManager>(relaxed = true)
        val locations = mockk<LocationManager>(relaxed = true)
        val accelerometer = mockk<Sensor>()
        every { sensors.getDefaultSensor(any()) } returns null
        every { sensors.getDefaultSensor(Sensor.TYPE_ACCELEROMETER) } returns accelerometer
        val context = mockk<Context>()
        every { context.getSystemService(Context.SENSOR_SERVICE) } returns sensors
        every { context.getSystemService(Context.LOCATION_SERVICE) } returns locations
        val manager = SensorStatusManager(context)
        var completions = 0
        manager.sample(location = false, motion = true, ambient = false) {
            completions++
            // A stale Invoke must not interrupt the owner's teardown.
            throw IllegalStateException("consumer disappeared")
        }
        val lateFailure = AtomicReference<Throwable?>()
        Handler(Looper.getMainLooper()).post {
            runCatching {
                manager.sample(location = false, motion = true, ambient = false) { completions++ }
            }.onFailure(lateFailure::set)
        }

        manager.shutdown()
        shadowOf(Looper.getMainLooper()).idleFor(3, TimeUnit.SECONDS)
        manager.shutdown()

        assertEquals(1, completions)
        assertTrue(lateFailure.get() is IllegalStateException)
        verify(exactly = 1) { sensors.registerListener(any(), accelerometer, any<Int>()) }
        verify(exactly = 1) { sensors.unregisterListener(any<android.hardware.SensorEventListener>()) }
    }
}
