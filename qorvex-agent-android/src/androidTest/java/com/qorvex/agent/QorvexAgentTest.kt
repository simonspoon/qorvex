// QorvexAgentTest.kt
// Instrumentation (UiAutomator) entry point for the Qorvex Android agent.
//
// This is the Android analog of the Swift `testRunAgent`. Launched via:
//   adb shell am instrument -w -e qorvex_port <port> \
//     -e class com.qorvex.agent.QorvexAgentTest#runAgent \
//     com.qorvex.agent.test/androidx.test.runner.AndroidJUnitRunner
//
// It opens a blocking TCP server on the device-side port and serves the Qorvex
// binary protocol until the instrumentation process is killed. The `-w` flag
// keeps the host adb call attached so the lifecycle (story #88) owns the handle.
// The agent operates on whatever app is foregrounded; SetTarget switches focus.

package com.qorvex.agent

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class QorvexAgentTest {

    @Test
    fun runAgent() {
        val args = InstrumentationRegistry.getArguments()
        val port = args.getString("qorvex_port")?.toIntOrNull() ?: DEFAULT_PORT

        val handler = CommandHandler()
        val server = AgentServer(port, handler)

        android.util.Log.i("qorvex-agent", "Agent starting, serving on port $port")
        // Blocks indefinitely until the instrumentation process is killed.
        server.serveForever()
    }

    companion object {
        private const val DEFAULT_PORT = 8080
    }
}
