package com.vcp.mobile.cli

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.Closeable
import java.io.File
import java.io.PipedInputStream
import java.io.PipedOutputStream
import java.nio.file.Files
import java.util.concurrent.atomic.AtomicReference

class CliArtifactOutputTest {
    @Test
    fun boundedPipesDrainToEofAfterTheSharedArtifactBudgetIsExhausted() {
        val root = Files.createTempDirectory("vcp-cli-drain-test").toFile()
        val stdout = File(root, "stdout")
        val stderr = File(root, "stderr")
        val budget = ArtifactBudget(7, stdout, stderr)
        val stdoutInput = PipedInputStream(1024)
        val stderrInput = PipedInputStream(1024)
        val stdoutPipe = PipedOutputStream(stdoutInput)
        val stderrPipe = PipedOutputStream(stderrInput)
        val failure = AtomicReference<Throwable?>(null)
        val readers = listOf(
            budget.startDrainThread("test-cli-stdout", stdoutInput, ArtifactStream.STDOUT),
            budget.startDrainThread("test-cli-stderr", stderrInput, ArtifactStream.STDERR),
        )
        val writers = listOf(stdoutPipe, stderrPipe).mapIndexed { index, pipe ->
            Thread({
                try {
                    pipe.use { it.write(ByteArray(64 * 1024) { (index + 1).toByte() }) }
                } catch (error: Throwable) {
                    failure.compareAndSet(null, error)
                }
            }, "test-cli-producer-$index").apply { isDaemon = true }
        }
        try {
            writers.forEach { it.start() }
            writers.forEach { it.join(5_000) }
            readers.forEach { it.join(5_000) }

            assertFalse("A full artifact budget must not block a child pipe", writers.any { it.isAlive })
            assertFalse("Readers must reach EOF before Host terminal publication", readers.any { it.isAlive })
            assertNull(failure.get())
            assertEquals(7L, budget.stdoutBytes.get() + budget.stderrBytes.get())
            assertEquals(7L, stdout.length() + stderr.length())
            assertTrue(budget.stdoutTruncated.get())
            assertTrue(budget.stderrTruncated.get())
        } finally {
            listOf<Closeable>(stdoutPipe, stderrPipe, stdoutInput, stderrInput).forEach { runCatching { it.close() } }
            writers.forEach { it.join(1_000) }
            readers.forEach { it.join(1_000) }
            budget.closeAll()
            root.deleteRecursively()
        }
    }
}
