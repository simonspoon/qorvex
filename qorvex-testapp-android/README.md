# qorvex-testapp-android

The Android verification target for Qorvex — the Android counterpart of
`qorvex-testapp/` (the iOS SwiftUI sample). It exposes at least one element
appropriate to **every** Qorvex `ActionType`, each with a stable resource-id so
the Kotlin UiAutomator agent (and therefore the `qorvex` CLI) can resolve it by
id, label, or type.

Package / `applicationId`: **`com.qorvex.testapp`** — identical to the iOS
bundle id, so `qorvex set-target com.qorvex.testapp` is the same on both
platforms.

## Action coverage (mirrors the iOS sample)

Five sections mirror the five iOS tabs. Tab buttons are themselves tappable,
labelled elements. Every `ActionType` has at least one target:

| ActionType | Element(s) | Behavior mirrored from iOS |
|---|---|---|
| `Tap` (id / label / type) | `controls_tap_button` | increments `controls_tap_count` (`Tapped: n`) |
| `TapLocation` | `gesture_tap_area` | records `gesture_tap_location` (`Tapped at: x, y`) |
| `Swipe` | `gesture_swipe_area` | sets `gesture_swipe_status` (`Swiped: up/down/left/right`) |
| `LongPress` | `gesture_longpress_target` | sets `gesture_longpress_status` (`Long pressed!`), 1.0s min |
| `SendKeys` (type) | `text_username_field` … | live-updates `text_username_value` |
| `GetValue` (editable) | `text_username_field` | editable node text → `value` |
| `GetValue` (label) / `GetScreenInfo` | every `*_value`/`*_status` label | text → `label` |
| `WaitFor` | `dynamic_delayed_label` | appears +2s after `dynamic_show_delayed` |
| `WaitForNot` | `dynamic_brief_label` | auto-disappears 3s after `dynamic_show_brief` |
| `GetScreenshot` | whole screen | UiAutomator `takeScreenshot` |
| `SetTarget` / `StartTarget` / `StopTarget` / `GetTargetInfo` | package `com.qorvex.testapp` | app lifecycle |

Other mirrored elements: toggle (`controls_toggle_wifi` → `controls_wifi_status`),
slider (`controls_slider_volume` → `controls_slider_value`), stepper
(`controls_stepper_minus`/`_plus` → `controls_stepper_value`), segmented picker
(`controls_picker_size` → `controls_picker_value`), destructive button
(`controls_delete_button` → waitable `controls_delete_status`), text submit/clear
(`text_submit_button`/`text_clear_button` → waitable `text_submit_result`),
navigation push/sheet/alert/confirm with waitable `nav_detail_label` /
`nav_sheet_content` and `nav_status_label`, a 50-row scroll list
(`scroll_list`, `scroll_item_1`…`scroll_item_50`), and the auto-incrementing
`dynamic_counter_value`.

The canonical id inventory is `com.qorvex.testapp.Ids`; `res/values/ids.xml`
declares each as an `@id` so `AccessibilityNodeInfo.viewIdResourceName` reports
the bare entry name (= `UIElement.identifier`, ADR-1). The pure-JVM
`IdsCompletenessTest` asserts every constant has a resource entry and every
action kind is represented.

### iOS parity: hyphen ↔ underscore ids

The iOS sample uses hyphenated accessibility ids (`controls-tap-button`).
Android resource-id entry names are Java identifiers and **cannot contain
hyphens**, so each id here is the 1:1 underscore transliteration
(`controls_tap_button`). The two id sets are in lock-step; the parity check maps
between forms with a single hyphen→underscore substitution. Element *kinds,
counts, labels, values, and timing constants* match iOS exactly.

## Build

```bash
# Assemble the debug APK (static-verified in CI; no emulator needed)
./gradlew assembleDebug

# Pure-JVM id-inventory test (no emulator)
./gradlew testDebugUnitTest

# Install on a running emulator/device
./gradlew installDebug
```

Requires `ANDROID_HOME` / `local.properties` pointing at an Android SDK with
platform 35 + build-tools. Versions match `qorvex-agent-android` (AGP 8.5.2,
Kotlin 1.9.24, Gradle 8.10.2, compileSdk 35, minSdk 24).

## Parity check

Two layers, mirroring how the iOS path is verified:

### 1. Static parity harness (runs in CI, no emulator) — primary

Runs the full `ActionType` matrix through the production `ActionExecutor`
against the **`AndroidDriver`** backend and asserts equivalence with the iOS
**`AgentDriver`** backend, both over a loopback mock agent:

```bash
cargo test -p qorvex-core --features test-support --test android_parity
```

This proves the AndroidDriver trait impl is behaviorally interchangeable with
the iOS driver for every action.

### 2. Live on-emulator parity (the single deferred step)

The on-device round-trip (real Kotlin agent + `adb forward` + UiAutomator
against this app) is driven by:

```bash
scripts/android_parity_live.sh [adb-serial]
```

It boots the Android agent (`qorvex start-agent --platform android`), targets
`com.qorvex.testapp`, and runs the live matrix (tap/swipe/long-press/type/
screen-info/get-value/wait-for/wait-for-not/screenshot/target lifecycle),
asserting the sample app's value labels change as expected. Requires a booted
emulator with this app and the agent installed (see the script header). This
step cannot run in an environment without an emulator/device.
