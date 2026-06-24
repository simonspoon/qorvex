.PHONY: setup android android-agent android-testapp android-test

setup:
	git config core.hooksPath .githooks

# --- Android (Kotlin agent + sample app) ---
# Requires a JDK 17+ and an Android SDK (platform 35 + build-tools) reachable via
# the ANDROID_HOME / ANDROID_SDK_ROOT env var, or a local.properties sdk.dir in
# each module. Gradle (8.10.2) is provided by the checked-in wrapper.

# Build every Android artifact: the agent host + instrumentation APKs and the
# sample-app APK.
android: android-agent android-testapp

# Kotlin UiAutomator agent: host APK + instrumentation (androidTest) APK.
android-agent:
	cd qorvex-agent-android && ./gradlew assembleDebug assembleDebugAndroidTest

# Android sample / verification app APK.
android-testapp:
	cd qorvex-testapp-android && ./gradlew assembleDebug

# Pure-JVM unit tests for both modules (no emulator needed).
android-test:
	cd qorvex-agent-android && ./gradlew testDebugUnitTest
	cd qorvex-testapp-android && ./gradlew testDebugUnitTest
