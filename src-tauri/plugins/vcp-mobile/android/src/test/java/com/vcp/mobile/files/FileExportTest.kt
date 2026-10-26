package com.vcp.mobile.files

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.nio.ByteBuffer

class FileExportTest {
    @get:Rule
    val temporaryFolder = TemporaryFolder(java.io.File("build").apply { mkdirs() })

    private fun fileTypeBox(major: String, vararg compatible: String): ByteArray {
        val bytes = ByteBuffer.allocate(16 + compatible.size * 4)
        bytes.putInt(bytes.capacity()).put("ftyp".toByteArray(Charsets.US_ASCII))
            .put(major.toByteArray(Charsets.US_ASCII)).putInt(0)
        compatible.forEach { bytes.put(it.toByteArray(Charsets.US_ASCII)) }
        return bytes.array()
    }

    @Test
    fun heifAndAvifBrandsUseTheirOriginalContainerMime() {
        assertEquals("image/heic", sniffBmffImageMime(fileTypeBox("heic", "mif1")))
        assertEquals("image/heic", sniffBmffImageMime(fileTypeBox("mif1", "heix")))
        assertEquals("image/heif", sniffBmffImageMime(fileTypeBox("mif1")))
        assertEquals("image/avif", sniffBmffImageMime(fileTypeBox("mif1", "avif")))
        assertEquals("image/avif", sniffBmffImageMime(fileTypeBox("avis", "msf1")))
        assertEquals("image/heic-sequence", sniffBmffImageMime(fileTypeBox("hevc", "msf1")))
    }

    @Test
    fun brandsMustBeInsideACompleteFileTypeBox() {
        val avif = fileTypeBox("avif", "mif1")
        assertNull(sniffBmffImageMime(avif.copyOf(avif.size - 1)))
        assertNull(sniffBmffImageMime(fileTypeBox("mp42") + "avif".toByteArray()))
        assertNull(sniffBmffImageMime("ftypavif is only text".toByteArray()))
        val invalid = avif.copyOf()
        ByteBuffer.wrap(invalid).putInt(Int.MAX_VALUE)
        assertNull(sniffBmffImageMime(invalid))
    }

    @Test
    fun extendedSizeFileTypeBoxIsReadWithoutUsingVersionAsABrand() {
        val box = ByteBuffer.allocate(28)
            .putInt(1).put("ftyp".toByteArray()).putLong(28)
            .put("mif1".toByteArray()).put("avif".toByteArray()).put("heic".toByteArray())
            .array()
        assertEquals("image/heic", sniffBmffImageMime(box))
    }

    @Test
    fun shortReadsAreCopiedWithoutDroppingBytesAtTheLimit() {
        val expected = ByteArray(150_003) { (it % 251).toByte() }
        val input = object : ByteArrayInputStream(expected) {
            override fun read(buffer: ByteArray, offset: Int, length: Int): Int =
                super.read(buffer, offset, minOf(length, 7))
        }
        val output = ByteArrayOutputStream()

        assertEquals(expected.size.toLong(), copyExportStream(input, output, expected.size.toLong()))
        assertArrayEquals(expected, output.toByteArray())
    }

    @Test
    fun unboundedSourceStopsAtBudgetWithoutWritingTheOverflowBlock() {
        var readBytes = 0L
        var writtenBytes = 0L
        val limit = 100_000L
        val input = object : InputStream() {
            override fun read(): Int = error("copy should use its fixed buffer")
            override fun read(buffer: ByteArray, offset: Int, length: Int): Int {
                assertTrue(length <= 64 * 1024)
                readBytes += length
                return length
            }
        }
        val output = object : OutputStream() {
            override fun write(value: Int) = error("copy should write blocks")
            override fun write(buffer: ByteArray, offset: Int, length: Int) {
                writtenBytes += length
            }
        }

        val error = runCatching { copyExportStream(input, output, limit) }.exceptionOrNull()
        assertTrue(error is IllegalArgumentException)
        assertTrue(writtenBytes <= limit)
        assertTrue(readBytes <= limit + 64 * 1024)
    }

    @Test
    fun failedWriteRemovesOnlyItsReservedDestination() {
        val directory = temporaryFolder.newFolder("downloads")
        val existing = directory.resolve("report.txt").apply { writeText("keep original") }
        val failure = IOException("source disconnected")

        val thrown = runCatching {
            writeLegacyExportFile(directory, "report.txt") { output ->
                output.write("partial".toByteArray())
                throw failure
            }
        }.exceptionOrNull()

        assertTrue(thrown === failure)
        assertEquals("keep original", existing.readText())
        assertFalse(directory.resolve("report_1.txt").exists())
        assertEquals(listOf("report.txt"), directory.listFiles()!!.map { it.name })
    }

    @Test
    fun completedWriteKeepsExistingFileAndReturnsActualDestination() {
        val directory = temporaryFolder.newFolder("pictures")
        directory.resolve("image.png").writeText("existing")

        val (saved, size) = writeLegacyExportFile(directory, "image.png") {
            copyExportStream(ByteArrayInputStream(byteArrayOf(1, 2, 3)), it, 3)
        }

        assertEquals("image_1.png", saved.name)
        assertEquals(3L, size)
        assertArrayEquals(byteArrayOf(1, 2, 3), saved.readBytes())
        assertEquals("existing", directory.resolve("image.png").readText())
    }

    @Test
    fun overflowRemovesPartialLegacyFile() {
        val directory = temporaryFolder.newFolder("overflow")
        val thrown = runCatching {
            writeLegacyExportFile(directory, "attachment.bin") {
                copyExportStream(ByteArrayInputStream(ByteArray(70_000)), it, 65_536)
            }
        }.exceptionOrNull()

        assertTrue(thrown is IllegalArgumentException)
        assertEquals(0, directory.listFiles()!!.size)
    }

    @Test
    fun cancellationArrivingWithEofRollsBackTheReservedFile() {
        val directory = temporaryFolder.newFolder("cancelled-at-eof")
        val input = object : ByteArrayInputStream(byteArrayOf(1, 2, 3)) {
            override fun read(buffer: ByteArray, offset: Int, length: Int): Int {
                val count = super.read(buffer, offset, length)
                if (count == -1) Thread.currentThread().interrupt()
                return count
            }
        }
        try {
            val error = runCatching {
                writeLegacyExportFile(directory, "attachment.bin") { copyExportStream(input, it, 3) }
            }.exceptionOrNull()
            assertTrue(error is java.io.InterruptedIOException)
            assertEquals(0, directory.listFiles()!!.size)
        } finally {
            Thread.interrupted()
        }
    }
}
