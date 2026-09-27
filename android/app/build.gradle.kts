plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "se.mach25.offload"
    compileSdk = 35

    defaultConfig {
        applicationId = "se.mach25.offload"
        minSdk = 29
        targetSdk = 35
        // A new code every build (minutes since 2026-01-01), so a device can tell one walk build from
        // the next. It did not by itself refresh the tablet taskbar's cached icon — restarting the
        // launcher did (docs/DEMO.md) — but a constant 1 gives every cache the excuse not to.
        versionCode = ((System.currentTimeMillis() - 1_767_225_600_000L) / 60_000L).toInt()
        versionName = "0.0.1"
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }

    // ADR-0066 §1: offloadd and offload ship as lib*.so so Android extracts them to
    // nativeLibraryDir — the one app-owned place it still lets an app execute a file.
    packaging { jniLibs { useLegacyPackaging = true } }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin { compilerOptions { jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17) } }
