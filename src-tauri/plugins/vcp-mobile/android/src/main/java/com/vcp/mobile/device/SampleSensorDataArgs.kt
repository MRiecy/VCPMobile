package com.vcp.mobile.device

import app.tauri.annotation.InvokeArg

@InvokeArg
class SampleSensorDataArgs {
    var location: Boolean = false
    var motion: Boolean = false
    var ambient: Boolean = false
}
