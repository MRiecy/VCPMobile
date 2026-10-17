package com.vcp.mobile

import android.app.Activity
import android.content.ClipData
import android.content.Intent
import android.net.Uri
import androidx.activity.result.ActivityResult
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import com.vcp.mobile.cli.CliProcessHostOwner
import com.vcp.mobile.cli.CliPtyHostOwner
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
import java.io.ByteArrayInputStream
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28])
class FilePickerBatchTest {
    class PickerActivity : Activity() {
        lateinit var pickerCache: File
        override fun getCacheDir(): File = if (::pickerCache.isInitialized) pickerCache else super.getCacheDir()
    }
    @get:Rule val temporary = TemporaryFolder(File("build").apply { mkdirs() })
    private lateinit var activity: Activity
    private lateinit var plugin: VcpMobilePlugin
    private lateinit var domains: PluginExecutorDomains
    private lateinit var uploads: File
    private lateinit var launched: Intent

    @Before fun setUp() {
        // The actual picker/worker runs; unrelated CLI process owners are inert.
        mockkObject(CliProcessHostOwner, CliPtyHostOwner)
        every { CliProcessHostOwner.get(any()) } returns mockk(relaxed = true)
        every { CliPtyHostOwner.get(any()) } returns mockk(relaxed = true)
        val cache = temporary.newFolder("cache")
        activity = Robolectric.buildActivity(PickerActivity::class.java).create().get().apply { pickerCache = cache }
        uploads = File(cache, "uploads").apply { mkdirs() }
        plugin = spyk(VcpMobilePlugin(activity))
        every { plugin.startActivityForResult(any(), any(), any()) } answers {
            launched = secondArg()
            Unit
        }
        domains = VcpMobilePlugin::class.java.getDeclaredField("executorDomains").let {
            it.isAccessible = true
            it.get(plugin) as PluginExecutorDomains
        }
    }

    @After fun tearDown() {
        if (::domains.isInitialized) domains.shutdownNow()
        unmockkAll()
    }

    private fun start(mode: String = "file", maxFiles: Int = 16, maxBytes: Long = 100): Invoke {
        val invoke = mockk<Invoke>(relaxed = true)
        every { invoke.parseArgs(PickFileArgs::class.java) } returns PickFileArgs().apply {
            this.mode = mode
            this.maxFiles = maxFiles
            this.maxBytes = maxBytes
            operationId = "batch_test"
            stagingDir = uploads.absolutePath
        }
        plugin.pickFile(invoke)
        return invoke
    }

    private fun source(name: String, bytes: ByteArray): Uri {
        val uri = Uri.parse("content://picker-test/$name")
        shadowOf(activity.contentResolver).registerInputStream(uri, ByteArrayInputStream(bytes))
        return uri
    }

    private fun deliver(invoke: Invoke, vararg uris: Uri) {
        val clip = ClipData.newRawUri("selection", uris.first())
        uris.drop(1).forEach { clip.addItem(ClipData.Item(it)) }
        plugin.onPickFileResult(invoke, ActivityResult(Activity.RESULT_OK, Intent().apply { clipData = clip }))
    }

    private fun drain() { domains.fileIoExecutor.submit { }.get(10, TimeUnit.SECONDS) }

    @Test fun middleCopyFailureKeepsSuccessesAndRemovesPartialFile() {
        val first = source("first", byteArrayOf(1, 2))
        val bad = source("too-large", ByteArray(8))
        val last = source("last", byteArrayOf(3, 4, 5))
        val invoke = start(maxBytes = 5)
        val response = slot<JSObject>()
        every { invoke.resolve(capture(response)) } just Runs
        assertTrue(launched.getBooleanExtra(Intent.EXTRA_ALLOW_MULTIPLE, false))
        deliver(invoke, first, bad, last)
        drain()
        val files = response.captured.getJSONArray("files")
        assertEquals(2, files.length())
        assertEquals(1, response.captured.getJSONArray("errors").length())
        assertArrayEquals(byteArrayOf(1, 2), File(files.getJSONObject(0).getString("path")).readBytes())
        assertArrayEquals(byteArrayOf(3, 4, 5), File(files.getJSONObject(1).getString("path")).readBytes())
        assertEquals(2, uploads.listFiles()!!.size)
        verify(exactly = 0) { invoke.reject(any<String>()) }
    }

    @Test fun cancellationDuringSecondCopyCleansEarlierSuccessAndIgnoresLateCompletion() {
        val entered = CountDownLatch(1)
        val resume = CountDownLatch(1)
        val slow = Uri.parse("content://picker-test/slow")
        shadowOf(activity.contentResolver).registerInputStream(slow, object : ByteArrayInputStream(ByteArray(8)) {
            override fun read(buffer: ByteArray, offset: Int, length: Int): Int {
                entered.countDown()
                check(resume.await(10, TimeUnit.SECONDS))
                return super.read(buffer, offset, length)
            }
        })
        val invoke = start()
        deliver(invoke, source("first", byteArrayOf(1)), slow, source("never", byteArrayOf(3)))
        try {
            assertTrue(entered.await(10, TimeUnit.SECONDS))
            val cancel = mockk<Invoke>(relaxed = true)
            every { cancel.parseArgs(CancelFileOperationArgs::class.java) } returns CancelFileOperationArgs().apply { operationId = "batch_test" }
            plugin.cancelFileOperation(cancel)
        } finally { resume.countDown() }
        drain()
        assertEquals(0, uploads.listFiles()!!.size)
        verify(exactly = 1) { invoke.reject("cancelled: file operation") }
        verify(exactly = 0) { invoke.resolve(any<JSObject>()) }
        // The previous native owner released its slot after the worker finished.
        val next = start()
        plugin.onPickFileResult(next, ActivityResult(Activity.RESULT_CANCELED, null))
        verify(exactly = 1) { next.reject("cancelled: file picker") }
    }

    @Test fun clipDataDeduplicatesAndSingleUriFallbackStillReturnsBatch() {
        val uri = source("same", byteArrayOf(7))
        val invoke = start(mode = "gallery")
        val response = slot<JSObject>()
        every { invoke.resolve(capture(response)) } just Runs
        deliver(invoke, uri, uri)
        drain()
        assertEquals(1, response.captured.getJSONArray("files").length())
        val next = start()
        every { next.resolve(capture(response)) } just Runs
        plugin.onPickFileResult(next, ActivityResult(Activity.RESULT_OK, Intent().setData(source("single", byteArrayOf(8)))))
        drain()
        assertEquals(1, response.captured.getJSONArray("files").length())
        assertEquals(0, response.captured.getJSONArray("errors").length())
    }

    @Test fun tooManySelectionsAreReportedWithoutDiscardingAllowedItems() {
        val invoke = start()
        val response = slot<JSObject>()
        every { invoke.resolve(capture(response)) } just Runs
        deliver(invoke, *(0..16).map { source("file-$it", byteArrayOf(it.toByte())) }.toTypedArray())
        drain()
        assertEquals(16, response.captured.getJSONArray("files").length())
        assertEquals("其余 1 项", response.captured.getJSONArray("errors").getJSONObject(0).getString("name"))
        assertEquals(16, uploads.listFiles()!!.size)
    }

    @Test fun configAndDefaultFileConsumersKeepSingleResult() {
        for (mode in listOf("config", "file")) {
            val invoke = start(mode = mode, maxFiles = if (mode == "config") 16 else 1)
            val response = slot<JSObject>()
            every { invoke.resolve(capture(response)) } just Runs
            assertFalse(launched.getBooleanExtra(Intent.EXTRA_ALLOW_MULTIPLE, false))
            plugin.onPickFileResult(invoke, ActivityResult(Activity.RESULT_OK, Intent().setData(source(mode, byteArrayOf(1)))))
            drain()
            assertTrue(response.captured.has("path"))
            assertFalse(response.captured.has("files"))
        }
        val avatar = start(mode = "avatar")
        assertFalse(launched.getBooleanExtra(Intent.EXTRA_ALLOW_MULTIPLE, false))
        plugin.onPickFileResult(avatar, ActivityResult(Activity.RESULT_CANCELED, null))
        verify(exactly = 1) { avatar.reject("cancelled: file picker") }
    }
}
