use super::harness::{harness, run, run_fail, run_json};

/// The testapp the harness installs, used as a known-present bundle id.
const TESTAPP_BUNDLE_ID: &str = "com.qorvex.testapp";

#[test]
#[ignore]
fn test_list_apps_includes_testapp() {
    harness();
    let output = run(&["list-apps"]);
    assert!(
        output.contains(TESTAPP_BUNDLE_ID),
        "list-apps should list the installed testapp: {output}"
    );
}

#[test]
#[ignore]
fn test_list_apps_json_is_an_array() {
    harness();
    let json = run_json(&["list-apps"]);
    let apps = json
        .as_array()
        .unwrap_or_else(|| panic!("list-apps --format json should return an array: {json}"));
    assert!(
        apps.iter().any(|a| a["bundle_id"] == TESTAPP_BUNDLE_ID),
        "list-apps JSON should contain the testapp: {json}"
    );
}

#[test]
#[ignore]
fn test_app_container_returns_bundle_path() {
    harness();
    let output = run(&["app-container", TESTAPP_BUNDLE_ID]);
    let path = output.trim();
    assert!(
        path.ends_with(".app"),
        "app-container should return the .app bundle path by default: {path}"
    );
}

#[test]
#[ignore]
fn test_app_container_data_differs_from_bundle() {
    harness();
    let bundle = run(&["app-container", TESTAPP_BUNDLE_ID, "app"])
        .trim()
        .to_string();
    let data = run(&["app-container", TESTAPP_BUNDLE_ID, "data"])
        .trim()
        .to_string();
    assert_ne!(
        bundle, data,
        "the data container must not be the app bundle"
    );
    assert!(
        data.contains("/data/Containers/Data/Application/"),
        "data container should live under the device's Data/Application tree: {data}"
    );
}

#[test]
#[ignore]
fn test_app_container_unknown_bundle_fails() {
    harness();
    let output = run_fail(&["app-container", "com.qorvex.no-such-app-xyz-999"]);
    assert!(
        !output.is_empty(),
        "app-container on an uninstalled app should produce error output"
    );
}

#[test]
#[ignore]
fn test_uninstall_then_reinstall_testapp() {
    harness();
    let app_path = run(&["app-container", TESTAPP_BUNDLE_ID, "app"])
        .trim()
        .to_string();

    run(&["uninstall-app", TESTAPP_BUNDLE_ID]);
    assert!(
        !run(&["list-apps"]).contains(TESTAPP_BUNDLE_ID),
        "testapp should be gone after uninstall-app"
    );

    // Put it back: the rest of the suite drives this app.
    run(&["install-app", &app_path]);
    assert!(
        run(&["list-apps"]).contains(TESTAPP_BUNDLE_ID),
        "testapp should be back after install-app"
    );
}

#[test]
#[ignore]
fn test_install_app_missing_path_fails() {
    harness();
    let output = run_fail(&["install-app", "/nonexistent/NoSuch.app"]);
    assert!(
        !output.is_empty(),
        "install-app on a missing bundle should produce error output"
    );
}

/// The destructive pair takes no UDID — that is what keeps a session off other
/// sessions' simulators. Passing one must be a usage error, not a shutdown of
/// whatever was named. The successful path is deliberately not exercised: it
/// would tear down the simulator this suite is running on.
#[test]
#[ignore]
fn test_shutdown_device_rejects_a_udid_argument() {
    harness();
    let output = run_fail(&["shutdown-device", "00000000-0000-0000-0000-000000000000"]);
    assert!(
        output.contains("unexpected argument"),
        "shutdown-device must reject a UDID argument: {output}"
    );
}

#[test]
#[ignore]
fn test_delete_device_rejects_udid_arguments() {
    harness();
    let output = run_fail(&["delete-device", "00000000-0000-0000-0000-000000000000"]);
    assert!(
        output.contains("unexpected argument"),
        "delete-device must reject a UDID argument: {output}"
    );
}
