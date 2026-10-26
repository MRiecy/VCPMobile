package com.vcp.mobile.service

import com.vcp.mobile.sse.ClientConnection
import com.vcp.mobile.sse.CONTENT_FRAME_CHARS
import com.vcp.mobile.sse.StreamSession
import com.vcp.mobile.sse.SsePowerLocks
import com.vcp.mobile.sse.SseSessionCoordinator
import com.vcp.mobile.sse.notificationReplyPreview

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
import android.content.Context
import androidx.test.core.app.ApplicationProvider
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import okhttp3.Request
import okhttp3.sse.EventSource
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.OutputStream
import java.net.ConnectException
import java.net.InetSocketAddress
import java.net.Socket
import java.net.SocketException
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.UUID
import kotlin.coroutines.CoroutineContext
import kotlinx.coroutines.CoroutineDispatcher

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
        val session = StreamSession("request-1")
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
        val session = StreamSession("request-2")
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
        val session = StreamSession("request-terminal")
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
        val session = StreamSession("request-snapshot")
        val prefix = "x".repeat(CONTENT_FRAME_CHARS - 1) + "😀尾声"
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
    private lateinit var context: Context
    private lateinit var coordinator: SseSessionCoordinator
    private val connections = mutableListOf<ClientConnection>()
    private val stopRequests = AtomicInteger(0)

    private class HeldClientDispatcher : CoroutineDispatcher() {
        val dispatched = CountDownLatch(1)
        private val pending = ConcurrentLinkedQueue<Runnable>()

        override fun dispatch(context: CoroutineContext, block: Runnable) {
            pending.add(block)
            dispatched.countDown()
        }

        fun drain() {
            while (true) (pending.poll() ?: return).run()
        }
    }

    @Before
    fun activateCoordinatorWithControlledUpstream() {
        context = ApplicationProvider.getApplicationContext()
        coordinator = SseSessionCoordinator(
            context,
            stopService = { stopRequests.incrementAndGet() },
            eventSourceFactory = EventSource.Factory { request, _ ->
                object : EventSource {
                    override fun request(): Request = request
                    override fun cancel() = Unit
                }
            },
        )
        coordinator.activate()
    }

    @After
    fun releaseService() {
        connections.forEach { it.close() }
        coordinator.destroy()
    }

    private fun connection(): Pair<ClientConnection, ByteArrayOutputStream> {
        val output = ByteArrayOutputStream()
        val connection = ClientConnection(Socket(), output)
        connections.add(connection)
        return connection to output
    }

    private fun session(content: String = "", requestId: String = UUID.randomUUID().toString()): StreamSession {
        val metadata = JSONObject().put("ownerType", "agent").put("ownerId", "agent-1")
            .put("topicId", "topic-1").put("messageId", "message-1")
        val (initial, _) = connection()
        val bodyToken = UUID.randomUUID().toString()
        val prepared = java.io.File(context.cacheDir, "request-bodies/prepared").apply { mkdirs() }
        java.io.File(prepared, "$bodyToken.body").writeText("{}")
        return requireNotNull(coordinator.handleStartStream(
            requestId, "https://example.invalid", "{}", bodyToken, 2, metadata, initial,
        )).also {
            it.detachConnection(initial)
            initial.close()
            it.fullContent.append(content)
        }
    }

    private fun holdsNativeWakeLock(): Boolean {
        val ownerField = SseSessionCoordinator::class.java.getDeclaredField("powerLocks")
            .apply { isAccessible = true }
        val lockField = SsePowerLocks::class.java.getDeclaredField("wakeLock")
            .apply { isAccessible = true }
        val lock = lockField.get(ownerField.get(coordinator)) as? android.os.PowerManager.WakeLock
        return lock?.isHeld == true
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

    private fun awaitCondition(timeoutMs: Long, condition: () -> Boolean) {
        val deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(timeoutMs)
        while (!condition() && System.nanoTime() < deadline) Thread.sleep(10)
        assertTrue("Condition did not become true within ${timeoutMs}ms", condition())
    }

    private fun retries(session: StreamSession): Int =
        synchronized(session) { session.publicationRetriesAttempted }

    private fun assertHandoffWakeLockReleased(session: StreamSession) {
        assertEquals(0L, session.handoffWakeDeadlineMs)
        awaitCondition(1_000) { !holdsNativeWakeLock() }
    }

    private fun startListener(): Int {
        val discovery = java.io.File(context.cacheDir, "sse_helper.port")
        coordinator.startListening()
        awaitCondition(2_000) { discovery.exists() && discovery.readText().toIntOrNull() != null }
        return discovery.readText().toInt()
    }

    private fun assertSocketDisconnected(socket: Socket) {
        socket.soTimeout = 1_000
        try {
            assertEquals(-1, socket.getInputStream().read())
        } catch (_: SocketException) {
            // Closing a socket with unread command bytes may reset the peer.
            // A read timeout is not a disconnect and deliberately fails this test.
        }
    }

    @Test
    fun intermediateFinishReasonsKeepReceivingBodyAndNeverWriteFiles() {
        val session = session()
        coordinator.acceptUpstreamEvent(session, "")
        coordinator.acceptUpstreamEvent(session, """{"choices":[{"delta":{"content":"first"},"finish_reason":"tool_calls"}]}""")
        coordinator.acceptUpstreamEvent(session, """{"choices":[{"delta":{"content":"second"},"finish_reason":"stop"}]}""")
        assertFalse(session.isCompleted)
        assertEquals("firstsecond", session.fullContent.toString())
        assertEquals("stop", session.lastFinishReason)
        assertFalse(java.io.File(context.cacheDir, "sse_offline_completed").exists())
    }

    @Test
    fun moreThanTwentyThousandUpstreamChunksRemainOneCompleteBody() {
        val session = session()
        val delta = """{"choices":[{"delta":{"content":"字"}}]}"""
        repeat(20_005) { coordinator.acceptUpstreamEvent(session, delta) }
        assertFalse(session.isCompleted)
        assertEquals("字".repeat(20_005), session.fullContent.toString())
        assertFalse(java.io.File(context.cacheDir, "sse_offline_completed").exists())
    }

    @Test
    fun doneEmitsOneTerminalAfterIntermediateRoundReasons() {
        val session = session()
        val (stream, output) = connection()
        session.replaceConnection(stream)
        coordinator.acceptUpstreamEvent(session, """{"choices":[{"delta":{"content":"first"},"finish_reason":"tool_calls"}]}""")
        coordinator.acceptUpstreamEvent(session, """{"choices":[{"delta":{"content":"second"},"finish_reason":"stop"}]}""")
        assertFalse(session.isCompleted)
        coordinator.acceptUpstreamEvent(session, "[DONE]")
        coordinator.acceptUpstreamEvent(session, "[DONE]")
        coordinator.acceptUpstreamEvent(session, """{"error":{"message":"late cancellation callback"}}""")
        val terminals = frames(stream, output).filter { it.optString("eventType") == "terminal" }
        assertEquals(1, terminals.size)
        assertEquals("stop", terminals.single().getString("finishReason"))
        assertEquals("firstsecond", session.fullContent.toString())
        assertTrue(session.isCompleted)
        assertSame(session, coordinator.findSession(session.requestId))
        assertFalse(java.io.File(context.cacheDir, "sse_offline_completed").exists())
    }

    @Test
    fun completedOnlineBodySurvivesUntilMatchingDurableCommitAck() {
        val session = session("finished")
        val (stream, _) = connection()
        session.replaceConnection(stream)
        session.complete("stop")
        coordinator.cleanupSessionIfCompletedAndDisconnected(session)
        assertSame(session, coordinator.findSession(session.requestId))
        assertFalse(java.io.File(context.cacheDir, "sse_offline_completed").exists())

        val (control, _) = connection()
        coordinator.handleControl("ack_committed", session.requestId, UUID.randomUUID().toString(), control)
        assertSame(session, coordinator.findSession(session.requestId))
        coordinator.handleControl("ack_committed", session.requestId, session.generation, control)
        assertNull(coordinator.findSession(session.requestId))
        assertTrue(stream.socket.isClosed)
        assertFalse(java.io.File(context.cacheDir, "sse_offline_completed").exists())
    }

    @Test
    fun offlineTerminalPublishesRawUtf8BodyAndMatchingAckDeletesIt() {
        val content = "😀第一行\n".repeat(4000)
        val session = session(content)
        session.complete("error", "connection reset")
        coordinator.cleanupSessionIfCompletedAndDisconnected(session)
        assertNull(coordinator.findSession(session.requestId))
        val directory = java.io.File(context.cacheDir, "sse_offline_completed")
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
        coordinator.handleControl("ack_committed", session.requestId, UUID.randomUUID().toString(), control)
        assertTrue(file.exists())
        coordinator.handleControl("ack_committed", session.requestId, session.generation, control)
        assertFalse(file.exists())
    }

    @Test
    fun publicationFailureRetainsTheOnlyBodyForResume() {
        val mailbox = java.io.File(context.cacheDir, "sse_offline_completed")
        mailbox.writeText("not a directory")
        val session = session("must survive disk failure")
        session.complete("stop")
        try {
            coordinator.cleanupSessionIfCompletedAndDisconnected(session)
            assertSame(session, coordinator.findSession(session.requestId))
            assertEquals("must survive disk failure", session.fullContent.toString())
            assertEquals(0L, session.handoffWakeDeadlineMs)
            assertFalse(holdsNativeWakeLock())
        } finally {
            mailbox.delete()
        }
    }

    @Test
    fun publicationRetriesWaitTwoThenFourSecondsAndPublishTheOriginalBody() {
        val mailbox = java.io.File(context.cacheDir, "sse_offline_completed")
        mailbox.writeText("block publication")
        val session = session("original reply survives both failures")
        session.complete("stop")
        coordinator.cleanupSessionIfCompletedAndDisconnected(session)
        assertHandoffWakeLockReleased(session)
        Thread.sleep(1_000)
        assertEquals(0, retries(session))
        awaitCondition(2_000) { retries(session) == 1 }
        assertHandoffWakeLockReleased(session)

        // The first retry has really failed. Restore the filesystem before the
        // second retry; its success must retain the exact original generation.
        assertTrue(mailbox.delete())
        assertTrue(mailbox.mkdir())
        Thread.sleep(3_000)
        assertSame(session, coordinator.findSession(session.requestId))
        assertEquals(1, retries(session))
        awaitCondition(2_000) { coordinator.findSession(session.requestId) == null }
        assertEquals(2, retries(session))
        val stored = mailbox.listFiles()!!.single { it.extension == "completed" }.readText()
        assertEquals(session.generation, JSONObject(stored.substringBefore('\n')).getString("generation"))
        assertEquals("original reply survives both failures", stored.substringAfter('\n'))
    }

    @Test
    fun exhaustedRetriesRetainBodyWithoutAnotherTimerOrWakeLock() {
        val mailbox = java.io.File(context.cacheDir, "sse_offline_completed")
        mailbox.writeText("persistent publication failure")
        val session = session("never discard the unsaved reply")
        session.complete("stop")
        coordinator.cleanupSessionIfCompletedAndDisconnected(session)
        awaitCondition(8_000) { retries(session) == 2 }
        assertHandoffWakeLockReleased(session)
        assertNull(synchronized(session) { session.handoffJob })
        assertSame(session, coordinator.findSession(session.requestId))
        assertEquals("never discard the unsaved reply", session.fullContent.toString())

        assertTrue(mailbox.delete())
        assertTrue(mailbox.mkdir())
        Thread.sleep(2_200)
        assertTrue(mailbox.listFiles()!!.isEmpty())
        assertEquals(2, retries(session))
        val (query, output) = connection()
        coordinator.handleQueryStream(session.requestId, query)
        assertEquals("completed", frames(query, output).single().getString("status"))
        assertEquals(2, retries(session))
    }

    @Test
    fun commitAckAndDiscardCancelPendingRetriesWithoutResurrectingFiles() {
        val mailbox = java.io.File(context.cacheDir, "sse_offline_completed")
        mailbox.writeText("block publication")
        val acknowledged = session("already committed")
        val discarded = session("deleted result")
        for (session in listOf(acknowledged, discarded)) {
            session.complete("stop")
            coordinator.cleanupSessionIfCompletedAndDisconnected(session)
        }
        val acknowledgedJob = synchronized(acknowledged) { acknowledged.handoffJob!! }
        val discardedJob = synchronized(discarded) { discarded.handoffJob!! }
        val (control, _) = connection()
        coordinator.handleControl("ack_committed", acknowledged.requestId, acknowledged.generation, control)
        coordinator.handleControl("discard", discarded.requestId, discarded.generation, control)
        assertTrue(acknowledgedJob.isCancelled)
        assertTrue(discardedJob.isCancelled)
        assertNull(coordinator.findSession(acknowledged.requestId))
        assertNull(coordinator.findSession(discarded.requestId))
        val replacement = session("new generation", acknowledged.requestId)
        assertTrue(mailbox.delete())
        assertTrue(mailbox.mkdir())
        Thread.sleep(2_200)
        assertTrue(mailbox.listFiles()!!.isEmpty())
        assertEquals(0, retries(acknowledged))
        assertEquals(0, retries(discarded))
        assertSame(replacement, coordinator.findSession(replacement.requestId))
        assertEquals("new generation", replacement.fullContent.toString())
    }

    @Test
    fun resumeReplacesRetryWithAckWaitWithoutResettingGenerationBudget() {
        val mailbox = java.io.File(context.cacheDir, "sse_offline_completed")
        mailbox.writeText("block publication")
        val session = session("resumable terminal body")
        session.complete("stop")
        coordinator.cleanupSessionIfCompletedAndDisconnected(session)
        awaitCondition(3_000) { retries(session) == 1 }
        val retryJob = synchronized(session) { session.handoffJob!! }
        val (stream, output) = connection()
        assertSame(session, coordinator.handleResumeStream(session.requestId, session.generation, stream))
        assertTrue(retryJob.isCancelled)
        assertEquals(1, retries(session))
        assertTrue(frames(stream, output).last().getBoolean("completed"))
        assertTrue(mailbox.delete())
        assertTrue(mailbox.mkdir())
        // The cancelled 4-second retry must not close the resumed consumer.
        Thread.sleep(4_200)
        assertSame(session, coordinator.findSession(session.requestId))
        assertFalse(stream.socket.isClosed)
        assertTrue(mailbox.listFiles()!!.isEmpty())
        awaitCondition(2_000) { coordinator.findSession(session.requestId) == null }
        assertEquals(1, retries(session))
        assertTrue(stream.socket.isClosed)
    }

    @Test
    fun destructionAttemptsPublicationOnceAndDoesNotScheduleNewRetries() {
        val mailbox = java.io.File(context.cacheDir, "sse_offline_completed")
        mailbox.writeText("block publication")
        val session = session("pending at destruction")
        session.complete("stop")
        coordinator.cleanupSessionIfCompletedAndDisconnected(session)
        val retryJob = synchronized(session) { session.handoffJob!! }
        coordinator.destroy()
        assertTrue(retryJob.isCancelled)
        assertNull(synchronized(session) { session.handoffJob })
        assertEquals(0, retries(session))
        assertHandoffWakeLockReleased(session)
        assertTrue(mailbox.delete())
    }

    @Test
    fun terminalDropsTheNetworkOwnerBeforeLateCancellationCallback() {
        val session = session("finished reply")
        val (stream, _) = connection()
        session.replaceConnection(stream)
        val cancellations = AtomicInteger(0)
        session.eventSource = object : EventSource {
            override fun request(): Request = Request.Builder().url("https://example.invalid").build()

            override fun cancel() {
                cancellations.incrementAndGet()
                assertNull(session.eventSource)
                coordinator.completeSession(session, "error", "late cancellation callback")
            }
        }
        coordinator.acceptUpstreamEvent(session, "[DONE]")
        assertNull(session.eventSource)
        assertEquals(1, cancellations.get())
        assertEquals("completed", session.lastFinishReason)
        assertNull(session.terminalError)
        assertEquals("finished reply", session.fullContent.toString())
    }

    @Test
    fun terminalWithoutAckPublishesAfterFiveSecondsBeforeClosingTheConsumer() {
        val session = session("durable before disconnect")
        val disconnected = CountDownLatch(1)
        val publishedAtDisconnect = java.util.concurrent.atomic.AtomicBoolean(false)
        val socket = object : Socket() {
            override fun close() {
                val files = java.io.File(context.cacheDir, "sse_offline_completed").listFiles()
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
        coordinator.acceptUpstreamEvent(session, "[DONE]")
        assertFalse(disconnected.await(1, TimeUnit.SECONDS))
        assertSame(session, coordinator.findSession(session.requestId))
        assertTrue(disconnected.await(6, TimeUnit.SECONDS))
        assertTrue(publishedAtDisconnect.get())
        assertNull(coordinator.findSession(session.requestId))

        val (control, _) = connection()
        coordinator.handleControl("ack_committed", session.requestId, session.generation, control)
        assertTrue(java.io.File(context.cacheDir, "sse_offline_completed").listFiles()!!
            .none { it.name.endsWith(".${session.generation}.completed") })
    }

    @Test
    fun emptyDeltaChunksDoNotProduceFramesOrDisruptTheConsumer() {
        val session = session()
        val (stream, output) = connection()
        session.replaceConnection(stream)
        repeat(500) {
            coordinator.acceptUpstreamEvent(session, """{"choices":[{"delta":{"reasoning_content":"private"}}]}""")
        }
        coordinator.acceptUpstreamEvent(session, """{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}""")
        val events = frames(stream, output)
        assertEquals(1, events.size)
        assertEquals("tool_calls", events.single().getString("finishReason"))
        assertFalse(stream.socket.isClosed)
    }

    @Test
    fun invalidStartIsRejectedBeforeAStartedFrameOrSessionExists() {
        val (stream, output) = connection()
        assertNull(coordinator.handleStartStream("bad-url", "not a URL", "{}", "", 0, null, stream))
        val events = frames(stream, output)
        assertEquals(listOf("error"), events.map { it.getString("eventType") })
        assertEquals("invalid_request", events.single().getString("code"))
        assertTrue(coordinator.isIdle)
    }

    @Test
    fun idleStopClosesAdmissionBeforeAndroidDestroysTheService() {
        coordinator.stopIfIdle(false)
        assertFalse(coordinator.isRunning)
        val (stream, output) = connection()
        assertNull(coordinator.handleStartStream("late-start", "https://example.invalid", "{}", "", 0, null, stream))
        assertEquals("service_stopping", frames(stream, output).single().getString("code"))
        assertTrue(coordinator.isIdle)
    }

    @Test
    fun listenerPublishesAUsableFramedEndpointAndIdleStopRemovesDiscovery() {
        val discovery = java.io.File(context.cacheDir, "sse_helper.port")
        coordinator.startListening()
        awaitCondition(2_000) { discovery.exists() && discovery.readText().toIntOrNull() != null }
        val port = discovery.readText().toInt()
        Socket("127.0.0.1", port).use { socket ->
            socket.soTimeout = 2_000
            val request = """{"action":"query","requestId":"missing"}""".toByteArray(Charsets.UTF_8)
            val writer = DataOutputStream(socket.getOutputStream())
            writer.writeInt(request.size)
            writer.write(request)
            writer.flush()
            val reader = DataInputStream(socket.getInputStream())
            val bytes = ByteArray(reader.readInt())
            reader.readFully(bytes)
            assertEquals("not_found", JSONObject(String(bytes, Charsets.UTF_8)).getString("status"))
        }

        assertEquals(0, stopRequests.get())
        // Reading the query reply does not mean the server coroutine has
        // finished releasing its accepted-socket registration yet.
        val clientsField = SseSessionCoordinator::class.java.getDeclaredField("clientSockets")
            .apply { isAccessible = true }
        awaitCondition(2_000) { (clientsField.get(coordinator) as Set<*>).isEmpty() }
        coordinator.stopIfIdle()
        assertEquals(1, stopRequests.get())
        assertFalse(coordinator.isRunning)
        assertFalse(discovery.exists())
        assertFalse(java.io.File(context.cacheDir, "sse_helper.port.tmp").exists())
        Socket().use { probe ->
            val refused = try {
                probe.connect(InetSocketAddress("127.0.0.1", port), 1_000)
                false
            } catch (_: ConnectException) {
                true
            }
            assertTrue("The stopped listener must refuse new TCP connections", refused)
        }
    }

    @Test
    fun destructionClosesAnAcceptedSocketBeforeItsCoroutineRuns() {
        coordinator.destroy()
        val dispatcher = HeldClientDispatcher()
        coordinator = SseSessionCoordinator(
            context,
            stopService = { stopRequests.incrementAndGet() },
            clientDispatcher = dispatcher,
        )
        coordinator.activate()
        val port = startListener()
        try {
            Socket("127.0.0.1", port).use { socket ->
                assertTrue(dispatcher.dispatched.await(2, TimeUnit.SECONDS))
                coordinator.destroy()
                assertSocketDisconnected(socket)
                assertFalse(java.io.File(context.cacheDir, "sse_helper.port").exists())
            }
        } finally {
            dispatcher.drain()
        }
    }

    @Test
    fun destructionInterruptsAPartialCommandReadWithoutWaitingForItsTimeout() {
        val port = startListener()
        Socket("127.0.0.1", port).use { socket ->
            val writer = DataOutputStream(socket.getOutputStream())
            writer.writeInt(128)
            writer.writeByte('{'.code)
            writer.flush()
            val socketsField = SseSessionCoordinator::class.java.getDeclaredField("clientSockets")
                .apply { isAccessible = true }
            awaitCondition(2_000) {
                @Suppress("UNCHECKED_CAST")
                val accepted = socketsField.get(coordinator) as Set<Socket>
                accepted.any { !it.isClosed && it.soTimeout == 5_000 }
            }

            coordinator.destroy()

            assertSocketDisconnected(socket)
            assertTrue(coordinator.isIdle)
        }
    }

    @Test
    fun taskRemovalWaitsForTheLastControlAckBeforeStopping() {
        val session = session("committed body")
        val (stream, _) = connection()
        session.replaceConnection(stream)
        coordinator.completeSession(session, "completed")
        coordinator.onTaskRemoved()
        val port = startListener()
        Socket("127.0.0.1", port).use { socket ->
            socket.soTimeout = 2_000
            val request = JSONObject().put("action", "ack_committed")
                .put("requestId", session.requestId).put("generation", session.generation)
                .toString().toByteArray(Charsets.UTF_8)
            val writer = DataOutputStream(socket.getOutputStream())
            writer.writeInt(request.size)
            writer.write(request)
            writer.flush()
            val reader = DataInputStream(socket.getInputStream())
            val bytes = ByteArray(reader.readInt())
            reader.readFully(bytes)
            val ack = JSONObject(String(bytes, Charsets.UTF_8))
            assertEquals("ack_committed_ack", ack.getString("action"))
            assertEquals(session.generation, ack.getString("generation"))
            assertTrue(ack.getBoolean("accepted"))
        }
        awaitCondition(2_000) { stopRequests.get() == 1 }
        assertFalse(coordinator.isRunning)
        assertNull(coordinator.findSession(session.requestId))
        assertFalse(java.io.File(context.cacheDir, "sse_helper.port").exists())
    }

    @Test
    fun queryKeepsTaskRemovedButResumeResetsIt() {
        val session = session()
        coordinator.onTaskRemoved()
        assertTrue(coordinator.isTaskRemoved)
        val (query, _) = connection()
        coordinator.handleQueryStream(session.requestId, query)
        assertTrue(coordinator.isTaskRemoved)
        val (stream, _) = connection()
        assertSame(session, coordinator.handleResumeStream(session.requestId, session.generation, stream))
        assertFalse(coordinator.isTaskRemoved)
    }

    @Test
    fun resumeSnapshotLiveSuffixAndCancellationShareOneOrderedConnection() {
        val prefix = "x".repeat(CONTENT_FRAME_CHARS - 1) + "😀" + "y".repeat(20000)
        val session = session(prefix)
        val (old, _) = connection()
        session.replaceConnection(old)
        val (resumed, output) = connection()
        assertSame(session, coordinator.handleResumeStream(session.requestId, session.generation, resumed))
        coordinator.acceptUpstreamEvent(session, """{"choices":[{"delta":{"content":"suffix"}}]}""")
        val (control, _) = connection()
        coordinator.handleControl("cancel", session.requestId, session.generation, control)

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
        assertSame(session, coordinator.findSession(session.requestId))
        assertSame(resumed, session.activeConnection)
        assertTrue(old.socket.isClosed)
        assertFalse(java.io.File(context.cacheDir, "sse_offline_completed").exists())
    }
}
