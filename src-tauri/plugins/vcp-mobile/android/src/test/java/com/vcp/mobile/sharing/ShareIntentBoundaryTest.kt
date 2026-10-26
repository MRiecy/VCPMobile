package com.vcp.mobile.sharing

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.Rule
import org.junit.rules.TemporaryFolder
import java.io.File
import java.util.UUID

class ShareIntentBoundaryTest {
    @get:Rule val temporary = TemporaryFolder(File("build").apply { mkdirs() })
    @Test
    fun providerDisplayNameIsReducedToSafeUnicodeBasename() {
        assertEquals("报告 2026.pdf", sanitizeSharedFileName("../../报告 2026.pdf"))
        assertEquals("photo.jpg", sanitizeSharedFileName("folder\\photo.jpg"))
        assertEquals("shared_file", sanitizeSharedFileName(".."))
        assertFalse(sanitizeSharedFileName("bad\u0000name.txt").contains('\u0000'))
    }

    @Test
    fun copyBudgetRejectsPerFileTotalAndDeadlineOverflow() {
        val perFile = ShareCopyBudget(
            maxFileBytes = 4,
            maxTotalBytes = 8,
            globalDeadlineNanos = 100,
            perFileNanos = 50,
        )
        perFile.startFile(4, nowNanos = 0)
        perFile.consume(4, nowNanos = 40)
        assertFails { perFile.consume(1, nowNanos = 41) }

        val total = ShareCopyBudget(
            maxFileBytes = 8,
            maxTotalBytes = 8,
            globalDeadlineNanos = 100,
            perFileNanos = 50,
        )
        total.startFile(4, nowNanos = 0)
        total.consume(4, nowNanos = 10)
        assertFails { total.startFile(5, nowNanos = 20) }

        val deadline = ShareCopyBudget(
            maxFileBytes = 8,
            maxTotalBytes = 8,
            globalDeadlineNanos = 100,
            perFileNanos = 50,
        )
        deadline.startFile(-1, nowNanos = 0)
        assertFails { deadline.consume(1, nowNanos = 51) }
    }

    @Test
    fun latestIntentOwnerInvalidatesEarlierCompletion() {
        val owner = ShareIntentOwner()
        val first = owner.begin()
        assertTrue(owner.isCurrent(first))

        val second = owner.begin()
        assertNotEquals(first, second)
        assertFalse(owner.isCurrent(first))
        assertTrue(owner.isCurrent(second))
    }

    @Test
    fun newIntentClearsCachedPayloadBeforeWebViewCanConsumeIt() {
        val payload = LatestSharePayload<String>()
        val first = payload.begin()
        assertTrue(payload.publish(first, "A"))

        val second = payload.begin()
        assertNull(payload.peek())
        assertFalse(payload.publish(first, "late-A"))

        assertTrue(payload.publish(second, "B"))
        assertEquals("B", payload.peek()?.value)
        assertEquals("B", payload.peek()?.value)
    }

    @Test
    fun destroyClearsTheOwnerAndRejectsLatePayloadPublication() {
        val payload = LatestSharePayload<String>()
        val owner = payload.begin()
        assertTrue(payload.publish(owner, "pending"))
        payload.clear()
        assertFalse(payload.isCurrent(owner))
        assertFalse(payload.publish(owner, "late"))
        assertNull(payload.peek())
    }

    @Test
    fun acknowledgingReceivingCannotConsumeReadyOrInvalidateItsOwner() {
        val payload = LatestSharePayload<String>()
        val first = payload.begin()
        payload.publish(first, "receiving")
        val receiving = requireNotNull(payload.peek())
        payload.publish(first, "ready")
        val ready = requireNotNull(payload.peek())
        assertFalse(payload.acknowledge(first, receiving.revision))
        assertEquals(ready, payload.peek())
        assertTrue(payload.acknowledge(first, ready.revision))
        assertNull(payload.peek())
        assertTrue(payload.isCurrent(first))
        val second = payload.begin()
        payload.publish(second, "new share")
        assertFalse(payload.acknowledge(first, ready.revision))
        assertEquals("new share", payload.peek()?.value)
    }

    @Test
    fun physicalNamesStayBoundedRegardlessOfUnicodeDisplayNameOrExtension() {
        val owner = UUID.randomUUID().toString()
        val ticket = UUID.randomUUID().toString()
        val displayName = "汉".repeat(60) + ".pdf"
        assertEquals(displayName, sanitizeSharedFileName(displayName))
        val source = shareSourceStagingName(owner, ticket)
        val upload = shareUploadStagingName(owner, ticket, "a".repeat(64))
        assertTrue(source.toByteArray(Charsets.UTF_8).size < 255)
        assertTrue(upload.toByteArray(Charsets.UTF_8).size < 255)
        assertTrue(source.all { it.code < 128 })
        assertFalse(upload.contains(displayName))
        temporary.newFolder("unicode-names").resolve(source).writeText(displayName)
    }

    @Test
    fun sameHashUsesTicketOwnedUploadStagingPaths() {
        val root = temporary.newFolder("share-upload-staging")
        try {
            val ownerId = UUID.randomUUID().toString()
            val firstTicket = UUID.randomUUID().toString()
            val secondTicket = UUID.randomUUID().toString()
            val hash = "a".repeat(64)
            val first = root.resolve(
                shareUploadStagingName(ownerId, firstTicket, hash),
            )
            val second = root.resolve(
                shareUploadStagingName(ownerId, secondTicket, hash),
            )

            assertNotEquals(first, second)
            first.writeText("first")
            second.writeText("second")

            assertTrue(first.delete())
            assertFalse(first.exists())
            assertTrue(second.exists())
            assertEquals("second", second.readText())

            val firstThumbnail = root.resolve(
                "${shareUploadStagingStem(ownerId, firstTicket, hash)}_thumb.webp",
            )
            val secondThumbnail = root.resolve(
                "${shareUploadStagingStem(ownerId, secondTicket, hash)}_thumb.webp",
            )
            assertNotEquals(firstThumbnail, secondThumbnail)
            firstThumbnail.writeText("first-thumbnail")
            secondThumbnail.writeText("second-thumbnail")
            assertTrue(firstThumbnail.delete())
            assertFalse(firstThumbnail.exists())
            assertTrue(secondThumbnail.exists())
            assertEquals("second-thumbnail", secondThumbnail.readText())
        } finally {
            root.deleteRecursively()
        }
    }


    private fun assertFails(block: () -> Unit) {
        var failed = false
        try {
            block()
        } catch (_: Throwable) {
            failed = true
        }
        assertTrue(failed)
    }
}
