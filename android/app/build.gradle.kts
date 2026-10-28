plugins {
    id("com.android.application")
}

android {
    namespace = "com.kurogane.engine"
    compileSdk = 34

    defaultConfig {
        applicationId = "com.kurogane.engine"
        minSdk = 26      // Android 8.0 — the project's floor (AAudio availability)
        targetSdk = 34
        versionCode = 6
        versionName = "0.6.0"
        ndk {
            // ABI filter matches the .so set produced by cargo-ndk.
            abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64", "x86")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            isShrinkResources = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    // jniLibs land in src/main/jniLibs via cargo-ndk's -o flag.
    sourceSets {
        getByName("main") {
            jniLibs.srcDir("src/main/jniLibs")
        }
    }
}

dependencies {
    // Intentionally dependency-free: plain Activity + Canvas-drawn controller.
    // The GLES 3.0 renderer milestone brings GameActivity.
}
