import java.io.File
import java.util.Properties
import org.gradle.api.GradleException
 
plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}
 
val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

val releaseKeystorePath = System.getenv("ANDROID_KEYSTORE_PATH")
val releaseKeyAlias = System.getenv("ANDROID_KEY_ALIAS")
val releaseKeystorePassword = System.getenv("ANDROID_KEYSTORE_PASSWORD")
val releaseKeyPassword = System.getenv("ANDROID_KEY_PASSWORD")
val resolvedReleaseKeystoreFile = releaseKeystorePath?.let {
    val candidate = File(it)
    if (candidate.isAbsolute) {
        candidate
    } else {
        rootProject.projectDir.resolve(it).normalize()
    }
}
val hasReleaseSigning = resolvedReleaseKeystoreFile?.exists() == true
    && !releaseKeyAlias.isNullOrBlank()
    && !releaseKeystorePassword.isNullOrBlank()
    && !releaseKeyPassword.isNullOrBlank()

val trustedLanMode = (
    providers.gradleProperty("vcp.trustedLanMode").orNull
        ?: System.getenv("VCP_TRUSTED_LAN_MODE")
        ?: "enabled"
).trim().lowercase()
if (trustedLanMode !in setOf("enabled", "disabled")) {
    throw GradleException("VCP trusted LAN mode must be exactly 'enabled' or 'disabled'")
}
val releaseTrustedLanCleartext = trustedLanMode == "enabled"
 
android {
    compileSdk = 36
    namespace = "com.vcp.avatar"

    signingConfigs {
        if (hasReleaseSigning) {
            create("release") {
                storeFile = resolvedReleaseKeystoreFile
                storePassword = releaseKeystorePassword
                keyAlias = releaseKeyAlias
                keyPassword = releaseKeyPassword
            }
        }
    }

    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        manifestPlaceholders["appName"] = "VCPMobile"
        applicationId = "com.vcp.avatar"
        minSdk = 26
        targetSdk = 36
        versionCode = tauriProperties.getProperty("tauri.android.versionCode", "1").toInt()
        versionName = tauriProperties.getProperty("tauri.android.versionName", "1.0.0")
    }
    buildTypes {
        getByName("debug") {
            applicationIdSuffix = ".debug"
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            manifestPlaceholders["appName"] = "VCP-Debug"
            versionNameSuffix = "-debug"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            if (hasReleaseSigning) {
                signingConfig = signingConfigs.getByName("release")
            }
            manifestPlaceholders["usesCleartextTraffic"] = releaseTrustedLanCleartext.toString()
            isMinifyEnabled = false
            proguardFiles(
                *fileTree(".") { include("**/*.pro") }
                    .plus(getDefaultProguardFile("proguard-android-optimize.txt"))
                    .toList().toTypedArray()
            )
        }
    }
    kotlinOptions {
        jvmTarget = "1.8"
    }
    buildFeatures {
        buildConfig = true
    }
    packaging {
        jniLibs {
            useLegacyPackaging = true
        }
    }
}

// Debug builds remain frictionless. Any task that actually produces/tests a release variant
// must receive the four explicit signing inputs; an unsigned or debug-signed release is invalid.
tasks.configureEach {
    if (name.contains("release", ignoreCase = true)) {
        doFirst {
            if (!hasReleaseSigning) {
                throw GradleException(
                    "Release signing is incomplete. Set ANDROID_KEYSTORE_PATH, " +
                        "ANDROID_KEY_ALIAS, ANDROID_KEYSTORE_PASSWORD and ANDROID_KEY_PASSWORD."
                )
            }
        }
    }
}

rust {
    rootDirRel = "../../../"
}

dependencies {
    implementation("androidx.core:core-splashscreen:1.2.0")
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = "tauri.build.gradle.kts")
