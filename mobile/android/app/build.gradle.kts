import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "io.github.ilovebinglu.p2pcli"
    compileSdk = 37

    defaultConfig {
        // 开发期间和 Tauri 版的手机端分开装，两个能同时在手机上对比；原生版稳定后换回正式的包名
        applicationId = "io.github.ilovebinglu.p2pcli.native"
        minSdk = 24
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.11"
        // Rust 库只编了 64 位 ARM，现在的手机都是
        ndk { abiFilters += "arm64-v8a" }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            // 测试包用本机的调试密钥签名，能直接装；上架前换成正式密钥
            signingConfig = signingConfigs.getByName("debug")
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
    // 终端模拟（Apache 2.0，见 termux-app 的 LICENSE.md）：解析终端输出、画字；连接、键盘是自己写的
    implementation("com.github.termux.termux-app:terminal-view:v0.118.3")
    implementation("com.github.termux.termux-app:terminal-emulator:v0.118.3")
}
