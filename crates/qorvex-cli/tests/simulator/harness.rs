use assert_cmd::Command;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Simulator the suite creates and drives when `QORVEX_TEST_SIM` is unset.
const TEST_SIM_NAME: &str = "qorvex-test";
const TESTAPP_BUNDLE_ID: &str = "com.qorvex.testapp";

static HARNESS: OnceLock<SimulatorHarness> = OnceLock::new();

/// Teardown state for the `atexit` hook below. `HARNESS` is a static and
/// statics are never dropped, so a `Drop` impl would not run at exit.
static TEARDOWN: OnceLock<Teardown> = OnceLock::new();

struct Teardown {
    session: String,
    udid: String,
    /// Whether this suite booted the simulator. A simulator that was already
    /// booted belongs to someone else and must be left running.
    booted_here: bool,
}

pub struct SimulatorHarness {
    pub session: String,
}

impl SimulatorHarness {
    fn init() -> Self {
        let session = format!("sim-test-{}", std::process::id());
        let udid = resolve_device();
        let booted_here = boot_device(&udid);

        // Register teardown as soon as we hold the simulator, so a panic in the
        // rest of setup — an unbuilt testapp, say — still stops the server and
        // releases the simulator instead of leaking it.
        let _ = TEARDOWN.set(Teardown {
            session: session.clone(),
            udid: udid.clone(),
            booted_here,
        });
        unsafe {
            libc::atexit(teardown);
        }

        install_testapp(&udid);
        launch_testapp(&udid);

        // Start server + session + agent, pinned to our own device so the
        // server never has to guess which simulator is meant.
        qorvex_cmd()
            .args(["-s", &session, "start", "--device", &udid])
            .timeout(std::time::Duration::from_secs(120))
            .assert()
            .success();

        // Set target to testapp
        qorvex_cmd()
            .args(["-s", &session, "set-target", TESTAPP_BUNDLE_ID])
            .timeout(std::time::Duration::from_secs(10))
            .assert()
            .success();

        // Wait a moment for agent to settle
        std::thread::sleep(std::time::Duration::from_secs(2));

        SimulatorHarness { session }
    }
}

/// Stop the session and release the simulator. Runs at process exit.
extern "C" fn teardown() {
    let Some(state) = TEARDOWN.get() else {
        return;
    };
    let _ = qorvex_cmd()
        .args(["-s", &state.session, "stop"])
        .timeout(std::time::Duration::from_secs(10))
        .output();
    if state.booted_here {
        simctl(&["shutdown", &state.udid]);
    }
}

/// Run `xcrun simctl` with `args` and return the raw output.
fn simctl(args: &[&str]) -> std::process::Output {
    std::process::Command::new("xcrun")
        .arg("simctl")
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("Failed to run xcrun simctl {}: {e}", args.join(" ")))
}

/// UDID of the simulator this suite drives: `QORVEX_TEST_SIM` (a name or a
/// UDID) if set, otherwise `qorvex-test`, created on first use. Resolving to a
/// UDID keeps every later call off `booted`, which is ambiguous once more than
/// one simulator is up.
fn resolve_device() -> String {
    let spec = std::env::var("QORVEX_TEST_SIM").unwrap_or_else(|_| TEST_SIM_NAME.to_string());
    if let Some(udid) = find_device(&spec) {
        return udid;
    }
    assert_eq!(
        spec, TEST_SIM_NAME,
        "QORVEX_TEST_SIM={spec} matches no available simulator"
    );
    create_test_sim();
    find_device(&spec).unwrap_or_else(|| panic!("Created {TEST_SIM_NAME} but cannot find it"))
}

/// Find an available simulator by UDID or by exact name. A name matching
/// several simulators is refused rather than guessed — the same reason the
/// server no longer guesses between several booted ones.
fn find_device(spec: &str) -> Option<String> {
    let output = simctl(&["list", "devices", "available", "-j"]);
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("Failed to parse simctl list devices JSON");
    let devices = json["devices"].as_object()?;
    let matches: Vec<&str> = devices
        .values()
        .filter_map(|v| v.as_array())
        .flatten()
        .filter(|d| d["udid"] == spec || d["name"] == spec)
        .filter_map(|d| d["udid"].as_str())
        .collect();
    assert!(
        matches.len() <= 1,
        "{} simulators are named {spec}: {}. Set QORVEX_TEST_SIM to one of these UDIDs",
        matches.len(),
        matches.join(", ")
    );
    matches.first().map(|udid| udid.to_string())
}

/// Create the `qorvex-test` simulator on the newest available iOS runtime.
fn create_test_sim() {
    let output = simctl(&["list", "runtimes", "-j"]);
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("Failed to parse simctl list runtimes JSON");
    let runtime = json["runtimes"]
        .as_array()
        .expect("simctl list runtimes returned no runtimes array")
        .iter()
        .filter(|r| r["isAvailable"] == true && r["platform"] == "iOS")
        .max_by_key(|r| {
            r["version"]
                .as_str()
                .unwrap_or_default()
                .split('.')
                .map(|p| p.parse::<u32>().unwrap_or(0))
                .collect::<Vec<_>>()
        })
        .expect("No available iOS runtime. Install one via Xcode > Settings > Components");
    // Take the device type from the runtime's own list so `create` cannot fail
    // on an unsupported pairing; the list is newest first.
    let device_type = runtime["supportedDeviceTypes"]
        .as_array()
        .expect("Runtime lists no supported device types")
        .iter()
        .find(|d| d["productFamily"] == "iPhone")
        .and_then(|d| d["identifier"].as_str())
        .expect("Runtime supports no iPhone device type");
    let runtime_id = runtime["identifier"]
        .as_str()
        .expect("Runtime has no identifier");

    let output = simctl(&["create", TEST_SIM_NAME, device_type, runtime_id]);
    assert!(
        output.status.success(),
        "Failed to create {TEST_SIM_NAME} simulator: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Boot `udid` and wait for it to finish booting. Returns whether this call did
/// the booting — an already-booted simulator must not be shut down afterwards.
fn boot_device(udid: &str) -> bool {
    let output = simctl(&["boot", udid]);
    let booted_here = output.status.success();
    if !booted_here {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("current state: Booted"),
            "Failed to boot {udid}: {stderr}"
        );
    }
    let output = simctl(&["bootstatus", udid, "-b"]);
    assert!(
        output.status.success(),
        "Simulator {udid} did not finish booting: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    booted_here
}

/// Install the testapp build product on `udid`.
fn install_testapp(udid: &str) {
    let app = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../qorvex-testapp/.build/Build/Products/Debug-iphonesimulator/QorvexTestApp.app");
    assert!(
        app.is_dir(),
        "qorvex-testapp is not built. Build it with: make -C qorvex-testapp build"
    );
    let output = simctl(&["install", udid, &app.to_string_lossy()]);
    assert!(
        output.status.success(),
        "Failed to install qorvex-testapp on {udid}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Relaunch the testapp so each run starts from a clean UI state (no keyboard
/// up, no leftover navigation).
fn launch_testapp(udid: &str) {
    simctl(&["terminate", udid, TESTAPP_BUNDLE_ID]);
    std::thread::sleep(std::time::Duration::from_secs(1));
    let output = simctl(&["launch", udid, TESTAPP_BUNDLE_ID]);
    assert!(
        output.status.success(),
        "Failed to launch qorvex-testapp on {udid}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::thread::sleep(std::time::Duration::from_secs(1));
}

/// Get or initialize the shared harness.
pub fn harness() -> &'static SimulatorHarness {
    HARNESS.get_or_init(SimulatorHarness::init)
}

/// Build a Command for the qorvex binary.
#[allow(deprecated)]
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
    if stderr.is_empty() {
        stdout
    } else {
        stderr
    }
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
    serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("Failed to parse JSON from stdout: {e}\nStdout was: {stdout}"))
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
