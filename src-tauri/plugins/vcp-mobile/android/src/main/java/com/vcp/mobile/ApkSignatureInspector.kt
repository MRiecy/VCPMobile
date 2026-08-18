package com.vcp.mobile

import android.content.Context
import android.content.pm.PackageManager
import android.content.pm.Signature
import android.os.Build
import java.security.MessageDigest

/**
 * APK 签名检查器：提取未安装 APK 与当前应用的签名证书 SHA-256，
 * 用于 OTA 安装前的证书连续性校验（防签名不一致导致的安装失败/替换攻击）。
 */
object ApkSignatureInspector {

    /** 字节数组 → 小写十六进制 SHA-256。纯函数，供 JVM 单测。 */
    fun sha256Hex(bytes: ByteArray): String {
        val digest = MessageDigest.getInstance("SHA-256").digest(bytes)
        return digest.joinToString("") { "%02x".format(it) }
    }

    /** 证书摘要一致性比对。纯函数，供 JVM 单测。 */
    fun digestsMatch(apkSha256: String?, selfSha256: String?): Boolean {
        return !apkSha256.isNullOrEmpty() && apkSha256.equals(selfSha256, ignoreCase = true)
    }

    /** 提取指定 APK 文件的首位签名者证书 SHA-256；文件非法或未签名返回 null。 */
    fun apkSignatureSha256(context: Context, apkPath: String): String? {
        val pm = context.packageManager
        val info = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            pm.getPackageArchiveInfo(
                apkPath,
                PackageManager.PackageInfoFlags.of(PackageManager.GET_SIGNING_CERTIFICATES.toLong()),
            )
        } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            @Suppress("DEPRECATION")
            pm.getPackageArchiveInfo(apkPath, PackageManager.GET_SIGNING_CERTIFICATES)
        } else {
            @Suppress("DEPRECATION")
            pm.getPackageArchiveInfo(apkPath, PackageManager.GET_SIGNATURES)
        } ?: return null

        return firstSigner(info)?.let { sha256Hex(it.toByteArray()) }
    }

    /** 提取当前应用自身签名证书的 SHA-256。 */
    fun selfSignatureSha256(context: Context): String? {
        val pm = context.packageManager
        val info = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            pm.getPackageInfo(
                context.packageName,
                PackageManager.PackageInfoFlags.of(PackageManager.GET_SIGNING_CERTIFICATES.toLong()),
            )
        } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            @Suppress("DEPRECATION")
            pm.getPackageInfo(context.packageName, PackageManager.GET_SIGNING_CERTIFICATES)
        } else {
            @Suppress("DEPRECATION")
            pm.getPackageInfo(context.packageName, PackageManager.GET_SIGNATURES)
        }

        return firstSigner(info)?.let { sha256Hex(it.toByteArray()) }
    }

    private fun firstSigner(info: android.content.pm.PackageInfo?): Signature? {
        if (info == null) return null
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            val signingInfo = info.signingInfo ?: return null
            // hasMultipleSigners 时取全体首位即可——比对场景下双方都是单签名发布包；
            // 多签名包在这里保守地取第一个内容签名者。
            @Suppress("DEPRECATION")
            signingInfo.apkContentsSigners?.firstOrNull()
        } else {
            @Suppress("DEPRECATION")
            info.signatures?.firstOrNull()
        }
    }
}
