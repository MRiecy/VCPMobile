package com.vcp.mobile.media

import app.tauri.annotation.InvokeArg

@InvokeArg
class MediaOperationArgs {
    lateinit var operationId: String
}

@InvokeArg
class ProcessVideoArgs {
    lateinit var operationId: String
    lateinit var path: String
    lateinit var outputDir: String
    var maxBytes: Long = 0
}

@InvokeArg
class ProcessAudioArgs {
    lateinit var operationId: String
    lateinit var path: String
    lateinit var outputDir: String
    var maxBytes: Long = 0
}
