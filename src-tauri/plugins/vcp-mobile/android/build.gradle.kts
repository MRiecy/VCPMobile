plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.vcp.mobile"
    compileSdk = 36
    ndkVersion = "29.0.13846066"

    defaultConfig {
        minSdk = 26
        ndk {
            abiFilters += "arm64-v8a"
        }
        externalNativeBuild {
            ndkBuild {
                cppFlags += listOf("-std=c++17", "-Wall", "-Wextra", "-Werror")
            }
        }
    }

    externalNativeBuild {
        ndkBuild {
            path = file("src/main/cpp/Android.mk")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }

    kotlinOptions {
        jvmTarget = "1.8"
    }

    sourceSets {
        getByName("main") {
            jniLibs.srcDirs("src/main/jniLibs")
            assets.srcDirs("../../../runtime-assets/vcp-cli/android-assets")
        }
    }

    testOptions {
        unitTests.isIncludeAndroidResources = true
        unitTests.all {
            it.systemProperty(
                "robolectric.dependency.repo.url",
                "https://repo.maven.apache.org/maven2",
            )
        }
    }
}

dependencies {
    implementation(project(":tauri-android"))
    implementation("androidx.appcompat:appcompat:1.6.1")
    implementation("androidx.core:core-ktx:1.12.0")
    implementation("androidx.lifecycle:lifecycle-common:2.7.0")
    implementation("androidx.lifecycle:lifecycle-process:2.7.0")
    implementation("com.github.topjohnwu.libsu:core:5.0.3")
    implementation("androidx.media3:media3-transformer:1.3.1")
    implementation("androidx.media3:media3-effect:1.3.1")
    implementation("androidx.media3:media3-common:1.3.1")
    
    // 原生轻量级推送监听器所需的 SSE 长连接支持
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
    implementation("com.squareup.okhttp3:okhttp-sse:4.12.0")

    testImplementation("junit:junit:4.13.2")
    testImplementation("com.fasterxml.jackson.core:jackson-databind:2.15.3")
    testImplementation("org.robolectric:robolectric:4.13")
    // Robolectric 默认会在测试运行期动态拉取 android-all；显式声明可让 Gradle 预解析并缓存，避免运行期 socket 失败。
    testImplementation("org.robolectric:android-all:9-robolectric-4913185-2")
    testImplementation("io.mockk:mockk:1.13.12")
    testImplementation("androidx.test:core:1.6.1")
    testImplementation("androidx.test.ext:junit:1.2.1")

    androidTestImplementation("androidx.test.ext:junit:1.2.1")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.6.1")
    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("androidx.test:rules:1.6.1")
    androidTestImplementation("io.mockk:mockk-android:1.13.12")
}
