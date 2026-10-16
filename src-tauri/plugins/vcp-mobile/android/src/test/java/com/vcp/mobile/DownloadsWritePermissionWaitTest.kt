package com.vcp.mobile

import app.tauri.plugin.Invoke
import io.mockk.mockk
import io.mockk.verify
import org.junit.Assert.assertEquals
import org.junit.Test

class DownloadsWritePermissionWaitTest {
    @Test
    fun destroyedPermissionWaitRejectsOnceAndIgnoresLateGrantedCallback() {
        val pending = DownloadsWritePermissionWait()
        val invoke = mockk<Invoke>(relaxed = true)
        var saves = 0
        pending.request(invoke) { }

        pending.destroy()
        pending.complete(invoke, true) { saves += 1 }
        pending.destroy()

        assertEquals(0, saves)
        verify(exactly = 1) { invoke.reject(any<String>()) }
    }

    @Test
    fun permissionLaunchFailureRejectsAndDoesNotLeaveARequestToResume() {
        val pending = DownloadsWritePermissionWait()
        val invoke = mockk<Invoke>(relaxed = true)
        var saves = 0

        pending.request(invoke) { throw IllegalStateException("activity unavailable") }
        pending.complete(invoke, true) { saves += 1 }
        pending.destroy()

        assertEquals(0, saves)
        verify(exactly = 1) { invoke.reject("无法申请下载目录写入权限: activity unavailable") }
    }

    @Test
    fun successfulHandoffIsNotRejectedByDestroyAndCannotRunTwice() {
        val pending = DownloadsWritePermissionWait()
        val invoke = mockk<Invoke>(relaxed = true)
        var saves = 0
        pending.request(invoke) { }

        pending.complete(invoke, true) { saves += 1 }
        pending.destroy()
        pending.complete(invoke, true) { saves += 1 }

        assertEquals(1, saves)
        verify(exactly = 0) { invoke.reject(any<String>()) }
    }

    @Test
    fun deniedPermissionRejectsWithoutStartingSave() {
        val pending = DownloadsWritePermissionWait()
        val invoke = mockk<Invoke>(relaxed = true)
        var saves = 0
        pending.request(invoke) { }

        pending.complete(invoke, false) { saves += 1 }
        pending.complete(invoke, false) { saves += 1 }
        pending.destroy()

        assertEquals(0, saves)
        verify(exactly = 1) { invoke.reject("保存到下载目录需要储存空间权限") }
    }
}
