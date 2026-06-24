plugins {
    id("com.android.application") version "8.5.2"
    id("org.jetbrains.kotlin.android") version "1.9.24"
}

android {
    namespace = "com.qorvex.testapp"
    compileSdk = 35

    defaultConfig {
        // Android package name == the iOS bundle id (com.qorvex.testapp), so the
        // same `set-target com.qorvex.testapp` selector works on both platforms.
        applicationId = "com.qorvex.testapp"
        minSdk = 24
        targetSdk = 35
        versionCode = 1
        versionName = "1.0"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    buildTypes {
        // Debug-only app, no signing config required (parity with the iOS
        // testapp's CODE_SIGNING_ALLOWED=NO).
        release {
            isMinifyEnabled = false
        }
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.appcompat:appcompat:1.7.0")
    implementation("com.google.android.material:material:1.12.0")

    // Pure-JVM unit test of the element-id inventory (no emulator needed).
    testImplementation("junit:junit:4.13.2")
}
