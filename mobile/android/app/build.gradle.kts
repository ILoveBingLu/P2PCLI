import org.jetbrains.kotlin.gradle.dsl.JvmTarget

// 版本号和桌面端同一个：app/src-tauri/tauri.conf.json。versionCode 按 主*1000000 + 次*1000 + 修订 算，只增不减
val appVersion = (groovy.json.JsonSlurper().parse(rootProject.file("../../app/src-tauri/tauri.conf.json")) as Map<*, *>)["version"] as String
val appVersionCode = appVersion.substringBefore('-').split('.').map { it.toInt() }.let { (a, b, c) -> a * 1_000_000 + b * 1_000 + c }

// 正式签名：发布工作流从仓库 Secrets 里取出密钥，经环境变量传进来。没有就用本机的调试密钥（本地打的测试包）
val releaseKeystore: String? = System.getenv("P2PCLI_ANDROID_KEYSTORE")?.takeIf { it.isNotBlank() }

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "io.github.ilovebinglu.p2pcli"
    compileSdk = 37

    defaultConfig {
        // 和 iOS 的包名一样。发布以后不能再改：改了就是另一个 App，装不上旧版的更新
        applicationId = "io.github.ilovebinglu.p2pcli"
        minSdk = 24
        targetSdk = 36
        versionCode = appVersionCode
        versionName = appVersion
        // Rust 库只编了 64 位 ARM，现在的手机都是
        ndk { abiFilters += "arm64-v8a" }
    }

    signingConfigs {
        if (releaseKeystore != null) {
            create("release") {
                storeFile = file(releaseKeystore)
                storePassword = System.getenv("P2PCLI_ANDROID_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("P2PCLI_ANDROID_KEY_ALIAS")
                keyPassword = System.getenv("P2PCLI_ANDROID_KEY_PASSWORD")
            }
        }
    }
    buildTypes {
        release {
            isMinifyEnabled = false
            // 正式密钥签的包和调试密钥签的包互相不能覆盖安装：换的时候先卸载
            signingConfig = signingConfigs.getByName(if (releaseKeystore != null) "release" else "debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures { compose = true }

    // Rust 库和 UniFFI 生成的 Kotlin 绑定，由 build-rust.sh 生成在这里
    sourceSets["main"].java.srcDir("build/rust/uniffi")
    sourceSets["main"].jniLibs.srcDir("build/rust/jniLibs")
}

kotlin {
    compilerOptions { jvmTarget.set(JvmTarget.JVM_17) }
}

// 界面文字和桌面端用同一份翻译：打包时把桌面的 en.json 拷进 assets/i18n
val copyI18n = tasks.register<Copy>("copyI18n") {
    from(rootProject.file("../../app/src/i18n/locales")) { include("en.json") }
    into(layout.buildDirectory.dir("generated/i18n/i18n"))
}
android.sourceSets["main"].assets.srcDir("build/generated/i18n")
tasks.named("preBuild") { dependsOn(copyI18n) }

// 打包前先编 Rust 库、生成绑定
val buildRust = tasks.register<Exec>("buildRust") {
    description = "编 crates/mobile-ffi 给 Android，生成 Kotlin 绑定"
    workingDir = rootProject.projectDir
    commandLine("sh", "build-rust.sh")
}
tasks.named("preBuild") { dependsOn(buildRust) }

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2025.12.01")
    implementation(composeBom)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-core")
    implementation("androidx.activity:activity-compose:1.10.1")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.10.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    // UniFFI 生成的绑定通过 JNA 调 Rust 库
    implementation("net.java.dev.jna:jna:5.17.0@aar")
    // 扫码：CameraX 取景，ML Kit 认二维码。用自带识别模型的版本，不依赖谷歌服务（鸿蒙这类手机也能用）
    implementation("androidx.camera:camera-camera2:1.5.1")
    implementation("androidx.camera:camera-lifecycle:1.5.1")
    implementation("androidx.camera:camera-view:1.5.1")
    implementation("com.google.mlkit:barcode-scanning:17.3.0")
    // 终端模拟（Apache 2.0，见 termux-app 的 LICENSE.md）：解析终端输出、画字；连接、键盘是自己写的
    implementation("com.github.termux.termux-app:terminal-view:v0.118.3")
    implementation("com.github.termux.termux-app:terminal-emulator:v0.118.3")
}
