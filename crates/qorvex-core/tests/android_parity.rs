//! Android ↔ iOS structural parity harness (story #90).
//!
//! This drives the **full `ActionType` matrix** through the production
//! [`ActionExecutor::execute`] entry point against both the iOS **`AgentDriver`**
//! and the **`AndroidDriver`** backends, feeding *both* the same canned agent
//! responses over a loopback mock agent (same `protocol.rs` wire format), and
//! asserts the two `ExecutionResult`s are equivalent. It is a structural /
//! request-shape test plus a matrix-completeness check — it runs in CI with no
//! emulator. It deliberately does **not** prove on-device behavioral parity;
//! that is what the deferred live script (below) is for.
//!
//! # What this actually proves (mock-based, runs in CI)
//!
//! Because both executors are fed the *same* canned responses through the *same*
//! mock agent, the per-action assertions prove only that, **given identical
//! agent responses**, the Android executor path consumes them and shapes the
//! `ExecutionResult` (`success`, `message`, `data`/`screenshot`) the same way the
//! iOS path does. Concretely it covers:
//!   - the executor wiring and result-shaping is backend-agnostic for every
//!     `ActionType` (the two paths differ only in which `Driver` impl they hold);
//!   - the `AndroidDriver` issues the expected protocol request sequence and
//!     correctly maps each canned `Response` into the user-facing result;
//!   - error propagation has the same shape on both backends;
//!   - the matrix-completeness test (exhaustive `match` over `ActionType`) forces
//!     every variant to be classified, so the matrix can't silently fall behind a
//!     newly added action.
//!
//! # What this does NOT prove (deferred to a live emulator)
//!
//! Both backends are fed the *same* mocked responses, so this does **not**
//! exercise the real on-device pipeline: the Kotlin agent, `adb forward`, and the
//! UiAutomator → ADR-1 node mapping. In particular the ADR-1 node-mapping
//! divergence (short element_type, FQCN role, hittable, bare resource id) lives
//! in the Kotlin agent and produces the responses this harness merely hand-feeds;
//! a regression there would not be caught here. That behavioral round-trip
//! against the real sample app is the single deferred step, driven by
//! `scripts/android_parity_live.sh` (documented in
//! `qorvex-testapp-android/README.md`) once an emulator is available. This
//! environment has no emulator/device, so that rung is deferred.
//!
//! Run this (static) harness with:
//! ```text
//! cargo test -p qorvex-core --features test-support --test android_parity
//! ```
#![cfg(feature = "test-support")]

mod common;

use common::{connected_android_executor, connected_executor};

use qorvex_core::action::ActionType;
use qorvex_core::executor::ExecutionResult;
use qorvex_core::protocol::Response;

/// Normalize an `ExecutionResult::data` payload for cross-backend comparison by
/// stripping the wall-clock `elapsed_ms` field. That field is measured
/// independently on each backend (see `executor.rs`), so byte-for-byte equality
/// of the raw JSON was inherently racy — comparing it directly made the tap
/// parity assertions fail ~1-in-3 runs. Everything *structural* in `data`
/// (e.g. the `frame` returned by `wait_for`) is preserved and still compared.
/// A `None` payload stays `None`; a non-object or non-JSON payload is compared
/// verbatim (wrapped as a JSON string) so unexpected shapes still mismatch loudly.
fn normalized_data(data: &Option<String>) -> Option<serde_json::Value> {
    data.as_ref()
        .map(|raw| match serde_json::from_str::<serde_json::Value>(raw) {
            Ok(serde_json::Value::Object(mut map)) => {
                map.remove("elapsed_ms");
                serde_json::Value::Object(map)
            }
            Ok(other) => other,
            Err(_) => serde_json::Value::String(raw.clone()),
        })
}

/// Assert two `ExecutionResult`s are equivalent across the iOS and Android
/// backends. The executor is backend-agnostic, so identical agent responses
/// must yield identical user-facing results.
fn assert_equivalent(label: &str, ios: &ExecutionResult, android: &ExecutionResult) {
    assert_eq!(
        ios.success, android.success,
        "{label}: success mismatch (iOS={}, Android={})\n iOS msg: {}\n And msg: {}",
        ios.success, android.success, ios.message, android.message
    );
    assert_eq!(ios.message, android.message, "{label}: message mismatch");
    assert_eq!(
        normalized_data(&ios.data),
        normalized_data(&android.data),
        "{label}: data mismatch (elapsed_ms stripped)\n iOS: {:?}\n And: {:?}",
        ios.data,
        android.data
    );
    assert_eq!(
        ios.screenshot, android.screenshot,
        "{label}: screenshot mismatch"
    );
}

/// Run one `ActionType` against both backends with the same agent responses and
/// assert equivalence. `ios_responses` includes the leading heartbeat that
/// `AgentDriver::connect` consumes; the Android executor is injected
/// already-connected, so it gets the same tail without that heartbeat.
async fn run_parity(label: &str, action: ActionType, ios_responses: Vec<Response>) {
    // Android path: same responses minus the leading heartbeat (the injected
    // client is already connected, so no connect-time heartbeat is read).
    let android_responses: Vec<Response> = ios_responses.iter().skip(1).cloned().collect();

    let ios_exec = connected_executor(ios_responses).await;
    let android_exec = connected_android_executor(android_responses).await;

    let ios_result = ios_exec.execute(action.clone()).await;
    let android_result = android_exec.execute(action).await;

    assert_equivalent(label, &ios_result, &android_result);
}

// ===========================================================================
// The full ActionType matrix — one parity assertion per action.
// ===========================================================================

// --- Tap (by id) ---
#[tokio::test]
async fn parity_tap_by_id() {
    run_parity(
        "tap",
        ActionType::Tap {
            selector: "controls_tap_button".to_string(),
            by_label: false,
            element_type: None,
            timeout_ms: None,
        },
        vec![Response::Ok, Response::Ok],
    )
    .await;
}

// --- Tap (by label) ---
#[tokio::test]
async fn parity_tap_by_label() {
    run_parity(
        "tap-by-label",
        ActionType::Tap {
            selector: "Tap Me".to_string(),
            by_label: true,
            element_type: None,
            timeout_ms: None,
        },
        vec![Response::Ok, Response::Ok],
    )
    .await;
}

// --- Tap (with element_type filter) ---
#[tokio::test]
async fn parity_tap_with_type() {
    run_parity(
        "tap-with-type",
        ActionType::Tap {
            selector: "Submit".to_string(),
            by_label: true,
            element_type: Some("Button".to_string()),
            timeout_ms: None,
        },
        vec![Response::Ok, Response::Ok],
    )
    .await;
}

// --- TapLocation (coordinate tap) ---
#[tokio::test]
async fn parity_tap_location() {
    run_parity(
        "tap-location",
        ActionType::TapLocation { x: 120, y: 240 },
        vec![Response::Ok, Response::Ok],
    )
    .await;
}

// --- Swipe ---
#[tokio::test]
async fn parity_swipe() {
    run_parity(
        "swipe",
        ActionType::Swipe {
            direction: "up".to_string(),
        },
        vec![Response::Ok, Response::Ok],
    )
    .await;
}

// --- LongPress ---
#[tokio::test]
async fn parity_long_press() {
    run_parity(
        "long-press",
        ActionType::LongPress {
            x: 150,
            y: 300,
            duration: 1.0,
        },
        vec![Response::Ok, Response::Ok],
    )
    .await;
}

// --- SendKeys (type text) ---
#[tokio::test]
async fn parity_send_keys() {
    run_parity(
        "send-keys",
        ActionType::SendKeys {
            text: "hello@example.com".to_string(),
        },
        vec![Response::Ok, Response::Ok],
    )
    .await;
}

// --- GetScreenInfo (dump-tree) ---
#[tokio::test]
async fn parity_get_screen_info() {
    // ADR-1 Android JSON: short element_type, FQCN role, hittable bool.
    let tree = r#"[{
        "AXUniqueId": "controls_tap_button",
        "AXLabel": "Tap Me",
        "type": "Button",
        "role": "android.widget.Button",
        "hittable": true,
        "frame": {"x": 0.0, "y": 100.0, "width": 200.0, "height": 48.0},
        "children": []
    }]"#;
    run_parity(
        "get-screen-info",
        ActionType::GetScreenInfo,
        vec![
            Response::Ok,
            Response::Tree {
                json: tree.to_string(),
            },
        ],
    )
    .await;
}

// --- GetValue (by id) ---
#[tokio::test]
async fn parity_get_value() {
    run_parity(
        "get-value",
        ActionType::GetValue {
            selector: "text_username_field".to_string(),
            by_label: false,
            element_type: None,
            timeout_ms: None,
        },
        vec![
            Response::Ok,
            Response::Value {
                value: Some("alice".to_string()),
            },
        ],
    )
    .await;
}

// --- GetValue (None / empty) ---
#[tokio::test]
async fn parity_get_value_none() {
    run_parity(
        "get-value-none",
        ActionType::GetValue {
            selector: "text_email_field".to_string(),
            by_label: false,
            element_type: None,
            timeout_ms: None,
        },
        vec![Response::Ok, Response::Value { value: None }],
    )
    .await;
}

// --- GetScreenshot ---
#[tokio::test]
async fn parity_screenshot() {
    let png = vec![0x89u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    run_parity(
        "screenshot",
        ActionType::GetScreenshot,
        vec![Response::Ok, Response::Screenshot { data: png }],
    )
    .await;
}

// --- WaitFor (element appears) — fast path (require_stable=false, one find) ---
#[tokio::test]
async fn parity_wait_for() {
    // A hittable element present on the first poll → fast-path success.
    let element = r#"{
        "AXUniqueId": "dynamic_delayed_label",
        "AXLabel": "I appeared!",
        "type": "TextView",
        "hittable": true,
        "frame": {"x": 0.0, "y": 0.0, "width": 100.0, "height": 20.0},
        "children": []
    }"#;
    run_parity(
        "wait-for",
        ActionType::WaitFor {
            selector: "dynamic_delayed_label".to_string(),
            by_label: false,
            element_type: None,
            timeout_ms: 5_000,
            require_stable: false,
        },
        vec![
            Response::Ok,
            Response::Element {
                json: element.to_string(),
            },
        ],
    )
    .await;
}

// --- WaitForNot (element absent) ---
#[tokio::test]
async fn parity_wait_for_not() {
    // Element already absent (null) → immediate success on both backends.
    run_parity(
        "wait-for-not",
        ActionType::WaitForNot {
            selector: "dynamic_brief_label".to_string(),
            by_label: false,
            element_type: None,
            timeout_ms: 5_000,
        },
        vec![
            Response::Ok,
            Response::Element {
                json: "null".to_string(),
            },
        ],
    )
    .await;
}

// --- SetTarget ---
#[tokio::test]
async fn parity_set_target() {
    run_parity(
        "set-target",
        ActionType::SetTarget {
            bundle_id: "com.qorvex.testapp".to_string(),
        },
        vec![Response::Ok, Response::Ok],
    )
    .await;
}

// --- StartTarget (launch) ---
#[tokio::test]
async fn parity_start_target() {
    run_parity(
        "start-target",
        ActionType::StartTarget,
        vec![Response::Ok, Response::Ok],
    )
    .await;
}

// --- StopTarget ---
#[tokio::test]
async fn parity_stop_target() {
    run_parity(
        "stop-target",
        ActionType::StopTarget,
        vec![Response::Ok, Response::Ok],
    )
    .await;
}

// --- GetTargetInfo ---
#[tokio::test]
async fn parity_get_target_info() {
    let info = r#"{
        "bundle_id": "com.qorvex.testapp",
        "display_name": "Qorvex TestApp",
        "version": "1.0",
        "build": "1",
        "state": "running"
    }"#;
    run_parity(
        "get-target-info",
        ActionType::GetTargetInfo,
        vec![
            Response::Ok,
            Response::TargetInfo {
                json: info.to_string(),
            },
        ],
    )
    .await;
}

// --- LogComment (driver-independent, must behave identically) ---
#[tokio::test]
async fn parity_log_comment() {
    run_parity(
        "comment",
        ActionType::LogComment {
            message: "parity check".to_string(),
        },
        // LogComment never touches the driver; only the heartbeat is consumed on
        // the iOS side at connect time.
        vec![Response::Ok],
    )
    .await;
}

// --- Error propagation parity (element-not-found) ---
#[tokio::test]
async fn parity_error_propagation() {
    run_parity(
        "tap-error",
        ActionType::Tap {
            selector: "missing".to_string(),
            by_label: false,
            element_type: None,
            timeout_ms: None,
        },
        vec![
            Response::Ok,
            Response::Error {
                message: "element not found".to_string(),
            },
        ],
    )
    .await;
}

// ===========================================================================
// Matrix completeness — every ActionType variant must be covered above.
// ===========================================================================

/// Exhaustive `match` over `ActionType` so the compiler forces this test to be
/// updated if a new action is ever added, and a name-set assertion documenting
/// which actions the parity matrix above exercises end-to-end vs which are
/// session/REPL-control actions with no agent round-trip.
#[test]
fn matrix_covers_every_action_type() {
    // The set of action `name()`s exercised by the parity tests above (each maps
    // to a backend driver call whose result we compared across iOS/Android).
    let covered_via_driver = [
        "tap",
        "tap_location",
        "swipe",
        "long_press",
        "send_keys",
        "get_screen_info",
        "get_value",
        "get_screenshot",
        "wait_for",
        "wait_for_not",
        "set_target",
        "start_target",
        "stop_target",
        "get_target_info",
        "log_comment",
    ];

    // Session/REPL control actions: no agent protocol round-trip, so they are
    // not part of the per-action driver matrix (they behave identically by
    // construction — the executor handles them backend-agnostically).
    let session_control = ["start_session", "end_session", "quit"];

    // Exhaustive match: adding a new ActionType variant fails to compile until
    // it is classified here, guaranteeing the matrix stays complete.
    fn classify(a: &ActionType) -> &'static str {
        match a {
            ActionType::Tap { .. }
            | ActionType::TapLocation { .. }
            | ActionType::Swipe { .. }
            | ActionType::LongPress { .. }
            | ActionType::SendKeys { .. }
            | ActionType::GetScreenInfo
            | ActionType::GetValue { .. }
            | ActionType::GetScreenshot
            | ActionType::WaitFor { .. }
            | ActionType::WaitForNot { .. }
            | ActionType::SetTarget { .. }
            | ActionType::StartTarget
            | ActionType::StopTarget
            | ActionType::GetTargetInfo
            | ActionType::LogComment { .. } => "driver",
            ActionType::StartSession | ActionType::EndSession | ActionType::Quit => "session",
        }
    }

    // Sanity: a representative of each classification routes as expected.
    assert_eq!(
        classify(&ActionType::GetScreenshot),
        "driver",
        "screenshot must be a driver action"
    );
    assert_eq!(
        classify(&ActionType::Quit),
        "session",
        "quit must be a session-control action"
    );

    // Total action count is the sum of the two disjoint classes.
    assert_eq!(
        covered_via_driver.len() + session_control.len(),
        18,
        "ActionType matrix size changed — update the parity matrix and this list"
    );
}
