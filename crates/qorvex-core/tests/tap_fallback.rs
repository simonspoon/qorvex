//! `tap --fallback-frame` and the tap report (`method`, `x`, `y`,
//! `ui_changed`), driven by a scripted mock agent (no device).

mod common;

use common::connected_executor;

use qorvex_core::action::ActionType;
use qorvex_core::executor::ExecutionResult;
use qorvex_core::protocol::Response;

fn tap(fallback_frame: bool, report: bool) -> ActionType {
    ActionType::Tap {
        selector: "Tap Me".to_string(),
        by_label: true,
        element_type: None,
        timeout_ms: None,
        fallback_frame,
        report,
    }
}

fn tap_failed() -> Response {
    Response::Error {
        message: "element not hittable".to_string(),
    }
}

fn foreground() -> Response {
    Response::TargetInfo {
        json: r#"{"state": "running_foreground"}"#.to_string(),
    }
}

fn element(x: f64, y: f64, w: f64, h: f64) -> Response {
    Response::Element {
        json: format!(
            r#"{{"AXLabel": "Tap Me", "type": "Button", "hittable": false,
                "frame": {{"x": {}, "y": {}, "width": {}, "height": {}}}, "children": []}}"#,
            x, y, w, h
        ),
    }
}

fn tree(label: &str) -> Response {
    Response::Tree {
        json: format!(
            r#"[{{"AXUniqueId": "status", "AXLabel": "{}", "type": "StaticText",
                "frame": {{"x": 0.0, "y": 0.0, "width": 100.0, "height": 20.0}}, "children": []}}]"#,
            label
        ),
    }
}

fn data(result: &ExecutionResult) -> serde_json::Value {
    serde_json::from_str(result.data.as_deref().unwrap()).unwrap()
}

#[tokio::test]
async fn plain_tap_reports_element_method_without_extra_requests() {
    // heartbeat + tap only: a plain tap sends nothing else.
    let exec = connected_executor(vec![Response::Ok, Response::Ok]).await;
    let result = exec.execute(tap(false, false)).await;
    assert!(result.success, "{}", result.message);
    let d = data(&result);
    assert_eq!(d["method"], "element");
    assert!(d["x"].is_null() && d["y"].is_null());
    assert!(d["elapsed_ms"].is_u64());
    assert!(d.get("ui_changed").is_none());
}

#[tokio::test]
async fn report_gives_element_centre_and_ui_changed_true() {
    let exec = connected_executor(vec![
        Response::Ok,                      // heartbeat
        tree("Tapped: 0"),                 // before
        Response::Ok,                      // tap
        element(100.0, 200.0, 80.0, 40.0), // lookup after the tap
        tree("Tapped: 1"),                 // settle poll 1
        tree("Tapped: 1"),                 // settle poll 2 (equal)
    ])
    .await;
    let result = exec.execute(tap(false, true)).await;
    assert!(result.success, "{}", result.message);
    let d = data(&result);
    assert_eq!(d["method"], "element");
    assert_eq!((d["x"].as_i64(), d["y"].as_i64()), (Some(140), Some(220)));
    assert_eq!(d["ui_changed"], true);
}

#[tokio::test]
async fn report_ui_changed_false_when_tree_equal() {
    let exec = connected_executor(vec![
        Response::Ok,
        tree("Static"),
        Response::Ok,
        element(0.0, 0.0, 10.0, 10.0),
        tree("Static"),
        tree("Static"),
    ])
    .await;
    let result = exec.execute(tap(false, true)).await;
    assert!(result.success, "{}", result.message);
    assert_eq!(data(&result)["ui_changed"], false);
}

#[tokio::test]
async fn report_ui_changed_null_when_dump_errors() {
    let exec = connected_executor(vec![
        Response::Ok,
        Response::Error {
            message: "dump failed".to_string(),
        }, // before dump errors
        Response::Ok,
        element(0.0, 0.0, 10.0, 10.0),
        // No settle dumps: a failed "before" dump skips them.
    ])
    .await;
    let result = exec.execute(tap(false, true)).await;
    assert!(result.success, "tap result unaffected: {}", result.message);
    assert!(data(&result)["ui_changed"].is_null());
}

#[tokio::test]
async fn fallback_taps_frame_centre_when_tap_fails() {
    let exec = connected_executor(vec![
        Response::Ok,                      // heartbeat
        tap_failed(),                      // normal tap
        foreground(),                      // target check
        element(100.0, 200.0, 41.0, 40.0), // lookup
        Response::Ok,                      // tap_location
    ])
    .await;
    let result = exec.execute(tap(true, false)).await;
    assert!(result.success, "{}", result.message);
    let d = data(&result);
    assert_eq!(d["method"], "frame_centre");
    // 100 + 20.5 rounds to 121.
    assert_eq!((d["x"].as_i64(), d["y"].as_i64()), (Some(121), Some(220)));
}

#[tokio::test]
async fn fallback_centre_is_clamped_to_zero() {
    let exec = connected_executor(vec![
        Response::Ok,
        tap_failed(),
        foreground(),
        element(-30.0, 10.4, 10.0, 1.0), // centre (-25, 10.9)
        Response::Ok,
    ])
    .await;
    let result = exec.execute(tap(true, false)).await;
    assert!(result.success, "{}", result.message);
    let d = data(&result);
    assert_eq!((d["x"].as_i64(), d["y"].as_i64()), (Some(0), Some(11)));
}

#[tokio::test]
async fn without_flag_failure_is_unchanged() {
    // No responses after the failed tap: any lookup or tap_location would hang.
    let exec = connected_executor(vec![Response::Ok, tap_failed()]).await;
    let result = exec.execute(tap(false, false)).await;
    assert!(!result.success);
    assert!(
        result.message.starts_with("Timeout after"),
        "{}",
        result.message
    );
    assert!(!result.message.contains("fallback"), "{}", result.message);
}

#[tokio::test]
async fn fallback_not_found_fails_with_original_error() {
    let exec = connected_executor(vec![
        Response::Ok,
        tap_failed(),
        foreground(),
        Response::Element {
            json: "null".to_string(),
        },
    ])
    .await;
    let result = exec.execute(tap(true, false)).await;
    assert!(!result.success);
    assert!(
        result.message.starts_with("Timeout after"),
        "{}",
        result.message
    );
    assert!(
        result.message.contains("not hittable"),
        "{}",
        result.message
    );
    assert!(result.message.contains("fallback frame: element not found"));
}

#[tokio::test]
async fn fallback_zero_size_frame_fails() {
    let exec = connected_executor(vec![
        Response::Ok,
        tap_failed(),
        foreground(),
        element(10.0, 10.0, 0.0, 40.0),
    ])
    .await;
    let result = exec.execute(tap(true, false)).await;
    assert!(!result.success);
    assert!(
        result.message.contains("not hittable"),
        "{}",
        result.message
    );
    assert!(
        result.message.contains("no non-empty frame"),
        "{}",
        result.message
    );
}

#[tokio::test]
async fn fallback_tap_error_is_noted() {
    let exec = connected_executor(vec![
        Response::Ok,
        tap_failed(),
        foreground(),
        element(0.0, 0.0, 10.0, 10.0),
        Response::Error {
            message: "boom".to_string(),
        },
    ])
    .await;
    let result = exec.execute(tap(true, false)).await;
    assert!(!result.success);
    assert!(
        result
            .message
            .contains("fallback frame tap at (5,5) failed"),
        "{}",
        result.message
    );
}

#[tokio::test]
async fn fallback_skipped_when_target_not_running() {
    let exec = connected_executor(vec![
        Response::Ok,
        tap_failed(),
        Response::TargetInfo {
            json: r#"{"state": "not_running"}"#.to_string(),
        },
    ])
    .await;
    let result = exec.execute(tap(true, false)).await;
    assert!(!result.success);
    assert!(
        result
            .message
            .contains("skipped: target state is not_running"),
        "{}",
        result.message
    );
}
