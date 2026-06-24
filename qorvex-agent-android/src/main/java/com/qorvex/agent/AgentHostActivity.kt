// AgentHostActivity.kt
// Minimal host activity for the Qorvex Android agent. The actual agent runs in
// the instrumentation (androidTest) APK via UiAutomator; this activity exists
// only so the agent has a host application package to instrument, mirroring the
// Swift agent's host app target.

package com.qorvex.agent

import android.app.Activity
import android.os.Bundle
import android.widget.TextView

class AgentHostActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(TextView(this).apply { text = "qorvex-agent" })
    }
}
