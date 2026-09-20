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

// --- wave 2: device state, logs and media -----------------------------------

#[test]
#[ignore]
fn test_set_appearance_round_trip() {
    harness();
    run(&["set-appearance", "dark"]);
    // Nothing reads the appearance back through simctl, so the assertion that
    // matters is that both directions are accepted; leave the device light.
    run(&["set-appearance", "light"]);
}

#[test]
#[ignore]
fn test_set_appearance_rejects_unknown_value() {
    harness();
    let output = run_fail(&["set-appearance", "sepia"]);
    assert!(
        output.contains("invalid value"),
        "set-appearance must reject anything but dark/light: {output}"
    );
}

#[test]
#[ignore]
fn test_set_content_size_accepts_accessibility_range() {
    harness();
    run(&["set-content-size", "accessibility-extra-extra-extra-large"]);
    run(&["set-content-size", "medium"]);
}

#[test]
#[ignore]
fn test_set_content_size_rejects_unknown_value() {
    harness();
    let output = run_fail(&["set-content-size", "enormous"]);
    assert!(
        output.contains("invalid value"),
        "set-content-size must reject a size outside the Dynamic Type range: {output}"
    );
}

#[test]
#[ignore]
fn test_device_log_returns_output() {
    harness();
    let output = run(&["device-log", "--last", "30s"]);
    assert!(
        !output.trim().is_empty(),
        "device-log should return recent log output"
    );
}

#[test]
#[ignore]
fn test_device_log_predicate_narrows_output() {
    harness();
    let all = run(&["device-log", "--last", "2m"]);
    let filtered = run(&[
        "device-log",
        "--last",
        "2m",
        "--predicate",
        "subsystem == \"com.qorvex.no-such-subsystem\"",
    ]);
    assert!(
        filtered.len() < all.len(),
        "a predicate matching nothing should return less than the unfiltered log"
    );
}

#[test]
#[ignore]
fn test_grant_permission_round_trip() {
    harness();
    run(&["grant-permission", "grant", "microphone", TESTAPP_BUNDLE_ID]);
    run(&[
        "grant-permission",
        "revoke",
        "microphone",
        TESTAPP_BUNDLE_ID,
    ]);
    run(&["grant-permission", "reset", "microphone", TESTAPP_BUNDLE_ID]);
}

#[test]
#[ignore]
fn test_grant_permission_rejects_unknown_service() {
    harness();
    let output = run_fail(&["grant-permission", "grant", "telepathy", TESTAPP_BUNDLE_ID]);
    assert!(
        output.contains("invalid value"),
        "grant-permission must reject a service simctl does not know: {output}"
    );
}

#[test]
#[ignore]
fn test_open_url_succeeds() {
    harness();
    run(&["open-url", "https://example.com"]);
}

#[test]
#[ignore]
fn test_add_media_missing_file_fails() {
    harness();
    let output = run_fail(&["add-media", "/nonexistent/no-such-image.png"]);
    assert!(
        !output.is_empty(),
        "add-media on a missing file should produce error output"
    );
}

#[test]
#[ignore]
fn test_add_media_requires_a_file() {
    harness();
    let output = run_fail(&["add-media"]);
    assert!(
        output.contains("required"),
        "add-media with no files must be a usage error: {output}"
    );
}

#[test]
#[ignore]
fn test_wait_for_boot_returns_on_a_booted_device() {
    harness();
    // The harness's device is already booted, so bootstatus should return
    // immediately rather than block.
    run(&["wait-for-boot"]);
}

/// `create-device` is the one device command that names a device, because no
/// device exists yet. The success path is deliberately not exercised: it would
/// leave a simulator behind on a machine other sessions share. What is checked
/// is that the three arguments are all required — a partial invocation must be
/// a usage error rather than a device created with defaults.
#[test]
#[ignore]
fn test_create_device_requires_name_type_and_runtime() {
    harness();
    let output = run_fail(&["create-device", "Aperture-test"]);
    assert!(
        output.contains("required"),
        "create-device must require a device type and runtime: {output}"
    );
}

#[test]
#[ignore]
fn test_wait_for_boot_rejects_a_udid_argument() {
    harness();
    let output = run_fail(&["wait-for-boot", "00000000-0000-0000-0000-000000000000"]);
    assert!(
        output.contains("unexpected argument"),
        "wait-for-boot must reject a UDID argument: {output}"
    );
}

#[test]
#[ignore]
fn test_device_log_rejects_a_udid_argument() {
    harness();
    let output = run_fail(&["device-log", "00000000-0000-0000-0000-000000000000"]);
    assert!(
        output.contains("unexpected argument"),
        "device-log must reject a UDID argument: {output}"
    );
}
