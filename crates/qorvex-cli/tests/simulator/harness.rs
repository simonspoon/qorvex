use assert_cmd::Command;
use std::sync::OnceLock;

static HARNESS: OnceLock<SimulatorHarness> = OnceLock::new();

pub struct SimulatorHarness {
    pub session: String,
}

impl SimulatorHarness {
    fn init() -> Self {
        preflight_check();

        let session = format!("sim-test-{}", std::process::id());

        // Start server + session + agent
        qorvex_cmd()
            .args(["-s", &session, "start"])
            .timeout(std::time::Duration::from_secs(30))
            .assert()
            .success();

        // Set target to testapp
        qorvex_cmd()
            .args(["-s", &session, "set-target", "com.qorvex.testapp"])
            .timeout(std::time::Duration::from_secs(10))
            .assert()
            .success();

        // Wait a moment for agent to settle
        std::thread::sleep(std::time::Duration::from_secs(2));

        SimulatorHarness { session }
    }
}

impl Drop for SimulatorHarness {
    fn drop(&mut self) {
        let _ = qorvex_cmd()
            .args(["-s", &self.session, "stop"])
            .timeout(std::time::Duration::from_secs(10))
            .output();
    }
}

fn preflight_check() {
    // Verify a simulator is booted
    let output = qorvex_cmd()
        .arg("list-devices")
        .timeout(std::time::Duration::from_secs(10))
        .output()
        .expect("Failed to run qorvex list-devices");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Booted"),
        "No booted simulator found. Boot one with: xcrun simctl boot <UDID>"
    );

    // Terminate any running instance to get a clean state (dismiss keyboard, etc.)
    let _ = std::process::Command::new("xcrun")
        .args(["simctl", "terminate", "booted", "com.qorvex.testapp"])
        .output();
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Verify testapp is installed by launching it fresh
    let status = std::process::Command::new("xcrun")
        .args(["simctl", "launch", "booted", "com.qorvex.testapp"])
        .output()
        .expect("Failed to run xcrun simctl launch");
    assert!(
        status.status.success(),
        "qorvex-testapp not installed. Install with: make -C qorvex-testapp run"
    );
    std::thread::sleep(std::time::Duration::from_secs(1));
}

/// Get or initialize the shared harness.
pub fn harness() -> &'static SimulatorHarness {
    HARNESS.get_or_init(SimulatorHarness::init)
}

/// Build a Command for the qorvex binary.
pub fn qorvex_cmd() -> Command {
    Command::cargo_bin("qorvex").unwrap()
}

/// Run a qorvex CLI command with the shared session. Asserts success and returns stdout.
///
/// Example: `run(&["tap", "my-button"])` runs `qorvex -s <session> tap my-button`
pub fn run(args: &[&str]) -> String {
    let h = harness();
    let mut all_args: Vec<&str> = vec!["-s", &h.session];
    all_args.extend_from_slice(args);
    let assert = qorvex_cmd()
        .args(&all_args)
        .timeout(std::time::Duration::from_secs(15))
        .assert()
        .success();
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

/// Run a qorvex CLI command expecting failure. Returns stderr.
pub fn run_fail(args: &[&str]) -> String {
    let h = harness();
    let mut all_args: Vec<&str> = vec!["-s", &h.session];
    all_args.extend_from_slice(args);
    let assert = qorvex_cmd()
        .args(&all_args)
        .timeout(std::time::Duration::from_secs(15))
        .assert()
        .failure();
    let output = assert.get_output();
    let stderr = String::from_utf8(output.stderr.clone()).unwrap();
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    // Return whichever has content (some errors go to stdout)
    if stderr.is_empty() { stdout } else { stderr }
}

/// Run a qorvex command with JSON output, parse the result.
pub fn run_json(args: &[&str]) -> serde_json::Value {
    let h = harness();
    let mut all_args: Vec<&str> = vec!["-s", &h.session, "-f", "json"];
    all_args.extend_from_slice(args);
    let assert = qorvex_cmd()
        .args(&all_args)
        .timeout(std::time::Duration::from_secs(15))
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("Failed to parse JSON from stdout: {e}\nStdout was: {stdout}")
    })
}

/// Get the value of an element by accessibility ID. Returns trimmed stdout.
pub fn get_value(selector: &str) -> String {
    run(&["get-value", selector]).trim().to_string()
}

/// Navigate to a tab by tapping its label in the tab bar.
/// Swipes down first to dismiss any keyboard, then taps the tab by label.
/// Also scrolls to top to ensure consistent starting position.
pub fn go_to_tab(label: &str) {
    // Swipe down to dismiss keyboard (scrollDismissesKeyboard on text input tab)
    // and scroll toward top
    for _ in 0..5 {
        let _ = try_run(&["swipe", "down"]);
    }
    settle();
    // Tap the tab by its label — should work now that keyboard is dismissed
    run(&["tap", label, "--label"]);
    settle();
    // Scroll to top
    for _ in 0..3 {
        let _ = try_run(&["swipe", "down"]);
    }
    settle();
}

/// Swipe up to scroll content down (reveal lower elements).
pub fn scroll_down() {
    run(&["swipe", "up"]);
    settle();
}

/// Run a qorvex command, returning Ok(stdout) on success or Err(stderr) on failure.
/// Does not panic on failure.
pub fn try_run(args: &[&str]) -> Result<String, String> {
    let h = harness();
    let mut all_args: Vec<&str> = vec!["-s", &h.session];
    all_args.extend_from_slice(args);
    let output = qorvex_cmd()
        .args(&all_args)
        .timeout(std::time::Duration::from_secs(15))
        .output()
        .expect("Failed to execute qorvex command");
    if output.status.success() {
        Ok(String::from_utf8(output.stdout).unwrap())
    } else {
        Err(String::from_utf8(output.stderr).unwrap())
    }
}

/// Short sleep for animations to settle (500ms).
pub fn settle() {
    std::thread::sleep(std::time::Duration::from_millis(500));
}
