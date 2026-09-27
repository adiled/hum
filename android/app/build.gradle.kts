plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "hum.daemon"
    compileSdk = 34

    defaultConfig {
        applicationId = "hum.daemon"
        minSdk = 29
        targetSdk = 34
        versionCode = 1
        versionName = "0.32.0"
    }

    // The native humd is a PIE ELF cross-compiled from the Rust workspace
    // (see android/scripts/build-humd.sh) and bundled as a plain asset.
    // The foreground service extracts it to filesDir and execs it — no
    // Termux, no root, no JNI. Keep it out of source control.
    sourceSets["main"].assets.srcDirs("src/main/assets")

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    implementation("androidx.core:core:1.13.1")
    implementation("androidx.lifecycle:lifecycle-service:2.8.4")
}
