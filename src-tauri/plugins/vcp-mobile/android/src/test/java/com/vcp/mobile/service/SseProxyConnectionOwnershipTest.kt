package com.vcp.mobile.service

import org.junit.Assert.assertFalse
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.OutputStream
import java.net.Socket
import java.util.concurrent.atomic.AtomicInteger

class SseProxyConnectionOwnershipTest {
    private class TrackingSocket : Socket() {
        val closeCount = AtomicInteger(0)

        override fun close() {
            closeCount.incrementAndGet()
        }
    }

    private class TrackingOutputStream : OutputStream() {
        val closeCount = AtomicInteger(0)

        override fun write(value: Int) = Unit

        override fun close() {
            closeCount.incrementAndGet()
        }
    }

    private data class TrackedConnection(
        val connection: ClientConnection,
        val socket: TrackingSocket,
        val output: TrackingOutputStream,
    )

    private fun connection(): TrackedConnection {
        val socket = TrackingSocket()
        val output = TrackingOutputStream()
        return TrackedConnection(ClientConnection(socket, output), socket, output)
    }

    @Test
    fun staleConnectionCannotDetachOrCloseCurrentResumeConnection() {
        val session = SseProxyService.StreamSession("request-1")
        val first = connection()
        val resumed = connection()

        session.replaceConnection(first.connection)
        session.replaceConnection(resumed.connection)?.close()

        assertFalse(session.detachConnection(first.connection))
        first.connection.close()

        assertSame(resumed.connection, session.activeConnection)
        assertFalse(resumed.socket.isClosed)
        assertEquals(0, resumed.socket.closeCount.get())
        assertEquals(0, resumed.output.closeCount.get())
    }

    @Test
    fun delayedFinalizersAcrossThreeResumesOnlyCloseTheirOwnConnections() {
        val session = SseProxyService.StreamSession("request-2")
        val first = connection()
        val second = connection()
        val third = connection()

        session.replaceConnection(first.connection)
        session.replaceConnection(second.connection)?.close()
        session.replaceConnection(third.connection)?.close()

        assertFalse(session.detachConnection(first.connection))
        assertFalse(session.detachConnection(second.connection))
        assertSame(third.connection, session.activeConnection)
        assertEquals(0, third.socket.closeCount.get())

        assertTrue(session.detachConnection(third.connection))
        third.connection.close()
        assertNull(session.activeConnection)
        assertEquals(1, first.socket.closeCount.get())
        assertEquals(1, second.socket.closeCount.get())
        assertEquals(1, third.socket.closeCount.get())
    }
}
