package com.vcp.mobile.files

import app.tauri.plugin.Invoke
import io.mockk.mockk
import io.mockk.every
import io.mockk.verify
import org.junit.Assert.assertEquals
import org.junit.Test

class LegacyWritePermissionWaitTest {
    @Test
    fun destroyedPermissionWaitRejectsOnceAndIgnoresLateGrantedCallback() {
        val pending = LegacyWritePermissionWait()
        val invoke = mockk<Invoke>(relaxed = true)
        var saves = 0
        pending.request(invoke, { saves += 1 }) { }
        every { invoke.reject(any<String>()) } throws IllegalStateException("WebView is gone")

        pending.destroy()
        pending.complete(invoke, true)
        pending.destroy()

        assertEquals(0, saves)
        verify(exactly = 1) { invoke.reject(any<String>()) }
    }

    @Test
    fun permissionLaunchFailureRejectsAndDoesNotLeaveARequestToResume() {
        val pending = LegacyWritePermissionWait()
        val invoke = mockk<Invoke>(relaxed = true)
        var saves = 0

        pending.request(invoke, { saves += 1 }) { throw IllegalStateException("activity unavailable") }
        pending.complete(invoke, true)
        pending.destroy()

        assertEquals(0, saves)
        verify(exactly = 1) { invoke.reject("无法申请储存空间权限: activity unavailable") }
    }

    @Test
    fun successfulHandoffIsNotRejectedByDestroyAndCannotRunTwice() {
        val pending = LegacyWritePermissionWait()
        val invoke = mockk<Invoke>(relaxed = true)
        var saves = 0
        pending.request(invoke, { saves += 1 }) { }

        pending.complete(invoke, true)
        pending.destroy()
        pending.complete(invoke, true)

        assertEquals(1, saves)
        verify(exactly = 0) { invoke.reject(any<String>()) }
    }

    @Test
    fun deniedPermissionRejectsWithoutStartingSave() {
        val pending = LegacyWritePermissionWait()
        val invoke = mockk<Invoke>(relaxed = true)
        var saves = 0
        pending.request(invoke, { saves += 1 }) { }

        pending.complete(invoke, false)
        pending.complete(invoke, false)
        pending.destroy()

        assertEquals(0, saves)
        verify(exactly = 1) { invoke.reject("保存文件需要储存空间权限") }
    }

    @Test
    fun anotherExportCannotReplaceTheWaitingGalleryContinuation() {
        val pending = LegacyWritePermissionWait()
        val gallery = mockk<Invoke>(relaxed = true)
        val download = mockk<Invoke>(relaxed = true)
        var gallerySaves = 0
        var downloadSaves = 0
        pending.request(gallery, { gallerySaves += 1 }) { }
        pending.request(download, { downloadSaves += 1 }) { error("must not request twice") }

        pending.complete(download, true)
        pending.complete(gallery, true)
        pending.complete(gallery, true)

        assertEquals(1, gallerySaves)
        assertEquals(0, downloadSaves)
        verify(exactly = 1) { download.reject("已有文件保存正在等待储存空间权限") }
        verify(exactly = 0) { gallery.reject(any<String>()) }
    }
}
