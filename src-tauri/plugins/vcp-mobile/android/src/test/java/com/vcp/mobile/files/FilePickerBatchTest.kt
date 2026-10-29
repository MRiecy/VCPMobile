package com.vcp.mobile.files

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.Intent
import android.content.pm.ActivityInfo
import android.content.pm.ApplicationInfo
import android.content.pm.ResolveInfo
import android.net.Uri
import androidx.activity.result.ActivityResult
import androidx.activity.result.contract.ActivityResultContracts.PickVisualMedia
import androidx.core.content.FileProvider
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
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
import java.io.ByteArrayInputStream
import java.io.File
import java.io.InputStream
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28])
class FilePickerBatchTest {
    companion object {
        // Android 13 wire constants; this suite compiles against an Android 9 test stub.
        private const val PICK_IMAGES = "android.provider.action.PICK_IMAGES"
        private const val PICK_IMAGES_MAX = "android.provider.extra.PICK_IMAGES_MAX"
    }
    class PickerActivity : Activity() {
        lateinit var pickerCache: File
        override fun getCacheDir(): File = if (::pickerCache.isInitialized) pickerCache else super.getCacheDir()
    }
    @get:Rule val temporary = TemporaryFolder(File("build").apply { mkdirs() })
    private lateinit var activity: Activity
    private lateinit var picker: NativeFilePicker
    private lateinit var runtime: PluginTaskRuntime
    private lateinit var uploads: File
    private lateinit var launched: Intent
    private val launchAttempts = mutableListOf<Intent>()
    private var photoLaunchError: RuntimeException? = null
    private var documentLaunchError: RuntimeException? = null

    @Before fun setUp() {
        val cache = temporary.newFolder("cache")
        activity = Robolectric.buildActivity(PickerActivity::class.java).create().get().apply { pickerCache = cache }
        // Model a system-provided Photo Picker on pre-Android 13 devices.
        shadowOf(activity.packageManager).addResolveInfoForIntent(
            Intent(PickVisualMedia.ACTION_SYSTEM_FALLBACK_PICK_IMAGES),
            ResolveInfo().apply {
                isDefault = true
                activityInfo = ActivityInfo().apply {
                    packageName = "com.example.systemphotopicker"
                    name = "PhotoPickerActivity"
                    applicationInfo = ApplicationInfo().apply { flags = ApplicationInfo.FLAG_SYSTEM }
                }
            },
        )
        uploads = File(cache, "uploads").apply { mkdirs() }
        runtime = PluginTaskRuntime()
        picker = NativeFilePicker(
            activity,
            runtime,
            launchActivity = { _, intent, _ ->
                launchAttempts.add(intent)
                val error = if (intent.action == Intent.ACTION_OPEN_DOCUMENT) documentLaunchError else photoLaunchError
                if (error != null) throw error
                launched = intent
            },
            requestPermission = { _, _, _ -> error("Unexpected permission request") },
            emitEvent = { _, _ -> },
            openSource = { uri, _ -> activity.contentResolver.openInputStream(uri) },
        )
    }

    @After fun tearDown() {
        if (::runtime.isInitialized) {
            runtime.beginDestroy()
            picker.destroy()
            runtime.shutdown()
        }
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
        picker.pickFile(invoke)
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
        picker.onPickFileResult(invoke, ActivityResult(Activity.RESULT_OK, Intent().apply { clipData = clip }))
    }

    private fun drain() { runtime.domains.fileIoExecutor.submit { }.get(10, TimeUnit.SECONDS) }

    @Test @Config(sdk = [34])
    fun galleryUsesFrameworkPhotoPickerWithMultipleSelectionAndImportsBothImages() {
        val invoke = start(mode = "gallery")
        assertEquals(PICK_IMAGES, launched.action)
        assertEquals("image/*", launched.type)
        assertEquals(16, launched.getIntExtra(PICK_IMAGES_MAX, 0))
        assertFalse(launched.hasCategory(Intent.CATEGORY_OPENABLE))
        val response = slot<JSObject>()
        every { invoke.resolve(capture(response)) } just Runs
        deliver(invoke, source("first-image", byteArrayOf(1)), source("second-image", byteArrayOf(2)))
        drain()
        assertEquals(2, response.captured.getJSONArray("files").length())
        assertEquals(0, response.captured.getJSONArray("errors").length())
    }

    @Test @Config(sdk = [34])
    fun avatarAlwaysUsesSinglePhotoSelectionAndCancelReleasesThePicker() {
        val avatar = start(mode = "avatar", maxFiles = 16)
        assertEquals(PICK_IMAGES, launched.action)
        assertEquals("image/*", launched.type)
        assertFalse(launched.hasExtra(PICK_IMAGES_MAX))
        assertFalse(launched.getBooleanExtra(Intent.EXTRA_ALLOW_MULTIPLE, false))
        picker.onPickFileResult(avatar, ActivityResult(Activity.RESULT_CANCELED, null))
        verify(exactly = 1) { avatar.reject("cancelled: file picker") }

        val file = start()
        assertEquals(Intent.ACTION_OPEN_DOCUMENT, launched.action)
        assertEquals("*/*", launched.type)
        assertTrue(launched.hasCategory(Intent.CATEGORY_OPENABLE))
        assertTrue(launched.getBooleanExtra(Intent.EXTRA_ALLOW_MULTIPLE, false))
        verify(exactly = 0) { file.reject(any<String>()) }
    }

    @Test fun olderAndroidUsesTheAvailableSystemPhotoPicker() {
        val gallery = start(mode = "gallery")
        assertEquals(PickVisualMedia.ACTION_SYSTEM_FALLBACK_PICK_IMAGES, launched.action)
        assertEquals("com.example.systemphotopicker", launched.component?.packageName)
        assertEquals("image/*", launched.type)
        assertEquals(16, launched.getIntExtra(PickVisualMedia.EXTRA_SYSTEM_FALLBACK_PICK_IMAGES_MAX, 0))
        picker.onPickFileResult(gallery, ActivityResult(Activity.RESULT_CANCELED, null))

        start(mode = "avatar")
        assertEquals(PickVisualMedia.ACTION_SYSTEM_FALLBACK_PICK_IMAGES, launched.action)
        assertFalse(launched.hasExtra(PickVisualMedia.EXTRA_SYSTEM_FALLBACK_PICK_IMAGES_MAX))
    }

    @Test fun missingPhotoPickerUsesImageDocumentsWithGalleryMultipleAndAvatarSingleSelection() {
        shadowOf(activity.packageManager).removeResolveInfosForIntent(
            Intent(PickVisualMedia.ACTION_SYSTEM_FALLBACK_PICK_IMAGES), "com.example.systemphotopicker",
        )
        val gallery = start(mode = "gallery")
        assertEquals(Intent.ACTION_OPEN_DOCUMENT, launched.action)
        assertEquals("image/*", launched.type)
        assertTrue(launched.getBooleanExtra(Intent.EXTRA_ALLOW_MULTIPLE, false))
        val response = slot<JSObject>()
        every { gallery.resolve(capture(response)) } just Runs
        deliver(gallery, source("first-image", byteArrayOf(1)), source("second-image", byteArrayOf(2)))
        drain()
        assertEquals(2, response.captured.getJSONArray("files").length())
        verify(exactly = 0) { gallery.reject(any<String>()) }

        val avatar = start(mode = "avatar")
        assertEquals(Intent.ACTION_OPEN_DOCUMENT, launched.action)
        assertEquals("image/*", launched.type)
        assertFalse(launched.getBooleanExtra(Intent.EXTRA_ALLOW_MULTIPLE, false))
        picker.onPickFileResult(avatar, ActivityResult(Activity.RESULT_CANCELED, null))
        verify(exactly = 1) { avatar.reject("cancelled: file picker") }
        assertEquals(2, launchAttempts.size)

        launchAttempts.clear()
        documentLaunchError = ActivityNotFoundException("missing document picker")
        val failed = start(mode = "gallery")
        assertEquals(listOf(Intent.ACTION_OPEN_DOCUMENT), launchAttempts.map { it.action })
        verify(exactly = 1) { failed.reject("native_picker: missing document picker") }
    }

    @Test @Config(sdk = [34])
    fun photoPickerLaunchFailuresFallBackOnceAndCancellationDoesNotLaunchAgain() {
        for (error in listOf(ActivityNotFoundException("missing photo picker"), SecurityException("photo picker denied"))) {
            photoLaunchError = error
            for (mode in listOf("gallery", "avatar")) {
                launchAttempts.clear()
                val invoke = start(mode = mode)
                assertEquals(listOf(PICK_IMAGES, Intent.ACTION_OPEN_DOCUMENT), launchAttempts.map { it.action })
                assertEquals("image/*", launched.type)
                assertTrue(launched.hasCategory(Intent.CATEGORY_OPENABLE))
                assertEquals(mode == "gallery", launched.getBooleanExtra(Intent.EXTRA_ALLOW_MULTIPLE, false))
                verify(exactly = 0) { invoke.reject(any<String>()) }
                picker.onPickFileResult(invoke, ActivityResult(Activity.RESULT_CANCELED, null))
                verify(exactly = 1) { invoke.reject("cancelled: file picker") }
                assertEquals(2, launchAttempts.size)
            }
        }
    }

    @Test @Config(sdk = [34])
    fun failedDocumentFallbackSettlesOnceWithoutRetryAndReleasesItsSlot() {
        photoLaunchError = ActivityNotFoundException("missing photo picker")
        documentLaunchError = ActivityNotFoundException("missing document picker")
        val gallery = start(mode = "gallery")
        assertEquals(2, launchAttempts.size)
        verify(exactly = 1) { gallery.reject("native_picker: missing document picker") }

        launchAttempts.clear()
        val file = start()
        assertEquals(listOf(Intent.ACTION_OPEN_DOCUMENT), launchAttempts.map { it.action })
        verify(exactly = 1) { file.reject("native_picker: missing document picker") }

        documentLaunchError = null
        val next = start()
        verify(exactly = 0) { next.reject(any<String>()) }
    }

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
            picker.cancelFileOperation(cancel)
        } finally { resume.countDown() }
        drain()
        assertEquals(0, uploads.listFiles()!!.size)
        verify(exactly = 1) { invoke.reject("cancelled: file operation") }
        verify(exactly = 0) { invoke.resolve(any<JSObject>()) }
        // The previous native owner released its slot after the worker finished.
        val next = start()
        picker.onPickFileResult(next, ActivityResult(Activity.RESULT_CANCELED, null))
        verify(exactly = 1) { next.reject("cancelled: file picker") }
    }

    @Test fun cancelledChooserKeepsItsSlotUntilTheNativeResultReturns() {
        val pending = start()
        val cancel = mockk<Invoke>(relaxed = true)
        every { cancel.parseArgs(CancelFileOperationArgs::class.java) } returns CancelFileOperationArgs().apply {
            operationId = "batch_test"
        }
        picker.cancelFileOperation(cancel)
        verify(exactly = 1) { pending.reject("cancelled: file operation") }

        val blocked = start()
        verify(exactly = 1) { blocked.reject("busy: native picker is still active") }
        picker.onPickFileResult(pending, ActivityResult(Activity.RESULT_CANCELED, null))
        verify(exactly = 1) { pending.reject(any<String>()) }

        val next = start()
        verify(exactly = 0) { next.reject(any<String>()) }
        picker.onPickFileResult(next, ActivityResult(Activity.RESULT_CANCELED, null))
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
        picker.onPickFileResult(next, ActivityResult(Activity.RESULT_OK, Intent().setData(source("single", byteArrayOf(8)))))
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
            picker.onPickFileResult(invoke, ActivityResult(Activity.RESULT_OK, Intent().setData(source(mode, byteArrayOf(1)))))
            drain()
            assertTrue(response.captured.has("path"))
            assertFalse(response.captured.has("files"))
        }
        val avatar = start(mode = "avatar")
        assertFalse(launched.getBooleanExtra(Intent.EXTRA_ALLOW_MULTIPLE, false))
        picker.onPickFileResult(avatar, ActivityResult(Activity.RESULT_CANCELED, null))
        verify(exactly = 1) { avatar.reject("cancelled: file picker") }
    }

    @Test fun cancellationClosesABlockedSourceEvenWhenProviderCancellationAlsoBlocks() {
        val entered = CountDownLatch(1)
        val closed = CountDownLatch(1)
        val cancellationEntered = CountDownLatch(1)
        val releaseCancellation = CountDownLatch(1)
        val uri = Uri.parse("content://picker-test/blocked")
        val input = object : InputStream() {
            override fun read(): Int = error("Expected a buffered read")
            override fun read(buffer: ByteArray, offset: Int, length: Int): Int {
                entered.countDown()
                check(closed.await(5, TimeUnit.SECONDS))
                return -1
            }
            override fun close() { closed.countDown() }
        }
        picker = NativeFilePicker(activity, runtime,
            launchActivity = { _, intent, _ -> launched = intent },
            requestPermission = { _, _, _ -> error("Unexpected permission request") },
            emitEvent = { _, _ -> },
            openSource = { _, cancellation ->
                cancellation.setOnCancelListener {
                    cancellationEntered.countDown()
                    check(releaseCancellation.await(5, TimeUnit.SECONDS))
                }
                input
            },
        )
        val invoke = start()
        deliver(invoke, uri)
        assertTrue(entered.await(5, TimeUnit.SECONDS))
        val cancel = mockk<Invoke>(relaxed = true)
        every { cancel.parseArgs(CancelFileOperationArgs::class.java) } returns CancelFileOperationArgs().apply {
            operationId = "batch_test"
        }
        try {
            picker.cancelFileOperation(cancel)
            assertTrue(cancellationEntered.await(5, TimeUnit.SECONDS))
            assertTrue(closed.await(5, TimeUnit.SECONDS))
            drain()
        } finally {
            releaseCancellation.countDown()
        }
        assertEquals(0, uploads.listFiles()!!.size)
        verify(exactly = 1) { invoke.reject("cancelled: file operation") }
        verify(exactly = 0) { invoke.resolve(any<JSObject>()) }
    }

    @Test fun lateResultCannotFinishTheNextPickerOwner() {
        val old = start()
        picker.onPickFileResult(old, ActivityResult(Activity.RESULT_CANCELED, null))
        val next = start()
        picker.onPickFileResult(old, ActivityResult(Activity.RESULT_CANCELED, null))
        verify(exactly = 0) { next.reject(any<String>()) }
        val response = slot<JSObject>()
        every { next.resolve(capture(response)) } just Runs
        deliver(next, source("current", byteArrayOf(7)))
        drain()
        assertEquals(1, response.captured.getJSONArray("files").length())
        verify(exactly = 1) { old.reject(any<String>()) }
    }

    @Test fun destroyDeletesTheCameraFileWithoutWaitingForItsActivityResult() {
        shadowOf(activity.application).grantPermissions(android.Manifest.permission.CAMERA)
        mockkStatic(FileProvider::class)
        val cameraFile = slot<File>()
        every { FileProvider.getUriForFile(any(), any(), capture(cameraFile)) } answers {
            Uri.fromFile(cameraFile.captured)
        }
        val invoke = start(mode = "camera")
        cameraFile.captured.writeBytes(byteArrayOf(1, 2))
        every { invoke.reject(any<String>()) } throws IllegalStateException("WebView is gone")
        runtime.beginDestroy()
        picker.destroy()
        assertFalse(cameraFile.captured.exists())
        cameraFile.captured.writeBytes(byteArrayOf(3, 4))
        picker.onCameraResult(invoke, ActivityResult(Activity.RESULT_OK, null))
        assertFalse(cameraFile.captured.exists())
        verify(exactly = 1) { invoke.reject("cancelled: plugin destroyed") }
        verify(exactly = 0) { invoke.resolve(any<JSObject>()) }
    }

}
