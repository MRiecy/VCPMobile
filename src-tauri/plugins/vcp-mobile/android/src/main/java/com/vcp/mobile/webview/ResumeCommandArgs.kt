package com.vcp.mobile.webview

import app.tauri.annotation.InvokeArg

@InvokeArg
class ConnectWebViewResumeArgs {
    var documentId: String = ""
}

@InvokeArg
class CompleteWebViewResumeArgs {
    var documentId: String = ""
    var token: String = ""
}
