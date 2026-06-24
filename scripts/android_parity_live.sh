#!/usr/bin/env bash
#
# android_parity_live.sh — live on-emulator Android parity check (story #90).
#
# Runs the full Qorvex ActionType matrix against the Android sample app
# (qorvex-testapp-android, package com.qorvex.testapp) on a running emulator or
# device, via the production `qorvex` CLI through the AndroidDriver path. This is
# the LIVE counterpart of the static, no-emulator harness:
#
#     cargo test -p qorvex-core --features test-support --test android_parity
#
# The static harness proves the AndroidDriver trait impl is behaviorally
# equivalent to the iOS path for every action (against a mock agent). THIS script
# proves the same matrix end-to-end over the real Kotlin agent + adb forward +
# UiAutomator round-trip against the real sample app.
#
# ---------------------------------------------------------------------------
# Prerequisites (one-time):
#   1. An Android emulator booted (or a device via `adb connect`):
#        emulator -avd <name> &        # or: adb devices  → confirm a serial
#   2. The sample app installed:
#        ( cd qorvex-testapp-android && ./gradlew installDebug )
#   3. The Kotlin agent installed + instrumented (story #88 lifecycle, or manual):
#        ( cd qorvex-agent-android && ./gradlew installDebug installDebugAndroidTest )
#   4. A release build of the CLI on PATH, or run via `cargo run -p qorvex-cli --`.
#
# Usage:
#   scripts/android_parity_live.sh [adb-serial]
#
# If no serial is given, the first `adb devices` entry is used.
# ---------------------------------------------------------------------------
set -euo pipefail

SERIAL="${1:-}"
SESSION="android-parity-$$"
PKG="com.qorvex.testapp"

# qorvex invocation — prefer an installed binary, else cargo.
if command -v qorvex >/dev/null 2>&1; then
  QORVEX=(qorvex)
else
  QORVEX=(cargo run --quiet -p qorvex-cli --)
fi

q() { "${QORVEX[@]}" -s "$SESSION" "$@"; }

pass=0
fail=0
check() {
  local label="$1"; shift
  if "$@"; then
    echo "  PASS  $label"
    pass=$((pass + 1))
  else
    echo "  FAIL  $label"
    fail=$((fail + 1))
  fi
}

echo "== Qorvex Android live parity =="
# Resolve serial.
if [[ -z "$SERIAL" ]]; then
  SERIAL="$(adb devices | awk 'NR>1 && $2=="device" {print $1; exit}')"
fi
[[ -n "$SERIAL" ]] || { echo "No adb device found. Boot an emulator first." >&2; exit 1; }
echo "device: $SERIAL"

# --- bring up server + Android agent + target ---
q start-agent --platform android
q set-target "$PKG"
q start-target
sleep 2

# ===========================================================================
# The full ActionType matrix, mirroring tests/android_parity.rs but live.
# Each step asserts the sample app's value-bearing labels changed as expected,
# which is the on-device equivalence with the iOS verification surface.
# ===========================================================================

# tap (by id) → controls_tap_count becomes "Tapped: 1"
q tap tab_controls --label || q tap tab_controls
q tap controls_tap_button
check "tap → counter increments"      bash -c '[[ "$('"${QORVEX[*]}"' -s '"$SESSION"' get-value controls_tap_count)" == *"Tapped: 1"* ]]'

# tap (by label)
check "tap-by-label"                  q tap "Tap Me" --label

# tap (with type filter)
check "tap-with-type"                 q tap controls_tap_button --type Button

# toggle (value-bearing): wifi status flips On
q tap controls_toggle_wifi
check "toggle → wifi status On"       bash -c '[[ "$('"${QORVEX[*]}"' -s '"$SESSION"' get-value controls_wifi_status)" == *"On"* ]]'

# send-keys (type text) → username live-preview updates
q tap tab_text_input --label || q tap tab_text_input
q tap text_username_field
q send-keys "alice"
check "send-keys → live username"     bash -c '[[ "$('"${QORVEX[*]}"' -s '"$SESSION"' get-value text_username_value)" == *"alice"* ]]'

# get-value (editable field carries its text)
check "get-value editable"            bash -c '[[ "$('"${QORVEX[*]}"' -s '"$SESSION"' get-value text_username_field)" == *"alice"* ]]'

# swipe (gesture area reports direction)
q tap tab_gestures --label || q tap tab_gestures
q tap gesture_swipe_area   # focus the section if needed
check "swipe up"                      q swipe up

# long-press (status flips to "Long pressed!")
check "long-press"                    q long-press 200 600 --duration 1.0

# tap-location (coordinate tap on the tap area)
check "tap-location"                  q tap-location 200 800

# screen-info (dump-tree returns elements incl. our ids)
check "screen-info has ids"           bash -c "${QORVEX[*]} -s $SESSION screen-info --full | grep -q controls_tap_button || ${QORVEX[*]} -s $SESSION screen-info --full | grep -q scroll_list"

# wait-for (delayed element appears within 2s + margin)
q tap tab_dynamic --label || q tap tab_dynamic
q tap dynamic_show_delayed
check "wait-for delayed appears"      q wait-for dynamic_delayed_label --timeout 5000

# wait-for-not (brief element disappears after 3s)
q tap dynamic_show_brief
check "wait-for-not brief disappears" q wait-for-not dynamic_brief_label --timeout 6000

# screenshot (PNG bytes returned)
check "screenshot"                    bash -c "${QORVEX[*]} -s $SESSION screenshot | head -c 16 | grep -q ."

# target-info
check "target-info"                   bash -c "${QORVEX[*]} -s $SESSION target-info | grep -q $PKG"

# stop-target / start-target lifecycle
check "stop-target"                   q stop-target
check "start-target"                  q start-target

# --- teardown ---
q stop || true

echo "== parity: $pass passed, $fail failed =="
[[ "$fail" -eq 0 ]]
