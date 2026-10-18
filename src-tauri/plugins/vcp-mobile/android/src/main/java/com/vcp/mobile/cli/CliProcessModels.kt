package com.vcp.mobile.cli

import app.tauri.annotation.InvokeArg

// Native bridge arguments. Keep field names and package identity stable for Invoke parsing.

@InvokeArg
class PrepareCliRuntimeArgs {
    lateinit var operationId: String
    lateinit var profileId: String
    var runtimeGeneration: Long = 0
    var rootfsArchiveBytes: Long = 0
    lateinit var rootfsArchiveSha256: String
    var prootBytes: Long = 0
    lateinit var prootSha256: String
    var prootLoaderBytes: Long = 0
    lateinit var prootLoaderSha256: String
}

@InvokeArg
class StartCliProcessArgs {
    lateinit var operationId: String
    lateinit var jobId: String
    lateinit var attemptId: String
    var runtimeGeneration: Long = 0
    lateinit var command: String
    lateinit var rootfsPath: String
    lateinit var cwd: String
    var artifactMaxBytes: Long = 0
    var backgroundLease: Boolean = false
    var timeoutMs: Long = 0
    lateinit var displayLabel: String
    var sessionId: String? = null
}

@InvokeArg
class InspectCliProcessArgs {
    lateinit var operationId: String
    lateinit var jobId: String
    lateinit var attemptId: String
    var runtimeGeneration: Long = 0
}

@InvokeArg
class CancelCliProcessArgs {
    lateinit var operationId: String
    lateinit var jobId: String
    lateinit var attemptId: String
    var runtimeGeneration: Long = 0
    var graceMs: Long = 0
}
