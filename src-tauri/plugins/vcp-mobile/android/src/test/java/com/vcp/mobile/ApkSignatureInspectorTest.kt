package com.vcp.mobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ApkSignatureInspectorTest {

    @Test
    fun `sha256Hex produces lowercase hex digest`() {
        // SHA-256("") 的公开测试向量
        assertEquals(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ApkSignatureInspector.sha256Hex(ByteArray(0)),
        )
        // SHA-256("abc") 的公开测试向量
        assertEquals(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ApkSignatureInspector.sha256Hex("abc".toByteArray()),
        )
    }

    @Test
    fun `digestsMatch compares case-insensitively and rejects empty`() {
        val digest = "a".repeat(64)
        assertTrue(ApkSignatureInspector.digestsMatch(digest, digest))
        assertTrue(ApkSignatureInspector.digestsMatch(digest.uppercase(), digest))
        assertFalse(ApkSignatureInspector.digestsMatch(digest, "b".repeat(64)))
        assertFalse(ApkSignatureInspector.digestsMatch(null, digest))
        assertFalse(ApkSignatureInspector.digestsMatch(digest, null))
        assertFalse(ApkSignatureInspector.digestsMatch("", digest))
    }
}
