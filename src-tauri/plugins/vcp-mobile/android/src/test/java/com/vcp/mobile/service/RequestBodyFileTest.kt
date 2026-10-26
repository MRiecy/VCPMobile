package com.vcp.mobile.service

import com.vcp.mobile.sse.MAX_REQUEST_BODY_BYTES
import com.vcp.mobile.sse.RequestBodyFile
import okio.Buffer
import okio.ForwardingSink
import okio.blackholeSink
import okio.buffer
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test
import java.io.File
import java.io.IOException
import java.io.RandomAccessFile
import java.nio.file.Files
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread

class RequestBodyFileTest {
    private lateinit var root: File

    @Before fun setup() {
        val fixtures = File("build/test-fixtures").apply { mkdirs() }
        root = Files.createTempDirectory(fixtures.toPath(), "request-body-").toFile()
    }

    @After fun cleanup() { root.deleteRecursively() }

    private fun prepared(size: Long): Pair<String, File> {
        val token = UUID.randomUUID().toString()
        val directory = File(root, "request-bodies/prepared").apply { mkdirs() }
        val file = File(directory, "$token.body")
        RandomAccessFile(file, "rw").use { it.setLength(size) }
        return token to file
    }

    @Test fun largeBodyLeavesTheControlFrameAndStreamsExactBytes() {
        val size = 11L * 1024 * 1024
        val (token, source) = prepared(size)
        val body = RequestBodyFile.claim(root, token, size, UUID.randomUUID().toString())
        assertFalse(source.exists())
        assertEquals(size, body.contentLength())
        var received = 0L
        val sink = object : ForwardingSink(blackholeSink()) {
            override fun write(source: Buffer, byteCount: Long) {
                received += byteCount
                super.write(source, byteCount)
            }
        }.buffer()
        body.writeTo(sink)
        sink.close()
        assertEquals(size, received)
        // A second upload is allowed until redirect/authentication settles.
        body.writeTo(blackholeSink().buffer())
        body.close()
        assertTrue(File(root, "request-bodies/owned").listFiles()!!.isEmpty())
    }

    @Test fun budgetAndSizeValidationLeaveUnacceptedFilesWithRust() {
        val (token, source) = prepared(MAX_REQUEST_BODY_BYTES)
        assertThrows(IllegalArgumentException::class.java) {
            RequestBodyFile.claim(root, token, MAX_REQUEST_BODY_BYTES + 1, UUID.randomUUID().toString())
        }
        assertTrue(source.exists())
        assertThrows(IllegalArgumentException::class.java) {
            RequestBodyFile.claim(root, token, MAX_REQUEST_BODY_BYTES - 1, UUID.randomUUID().toString())
        }
        val body = RequestBodyFile.claim(root, token, MAX_REQUEST_BODY_BYTES, UUID.randomUUID().toString())
        body.close()
        assertFalse(source.exists())
    }

    @Test fun cancellationWaitsForTheCurrentUploadAndPreventsAnotherWrite() {
        val (token, _) = prepared(32 * 1024)
        val body = RequestBodyFile.claim(root, token, 32 * 1024, UUID.randomUUID().toString())
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val sink = object : ForwardingSink(blackholeSink()) {
            override fun write(source: Buffer, byteCount: Long) {
                entered.countDown()
                check(release.await(5, TimeUnit.SECONDS))
                super.write(source, byteCount)
            }
        }.buffer()
        val uploading = thread { body.writeTo(sink); sink.close() }
        try {
            assertTrue(entered.await(2, TimeUnit.SECONDS))
            body.close()
            assertEquals(1, File(root, "request-bodies/owned").listFiles()!!.size)
            assertThrows(IOException::class.java) { body.writeTo(blackholeSink().buffer()) }
        } finally {
            release.countDown()
            uploading.join(5000)
        }
        assertFalse(uploading.isAlive)
        assertTrue(File(root, "request-bodies/owned").listFiles()!!.isEmpty())
    }
}
