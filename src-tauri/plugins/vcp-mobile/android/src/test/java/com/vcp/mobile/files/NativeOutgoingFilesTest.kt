package com.vcp.mobile.files

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.os.Looper
import androidx.core.content.FileProvider
import app.tauri.plugin.Invoke
import com.vcp.mobile.runtime.PluginTaskRuntime
import io.mockk.*
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import org.robolectric.annotation.LooperMode
import java.io.File
import java.util.concurrent.TimeUnit

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28])
@LooperMode(LooperMode.Mode.PAUSED)
class NativeOutgoingFilesTest {
    class OutgoingActivity : Activity() {
        lateinit var outgoingCache: File
        var rejectActivity = false
        var starts = 0
        override fun getCacheDir(): File = if (::outgoingCache.isInitialized) outgoingCache else super.getCacheDir()
        override fun startActivity(intent: Intent) {
            check(Looper.myLooper() == Looper.getMainLooper())
            if (rejectActivity) throw ActivityNotFoundException("No receiver")
            starts += 1
        }
    }

    @get:Rule val temporary = TemporaryFolder(File("build").apply { mkdirs() })
    private lateinit var activity: OutgoingActivity
    private lateinit var runtime: PluginTaskRuntime
    private lateinit var outgoing: NativeOutgoingFiles
    private lateinit var source: File

    @Before fun setUp() {
        activity = Robolectric.buildActivity(OutgoingActivity::class.java).create().get().apply {
            outgoingCache = temporary.newFolder("cache")
        }
        source = File(activity.cacheDir, "report.txt").apply { writeText("receiver-owned contents") }
        runtime = PluginTaskRuntime()
        outgoing = NativeOutgoingFiles(activity, runtime)
        mockkStatic(FileProvider::class)
        every { FileProvider.getUriForFile(any(), any(), any()) } answers {
            Uri.parse("content://outgoing-test/${thirdArg<File>().name}")
        }
    }

    @After fun tearDown() {
        runtime.beginDestroy()
        shadowOf(Looper.getMainLooper()).idle()
        runtime.shutdown()
        unmockkAll()
    }

    private fun prepare(): Invoke {
        val invoke = mockk<Invoke>(relaxed = true)
        every { invoke.parseArgs(OpenFileArgs::class.java) } returns OpenFileArgs().apply { path = source.absolutePath }
        outgoing.openFile(invoke)
        runtime.domains.fileIoExecutor.submit { }.get(5, TimeUnit.SECONDS)
        return invoke
    }

    private fun retainedFiles(): List<File> = File(activity.cacheDir, "native-outgoing")
        .walkTopDown().filter { it.isFile }.toList()

    @Test fun failedDispatchDeletesOnlyItsUndeliveredCopy() {
        activity.rejectActivity = true
        val invoke = prepare()
        shadowOf(Looper.getMainLooper()).idle()
        assertTrue(retainedFiles().isEmpty())
        assertEquals("receiver-owned contents", source.readText())
        verify(exactly = 1) { invoke.reject(any<String>()) }
        verify(exactly = 0) { invoke.resolve() }
    }

    @Test fun destructionBeforeUiDispatchCannotLaunchTheOldActivity() {
        val invoke = prepare()
        assertEquals(1, retainedFiles().size)
        runtime.beginDestroy()
        shadowOf(Looper.getMainLooper()).idle()
        assertEquals(0, activity.starts)
        assertTrue(retainedFiles().isEmpty())
        verify(exactly = 1) { invoke.reject(any<String>()) }
    }

    @Test fun successfulDispatchKeepsItsCopyAfterTheCallerReleasesTheSource() {
        val invoke = prepare()
        shadowOf(Looper.getMainLooper()).idle()
        assertEquals(1, activity.starts)
        assertTrue(source.delete())
        assertEquals("receiver-owned contents", retainedFiles().single().readText())
        verify(exactly = 1) { invoke.resolve() }
        verify(exactly = 0) { invoke.reject(any<String>()) }
    }
}
