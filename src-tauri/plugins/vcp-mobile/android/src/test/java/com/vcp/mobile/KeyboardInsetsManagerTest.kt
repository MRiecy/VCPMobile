package com.vcp.mobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class KeyboardInsetsManagerTest {
    @Test
    fun snapshotMergesSystemBarsAndDisplayCutoutPerEdge() {
        val snapshot = mergeInsetSnapshot(
            systemBars = EdgeInsetsPx(top = 24, right = 3, bottom = 48, left = 1),
            displayCutout = EdgeInsetsPx(top = 42, right = 8, bottom = 0, left = 12),
            imeBottomPx = 520,
            imeVisible = true,
        )

        assertEquals(42, snapshot.safeTopPx)
        assertEquals(8, snapshot.safeRightPx)
        assertEquals(48, snapshot.safeBottomPx)
        assertEquals(12, snapshot.safeLeftPx)
        assertEquals(520, snapshot.imeBottomPx)
        assertTrue(snapshot.imeVisible)
        assertEquals(472, netImeBottomPx(snapshot))
    }

    @Test
    fun hiddenImeIsNormalizedToZero() {
        val snapshot = mergeInsetSnapshot(
            systemBars = EdgeInsetsPx(top = 0, right = 0, bottom = 48, left = 0),
            displayCutout = EdgeInsetsPx(top = 0, right = 0, bottom = 0, left = 0),
            imeBottomPx = 520,
            imeVisible = false,
        )

        assertEquals(0, snapshot.imeBottomPx)
        assertFalse(snapshot.imeVisible)
        assertEquals(0, netImeBottomPx(snapshot))
    }

    @Test
    fun netImeNeverSubtractsBelowZero() {
        val snapshot = InsetSnapshot(
            safeTopPx = 0,
            safeRightPx = 0,
            safeBottomPx = 72,
            safeLeftPx = 0,
            imeBottomPx = 48,
            imeVisible = true,
        )

        assertEquals(0, netImeBottomPx(snapshot))
    }

    @Test
    fun detachStateClearsImeAndPreservesEverySafeEdge() {
        val attached = InsetSnapshot(
            safeTopPx = 42,
            safeRightPx = 8,
            safeBottomPx = 48,
            safeLeftPx = 12,
            imeBottomPx = 520,
            imeVisible = true,
        )

        assertEquals(
            InsetSnapshot(
                safeTopPx = 42,
                safeRightPx = 8,
                safeBottomPx = 48,
                safeLeftPx = 12,
                imeBottomPx = 0,
                imeVisible = false,
            ),
            attached.withoutIme(),
        )
    }

    @Test
    fun javascriptStoresReplaySnapshotBeforeDispatchingEvent() {
        val script = buildInsetsJavascript(
            InsetSnapshot(
                safeTopPx = 42,
                safeRightPx = 8,
                safeBottomPx = 48,
                safeLeftPx = 12,
                imeBottomPx = 520,
                imeVisible = true,
            ),
        )

        val replayAssignment = script.indexOf("window.__VCP_NATIVE_INSETS__ =")
        val eventDispatch = script.indexOf("window.dispatchEvent")
        assertTrue(replayAssignment >= 0)
        assertTrue(eventDispatch > replayAssignment)
        assertTrue(script.contains("\"safeTopPx\":42"))
        assertTrue(script.contains("\"safeRightPx\":8"))
        assertTrue(script.contains("\"safeBottomPx\":48"))
        assertTrue(script.contains("\"safeLeftPx\":12"))
        assertTrue(script.contains("\"imeBottomPx\":520"))
        assertTrue(script.contains("\"imeVisible\":true"))
        assertTrue(script.contains("detail: window.__VCP_NATIVE_INSETS__"))
    }
}
