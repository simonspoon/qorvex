#![allow(deprecated)]

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
fn test_memory_info_accepts_tag_flag() {
    Command::cargo_bin("qorvex")
        .unwrap()
        .args(["memory-info", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--tag"));
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
    // list-devices shells out to simctl; skip when only CommandLineTools is
    // selected (xcrun cannot find simctl without full Xcode).
    let simctl_found = std::process::Command::new("xcrun")
        .args(["--find", "simctl"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !simctl_found {
        eprintln!("skipping test_list_devices_runs: `xcrun --find simctl` failed (no full Xcode)");
        return;
    }

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

/// Regression test for task 108: piping large stdout into a reader that exits
/// early (e.g. `qorvex ... | head`) must not panic with "failed printing to
/// stdout: Broken pipe". With `SIGPIPE` restored to `SIG_DFL` the process is
/// terminated by the signal and writes nothing alarming to stderr.
#[cfg(unix)]
#[test]
fn test_broken_pipe_does_not_panic() {
    use std::io::{Read, Write};
    use std::process::{Command as StdCommand, Stdio};

    // Build an input large enough that `convert`'s output overflows the OS pipe
    // buffer, so the writer is still flushing when the reader closes the pipe.
    let dir = std::env::temp_dir().join("qorvex_sigpipe_test");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("big.jsonl");
    {
        let mut f = std::fs::File::create(&input).unwrap();
        for i in 0..60_000 {
            writeln!(
                f,
                "{{\"id\":\"{i:08}-e5f6-7890-abcd-ef1234567890\",\"timestamp\":\"2026-02-20T10:00:00Z\",\"action\":{{\"type\":\"Tap\",\"selector\":\"button-{i}\",\"by_label\":false,\"element_type\":null}},\"result\":\"Success\",\"screenshot\":null,\"duration_ms\":45}}"
            )
            .unwrap();
        }
    }

    let bin = assert_cmd::cargo::cargo_bin("qorvex");
    let mut child = StdCommand::new(bin)
        .args(["convert", input.to_str().unwrap()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // Read a single small chunk, then drop stdout to close the read end early.
    {
        let mut stdout = child.stdout.take().unwrap();
        let mut buf = [0u8; 16];
        let _ = stdout.read(&mut buf);
    }

    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    let status = child.wait().unwrap();

    assert!(
        !stderr.contains("panicked") && !stderr.contains("Broken pipe"),
        "broken pipe should be handled silently, got stderr: {stderr}"
    );
    // The writer should die from SIGPIPE (signal 13), never a Rust panic (101).
    assert_ne!(status.code(), Some(101), "process panicked on broken pipe");
}

#[test]
fn test_screen_info_json_failure_is_json_object() {
    let home = std::env::temp_dir().join("qorvex_screen_info_json_home");
    std::fs::create_dir_all(&home).unwrap();
    let out = Command::cargo_bin("qorvex")
        .unwrap()
        .env("QORVEX_HOME", &home)
        .args(["-s", "nosuch", "-f", "json", "screen-info"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("Error:"))
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["success"], false);
    assert!(v["message"]
        .as_str()
        .unwrap()
        .starts_with("Connection error:"));
}

#[test]
fn test_screen_info_failure_without_json_has_empty_stdout() {
    let home = std::env::temp_dir().join("qorvex_screen_info_json_home");
    std::fs::create_dir_all(&home).unwrap();
    Command::cargo_bin("qorvex")
        .unwrap()
        .env("QORVEX_HOME", &home)
        .args(["-s", "nosuch", "screen-info"])
        .assert()
        .code(2)
        .stdout("")
        .stderr(predicate::str::contains("Error:"));
}

/// Short temp dir (macOS socket paths are limited to 104 bytes) holding a
/// `qorvex_hang.sock` that accepts connections and never answers.
fn hung_server(tag: &str) -> (PathBuf, std::thread::JoinHandle<()>) {
    use std::os::unix::net::UnixListener;
    let home = PathBuf::from(format!("/tmp/qvx_{}_{}", tag, std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let listener = UnixListener::bind(home.join("qorvex_hang.sock")).unwrap();
    let handle = std::thread::spawn(move || {
        // Keep accepted streams alive so the client never sees EOF.
        let mut held = Vec::new();
        for stream in listener.incoming().flatten() {
            held.push(stream);
        }
    });
    (home, handle)
}

fn run_hung(home: &PathBuf, envs: &[(&str, &str)], args: &[&str]) -> (std::process::Output, u128) {
    let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("qorvex"));
    cmd.env("QORVEX_HOME", home)
        .env_remove("QORVEX_TIMEOUT")
        .env_remove("QORVEX_COMMAND_TIMEOUT")
        .args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let start = std::time::Instant::now();
    let out = cmd.output().unwrap();
    (out, start.elapsed().as_millis())
}

#[test]
fn test_deadline_flag_kills_hung_command() {
    let (home, _h) = hung_server("flag");
    let (out, ms) = run_hung(&home, &[], &["-s", "hang", "status", "--timeout", "300"]);
    assert_eq!(out.status.code(), Some(124));
    assert!(ms < 2000, "took {ms}ms");
    assert!(out.stdout.is_empty());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("Timed out after 300ms: status"), "{err}");
    assert_eq!(err.lines().count(), 1, "{err}");
}

#[test]
fn test_deadline_env_kills_hung_command() {
    let (home, _h) = hung_server("env");
    let (out, ms) = run_hung(
        &home,
        &[("QORVEX_COMMAND_TIMEOUT", "300")],
        &["-s", "hang", "status"],
    );
    assert_eq!(out.status.code(), Some(124));
    assert!(ms < 2000, "took {ms}ms");
    assert!(String::from_utf8_lossy(&out.stderr).contains("Timed out after 300ms: status"));
}

#[test]
fn test_deadline_flag_beats_element_timeout_env() {
    let (home, _h) = hung_server("beat");
    let (out, _) = run_hung(
        &home,
        &[("QORVEX_TIMEOUT", "300")],
        &["-s", "hang", "status", "--timeout", "800"],
    );
    assert_eq!(out.status.code(), Some(124));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Timed out after 800ms: status"));
}

#[test]
fn test_deadline_screen_info_json_object() {
    let (home, _h) = hung_server("json");
    let (out, _) = run_hung(
        &home,
        &[],
        &[
            "-s",
            "hang",
            "-f",
            "json",
            "screen-info",
            "--timeout",
            "300",
        ],
    );
    assert_eq!(out.status.code(), Some(124));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["success"], false);
    assert!(v["message"]
        .as_str()
        .unwrap()
        .starts_with("Timed out after 300ms"));
}

#[test]
fn test_fast_failure_is_not_a_timeout() {
    let (home, _h) = hung_server("fast");
    let (out, _) = run_hung(&home, &[], &["-s", "nosuch", "status", "--timeout", "300"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("Timed out"));
}

#[test]
fn test_timeout_non_numeric_is_usage_error() {
    Command::cargo_bin("qorvex")
        .unwrap()
        .args(["status", "--timeout", "abc"])
        .assert()
        .code(2);
}

#[test]
fn test_command_timeout_env_non_numeric_is_usage_error() {
    Command::cargo_bin("qorvex")
        .unwrap()
        .env("QORVEX_COMMAND_TIMEOUT", "abc")
        .args(["status"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("QORVEX_COMMAND_TIMEOUT"));
}

/// Adds a bound-then-dropped socket (file stays, nothing listens) to `home`.
fn add_dead_socket(home: &std::path::Path, name: &str) -> PathBuf {
    let path = home.join(format!("qorvex_{name}.sock"));
    drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
    assert!(path.exists());
    path
}

#[test]
fn test_dead_session_error_and_reap() {
    let (home, _h) = hung_server("dead1");
    let dead = add_dead_socket(&home, "dead");
    let (out, _) = run_hung(&home, &[], &["-s", "dead", "status"]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("session 'dead' is dead"), "{err}");
    assert!(
        err.contains("no server listening); run `qorvex start`"),
        "{err}"
    );
    assert_eq!(err.lines().count(), 1, "{err}");
    assert!(err.starts_with("Error: Connection error: session 'dead' is dead"));
    assert!(!dead.exists());
}

#[test]
fn test_list_sessions_reaps_dead_keeps_hung() {
    let (home, _h) = hung_server("dead2");
    let dead = add_dead_socket(&home, "dead");
    let (out, ms) = run_hung(&home, &[], &["list-sessions"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(ms < 1000, "took {ms}ms");
    let so = String::from_utf8_lossy(&out.stdout);
    assert!(so.contains("hang"), "{so}");
    assert!(!so.contains("dead"), "{so}");
    assert!(!dead.exists());
    assert!(home.join("qorvex_hang.sock").exists());
}

#[test]
fn test_list_sessions_json_omits_dead() {
    let (home, _h) = hung_server("dead3");
    add_dead_socket(&home, "dead");
    let (out, _) = run_hung(&home, &[], &["-f", "json", "list-sessions"]);
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> = v["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["hang"]);
}

#[test]
fn test_missing_socket_message_unchanged() {
    let (home, _h) = hung_server("dead4");
    let (out, _) = run_hung(&home, &[], &["-s", "nosuch", "status"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("socket not found"));
}

#[test]
fn test_start_help_lists_wait_ready() {
    Command::cargo_bin("qorvex")
        .unwrap()
        .args(["start", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--wait-ready"));
}

#[test]
fn test_doctor_without_adb_fails() {
    // Empty env, PATH limited to system dirs; HOME points at a missing dir so the
    // result does not depend on a real ~/Library/Android/sdk.
    let home = "/nonexistent-qorvex-doctor-home";
    let run = |args: &[&str]| {
        Command::cargo_bin("qorvex")
            .unwrap()
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", home)
            .args(args)
            .assert()
            .code(1)
            .get_output()
            .clone()
    };

    let text = run(&["doctor"]);
    let stdout = String::from_utf8(text.stdout).unwrap();
    assert!(stdout.contains("[FAIL] adb"), "{stdout}");
    assert!(stdout.contains("[skip] devices"), "{stdout}");

    let json = run(&["-f", "json", "doctor"]);
    let v: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["checks"][0]["name"], "adb");
    assert_eq!(v["checks"][0]["status"], "fail");
}

#[test]
fn test_screen_info_all_with_full_is_usage_error() {
    Command::cargo_bin("qorvex")
        .unwrap()
        .args(["screen-info", "--all", "--full"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("cannot be used with"));
}

/// Short temp dir holding `qorvex_<name>.sock`, a server that answers every
/// request line with `reply`.
fn canned_server(name: &str, reply: &'static str) -> PathBuf {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    let home = PathBuf::from(format!("/tmp/qvx_{}_{}", name, std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let sock = home.join(format!("qorvex_{}.sock", name));
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(sock).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut writer = stream.try_clone().unwrap();
            for line in BufReader::new(stream).lines() {
                if line.is_err() {
                    break;
                }
                let _ = writeln!(writer, "{}", reply);
            }
        }
    });
    home
}

const STOPPED_WITH_EVENTS: &str = r#"{"type":"RecordStopped","file":"/tmp/v.mp4","bytes":42,"events_file":"/tmp/v.mp4.events.json","event_count":3}"#;
const STOPPED_WARNED: &str = r#"{"type":"RecordStopped","file":"/tmp/v.mp4","bytes":42,"events_warning":"could not write events file /tmp/v.mp4.events.json: Is a directory"}"#;

#[test]
fn test_record_stop_prints_events_line() {
    let home = canned_server("rsev", STOPPED_WITH_EVENTS);
    let (out, _) = run_hung(&home, &[], &["-s", "rsev", "record", "stop"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Recording saved: /tmp/v.mp4 (42 bytes)\nEvents: /tmp/v.mp4.events.json (3 events)\n"
    );
    let (out, _) = run_hung(&home, &[], &["-s", "rsev", "-f", "json", "record", "stop"]);
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["file"], "/tmp/v.mp4");
    assert_eq!(v["events_file"], "/tmp/v.mp4.events.json");
    assert_eq!(v["event_count"], 3);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn test_record_stop_sidecar_warning() {
    let home = canned_server("rswn", STOPPED_WARNED);
    let (out, _) = run_hung(&home, &[], &["-s", "rswn", "record", "stop"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Recording saved: /tmp/v.mp4 (42 bytes)\n"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Warning: could not write events file /tmp/v.mp4.events.json"),
        "{stderr}"
    );
    let (out, _) = run_hung(&home, &[], &["-s", "rswn", "-f", "json", "record", "stop"]);
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v.get("events_file").is_none());
    assert!(v.get("event_count").is_none());
    let _ = std::fs::remove_dir_all(&home);
}
