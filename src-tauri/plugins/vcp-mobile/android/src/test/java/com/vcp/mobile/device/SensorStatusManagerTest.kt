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
