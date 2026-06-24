plugins {
    id("com.android.application") version "8.5.2"
    id("org.jetbrains.kotlin.android") version "1.9.24"
}

android {
    namespace = "com.qorvex.agent"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.qorvex.agent"
        minSdk = 24
        targetSdk = 35
        versionCode = 1
        versionName = "1.0"

        // The instrumentation runner that hosts the long-lived agent entry point.
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    // No code signing required for an instrumentation agent APK.
    packaging {
        resources.excludes += setOf("META-INF/LICENSE*", "META-INF/AL2.0", "META-INF/LGPL2.1")
    }
}

dependencies {
    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("androidx.test:rules:1.6.1")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
    androidTestImplementation("androidx.test.uiautomator:uiautomator:2.3.0")

    // JVM-side unit tests for the pure protocol/serializer codec (host build).
    testImplementation("junit:junit:4.13.2")
}
