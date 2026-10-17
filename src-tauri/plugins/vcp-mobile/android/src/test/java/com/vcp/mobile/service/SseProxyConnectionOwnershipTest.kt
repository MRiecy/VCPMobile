package com.vcp.mobile.service

import org.junit.Assert.assertFalse
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.After
import org.junit.Before
import org.junit.runner.RunWith
import org.json.JSONObject
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.DataInputStream
import java.io.OutputStream
import java.net.Socket
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.ConcurrentHashMap
import java.util.UUID

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

    private class BlockingOutputStream : OutputStream() {
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)

        override fun write(value: Int) {
            entered.countDown()
            release.await()
        }

        override fun close() {
            release.countDown()
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

    @Test
    fun slowClientWriterUsesABoundedQueue() {
        val socket = TrackingSocket()
        val output = BlockingOutputStream()
        val connection = ClientConnection(socket, output)

        assertTrue(connection.enqueue("first"))
        assertTrue(output.entered.await(1, TimeUnit.SECONDS))
        repeat(128) { assertTrue(connection.enqueue("queued-$it")) }
        assertFalse(connection.enqueue("overflow"))

        output.release.countDown()
        connection.close()
    }

    @Test
    fun aTerminalSessionCannotBeReclassifiedByLateCloseOrFailure() {
        val session = SseProxyService.StreamSession("request-terminal")
        session.fullContent.append("received prefix")
        session.lastFinishReason = "tool_calls"
        assertFalse(session.isCompleted)

        assertTrue(session.complete("error", "connection reset"))
        assertFalse(session.complete("completed"))
        assertFalse(session.complete("cancelled_by_user"))
        assertEquals("received prefix", session.fullContent.toString())
        assertEquals("error", session.lastFinishReason)
        assertEquals("connection reset", session.terminalError)
    }

    @Test
    fun snapshotPrefixStaysExactAcrossUnicodeBoundariesAndConcurrentAppend() {
        val session = SseProxyService.StreamSession("request-snapshot")
        val prefix = "x".repeat(SseProxyService.CONTENT_FRAME_CHARS - 1) + "😀尾声"
        session.fullContent.append(prefix)
        val boundary = session.fullContent.length
        val first = session.contentChunk(0, boundary)
        assertFalse(Character.isHighSurrogate(first.last()))
        session.fullContent.append(" later")
        val second = session.contentChunk(first.length, boundary)
        assertEquals(prefix, first + second)
        assertEquals(prefix + " later", session.fullContent.toString())
    }

    @Test
    fun longSnapshotUsesOneWriterTaskAndPrecedesLiveData() {
        val output = ByteArrayOutputStream()
        val connection = ClientConnection(TrackingSocket(), output)
        val started = CountDownLatch(1)
        val release = CountDownLatch(1)
        try {
            assertTrue(connection.enqueueFrames { emit ->
                started.countDown()
                assertTrue(release.await(2, TimeUnit.SECONDS))
                repeat(300) { emit("snapshot-$it") }
            })
            assertTrue(started.await(1, TimeUnit.SECONDS))
            assertTrue(connection.enqueue("live-data"))
            release.countDown()
            assertTrue(connection.enqueueAndAwait("barrier"))
            val reader = DataInputStream(ByteArrayInputStream(output.toByteArray()))
            fun frame(): String {
                val bytes = ByteArray(reader.readInt())
                reader.readFully(bytes)
                return String(bytes, Charsets.UTF_8)
            }
            repeat(300) { assertEquals("snapshot-$it", frame()) }
            assertEquals("live-data", frame())
            assertEquals("barrier", frame())
            assertEquals(0, reader.available())
        } finally {
            release.countDown()
            connection.close()
        }
    }

    @Test
    fun notificationPreviewSkipsReasoningBeyondFrameBoundaries() {
        val text = StringBuilder("<THINK>")
            .append("private reasoning".repeat(5000))
            .append("</THINK>\n[--- VCP元思考链:planning ---]")
            .append("more reasoning".repeat(5000))
            .append("[--- 元思考链结束 ---]\n\nVisible reply\r\nwith spacing")
        assertEquals("Visible reply with spacing", notificationReplyPreview(text))
        assertEquals("", notificationReplyPreview(StringBuilder("<think>never closed")))
        assertEquals("Safe prefix", notificationReplyPreview(StringBuilder("Safe prefix<think>private")))
        assertEquals("😀".repeat(80) + "...", notificationReplyPreview(StringBuilder("😀".repeat(81))))
    }
}

@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE, sdk = [28])
class SseProxyProtocolTest {
    private lateinit var service: SseProxyService
    private lateinit var sessions: ConcurrentHashMap<String, SseProxyService.StreamSession>
    private val connections = mutableListOf<ClientConnection>()

    @Before
    fun attachServiceWithoutStartingNetworkOrForegroundWork() {
        service = Robolectric.buildService(SseProxyService::class.java).get()
        val field = SseProxyService::class.java.getDeclaredField("activeSessions").apply { isAccessible = true }
        @Suppress("UNCHECKED_CAST")
        sessions = field.get(service) as ConcurrentHashMap<String, SseProxyService.StreamSession>
        SseProxyService.isServiceRunning = true
    }

    @After
    fun releaseService() {
        connections.forEach { it.close() }
        service.onDestroy()
    }

    private fun connection(): Pair<ClientConnection, ByteArrayOutputStream> {
        val output = ByteArrayOutputStream()
        val connection = ClientConnection(Socket(), output)
        connections.add(connection)
        return connection to output
    }

    private fun session(content: String = ""): SseProxyService.StreamSession {
        val context = JSONObject().put("ownerType", "agent").put("ownerId", "agent-1")
            .put("topicId", "topic-1").put("messageId", "message-1")
        return SseProxyService.StreamSession(UUID.randomUUID().toString(), contextJson = context).also {
            it.fullContent.append(content)
            sessions[it.requestId] = it
        }
    }

    private fun invoke(name: String, vararg arguments: Any?): Any? {
        val method = SseProxyService::class.java.declaredMethods.single {
            it.name == name && it.parameterTypes.size == arguments.size
        }.apply { isAccessible = true }
        return method.invoke(service, *arguments)
    }

    private fun frames(connection: ClientConnection, output: ByteArrayOutputStream): List<JSONObject> {
        assertTrue(connection.enqueueAndAwait("{}"))
        val reader = DataInputStream(ByteArrayInputStream(output.toByteArray()))
        val frames = mutableListOf<JSONObject>()
        while (reader.available() > 0) {
            val bytes = ByteArray(reader.readInt())
            reader.readFully(bytes)
            frames.add(JSONObject(String(bytes, Charsets.UTF_8)))
        }
        return frames.dropLast(1)
    }

    @Test
    fun intermediateFinishReasonsKeepReceivingBodyAndNeverWriteFiles() {
        val session = session()
        invoke("acceptUpstreamEvent", session, "")
        invoke("acceptUpstreamEvent", session, """{"choices":[{"delta":{"content":"first"},"finish_reason":"tool_calls"}]}""")
        invoke("acceptUpstreamEvent", session, """{"choices":[{"delta":{"content":"second"},"finish_reason":"stop"}]}""")
        assertFalse(session.isCompleted)
        assertEquals("firstsecond", session.fullContent.toString())
        assertEquals("stop", session.lastFinishReason)
        assertFalse(java.io.File(service.cacheDir, "sse_offline_completed").exists())
    }

    @Test
    fun moreThanTwentyThousandUpstreamChunksRemainOneCompleteBody() {
        val session = session()
        val delta = """{"choices":[{"delta":{"content":"字"}}]}"""
        repeat(20_005) { invoke("acceptUpstreamEvent", session, delta) }
        assertFalse(session.isCompleted)
        assertEquals("字".repeat(20_005), session.fullContent.toString())
        assertFalse(java.io.File(service.cacheDir, "sse_offline_completed").exists())
    }

    @Test
    fun doneEmitsOneTerminalAfterIntermediateRoundReasons() {
        val session = session()
        val (stream, output) = connection()
        session.replaceConnection(stream)
        invoke("acceptUpstreamEvent", session, """{"choices":[{"delta":{"content":"first"},"finish_reason":"tool_calls"}]}""")
        invoke("acceptUpstreamEvent", session, """{"choices":[{"delta":{"content":"second"},"finish_reason":"stop"}]}""")
        assertFalse(session.isCompleted)
        invoke("acceptUpstreamEvent", session, "[DONE]")
        invoke("acceptUpstreamEvent", session, "[DONE]")
        invoke("acceptUpstreamEvent", session, """{"error":{"message":"late cancellation callback"}}""")
        val terminals = frames(stream, output).filter { it.optString("eventType") == "terminal" }
        assertEquals(1, terminals.size)
        assertEquals("stop", terminals.single().getString("finishReason"))
        assertEquals("firstsecond", session.fullContent.toString())
        assertTrue(session.isCompleted)
        assertSame(session, sessions[session.requestId])
        assertFalse(java.io.File(service.cacheDir, "sse_offline_completed").exists())
    }

    @Test
    fun completedOnlineBodySurvivesUntilMatchingDurableCommitAck() {
        val session = session("finished")
        val (stream, _) = connection()
        session.replaceConnection(stream)
        session.complete("stop")
        invoke("cleanupSessionIfCompletedAndDisconnected", session)
        assertSame(session, sessions[session.requestId])
        assertFalse(java.io.File(service.cacheDir, "sse_offline_completed").exists())

        val (control, _) = connection()
        invoke("handleControl", "ack_committed", session.requestId, UUID.randomUUID().toString(), control)
        assertSame(session, sessions[session.requestId])
        invoke("handleControl", "ack_committed", session.requestId, session.generation, control)
        assertNull(sessions[session.requestId])
        assertTrue(stream.socket.isClosed)
        assertFalse(java.io.File(service.cacheDir, "sse_offline_completed").exists())
    }

    @Test
    fun offlineTerminalPublishesRawUtf8BodyAndMatchingAckDeletesIt() {
        val content = "😀第一行\n".repeat(4000)
        val session = session(content)
        session.complete("error", "connection reset")
        invoke("cleanupSessionIfCompletedAndDisconnected", session)
        assertNull(sessions[session.requestId])
        val directory = java.io.File(service.cacheDir, "sse_offline_completed")
        val file = directory.listFiles()!!.single { it.extension == "completed" }
        val stored = file.readText(Charsets.UTF_8)
        val metadata = JSONObject(stored.substringBefore('\n'))
        assertEquals(session.requestId, metadata.getString("requestId"))
        assertEquals(session.generation, metadata.getString("generation"))
        assertEquals("message-1", metadata.getJSONObject("context").getString("messageId"))
        assertEquals("error", metadata.getString("finishReason"))
        assertEquals("connection reset", metadata.getString("error"))
        assertEquals(content, stored.substringAfter('\n'))
        assertTrue(directory.listFiles()!!.none { it.extension == "tmp" })

        val (control, _) = connection()
        invoke("handleControl", "ack_committed", session.requestId, UUID.randomUUID().toString(), control)
        assertTrue(file.exists())
        invoke("handleControl", "ack_committed", session.requestId, session.generation, control)
        assertFalse(file.exists())
    }

    @Test
    fun publicationFailureRetainsTheOnlyBodyForResume() {
        val mailbox = java.io.File(service.cacheDir, "sse_offline_completed")
        mailbox.writeText("not a directory")
        val session = session("must survive disk failure")
        session.complete("stop")
        try {
            invoke("cleanupSessionIfCompletedAndDisconnected", session)
            assertSame(session, sessions[session.requestId])
            assertEquals("must survive disk failure", session.fullContent.toString())
            assertEquals(0L, session.handoffWakeDeadlineMs)
            val wakeLock = SseProxyService::class.java.getDeclaredField("wakeLock").apply { isAccessible = true }
            assertNull(wakeLock.get(service))
        } finally {
            mailbox.delete()
        }
    }

    @Test
    fun terminalWithoutAckPublishesAfterFiveSecondsBeforeClosingTheConsumer() {
        val session = session("durable before disconnect")
        val disconnected = CountDownLatch(1)
        val publishedAtDisconnect = java.util.concurrent.atomic.AtomicBoolean(false)
        val socket = object : Socket() {
            override fun close() {
                val files = java.io.File(service.cacheDir, "sse_offline_completed").listFiles()
                publishedAtDisconnect.set(files?.any {
                    it.name.endsWith(".${session.generation}.completed") &&
                        it.readText().substringAfter('\n') == "durable before disconnect"
                } == true)
                super.close()
                disconnected.countDown()
            }
        }
        val stream = ClientConnection(socket, ByteArrayOutputStream())
        connections.add(stream)
        session.replaceConnection(stream)
        invoke("acceptUpstreamEvent", session, "[DONE]")
        assertFalse(disconnected.await(1, TimeUnit.SECONDS))
        assertSame(session, sessions[session.requestId])
        assertTrue(disconnected.await(6, TimeUnit.SECONDS))
        assertTrue(publishedAtDisconnect.get())
        assertNull(sessions[session.requestId])

        val (control, _) = connection()
        invoke("handleControl", "ack_committed", session.requestId, session.generation, control)
        assertTrue(java.io.File(service.cacheDir, "sse_offline_completed").listFiles()!!
            .none { it.name.endsWith(".${session.generation}.completed") })
    }

    @Test
    fun emptyDeltaChunksDoNotProduceFramesOrDisruptTheConsumer() {
        val session = session()
        val (stream, output) = connection()
        session.replaceConnection(stream)
        repeat(500) {
            invoke("acceptUpstreamEvent", session, """{"choices":[{"delta":{"reasoning_content":"private"}}]}""")
        }
        invoke("acceptUpstreamEvent", session, """{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}""")
        val events = frames(stream, output)
        assertEquals(1, events.size)
        assertEquals("tool_calls", events.single().getString("finishReason"))
        assertFalse(stream.socket.isClosed)
    }

    @Test
    fun invalidStartIsRejectedBeforeAStartedFrameOrSessionExists() {
        val (stream, output) = connection()
        assertNull(invoke("handleStartStream", "bad-url", "not a URL", "{}", "", null, stream))
        val events = frames(stream, output)
        assertEquals(listOf("error"), events.map { it.getString("eventType") })
        assertEquals("invalid_request", events.single().getString("code"))
        assertTrue(sessions.isEmpty())
    }

    @Test
    fun idleStopClosesAdmissionBeforeAndroidDestroysTheService() {
        invoke("stopIfIdle", false)
        assertFalse(SseProxyService.isServiceRunning)
        val (stream, output) = connection()
        assertNull(invoke("handleStartStream", "late-start", "https://example.invalid", "{}", "", null, stream))
        assertEquals("service_stopping", frames(stream, output).single().getString("code"))
        assertTrue(sessions.isEmpty())
    }

    @Test
    fun queryKeepsTaskRemovedButResumeResetsIt() {
        val session = session()
        service.onTaskRemoved(null)
        val removed = SseProxyService::class.java.getDeclaredField("isTaskRemoved").apply { isAccessible = true }
        assertTrue(removed.getBoolean(service))
        val (query, _) = connection()
        invoke("handleQueryStream", session.requestId, query)
        assertTrue(removed.getBoolean(service))
        val (stream, _) = connection()
        assertSame(session, invoke("handleResumeStream", session.requestId, session.generation, stream))
        assertFalse(removed.getBoolean(service))
    }

    @Test
    fun resumeSnapshotLiveSuffixAndCancellationShareOneOrderedConnection() {
        val prefix = "x".repeat(SseProxyService.CONTENT_FRAME_CHARS - 1) + "😀" + "y".repeat(20000)
        val session = session(prefix)
        val (old, _) = connection()
        session.replaceConnection(old)
        val (resumed, output) = connection()
        assertSame(session, invoke("handleResumeStream", session.requestId, session.generation, resumed))
        invoke("acceptUpstreamEvent", session, """{"choices":[{"delta":{"content":"suffix"}}]}""")
        val (control, _) = connection()
        invoke("handleControl", "cancel", session.requestId, session.generation, control)

        val events = frames(resumed, output)
        assertEquals("snapshot_start", events.first().getString("eventType"))
        assertEquals(prefix, events.filter { it.optString("eventType") == "snapshot_chunk" }
            .joinToString("") { it.getString("content") })
        val snapshotEnd = events.indexOfFirst { it.optString("eventType") == "snapshot_end" }
        assertFalse(events[snapshotEnd].getBoolean("completed"))
        assertEquals("data", events[snapshotEnd + 1].getString("eventType"))
        assertEquals("suffix", events[snapshotEnd + 1].getString("content"))
        assertEquals("terminal", events.last().getString("eventType"))
        assertEquals("cancelled_by_user", events.last().getString("finishReason"))
        assertSame(session, sessions[session.requestId])
        assertSame(resumed, session.activeConnection)
        assertTrue(old.socket.isClosed)
        assertFalse(java.io.File(service.cacheDir, "sse_offline_completed").exists())
    }
}
