package com.vcp.mobile

import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.TimeUnit

class PluginExecutorDomainsTest {
    @Test
    fun defaultFileDomainPreservesSerialExecution() {
        val domains = PluginExecutorDomains()
        val releaseFirst = CountDownLatch(1)
        val firstStarted = CountDownLatch(1)
        val secondStarted = CountDownLatch(1)

        try {
            domains.fileIoExecutor.execute {
                firstStarted.countDown()
                releaseFirst.await()
            }
            domains.fileIoExecutor.execute { secondStarted.countDown() }

            assertTrue(firstStarted.await(1, TimeUnit.SECONDS))
            assertTrue(!secondStarted.await(100, TimeUnit.MILLISECONDS))
            releaseFirst.countDown()
            assertTrue(secondStarted.await(1, TimeUnit.SECONDS))
        } finally {
            releaseFirst.countDown()
            domains.shutdownNow()
        }
    }

    @Test
    fun blockedOomGuardAndRootCommandDoNotStarveFileIo() {
        val domains = PluginExecutorDomains(fileThreadCount = 1, rootQueueCapacity = 1, fileQueueCapacity = 2)
        val releaseBlockers = CountDownLatch(1)
        val guardStarted = CountDownLatch(1)
        val rootStarted = CountDownLatch(1)
        val fileCompleted = CountDownLatch(1)

        try {
            domains.oomScheduler.execute {
                guardStarted.countDown()
                releaseBlockers.await()
            }
            domains.rootExecutor.execute {
                rootStarted.countDown()
                releaseBlockers.await()
            }
            domains.fileIoExecutor.execute { fileCompleted.countDown() }

            assertTrue(guardStarted.await(1, TimeUnit.SECONDS))
            assertTrue(rootStarted.await(1, TimeUnit.SECONDS))
            assertTrue(fileCompleted.await(1, TimeUnit.SECONDS))
        } finally {
            releaseBlockers.countDown()
            domains.shutdownNow()
        }
    }

    @Test(expected = RejectedExecutionException::class)
    fun boundedFileQueueRejectsInsteadOfGrowingWithoutLimit() {
        val domains = PluginExecutorDomains(fileThreadCount = 1, rootQueueCapacity = 1, fileQueueCapacity = 1)
        val releaseBlocker = CountDownLatch(1)
        val running = CountDownLatch(1)

        try {
            domains.fileIoExecutor.execute {
                running.countDown()
                releaseBlocker.await()
            }
            assertTrue(running.await(1, TimeUnit.SECONDS))
            domains.fileIoExecutor.execute { }
            domains.fileIoExecutor.execute { }
        } finally {
            releaseBlocker.countDown()
            domains.shutdownNow()
        }
    }

    @Test
    fun shutdownNowTerminatesAllExecutorDomains() {
        val domains = PluginExecutorDomains()

        domains.shutdownNow()

        assertTrue(domains.oomScheduler.awaitTermination(1, TimeUnit.SECONDS))
        assertTrue(domains.rootExecutor.awaitTermination(1, TimeUnit.SECONDS))
        assertTrue(domains.fileIoExecutor.awaitTermination(1, TimeUnit.SECONDS))
    }
}
