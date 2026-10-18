package com.vcp.mobile.files

import android.content.Context
import java.io.File

/** Physical files exposed to native file operations must remain in app-owned storage. */
internal fun isSafeLocalPath(context: Context, path: String): Boolean {
    return try {
        val file = File(path).canonicalFile
        val cacheDir = context.cacheDir.canonicalFile
        val filesDir = context.filesDir.canonicalFile
        val externalFilesDir = context.getExternalFilesDir(null)?.canonicalFile
        val externalCacheDir = context.externalCacheDir?.canonicalFile
        file.path.startsWith(cacheDir.path + File.separator) ||
            file.path.startsWith(filesDir.path + File.separator) ||
            (externalFilesDir != null && file.path.startsWith(externalFilesDir.path + File.separator)) ||
            (externalCacheDir != null && file.path.startsWith(externalCacheDir.path + File.separator))
    } catch (_: Exception) {
        false
    }
}
