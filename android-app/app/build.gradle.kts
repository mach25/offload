plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "se.mach25.offload.app"
    compileSdk = 35

    defaultConfig {
        // Not the harness's id (se.mach25.offload): the two install side by side (ADR-0071 §1).
        applicationId = "se.mach25.offload.app"
        minSdk = 29
        targetSdk = 35
        versionCode = ((System.currentTimeMillis() - 1_767_225_600_000L) / 60_000L).toInt()
        versionName = "0.1.0"
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }

    // The daemon and the CLI ship as lib*.so so Android extracts them where an app may execute a
    // file (ADR-0066 §1); liboffload_mobile.so is the client the UI calls (ADR-0071 §2).
    packaging { jniLibs { useLegacyPackaging = true } }

    // The Kotlin UniFFI generated from the built library, by scripts/build-android-product.sh.
    sourceSets["main"].java.srcDir("src/generated/uniffi")

    buildFeatures { compose = true }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin { compilerOptions { jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17) } }

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2025.05.00")
    implementation(composeBom)
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-core")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    debugImplementation("androidx.compose.ui:ui-tooling")
    implementation("androidx.activity:activity-compose:1.10.1")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.9.0")
    implementation("androidx.core:core-ktx:1.16.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    // UniFFI's Kotlin calls the Rust library through JNA.
    implementation("net.java.dev.jna:jna:5.17.0@aar")
}
