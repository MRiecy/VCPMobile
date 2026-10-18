package com.vcp.mobile.cli

import android.os.Process as AndroidProcess
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import java.io.File
import java.util.concurrent.TimeUnit

// Android process identity and signaling only. Registration, output drain and terminal
// publication remain in CliProcessHost, including serialization of cancellation.

private const val GROUP_KILL_WAIT_MS = 2_000L
private const val PROOT_TRACEES_QUIT_GRACE_MS = 1_000L

internal data class ProcIdentity(
    val pid: Int,
    val pgid: Int,
    val sessionId: Int,
    val startTimeTicks: Long,
    val uid: Int,
)

internal data class GroupTermination(
    val termSent: Boolean,
    val killSent: Boolean,
    val groupGone: Boolean,
)

internal fun parseProcStat(stat: String, uid: Int): ProcIdentity {
    val openParen = stat.indexOf('(')
    val closeParen = stat.lastIndexOf(')')
    require(openParen > 0 && closeParen > openParen) { "malformed /proc stat comm field" }
    val pid = stat.substring(0, openParen).trim().toInt()
    val fields = stat.substring(closeParen + 1).trim().split(Regex("\\s+"))
    require(fields.size > 19) { "incomplete /proc stat" }
    return ProcIdentity(
        pid = pid,
        pgid = fields[2].toInt(),
        sessionId = fields[3].toInt(),
        startTimeTicks = fields[19].toLong(),
        uid = uid,
    )
}

internal object OwnedProcessGroup {
    fun readIdentity(pid: Int): ProcIdentity? = try {
        val procDirectory = File("/proc/$pid")
        val uid = Os.stat(procDirectory.absolutePath).st_uid
        val stat = File(procDirectory, "stat").readText(Charsets.US_ASCII)
        parseProcStat(stat, uid)
    } catch (_: Exception) {
        null
    }

    private fun groupHasOnlyVisibleAppUidMembers(pgid: Int): Boolean {
        var found = false
        val entries = File("/proc").listFiles() ?: return false
        for (entry in entries) {
            val pid = entry.name.toIntOrNull() ?: continue
            val identity = readIdentity(pid) ?: continue
            if (identity.pgid == pgid) {
                if (identity.uid != AndroidProcess.myUid()) return false
                found = true
            }
        }
        return found
    }

    private fun signalGroup(pgid: Int, signal: Int): Boolean = try {
        Os.kill(-pgid, signal)
        true
    } catch (error: ErrnoException) {
        if (error.errno == OsConstants.ESRCH) false else throw error
    }

    fun isAlive(pgid: Int): Boolean = try {
        Os.kill(-pgid, 0)
        true
    } catch (error: ErrnoException) {
        when (error.errno) {
            OsConstants.ESRCH -> false
            OsConstants.EPERM -> true
            else -> throw error
        }
    }

    private fun signalOwnedLeader(identity: ProcIdentity, signal: Int): Boolean {
        val current = readIdentity(identity.pid) ?: return false
        require(current == identity && current.uid == AndroidProcess.myUid()) {
            "CLI process identity changed before signaling its PRoot leader"
        }
        return try {
            Os.kill(current.pid, signal)
            true
        } catch (error: ErrnoException) {
            if (error.errno == OsConstants.ESRCH) false else throw error
        }
    }

    private fun waitForGroupGone(pgid: Int, waitMs: Long): Boolean {
        val deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(waitMs)
        do {
            if (!isAlive(pgid)) return true
            if (System.nanoTime() >= deadline) return false
            Thread.sleep(20)
        } while (true)
    }

    fun terminate(identity: ProcIdentity, graceMs: Long): GroupTermination {
        if (!isAlive(identity.pgid)) {
            return GroupTermination(termSent = false, killSent = false, groupGone = true)
        }
        require(canSignalOwnedIdentity(identity)) { "CLI process identity no longer matches its owned process group" }
        val termSent = signalGroup(identity.pgid, OsConstants.SIGTERM)
        if (waitForGroupGone(identity.pgid, graceMs)) {
            return GroupTermination(termSent = termSent, killSent = false, groupGone = true)
        }

        // PRoot handles SIGQUIT by stopping its remaining tracees. Give that
        // --kill-on-exit path a bounded chance to reap guest processes that
        // created a new session before the outer process group is force-killed.
        if (signalOwnedLeader(identity, OsConstants.SIGQUIT) &&
            waitForGroupGone(identity.pgid, PROOT_TRACEES_QUIT_GRACE_MS)
        ) {
            return GroupTermination(termSent = termSent, killSent = false, groupGone = true)
        }
        require(canSignalOwnedIdentity(identity)) { "CLI process group ownership changed before SIGKILL" }
        val killSent = signalGroup(identity.pgid, OsConstants.SIGKILL)
        return GroupTermination(
            termSent = termSent,
            killSent = killSent,
            groupGone = waitForGroupGone(identity.pgid, GROUP_KILL_WAIT_MS),
        )
    }

    private fun canSignalOwnedIdentity(identity: ProcIdentity): Boolean {
        val current = readIdentity(identity.pid)
        if (current != null) {
            return current.pid == identity.pid &&
                current.pgid == identity.pgid &&
                current.sessionId == identity.sessionId &&
                current.startTimeTicks == identity.startTimeTicks &&
                current.uid == identity.uid &&
                current.uid == AndroidProcess.myUid()
        }
        return groupHasOnlyVisibleAppUidMembers(identity.pgid)
    }

    fun abort(identity: ProcIdentity) {
        if (isAlive(identity.pgid) && canSignalOwnedIdentity(identity)) {
            signalGroup(identity.pgid, OsConstants.SIGKILL)
            waitForGroupGone(identity.pgid, GROUP_KILL_WAIT_MS)
        }
    }
}
