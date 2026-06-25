package com.qorvex.testapp

import android.app.Activity
import android.app.AlertDialog
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.text.Editable
import android.text.TextWatcher
import android.view.Gravity
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup.LayoutParams.MATCH_PARENT
import android.view.ViewGroup.LayoutParams.WRAP_CONTENT
import android.widget.Button
import android.widget.CheckBox
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.ProgressBar
import android.widget.ScrollView
import android.widget.SeekBar
import android.widget.TextView
import kotlin.math.abs

/**
 * The Qorvex Android sample app — the Android counterpart of `qorvex-testapp/`
 * (the iOS SwiftUI sample). It exposes at least one element appropriate to every
 * Qorvex `ActionType`, each with a stable resource-id from [Ids] so the Kotlin
 * agent (and therefore `qorvex tap/get-value/wait-for/screen-info`) can resolve
 * them by id, label, or type (ADR-1).
 *
 * Built programmatically (no XML layouts) to keep the module dependency-light;
 * ids come from `res/values/ids.xml` via [View.setId] so
 * `AccessibilityNodeInfo.viewIdResourceName` reports the bare entry name.
 *
 * Five sections mirror the five iOS tabs; tab buttons (themselves tappable,
 * labelled elements) switch between them. Timing constants (delayed appearance,
 * brief element, loading, counter tick, long-press min) match the iOS app
 * exactly so wait-for / wait-for-not parity holds.
 */
class MainActivity : Activity() {

    private val handler = Handler(Looper.getMainLooper())

    // Section root containers, swapped on tab tap.
    private lateinit var sections: Map<String, View>

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            id = resId(Ids.MAIN_TAB_VIEW)
        }

        // --- Tab bar (5 tappable, labelled tabs, mirror iOS tab labels) ---
        val tabBar = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
        }
        val controls = buildControls()
        val textInput = buildTextInput()
        val navigation = buildNavigation()
        val gestures = buildGestures()
        val dynamic = buildDynamic()
        sections = linkedMapOf(
            "Controls" to controls,
            "Text Input" to textInput,
            "Navigation" to navigation,
            "Gestures" to gestures,
            "Dynamic" to dynamic,
        )
        val container = FrameLayout(this)
        sections.values.forEach { container.addView(it) }

        tabButton(Ids.TAB_CONTROLS, "Controls").also { tabBar.addView(it) }
        tabButton(Ids.TAB_TEXT_INPUT, "Text Input").also { tabBar.addView(it) }
        tabButton(Ids.TAB_NAVIGATION, "Navigation").also { tabBar.addView(it) }
        tabButton(Ids.TAB_GESTURES, "Gestures").also { tabBar.addView(it) }
        tabButton(Ids.TAB_DYNAMIC, "Dynamic").also { tabBar.addView(it) }

        root.addView(tabBar)
        root.addView(container)
        setContentView(root)

        showSection("Controls")
    }

    private fun showSection(name: String) {
        sections.forEach { (k, v) -> v.visibility = if (k == name) View.VISIBLE else View.GONE }
    }

    // ----- helpers -----

    private fun resId(name: String): Int =
        resources.getIdentifier(name, "id", packageName)

    private fun tabButton(id: String, label: String): Button =
        Button(this).apply {
            setId(resId(id))
            // Android Buttons default to all-caps display, which surfaces as an
            // uppercased accessibility label ("CONTROLS"); the iOS sample shows
            // the authored casing. Disable it so labels match cross-platform and
            // tap-by-label parity holds.
            isAllCaps = false
            text = label
            setOnClickListener { showSection(label) }
        }

    private fun scrollSection(build: LinearLayout.() -> Unit): ScrollView {
        val col = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(24, 24, 24, 24)
            build()
        }
        return ScrollView(this).apply { addView(col) }
    }

    private fun LinearLayout.heading(text: String) {
        addView(TextView(context).apply {
            this.text = text
            textSize = 18f
        })
    }

    private fun LinearLayout.button(id: String, label: String, onClick: () -> Unit): Button {
        val b = Button(context).apply {
            setId(resId(id))
            isAllCaps = false // keep authored casing for cross-platform label parity
            text = label
            setOnClickListener { onClick() }
        }
        addView(b)
        return b
    }

    private fun LinearLayout.label(id: String, text: String): TextView {
        val t = TextView(context).apply {
            setId(resId(id))
            this.text = text
        }
        addView(t)
        return t
    }

    private fun LinearLayout.field(id: String, hint: String): EditText {
        val e = EditText(context).apply {
            setId(resId(id))
            this.hint = hint
        }
        addView(e)
        return e
    }

    // ===================================================================
    // Tab 1 — Controls  (mirror ControlsView.swift)
    // ===================================================================
    private fun buildControls(): View = scrollSection {
        var tapCount = 0
        var quantity = 1
        var pickerIndex = 1
        val sizes = listOf("Small", "Medium", "Large")

        heading("Tap Button")
        val tapCountLabel = label(Ids.CONTROLS_TAP_COUNT, "Tapped: 0")
        button(Ids.CONTROLS_TAP_BUTTON, "Tap Me") {
            tapCount += 1
            tapCountLabel.text = "Tapped: $tapCount"
        }

        heading("Toggle")
        val wifiStatus = label(Ids.CONTROLS_WIFI_STATUS, "Off")
        val wifi = CheckBox(this@MainActivity).apply {
            setId(resId(Ids.CONTROLS_TOGGLE_WIFI))
            text = "Wi-Fi"
            setOnCheckedChangeListener { _, checked -> wifiStatus.text = if (checked) "On" else "Off" }
        }
        addView(wifi)

        heading("Slider")
        val sliderValue = label(Ids.CONTROLS_SLIDER_VALUE, "Volume: 50")
        val slider = SeekBar(this@MainActivity).apply {
            setId(resId(Ids.CONTROLS_SLIDER_VOLUME))
            contentDescription = "Volume"
            max = 100
            progress = 50
            setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
                override fun onProgressChanged(sb: SeekBar?, p: Int, fromUser: Boolean) {
                    sliderValue.text = "Volume: $p"
                }
                override fun onStartTrackingTouch(sb: SeekBar?) {}
                override fun onStopTrackingTouch(sb: SeekBar?) {}
            })
        }
        addView(slider)

        heading("Stepper")
        val stepperValue = label(Ids.CONTROLS_STEPPER_VALUE, "Quantity: 1")
        val stepperRow = LinearLayout(this@MainActivity).apply { orientation = LinearLayout.HORIZONTAL }
        val minus = Button(this@MainActivity).apply {
            setId(resId(Ids.CONTROLS_STEPPER_MINUS)); text = "-"
            setOnClickListener {
                if (quantity > 0) { quantity -= 1; stepperValue.text = "Quantity: $quantity" }
            }
        }
        val plus = Button(this@MainActivity).apply {
            setId(resId(Ids.CONTROLS_STEPPER_PLUS)); text = "+"
            setOnClickListener {
                if (quantity < 20) { quantity += 1; stepperValue.text = "Quantity: $quantity" }
            }
        }
        stepperRow.addView(minus); stepperRow.addView(plus)
        addView(stepperRow)

        heading("Segmented Picker")
        val pickerValue = label(Ids.CONTROLS_PICKER_VALUE, "Medium")
        button(Ids.CONTROLS_PICKER_SIZE, "Size: Medium") {
            pickerIndex = (pickerIndex + 1) % sizes.size
            val sz = sizes[pickerIndex]
            pickerValue.text = sz
        }

        heading("Destructive Button")
        val deleteStatus = label(Ids.CONTROLS_DELETE_STATUS, "").apply { visibility = View.GONE }
        button(Ids.CONTROLS_DELETE_BUTTON, "Delete") {
            deleteStatus.text = "Deleted!"
            deleteStatus.visibility = View.VISIBLE
        }
    }

    // ===================================================================
    // Tab 2 — Text Input  (mirror TextInputView.swift)
    // ===================================================================
    private fun buildTextInput(): View = scrollSection {
        heading("Username")
        val usernameValue: TextView
        val username = field(Ids.TEXT_USERNAME_FIELD, "Username")

        heading("Email")
        val email = field(Ids.TEXT_EMAIL_FIELD, "Email")

        heading("Password")
        val password = EditText(this@MainActivity).apply {
            setId(resId(Ids.TEXT_PASSWORD_FIELD))
            hint = "Password"
            inputType = android.text.InputType.TYPE_CLASS_TEXT or
                android.text.InputType.TYPE_TEXT_VARIATION_PASSWORD
        }
        addView(password)

        heading("Search")
        val search = field(Ids.TEXT_SEARCH_FIELD, "Search...")

        heading("Notes")
        val notes = EditText(this@MainActivity).apply {
            setId(resId(Ids.TEXT_NOTES_EDITOR))
            minLines = 3
            isSingleLine = false
        }
        addView(notes)

        heading("Actions")
        val submitResult = label(Ids.TEXT_SUBMIT_RESULT, "").apply { visibility = View.GONE }
        val actionRow = LinearLayout(this@MainActivity).apply { orientation = LinearLayout.HORIZONTAL }
        actionRow.addView(Button(this@MainActivity).apply {
            setId(resId(Ids.TEXT_SUBMIT_BUTTON)); text = "Submit"
            setOnClickListener {
                submitResult.text = "Submitted: ${username.text}"
                submitResult.visibility = View.VISIBLE
            }
        })
        actionRow.addView(Button(this@MainActivity).apply {
            setId(resId(Ids.TEXT_CLEAR_BUTTON)); text = "Clear All"
            setOnClickListener {
                username.setText(""); email.setText(""); password.setText("")
                search.setText(""); notes.setText("")
                submitResult.text = ""; submitResult.visibility = View.GONE
            }
        })
        addView(actionRow)

        heading("Live Preview")
        usernameValue = label(Ids.TEXT_USERNAME_VALUE, "Username: ")
        val emailValue = label(Ids.TEXT_EMAIL_VALUE, "Email: ")
        username.addTextChangedListener(simpleWatcher { usernameValue.text = "Username: $it" })
        email.addTextChangedListener(simpleWatcher { emailValue.text = "Email: $it" })
    }

    private fun simpleWatcher(onChange: (String) -> Unit) = object : TextWatcher {
        override fun beforeTextChanged(s: CharSequence?, start: Int, count: Int, after: Int) {}
        override fun onTextChanged(s: CharSequence?, start: Int, before: Int, count: Int) {
            onChange(s?.toString() ?: "")
        }
        override fun afterTextChanged(s: Editable?) {}
    }

    // ===================================================================
    // Tab 3 — Navigation  (mirror NavigationTestView.swift)
    // ===================================================================
    private fun buildNavigation(): View = scrollSection {
        val statusLabel: TextView

        heading("Push Navigation")
        // Detail "page" rendered inline; appears after push, mirrors nav-detail-label.
        val detailLabel = label(Ids.NAV_DETAIL_LABEL, "Detail Page").apply { visibility = View.GONE }
        button(Ids.NAV_PUSH_BUTTON, "Go to Detail") { detailLabel.visibility = View.VISIBLE }

        heading("Sheet")
        val sheetContent = label(Ids.NAV_SHEET_CONTENT, "Sheet Content").apply { visibility = View.GONE }
        val sheetDismiss = button(Ids.NAV_SHEET_DISMISS, "Dismiss") { }.apply { visibility = View.GONE }
        button(Ids.NAV_SHEET_BUTTON, "Show Sheet") {
            sheetContent.visibility = View.VISIBLE
            sheetDismiss.visibility = View.VISIBLE
        }

        heading("Alert")
        button(Ids.NAV_ALERT_BUTTON, "Show Alert") {
            val dlg = AlertDialog.Builder(this@MainActivity)
                .setTitle("Test Alert")
                .setMessage("This is a test alert")
                .setPositiveButton("OK", null)
                .create()
            dlg.setOnShowListener {
                val ok = dlg.getButton(AlertDialog.BUTTON_POSITIVE)
                ok.setId(resId(Ids.NAV_ALERT_OK))
                ok.setOnClickListener {
                    findViewById<TextView>(resId(Ids.NAV_STATUS_LABEL)).text = "Alert confirmed"
                    dlg.dismiss()
                }
            }
            dlg.show()
        }

        heading("Confirmation")
        button(Ids.NAV_CONFIRM_BUTTON, "Show Confirmation") {
            val dlg = AlertDialog.Builder(this@MainActivity)
                .setTitle("Confirm Action")
                .setNegativeButton("Cancel") { _, _ ->
                    findViewById<TextView>(resId(Ids.NAV_STATUS_LABEL)).text = "Cancelled"
                }
                .setPositiveButton("Delete", null)
                .create()
            dlg.setOnShowListener {
                val del = dlg.getButton(AlertDialog.BUTTON_POSITIVE)
                del.setId(resId(Ids.NAV_CONFIRM_DELETE))
                del.setOnClickListener {
                    findViewById<TextView>(resId(Ids.NAV_STATUS_LABEL)).text = "Deleted"
                    dlg.dismiss()
                }
            }
            dlg.show()
        }

        // Wire sheet-dismiss now that statusLabel exists.
        statusLabel = label(Ids.NAV_STATUS_LABEL, "No action yet")
        sheetDismiss.setOnClickListener {
            sheetContent.visibility = View.GONE
            sheetDismiss.visibility = View.GONE
            statusLabel.text = "Sheet dismissed"
        }
    }

    // ===================================================================
    // Tab 4 — Gestures  (mirror ScrollGesturesView.swift)
    // ===================================================================
    private fun buildGestures(): View = scrollSection {
        heading("Scrollable List")
        val list = LinearLayout(this@MainActivity).apply {
            setId(resId(Ids.SCROLL_LIST))
            orientation = LinearLayout.VERTICAL
        }
        for (n in 1..Ids.SCROLL_ITEM_COUNT) {
            list.addView(TextView(this@MainActivity).apply {
                setId(resId(Ids.SCROLL_ITEM_PREFIX + n))
                text = "Item $n"
            })
        }
        addView(list)

        heading("Long-Press Target")
        val longpressStatus = label(Ids.GESTURE_LONGPRESS_STATUS, "Tap and hold")
        val longpressTarget = TextView(this@MainActivity).apply {
            setId(resId(Ids.GESTURE_LONGPRESS_TARGET))
            text = "[long-press here]"
            gravity = Gravity.CENTER
            setPadding(0, 48, 0, 48)
            isLongClickable = true
            setOnLongClickListener { longpressStatus.text = "Long pressed!"; true }
        }
        addView(longpressTarget)

        heading("Drag / Swipe Area")
        val swipeStatus = label(Ids.GESTURE_SWIPE_STATUS, "Swipe here")
        val swipeArea = TextView(this@MainActivity).apply {
            setId(resId(Ids.GESTURE_SWIPE_AREA))
            text = "[swipe area]"
            gravity = Gravity.CENTER
            setPadding(0, 72, 0, 72)
        }
        attachSwipeDetector(swipeArea) { dir -> swipeStatus.text = "Swiped: $dir" }
        addView(swipeArea)

        heading("Tap Coordinate Display")
        val tapLocation = label(Ids.GESTURE_TAP_LOCATION, "Tap the area above")
        val tapArea = TextView(this@MainActivity).apply {
            setId(resId(Ids.GESTURE_TAP_AREA))
            text = "[tap area]"
            gravity = Gravity.CENTER
            setPadding(0, 72, 0, 72)
            setOnTouchListener { _, e ->
                if (e.action == MotionEvent.ACTION_UP) {
                    tapLocation.text = "Tapped at: ${e.x.toInt()}, ${e.y.toInt()}"
                    performClick()
                }
                true
            }
        }
        addView(tapArea)
    }

    /** Min-distance 20px swipe detector → emits left/right/up/down (mirror iOS). */
    private fun attachSwipeDetector(v: View, onSwipe: (String) -> Unit) {
        var downX = 0f
        var downY = 0f
        v.setOnTouchListener { _, e ->
            when (e.action) {
                MotionEvent.ACTION_DOWN -> { downX = e.x; downY = e.y; true }
                MotionEvent.ACTION_UP -> {
                    val dx = e.x - downX
                    val dy = e.y - downY
                    if (abs(dx) > 20 || abs(dy) > 20) {
                        val dir = if (abs(dx) > abs(dy)) {
                            if (dx > 0) "right" else "left"
                        } else {
                            if (dy > 0) "down" else "up"
                        }
                        onSwipe(dir)
                    }
                    v.performClick()
                    true
                }
                else -> true
            }
        }
    }

    // ===================================================================
    // Tab 5 — Dynamic  (mirror DynamicView.swift) — timing / waitable tab
    // ===================================================================
    private fun buildDynamic(): View = scrollSection {
        var counterRunning = false
        var counterValue = 0
        val counterTick = object : Runnable {
            override fun run() {
                if (counterRunning) {
                    counterValue += 1
                    findViewById<TextView>(resId(Ids.DYNAMIC_COUNTER_VALUE)).text = "Count: $counterValue"
                    handler.postDelayed(this, Ids.COUNTER_TICK_MS)
                }
            }
        }

        heading("Delayed Appearance")
        val delayedLabel = label(Ids.DYNAMIC_DELAYED_LABEL, "I appeared!").apply { visibility = View.GONE }
        button(Ids.DYNAMIC_SHOW_DELAYED, "Show After Delay") {
            handler.postDelayed({ delayedLabel.visibility = View.VISIBLE }, Ids.DELAYED_APPEAR_MS)
        }

        heading("Auto-Disappearing Element")
        val briefLabel = label(Ids.DYNAMIC_BRIEF_LABEL, "Now you see me").apply { visibility = View.GONE }
        button(Ids.DYNAMIC_SHOW_BRIEF, "Show Briefly") {
            briefLabel.visibility = View.VISIBLE
            handler.postDelayed({ briefLabel.visibility = View.GONE }, Ids.BRIEF_VISIBLE_MS)
        }

        heading("Loading Indicator")
        val spinner = ProgressBar(this@MainActivity).apply {
            setId(resId(Ids.DYNAMIC_LOADING_SPINNER)); visibility = View.GONE
        }
        val loadingDone = label(Ids.DYNAMIC_LOADING_DONE, "Loading complete").apply { visibility = View.GONE }
        button(Ids.DYNAMIC_START_LOADING, "Start Loading") {
            spinner.visibility = View.VISIBLE
            loadingDone.visibility = View.GONE
            handler.postDelayed({
                spinner.visibility = View.GONE
                loadingDone.visibility = View.VISIBLE
            }, Ids.LOADING_MS)
        }
        addView(spinner)

        heading("Toggle Visibility")
        val togglable = label(Ids.DYNAMIC_TOGGLABLE, "Visible Element").apply { visibility = View.GONE }
        button(Ids.DYNAMIC_TOGGLE_VISIBILITY, "Toggle Element") {
            togglable.visibility = if (togglable.visibility == View.VISIBLE) View.GONE else View.VISIBLE
        }

        heading("Counter with Auto-Increment")
        val counterValueLabel = label(Ids.DYNAMIC_COUNTER_VALUE, "Count: 0").apply { visibility = View.GONE }
        val stopCounter = button(Ids.DYNAMIC_STOP_COUNTER, "Stop Counter") { }.apply { visibility = View.GONE }
        button(Ids.DYNAMIC_START_COUNTER, "Start Counter") {
            if (!counterRunning) {
                counterRunning = true
                counterValue = 0
                counterValueLabel.text = "Count: 0"
                counterValueLabel.visibility = View.VISIBLE
                stopCounter.visibility = View.VISIBLE
                handler.postDelayed(counterTick, Ids.COUNTER_TICK_MS)
            }
        }
        stopCounter.setOnClickListener {
            counterRunning = false
            counterValueLabel.visibility = View.GONE
            stopCounter.visibility = View.GONE
        }

        heading("Reset")
        button(Ids.DYNAMIC_RESET, "Reset All") {
            counterRunning = false
            counterValue = 0
            delayedLabel.visibility = View.GONE
            briefLabel.visibility = View.GONE
            spinner.visibility = View.GONE
            loadingDone.visibility = View.GONE
            togglable.visibility = View.GONE
            counterValueLabel.visibility = View.GONE
            stopCounter.visibility = View.GONE
        }
    }
}
