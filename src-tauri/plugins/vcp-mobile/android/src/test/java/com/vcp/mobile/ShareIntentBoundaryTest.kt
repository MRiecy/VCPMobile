package com.vcp.mobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.nio.file.Files
import java.util.UUID

class ShareIntentBoundaryTest {
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
        val delivered = mutableListOf<String>()
        assertFalse(payload.consumeCurrent(delivered::add))
        assertFalse(payload.publish(first, "late-A"))

        assertTrue(payload.publish(second, "B"))
        assertTrue(payload.consumeCurrent(delivered::add))
        assertEquals(listOf("B"), delivered)
    }

    @Test
    fun sameHashUsesTicketOwnedUploadStagingPaths() {
        val root = Files.createTempDirectory("share-upload-staging").toFile()
        try {
            val ownerId = UUID.randomUUID().toString()
            val firstTicket = UUID.randomUUID().toString()
            val secondTicket = UUID.randomUUID().toString()
            val hash = "a".repeat(64)
            val first = root.resolve(
                shareUploadStagingName(ownerId, firstTicket, hash, ".bin"),
            )
            val second = root.resolve(
                shareUploadStagingName(ownerId, secondTicket, hash, ".bin"),
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

    @Test
    fun sameHashPickerSelectionsUseDifferentStagingPaths() {
        val root = Files.createTempDirectory("picker-upload-staging").toFile()
        try {
            val hash = "b".repeat(64)
            val firstTicket = UUID.randomUUID().toString()
            val secondTicket = UUID.randomUUID().toString()
            val first = root.resolve(pickerUploadStagingName(firstTicket, hash, ".png"))
            val second = root.resolve(pickerUploadStagingName(secondTicket, hash, ".png"))
            val firstThumbnail = root.resolve(
                "${pickerUploadStagingStem(firstTicket, hash)}_thumb.webp",
            )
            val secondThumbnail = root.resolve(
                "${pickerUploadStagingStem(secondTicket, hash)}_thumb.webp",
            )

            assertNotEquals(first, second)
            assertNotEquals(firstThumbnail, secondThumbnail)
            first.writeText("first")
            second.writeText("second")
            firstThumbnail.writeText("first-thumbnail")
            secondThumbnail.writeText("second-thumbnail")

            assertTrue(first.delete())
            assertTrue(firstThumbnail.delete())
            assertTrue(second.exists())
            assertTrue(secondThumbnail.exists())
            assertEquals("second", second.readText())
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
