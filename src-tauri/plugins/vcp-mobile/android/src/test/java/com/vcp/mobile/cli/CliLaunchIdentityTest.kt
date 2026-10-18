package com.vcp.mobile.cli

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class CliLaunchIdentityTest {
    private fun args(session: String?) = StartCliProcessArgs().apply {
        operationId = "request"
        jobId = "job"
        attemptId = "attempt"
        runtimeGeneration = 1
        command = "printenv VCP_CLI_SESSION_ID"
        rootfsPath = "/app/rootfs/base"
        cwd = "/workspace"
        artifactMaxBytes = 1024
        backgroundLease = true
        timeoutMs = 30_000
        displayLabel = "session"
        sessionId = session
    }

    @Test fun sessionEnvironmentParticipatesInIdempotentLaunchIdentity() {
        val first = args("session-a")
        val retry = args("session-a").apply { operationId = "retry" }
        val differentSession = args("session-b")
        assertTrue(buildProotArguments("/proot", first.rootfsPath, "/workspace", first.cwd, first.command, first.sessionId)
            .contains("VCP_CLI_SESSION_ID=session-a"))
        assertEquals(fingerprint(first), fingerprint(retry))
        assertNotEquals(fingerprint(first), fingerprint(differentSession))
        assertNotEquals(fingerprint(first), fingerprint(args(null)))
    }
}
