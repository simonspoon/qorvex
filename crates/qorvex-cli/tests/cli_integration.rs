use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

#[test]
fn test_help_exits_zero() {
    Command::cargo_bin("qorvex")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("qorvex"));
}

#[test]
fn test_convert_basic_session() {
    let fixture = fixture_path("basic_session.jsonl");

    let assert = Command::cargo_bin("qorvex")
        .unwrap()
        .args(["convert", fixture.to_str().unwrap()])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    // Should produce a valid bash script header
    assert!(stdout.contains("#!/usr/bin/env bash"));
    assert!(stdout.contains("set -euo pipefail"));

    // Should contain converted commands for actionable entries
    assert!(stdout.contains("qorvex tap login-button"));
    assert!(stdout.contains("qorvex send-keys"));
    assert!(stdout.contains("qorvex screenshot"));
    assert!(stdout.contains("qorvex swipe up"));

    // StartSession and EndSession should be skipped (no output for them)
    assert!(!stdout.contains("StartSession"));
    assert!(!stdout.contains("EndSession"));
}

#[test]
fn test_convert_error_session() {
    let fixture = fixture_path("error_session.jsonl");

    let assert = Command::cargo_bin("qorvex")
        .unwrap()
        .args(["convert", fixture.to_str().unwrap()])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    // Should still produce output even with failure results
    // (converter maps actions to commands regardless of result)
    assert!(stdout.contains("#!/usr/bin/env bash"));

    // Tap with failure result should still appear as a command
    assert!(stdout.contains("qorvex tap missing-element"));

    // LogComment should become a bash comment
    assert!(stdout.contains("# Retrying after failure"));

    // TapLocation should be converted
    assert!(stdout.contains("qorvex tap-location 150 300"));

    // SendKeys with special characters should be shell-escaped
    assert!(stdout.contains("qorvex send-keys"));
}

#[test]
fn test_convert_nonexistent_file() {
    Command::cargo_bin("qorvex")
        .unwrap()
        .args(["convert", "nonexistent_file_that_does_not_exist.jsonl"])
        .assert()
        .failure();
}

#[test]
fn test_list_devices_runs() {
    // list-devices should not panic; it may output an empty list
    // if no simulators are available, but should exit cleanly
    Command::cargo_bin("qorvex")
        .unwrap()
        .arg("list-devices")
        .assert()
        .success();
}

#[test]
fn test_unknown_subcommand() {
    Command::cargo_bin("qorvex")
        .unwrap()
        .arg("totally-fake-command")
        .assert()
        .failure()
        .stderr(predicate::str::contains("error"));
}
