package com.qorvex.testapp

/**
 * Canonical stable element-id inventory for the Qorvex Android sample app.
 *
 * Each constant is the Android resource-id *entry name* assigned (via
 * [android.view.View.setId] backed by an `@id` resource) to one UI element. The
 * Kotlin agent's [NodeMapper.bareResourceId] reports exactly this bare entry
 * name as the `UIElement.identifier`, so `qorvex find <id>` / `tap <id>` /
 * `get-value <id>` resolve these directly (ADR-1).
 *
 * # iOS parity (hyphen ↔ underscore)
 *
 * The iOS sample app (`qorvex-testapp/`) uses hyphenated accessibility
 * identifiers (e.g. `controls-tap-button`). Android resource-id entry names are
 * Java identifiers and **cannot contain hyphens**, so each id here is the 1:1
 * underscore transliteration (`controls_tap_button`). The two id sets are in
 * lock-step; the parity harness maps between the forms with a single
 * hyphen→underscore substitution. The *element kinds, counts, labels, values,
 * and timing* mirror iOS exactly.
 *
 * This object is the single source of truth: the UI wires every id from here and
 * [IdsCompletenessTest] asserts every ActionType-relevant kind is represented.
 */
object Ids {
    // Root container (mirrors iOS `main-tab-view`).
    const val MAIN_TAB_VIEW = "main_tab_view"

    // Tab buttons — tappable, labelled by their tab name (mirror iOS tab labels).
    const val TAB_CONTROLS = "tab_controls"
    const val TAB_TEXT_INPUT = "tab_text_input"
    const val TAB_NAVIGATION = "tab_navigation"
    const val TAB_GESTURES = "tab_gestures"
    const val TAB_DYNAMIC = "tab_dynamic"

    // ---- Tab 1: Controls (mirror ControlsView.swift) ----
    const val CONTROLS_TAP_BUTTON = "controls_tap_button"           // tappable
    const val CONTROLS_TAP_COUNT = "controls_tap_count"             // value-bearing
    const val CONTROLS_TOGGLE_WIFI = "controls_toggle_wifi"         // tappable toggle
    const val CONTROLS_WIFI_STATUS = "controls_wifi_status"         // value-bearing
    const val CONTROLS_SLIDER_VOLUME = "controls_slider_volume"     // value-bearing slider
    const val CONTROLS_SLIDER_VALUE = "controls_slider_value"       // value-bearing
    const val CONTROLS_STEPPER_MINUS = "controls_stepper_minus"     // tappable
    const val CONTROLS_STEPPER_PLUS = "controls_stepper_plus"       // tappable
    const val CONTROLS_STEPPER_VALUE = "controls_stepper_value"     // value-bearing
    const val CONTROLS_PICKER_SIZE = "controls_picker_size"         // tappable (cycles)
    const val CONTROLS_PICKER_VALUE = "controls_picker_value"       // value-bearing
    const val CONTROLS_DELETE_BUTTON = "controls_delete_button"     // tappable
    const val CONTROLS_DELETE_STATUS = "controls_delete_status"     // waitable (appears on tap)

    // ---- Tab 2: Text Input (mirror TextInputView.swift) ----
    const val TEXT_USERNAME_FIELD = "text_username_field"           // text input
    const val TEXT_EMAIL_FIELD = "text_email_field"                 // text input
    const val TEXT_PASSWORD_FIELD = "text_password_field"           // secure text input
    const val TEXT_SEARCH_FIELD = "text_search_field"               // text input
    const val TEXT_NOTES_EDITOR = "text_notes_editor"               // multiline text input
    const val TEXT_SUBMIT_BUTTON = "text_submit_button"             // tappable
    const val TEXT_CLEAR_BUTTON = "text_clear_button"               // tappable
    const val TEXT_SUBMIT_RESULT = "text_submit_result"             // waitable
    const val TEXT_USERNAME_VALUE = "text_username_value"           // value-bearing (live)
    const val TEXT_EMAIL_VALUE = "text_email_value"                 // value-bearing (live)

    // ---- Tab 3: Navigation (mirror NavigationTestView.swift) ----
    const val NAV_PUSH_BUTTON = "nav_push_button"                   // tappable (push)
    const val NAV_SHEET_BUTTON = "nav_sheet_button"                 // tappable (dialog)
    const val NAV_ALERT_BUTTON = "nav_alert_button"                 // tappable (dialog)
    const val NAV_CONFIRM_BUTTON = "nav_confirm_button"             // tappable (dialog)
    const val NAV_STATUS_LABEL = "nav_status_label"                 // value-bearing
    const val NAV_DETAIL_LABEL = "nav_detail_label"                 // waitable (after push)
    const val NAV_SHEET_CONTENT = "nav_sheet_content"               // waitable (in sheet)
    const val NAV_SHEET_DISMISS = "nav_sheet_dismiss"               // tappable (in sheet)
    const val NAV_ALERT_OK = "nav_alert_ok"                         // tappable (in alert)
    const val NAV_CONFIRM_DELETE = "nav_confirm_delete"             // tappable (in dialog)

    // ---- Tab 4: Gestures (mirror ScrollGesturesView.swift) ----
    const val SCROLL_LIST = "scroll_list"                           // scrollable list
    // scroll_item_1 .. scroll_item_50 are assigned dynamically (see SCROLL_ITEM_COUNT).
    const val SCROLL_ITEM_PREFIX = "scroll_item_"                   // + 1..50
    const val SCROLL_ITEM_COUNT = 50
    const val GESTURE_LONGPRESS_TARGET = "gesture_longpress_target" // long-pressable
    const val GESTURE_LONGPRESS_STATUS = "gesture_longpress_status" // value-bearing
    const val GESTURE_SWIPE_AREA = "gesture_swipe_area"             // swipeable
    const val GESTURE_SWIPE_STATUS = "gesture_swipe_status"         // value-bearing
    const val GESTURE_TAP_AREA = "gesture_tap_area"                 // tap-coordinate target
    const val GESTURE_TAP_LOCATION = "gesture_tap_location"         // value-bearing

    // ---- Tab 5: Dynamic (mirror DynamicView.swift) — timing/waitable tab ----
    const val DYNAMIC_SHOW_DELAYED = "dynamic_show_delayed"         // tappable
    const val DYNAMIC_DELAYED_LABEL = "dynamic_delayed_label"       // waitable (+2s)
    const val DYNAMIC_SHOW_BRIEF = "dynamic_show_brief"             // tappable
    const val DYNAMIC_BRIEF_LABEL = "dynamic_brief_label"           // auto-disappears (-3s) -> wait-not
    const val DYNAMIC_START_LOADING = "dynamic_start_loading"       // tappable
    const val DYNAMIC_LOADING_SPINNER = "dynamic_loading_spinner"   // transient (2s)
    const val DYNAMIC_LOADING_DONE = "dynamic_loading_done"         // waitable (+2s)
    const val DYNAMIC_TOGGLE_VISIBILITY = "dynamic_toggle_visibility" // tappable
    const val DYNAMIC_TOGGLABLE = "dynamic_togglable"               // toggle visible/hidden
    const val DYNAMIC_START_COUNTER = "dynamic_start_counter"       // tappable
    const val DYNAMIC_COUNTER_VALUE = "dynamic_counter_value"       // auto-incrementing value
    const val DYNAMIC_STOP_COUNTER = "dynamic_stop_counter"         // tappable
    const val DYNAMIC_RESET = "dynamic_reset"                       // tappable

    // Timing constants mirrored from the iOS DynamicView (milliseconds).
    const val DELAYED_APPEAR_MS = 2_000L
    const val BRIEF_VISIBLE_MS = 3_000L
    const val LOADING_MS = 2_000L
    const val COUNTER_TICK_MS = 1_000L
    const val LONG_PRESS_MIN_MS = 1_000L
}
