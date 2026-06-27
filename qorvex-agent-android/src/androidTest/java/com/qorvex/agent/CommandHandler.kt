// CommandHandler.kt
// Dispatches decoded protocol requests to UiAutomator actions, mirroring the
// Swift CommandHandler. Element resolution walks the live AccessibilityNodeInfo
// tree (via UiAutomator's root) using NodeMapper, applies the ADR-1 mapping for
// dump-tree/find-element, and surfaces actionable errors that distinguish
// element-not-found, timeout, and target-not-running.

package com.qorvex.agent

import android.accessibilityservice.AccessibilityServiceInfo
import android.app.UiAutomation
import android.os.SystemClock
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.UiDevice
import android.view.accessibility.AccessibilityNodeInfo
import android.view.accessibility.AccessibilityWindowInfo
import java.io.ByteArrayOutputStream

class CommandHandler {

    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val uiDevice: UiDevice = UiDevice.getInstance(instrumentation)
    private val uiAutomation: UiAutomation = instrumentation.uiAutomation

    init {
        // The agent serves commands on the instrumentation test thread (no
        // running Looper of its own). A bare `instrumentation.uiAutomation`
        // never enables window-content retrieval, so `rootInActiveWindow`
        // returns null on a real device even when an app is foregrounded
        // (device-side `uiautomator dump` works, proving the a11y pipeline is
        // healthy — this is purely the UiAutomation connection not being
        // configured). [rebindWindow] sets the service info, which forces the
        // accessibility connection to (re)establish and start tracking the
        // active window — and, critically, enables `getWindows()` so [rootNode]'s
        // window-list fallback can recover a stale-bound first connect without a
        // second start-agent.
        rebindWindow()
    }

    /** Currently targeted Android package name (set via SetTarget). */
    @Volatile
    private var targetPackage: String? = null

    private val keyguardManager =
        instrumentation.targetContext.getSystemService(android.content.Context.KEYGUARD_SERVICE)
            as android.app.KeyguardManager

    fun handle(request: AgentRequest): AgentResponse = try {
        // A real device's display times out while the operator drives Qorvex
        // from the host, which sleeps the screen and (on a secured device)
        // raises the keyguard — pausing the target app so commands that need its
        // UI fail. screenGate() wakes the device and, if a secure keyguard
        // remains, returns an actionable error instead of dumping the keyguard
        // or a null window. Simulators never sleep, so iOS has no analogue.
        screenGate(request) ?: when (request) {
            is AgentRequest.Heartbeat -> AgentResponse.Ok
            is AgentRequest.TapCoord -> handleTapCoord(request.x, request.y)
            is AgentRequest.TapElement ->
                handleTap(request.selector, byLabel = false, elementType = null, timeoutMs = request.timeoutMs)
            is AgentRequest.TapByLabel ->
                handleTap(request.label, byLabel = true, elementType = null, timeoutMs = request.timeoutMs)
            is AgentRequest.TapWithType ->
                handleTap(request.selector, request.byLabel, request.elementType, request.timeoutMs)
            is AgentRequest.TypeText -> handleTypeText(request.text)
            is AgentRequest.Swipe ->
                handleSwipe(request.startX, request.startY, request.endX, request.endY, request.duration)
            is AgentRequest.LongPress -> handleLongPress(request.x, request.y, request.duration)
            is AgentRequest.GetValue ->
                handleGetValue(request.selector, request.byLabel, request.elementType, request.timeoutMs)
            is AgentRequest.DumpTree -> handleDumpTree()
            is AgentRequest.Screenshot -> handleScreenshot()
            is AgentRequest.SetTarget -> handleSetTarget(request.bundleId)
            is AgentRequest.FindElement ->
                handleFindElement(request.selector, request.byLabel, request.elementType)
            is AgentRequest.GetTargetInfo -> handleGetTargetInfo()
        }
    } catch (e: Exception) {
        AgentResponse.Error("Internal agent error: ${e.message ?: e.javaClass.simpleName}")
    }

    // -- Screen / keyguard gate ---------------------------------------------

    /**
     * Commands that read or act on the target's accessibility window. The
     * lifecycle/status requests (Heartbeat, SetTarget, GetTargetInfo) inspect no
     * live window, so they neither wake the device nor get blocked by a lock.
     */
    private fun needsTargetWindow(request: AgentRequest): Boolean = when (request) {
        is AgentRequest.Heartbeat,
        is AgentRequest.SetTarget,
        is AgentRequest.GetTargetInfo -> false
        else -> true
    }

    /**
     * Wake the screen for UI commands and, if the device is still locked behind a
     * secure keyguard, return an actionable error; otherwise null (proceed).
     * Best-effort: a swipe-only keyguard is dismissed and the target restored, so
     * the command runs normally.
     */
    private fun screenGate(request: AgentRequest): AgentResponse? {
        if (!needsTargetWindow(request)) return null
        ensureInteractive()
        return if (isScreenLocked()) {
            AgentResponse.Error("Device screen is locked: unlock the phone to control the target")
        } else {
            null
        }
    }

    /**
     * Wake the display and dismiss an insecure keyguard if the screen is off.
     * On a real device the display times out while the operator works from the
     * host; the target app is then paused and rootInActiveWindow points at the
     * keyguard (or is null). A *secured* keyguard cannot be dismissed here — that
     * is reported by [isScreenLocked]. All steps are best-effort.
     */
    private fun ensureInteractive() {
        try {
            val asleep = !uiDevice.isScreenOn
            if (asleep) uiDevice.wakeUp()
            // Dismiss based on keyguard state, not screen state: a swipe-only
            // keyguard can be up while the screen is already on (woken by a
            // notification or ambient tap), and gating dismissal on `asleep`
            // would leave it — and fail the command with a false "locked".
            val locked = keyguardManager.isKeyguardLocked
            if (locked) uiDevice.executeShellCommand("wm dismiss-keyguard")
            if (asleep || locked) uiDevice.waitForIdle(WAKE_IDLE_TIMEOUT_MS)
        } catch (_: Exception) {
            // Non-fatal: let the command run and surface its own diagnostic.
        }
    }

    /** Whether a keyguard is still up (screen off, or a secure lock not dismissed). */
    private fun isScreenLocked(): Boolean =
        try {
            !uiDevice.isScreenOn || keyguardManager.isKeyguardLocked
        } catch (_: Exception) {
            false
        }

    // -- Root accessor ------------------------------------------------------

    /**
     * The active accessibility root window node, or null if none is available.
     *
     * Falls back to the live window list when `rootInActiveWindow` is null. The
     * reported first-connect symptom is a null active-window root while an app is
     * foreground — yet raw `uiautomator dump`, which reads the window list, still
     * works. [activeWindowRoot] reads that same source so a single attach can
     * recover (re-asserting `serviceInfo` alone — what init does — is what proved
     * insufficient at first connect). Callers own and recycle the returned node.
     */
    private fun rootNode(): AccessibilityNodeInfo? =
        uiAutomation.rootInActiveWindow ?: activeWindowRoot()

    /**
     * Pick the current foreground window's root from the live window list,
     * preferring the focused window, then the active one, so the app's window
     * wins over system chrome (status/nav bars, IME). This is the source
     * `uiautomator dump` uses, so it succeeds when `rootInActiveWindow` is stale.
     */
    private fun activeWindowRoot(): AccessibilityNodeInfo? {
        val windows = try {
            uiAutomation.windows
        } catch (_: Exception) {
            return null
        }
        // Rank: focused > active > application-typed. The type tiebreak matters
        // because the sort is stable — without it, when no window is focused or
        // active (a transient post-launch state) the first list entry wins, which
        // can be system chrome (status/nav bar, IME) rather than the app.
        val ordered = windows.sortedByDescending {
            (if (it.isFocused) 4 else 0) +
                (if (it.isActive) 2 else 0) +
                (if (it.type == AccessibilityWindowInfo.TYPE_APPLICATION) 1 else 0)
        }
        var root: AccessibilityNodeInfo? = null
        for (w in ordered) {
            if (root == null) {
                root = try { w.root } catch (_: Exception) { null }
            }
            // Release the window-info handle; the returned root is a separate
            // node owned by the caller and survives this. No-op on API 33+.
            @Suppress("DEPRECATION")
            try { w.recycle() } catch (_: Exception) {}
        }
        return root
    }

    /**
     * (Re)assert the UiAutomation service info to force the accessibility
     * connection to re-establish and start tracking the *current* active window.
     * This is the same configuration applied at startup; re-applying it is how a
     * connection that latched onto a stale launch/splash window recovers.
     *
     * Guard the *assignment*, not just the mutation: `serviceInfo` is @NonNull,
     * so writing back null (if the getter ever returns null) would throw. A
     * setter failure is left to propagate — at startup it fails start-agent
     * loudly (as the original init did); per-request it is caught by `handle`'s
     * outer guard and returned as an error rather than silently degrading.
     */
    private fun rebindWindow() {
        uiAutomation.serviceInfo?.let { info ->
            info.flags = info.flags or
                AccessibilityServiceInfo.FLAG_RETRIEVE_INTERACTIVE_WINDOWS or
                AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS
            uiAutomation.serviceInfo = info
        }
    }

    // -- Tap coordinate -----------------------------------------------------

    private fun handleTapCoord(x: Int, y: Int): AgentResponse {
        return if (uiDevice.click(x, y)) AgentResponse.Ok
        else AgentResponse.Error("Tap failed at ($x, $y)")
    }

    // -- Tap by selector ----------------------------------------------------

    private fun handleTap(
        selector: String,
        byLabel: Boolean,
        elementType: String?,
        timeoutMs: Long?,
    ): AgentResponse {
        val lookupKind = if (byLabel) "label" else "identifier"
        val typeInfo = elementType?.let { " and type '$it'" } ?: ""
        val description = "$lookupKind '$selector'$typeInfo"

        // Track diagnostics across poll attempts so we can distinguish, on
        // timeout, between three cases (A4 contract): target-not-running,
        // element-present-but-not-actionable, and element-absent. `poll` returns
        // a hittable node when it finds one; we capture the other states in the
        // surrounding vars and recycle every non-returned node we resolved.
        var sawRoot = false
        var sawElement = false

        val node = poll(timeoutMs) {
            val root = rootNode() ?: return@poll null
            sawRoot = true
            val n = NodeMapper.resolve(root, selector, byLabel, elementType)
            // Recycle the fresh root handle unless resolve returned it (root
            // itself matched) — that handle is now owned by the result path.
            if (n !== root) NodeMapper.recycle(root)
            if (n == null) return@poll null
            // Element exists this poll. Keep it only if hittable; otherwise note
            // its presence and recycle the handle before retrying.
            if (NodeMapper.hittable(n)) {
                n
            } else {
                sawElement = true
                NodeMapper.recycle(n)
                null
            }
        }

        if (node == null) {
            // Distinguish the three failure modes with actionable messages.
            // `sawRoot` stays false only when every poll saw a null root; a
            // re-check guards against a root that just appeared. Recycle that
            // probe handle so the diagnostic path leaks nothing either.
            if (!sawRoot) {
                val probe = rootNode()
                if (probe == null) {
                    return AgentResponse.Error(
                        "Target application is not running: no active window (set a target or foreground the app)"
                    )
                }
                NodeMapper.recycle(probe)
            }
            val timeoutSuffix = if (timeoutMs != null) " within ${timeoutMs}ms" else ""
            return if (sawElement) {
                AgentResponse.Error(
                    "Element with $description exists but is not hittable$timeoutSuffix " +
                        "(disabled, off-screen, or obscured)"
                )
            } else {
                AgentResponse.Error("Element with $description not found$timeoutSuffix")
            }
        }

        try {
            val frame = NodeMapper.frame(node)
            val cx = (frame.x + frame.width / 2.0).toInt()
            val cy = (frame.y + frame.height / 2.0).toInt()
            // Prefer the node's own click action (coordinate-independent).
            if (node.performAction(AccessibilityNodeInfo.ACTION_CLICK)) {
                return AgentResponse.Ok
            }
            // Coordinate-tap fallback: only valid when the computed center is a
            // real on-screen point. A zero-area or off-screen frame would tap a
            // bogus location and falsely report success, so reject it.
            if (frame.width <= 0.0 || frame.height <= 0.0 || cx < 0 || cy < 0) {
                return AgentResponse.Error(
                    "Tap failed for $description: element has no tappable on-screen frame " +
                        "(center=($cx, $cy), size=${frame.width.toInt()}x${frame.height.toInt()})"
                )
            }
            return if (uiDevice.click(cx, cy)) AgentResponse.Ok
            else AgentResponse.Error("Tap failed for $description")
        } finally {
            NodeMapper.recycle(node)
        }
    }

    // -- Type text ----------------------------------------------------------

    private fun handleTypeText(text: String): AgentResponse {
        val focused = uiAutomation.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)
            ?: rootNode()?.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)
        if (focused == null) {
            return AgentResponse.Error("No focused input; tap a text field first")
        }
        val args = android.os.Bundle().apply {
            putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text)
        }
        val ok = focused.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)
        return if (ok) AgentResponse.Ok
        else AgentResponse.Error("TypeText failed: could not set text on focused element")
    }

    // -- Swipe --------------------------------------------------------------

    private fun handleSwipe(
        startX: Int,
        startY: Int,
        endX: Int,
        endY: Int,
        duration: Double?,
    ): AgentResponse {
        // UiDevice.swipe takes a step count (~5ms per step). Convert duration
        // seconds to steps; default 0.3s like the Swift agent.
        val seconds = duration ?: 0.3
        val steps = (seconds * 200).toInt().coerceAtLeast(1)
        return if (uiDevice.swipe(startX, startY, endX, endY, steps)) AgentResponse.Ok
        else AgentResponse.Error("Swipe failed from ($startX, $startY) to ($endX, $endY)")
    }

    // -- Long press ---------------------------------------------------------

    private fun handleLongPress(x: Int, y: Int, duration: Double): AgentResponse {
        // A zero-distance swipe with a step count proportional to the hold
        // duration performs a press-and-hold at the point. ~5ms per step.
        val steps = (duration * 200).toInt().coerceAtLeast(1)
        return if (uiDevice.swipe(x, y, x, y, steps)) AgentResponse.Ok
        else AgentResponse.Error("Long press failed at ($x, $y)")
    }

    // -- Get value ----------------------------------------------------------

    private fun handleGetValue(
        selector: String,
        byLabel: Boolean,
        elementType: String?,
        timeoutMs: Long?,
    ): AgentResponse {
        val node = poll(timeoutMs) {
            val root = rootNode() ?: return@poll null
            val n = NodeMapper.resolve(root, selector, byLabel, elementType)
            if (n !== root) NodeMapper.recycle(root)
            n
        }
        if (node == null) {
            if (rootNode() == null) {
                return AgentResponse.Error("Target application is not running: no active window")
            }
            val lookupKind = if (byLabel) "label" else "identifier"
            val typeInfo = elementType?.let { " and type '$it'" } ?: ""
            val msg =
                if (timeoutMs != null) "Element with $lookupKind '$selector'$typeInfo not found within ${timeoutMs}ms (timeout)"
                else "Element with $lookupKind '$selector'$typeInfo not found"
            return AgentResponse.Error(msg)
        }
        try {
            return AgentResponse.Value(getValueForNode(node))
        } finally {
            NodeMapper.recycle(node)
        }
    }

    /**
     * Resolve the get-value result for a node, matching iOS semantics:
     * an editable control reports its value field only (empty editable -> None,
     * never the hint/contentDescription), while a non-editable node falls back
     * to its label so static text stays retrievable. See ADR-1 (value =
     * editable text else null) and the Swift agent's handleGetValue.
     */
    internal fun getValueForNode(node: AccessibilityNodeInfo): String? =
        getValueResult(
            editable = NodeMapper.isEditable(node),
            value = NodeMapper.value(node),
            label = NodeMapper.label(node),
        )

    // -- Dump tree ----------------------------------------------------------

    private fun handleDumpTree(): AgentResponse {
        val root = rootNode()
            ?: return AgentResponse.Error("Target application is not running: no active window to dump")
        try {
            val count = intArrayOf(0)
            val tree = NodeMapper.serialize(root, depth = 0, count = count)
                ?: return AgentResponse.Tree("[]")
            // Wrap in an array to match the Rust Vec<UIElement> format.
            return AgentResponse.Tree(serializeTree(listOf(tree)))
        } finally {
            // serialize() does not recycle its root argument (caller owns it);
            // release this fresh active-window handle now that the tree is built.
            NodeMapper.recycle(root)
        }
    }

    // -- Screenshot ---------------------------------------------------------

    private fun handleScreenshot(): AgentResponse {
        // `takeScreenshot` is the one capture path that does not go through
        // [rootNode], so it can't lean on its window-list fallback. If the a11y
        // connection is stale it can return null; re-assert it, let the UI
        // settle, and retry once before giving up.
        val bitmap = (uiAutomation.takeScreenshot() ?: run {
            rebindWindow()
            try {
                uiDevice.waitForIdle(REBIND_TIMEOUT_MS)
            } catch (_: Exception) {
                // Best-effort settle; the retry below surfaces a failure if it didn't help.
            }
            uiAutomation.takeScreenshot()
        }) ?: return AgentResponse.Error("Screenshot failed: no bitmap produced")
        val out = ByteArrayOutputStream()
        val ok = bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, out)
        bitmap.recycle()
        if (!ok) return AgentResponse.Error("Screenshot failed: PNG compression failed")
        return AgentResponse.Screenshot(out.toByteArray())
    }

    // -- Set target ---------------------------------------------------------

    private fun handleSetTarget(bundleId: String): AgentResponse {
        targetPackage = bundleId
        // Launch the package's launcher activity so its UI is foregrounded.
        val context = instrumentation.context
        val intent = context.packageManager.getLaunchIntentForPackage(bundleId)
            ?: return AgentResponse.Error("Target application is not installed: no launch intent for '$bundleId'")
        intent.addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
        context.startActivity(intent)
        uiDevice.wait(
            androidx.test.uiautomator.Until.hasObject(
                androidx.test.uiautomator.By.pkg(bundleId).depth(0),
            ),
            5000,
        )
        return AgentResponse.Ok
    }

    // -- Get target info ----------------------------------------------------

    private fun handleGetTargetInfo(): AgentResponse {
        val pkg = targetPackage ?: uiDevice.currentPackageName ?: ""
        val pm = instrumentation.context.packageManager

        var displayName = ""
        var version = ""
        var build = ""
        var installed = false
        if (pkg.isNotEmpty()) {
            try {
                val info = pm.getPackageInfo(pkg, 0)
                installed = true
                version = info.versionName ?: ""
                build = packageVersionCode(info).toString()
                displayName = pm.getApplicationLabel(info.applicationInfo!!).toString()
            } catch (e: Exception) {
                installed = false
            }
        }

        val running = uiDevice.currentPackageName == pkg && pkg.isNotEmpty()
        val state = when {
            !installed -> "not_running"
            running -> "running_foreground"
            else -> "not_running"
        }

        val json = buildString {
            append('{')
            append("\"bundle_id\":").append(UIElementJSON.quote(pkg)).append(',')
            append("\"display_name\":").append(UIElementJSON.quote(displayName)).append(',')
            append("\"version\":").append(UIElementJSON.quote(version)).append(',')
            append("\"build\":").append(UIElementJSON.quote(build)).append(',')
            append("\"state\":").append(UIElementJSON.quote(state))
            append('}')
        }
        return AgentResponse.TargetInfo(json)
    }

    @Suppress("DEPRECATION")
    private fun packageVersionCode(info: android.content.pm.PackageInfo): Long =
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.P) {
            info.longVersionCode
        } else {
            info.versionCode.toLong()
        }

    // -- Find element -------------------------------------------------------

    private fun handleFindElement(
        selector: String,
        byLabel: Boolean,
        elementType: String?,
    ): AgentResponse {
        val root = rootNode() ?: return AgentResponse.Element("null")
        val node = NodeMapper.resolve(root, selector, byLabel, elementType)
        if (node !== root) NodeMapper.recycle(root)
        if (node == null) return AgentResponse.Element("null")
        try {
            val count = intArrayOf(0)
            val serialized = NodeMapper.serialize(node, depth = 0, count = count)
                ?: return AgentResponse.Element("null")
            return AgentResponse.Element(serialized.toJson())
        } finally {
            // Release the resolved handle now that its data is copied into JSON.
            NodeMapper.recycle(node)
        }
    }

    // -- Poll helper --------------------------------------------------------

    /**
     * Poll `action` until it returns a non-null result or the timeout elapses.
     * With no timeout, runs `action` exactly once. Mirrors the Swift agent's
     * agent-side retry backing the trait's `*_with_timeout` defaults.
     */
    private fun <T> poll(timeoutMs: Long?, intervalMs: Long = 50, action: () -> T?): T? {
        val timeout = timeoutMs ?: 0L
        val deadline = SystemClock.uptimeMillis() + timeout
        while (true) {
            val result = action()
            if (result != null) return result
            if (timeout <= 0L || SystemClock.uptimeMillis() >= deadline) return null
            SystemClock.sleep(intervalMs)
        }
    }

    private companion object {
        /** Settle time after waking the screen / dismissing an insecure keyguard. */
        const val WAKE_IDLE_TIMEOUT_MS = 2000L

        /** Max time to wait for the UI to settle (waitForIdle) after a window re-bind. */
        const val REBIND_TIMEOUT_MS = 2000L
    }
}
