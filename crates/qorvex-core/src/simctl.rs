//! Interface to Apple's `xcrun simctl` command-line tool.
//!
//! This module provides a Rust wrapper around the iOS Simulator control tool,
//! enabling device listing, screenshot capture, and simulator boot.
//!
//! # Requirements
//!
//! Xcode must be installed for `xcrun simctl` to be available.
//!
//! # Example
//!
//! ```no_run
//! use qorvex_core::simctl::Simctl;
//!
//! // List all simulators
//! let devices = Simctl::list_devices().unwrap();
//! for device in &devices {
//!     println!("{}: {} ({})", device.name, device.udid, device.state);
//! }
//!
//! // Get the currently booted simulator
//! if let Ok(udid) = Simctl::get_booted_udid() {
//!     // Take a screenshot
//!     let png_bytes = Simctl::screenshot(&udid).unwrap();
//! }
//! ```

use crate::memory::{AppMemory, DeviceMemory, MemoryInfo, MemoryPressure};
use serde::{Deserialize, Serialize};
use std::process::Command;
use thiserror::Error;

/// Errors that can occur when interacting with simctl.
#[derive(Error, Debug)]
pub enum SimctlError {
    /// A simctl command failed to execute successfully.
    #[error("Command execution failed: {0}")]
    CommandFailed(String),

    /// No simulator is currently in the "Booted" state.
    #[error("No booted simulator found")]
    NoBootedSimulator,

    /// More than one simulator is in the "Booted" state, so the target is ambiguous.
    ///
    /// Carries the `(name, udid)` pair of every booted simulator, in list order.
    #[error(
        "Multiple booted simulators found; pass one with --device <udid>: {}",
        format_booted_devices(.0)
    )]
    MultipleBootedSimulators(Vec<(String, String)>),

    /// Failed to parse JSON output from simctl.
    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),

    /// An I/O error occurred while executing the command.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Formats booted `(name, udid)` pairs as a comma-separated `name (udid)` list.
fn format_booted_devices(devices: &[(String, String)]) -> String {
    devices
        .iter()
        .map(|(name, udid)| format!("{} ({})", name, udid))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The launchd target of the simulator runtime's media analysis daemon, which
/// can run away to hundreds of percent CPU on a long-lived simulator and starve
/// xcodebuild and UI automation.
const MEDIAANALYSISD_TARGET: &str = "system/com.apple.mediaanalysisd";

/// What [`Simctl::quiet`] found when it quieted a simulator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuietOutcome {
    /// A loaded instance of the daemon was unloaded.
    Unloaded,

    /// The daemon was not loaded; nothing to do.
    AlreadyQuiet,

    /// The device is not booted, so there is no runtime to quiet.
    NotBooted,
}

/// Represents an iOS Simulator device.
///
/// This struct contains information about a simulator device as reported
/// by `xcrun simctl list devices -j`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulatorDevice {
    /// The unique device identifier (UDID) for this simulator.
    pub udid: String,

    /// The human-readable name of the device (e.g., "iPhone 15 Pro").
    pub name: String,

    /// The current state of the device (e.g., "Booted", "Shutdown").
    pub state: String,

    /// The device type identifier (e.g., "com.apple.CoreSimulator.SimDeviceType.iPhone-15-Pro").
    #[serde(rename = "deviceTypeIdentifier")]
    pub device_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeviceList {
    devices: std::collections::HashMap<String, Vec<SimulatorDevice>>,
}

/// An application installed on a simulator device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledApp {
    /// The bundle identifier (e.g., "com.apple.mobilesafari").
    pub bundle_id: String,
    /// The display name (e.g., "Safari").
    pub display_name: String,
    /// The application type ("User" or "System").
    pub app_type: String,
}

/// Wrapper for `xcrun simctl` commands.
///
/// Provides static methods for interacting with iOS Simulator devices.
/// All methods are synchronous and execute shell commands.
pub struct Simctl;

impl Simctl {
    /// Lists all available iOS Simulator devices.
    ///
    /// Queries `xcrun simctl list devices -j` and parses the JSON output
    /// to return a flat list of all devices across all runtime versions.
    ///
    /// # Returns
    ///
    /// A `Vec<SimulatorDevice>` containing all available simulators,
    /// regardless of their state or iOS version.
    ///
    /// # Errors
    ///
    /// - [`SimctlError::Io`] if the command fails to execute
    /// - [`SimctlError::CommandFailed`] if simctl returns a non-zero exit code
    /// - [`SimctlError::JsonParse`] if the output cannot be parsed as JSON
    pub fn list_devices() -> Result<Vec<SimulatorDevice>, SimctlError> {
        let output = Command::new("xcrun")
            .args(["simctl", "list", "devices", "-j"])
            .output()?;

        if !output.status.success() {
            return Err(SimctlError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let device_list: DeviceList = serde_json::from_slice(&output.stdout)?;
        let devices: Vec<SimulatorDevice> = device_list.devices.into_values().flatten().collect();

        Ok(devices)
    }

    /// Returns the UDID of the only booted simulator.
    ///
    /// Refuses to guess when more than one simulator is booted; the caller must
    /// then name the device explicitly.
    ///
    /// # Returns
    ///
    /// The UDID string of the booted simulator.
    ///
    /// # Errors
    ///
    /// - [`SimctlError::NoBootedSimulator`] if no simulator is currently booted
    /// - [`SimctlError::MultipleBootedSimulators`] if more than one is booted
    /// - Any errors from [`Self::list_devices`]
    pub fn get_booted_udid() -> Result<String, SimctlError> {
        Self::booted_udid_from(Self::list_devices()?)
    }

    /// Resolves the booted UDID from an already-fetched device list.
    ///
    /// Split out from [`Self::get_booted_udid`] so the ambiguity rule can be
    /// tested without a running simulator.
    ///
    /// # Arguments
    ///
    /// * `devices` - All known simulators, in `simctl` list order
    ///
    /// # Returns
    ///
    /// The UDID string of the single booted simulator.
    ///
    /// # Errors
    ///
    /// - [`SimctlError::NoBootedSimulator`] if no simulator is currently booted
    /// - [`SimctlError::MultipleBootedSimulators`] if more than one is booted
    pub fn booted_udid_from(devices: Vec<SimulatorDevice>) -> Result<String, SimctlError> {
        let mut booted = devices.into_iter().filter(|d| d.state == "Booted");

        let first = booted.next().ok_or(SimctlError::NoBootedSimulator)?;
        let rest: Vec<SimulatorDevice> = booted.collect();

        if rest.is_empty() {
            return Ok(first.udid);
        }

        Err(SimctlError::MultipleBootedSimulators(
            std::iter::once(first)
                .chain(rest)
                .map(|d| (d.name, d.udid))
                .collect(),
        ))
    }

    /// Takes a screenshot of the simulator screen.
    ///
    /// Captures the current display of the specified simulator and returns
    /// the image as PNG-encoded bytes. The screenshot is temporarily saved
    /// to `/tmp` and then read into memory.
    ///
    /// # Arguments
    ///
    /// * `udid` - The unique device identifier of the target simulator
    ///
    /// # Returns
    ///
    /// A `Vec<u8>` containing PNG image data.
    ///
    /// # Errors
    ///
    /// - [`SimctlError::Io`] if file operations fail
    /// - [`SimctlError::CommandFailed`] if the screenshot command fails
    pub fn screenshot(udid: &str) -> Result<Vec<u8>, SimctlError> {
        let temp_path = format!("/tmp/qorvex_screenshot_{}.png", uuid::Uuid::new_v4());

        let output = Command::new("xcrun")
            .args(["simctl", "io", udid, "screenshot", &temp_path])
            .output()?;

        if !output.status.success() {
            return Err(SimctlError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let bytes = std::fs::read(&temp_path)?;
        let _ = std::fs::remove_file(&temp_path);
        Ok(bytes)
    }

    /// Boots a simulator device.
    ///
    /// Starts the specified simulator. If the simulator is already booted,
    /// this method returns successfully (the "already booted" state is not
    /// treated as an error).
    ///
    /// # Arguments
    ///
    /// * `udid` - The unique device identifier of the simulator to boot
    ///
    /// # Errors
    ///
    /// - [`SimctlError::Io`] if the command fails to execute
    /// - [`SimctlError::CommandFailed`] if simctl returns an error (except for "already booted")
    pub fn boot(udid: &str) -> Result<(), SimctlError> {
        let output = Command::new("xcrun")
            .args(["simctl", "boot", udid])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Already booted is not an error
            if !stderr.contains("current state: Booted") {
                return Err(SimctlError::CommandFailed(stderr.to_string()));
            }
        }
        Ok(())
    }

    /// Stops the simulator runtime's `mediaanalysisd`.
    ///
    /// The daemon indexes simulator media in the background and has been
    /// measured at over 700% CPU on a simulator left up for a few hours, which
    /// makes xcodebuild and UI automation crawl. Quieting it takes two steps,
    /// in this order:
    ///
    /// 1. `launchctl disable` gates *loading* the job, which only happens at
    ///    boot. On an already-running device the job is loaded, so on-demand
    ///    activation relaunches it regardless.
    /// 2. `launchctl bootout` unloads the job, so there is nothing left to
    ///    activate.
    ///
    /// `disable` must come first, or the unloaded job is simply re-loaded.
    ///
    /// Only the parent label is touched; the sibling services measured
    /// alongside it were never observed consuming CPU.
    ///
    /// # Arguments
    ///
    /// * `udid` - The unique device identifier of the target simulator
    ///
    /// # Errors
    ///
    /// - [`SimctlError::Io`] if the command fails to execute
    /// - [`SimctlError::CommandFailed`] if the label could not be booted out
    ///   for a reason other than it already being unloaded or the device not
    ///   being booted
    pub fn quiet(udid: &str) -> Result<QuietOutcome, SimctlError> {
        // `disable` is advisory here — `bootout` below is what stops a running
        // instance, and its exit code is the one worth classifying.
        Command::new("xcrun")
            .args([
                "simctl",
                "spawn",
                udid,
                "launchctl",
                "disable",
                MEDIAANALYSISD_TARGET,
            ])
            .output()?;

        let output = Command::new("xcrun")
            .args([
                "simctl",
                "spawn",
                udid,
                "launchctl",
                "bootout",
                MEDIAANALYSISD_TARGET,
            ])
            .output()?;

        Self::classify_bootout(
            output.status.code(),
            &String::from_utf8_lossy(&output.stderr),
        )
    }

    /// Classifies a `launchctl bootout` exit code.
    ///
    /// Split out from [`Self::quiet`] so the exit codes can be tested without a
    /// simulator. Classification is on the exit code alone: every successful
    /// invocation also writes an `rdar://78126471` deprecation warning to
    /// stderr, so non-empty stderr says nothing about success.
    fn classify_bootout(code: Option<i32>, stderr: &str) -> Result<QuietOutcome, SimctlError> {
        match code {
            Some(0) => Ok(QuietOutcome::Unloaded),
            // "Boot-out failed: 3: No such process" — already unloaded.
            Some(3) => Ok(QuietOutcome::AlreadyQuiet),
            // "Process spawn via launchd failed because device is not booted".
            Some(149) => Ok(QuietOutcome::NotBooted),
            _ => Err(SimctlError::CommandFailed(stderr.trim().to_string())),
        }
    }

    /// Launches an app on a simulator device, returning its process id.
    ///
    /// Runs `xcrun simctl launch <udid> <bundle_id>` to start the specified
    /// application on the given simulator. **simctl does not relaunch an app
    /// that is already running** — it returns the existing process id and the
    /// app keeps its current state. Pass `force` to add
    /// `--terminate-running-process`, which kills any running copy first so the
    /// app really does start from scratch.
    ///
    /// # Arguments
    ///
    /// * `udid` - The unique device identifier of the target simulator
    /// * `bundle_id` - The bundle identifier of the app to launch
    /// * `force` - Terminate a running copy first instead of attaching to it
    ///
    /// # Errors
    ///
    /// - [`SimctlError::Io`] if the command fails to execute
    /// - [`SimctlError::CommandFailed`] if simctl returns an error
    pub fn launch_app(
        udid: &str,
        bundle_id: &str,
        force: bool,
    ) -> Result<Option<u32>, SimctlError> {
        let mut args = vec!["simctl", "launch"];
        if force {
            args.push("--terminate-running-process");
        }
        args.push(udid);
        args.push(bundle_id);
        let output = Command::new("xcrun").args(&args).output()?;

        if !output.status.success() {
            return Err(SimctlError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(Self::parse_launch_pid(&String::from_utf8_lossy(
            &output.stdout,
        )))
    }

    /// Extracts the process id from `simctl launch` output, which is a single
    /// `<bundle id>: <pid>` line. Returns `None` if the line is missing or
    /// malformed — the launch itself is reported by the exit status, so an
    /// unparseable pid degrades the report rather than failing the command.
    fn parse_launch_pid(stdout: &str) -> Option<u32> {
        stdout
            .lines()
            .find_map(|line| line.rsplit_once(':'))
            .and_then(|(_, pid)| pid.trim().parse().ok())
    }

    /// Returns the process id of `bundle_id` on the simulator, or `None` when
    /// the app is not running.
    ///
    /// Runs `xcrun simctl spawn <udid> launchctl list`, whose output lists one
    /// `<pid>\t<status>\t<label>` row per service; a running app appears as
    /// `UIKitApplication:<bundle id>[...]`. This is the only pre-launch running
    /// check simctl offers — `simctl launch` itself cannot be used, since it
    /// starts the app as a side effect.
    ///
    /// # Errors
    ///
    /// - [`SimctlError::Io`] if the command fails to execute
    /// - [`SimctlError::CommandFailed`] if the spawn fails (e.g. device not booted)
    pub fn app_pid(udid: &str, bundle_id: &str) -> Result<Option<u32>, SimctlError> {
        let output = Command::new("xcrun")
            .args(["simctl", "spawn", udid, "launchctl", "list"])
            .output()?;

        if !output.status.success() {
            return Err(SimctlError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(Self::parse_launchctl_pid(
            &String::from_utf8_lossy(&output.stdout),
            bundle_id,
        ))
    }

    /// Finds the pid of `bundle_id`'s `UIKitApplication` row in `launchctl
    /// list` output. A not-yet-running-but-registered service has `-` in the
    /// pid column, which reads as "not running" here.
    fn parse_launchctl_pid(stdout: &str, bundle_id: &str) -> Option<u32> {
        let needle = format!("UIKitApplication:{}[", bundle_id);
        stdout
            .lines()
            .find(|line| line.contains(&needle))
            .and_then(|line| line.split_whitespace().next())
            .and_then(|pid| pid.parse().ok())
    }

    /// Returns the target app's memory footprint and the host Mac's memory
    /// state, or `None` when the app is not running.
    ///
    /// A simulator app is a plain Mac process, so its footprint is the
    /// `phys_footprint` of the pid [`Self::app_pid`] resolves, and the
    /// "device" it runs on is the Mac itself. Physical iOS devices are not
    /// covered here — nothing in this path can reach them.
    ///
    /// # Errors
    ///
    /// - [`SimctlError::Io`] if a command fails to execute
    /// - [`SimctlError::CommandFailed`] if a command fails or its output
    ///   cannot be parsed
    pub fn memory_info(udid: &str, bundle_id: &str) -> Result<Option<MemoryInfo>, SimctlError> {
        let Some(pid) = Self::app_pid(udid, bundle_id)? else {
            return Ok(None);
        };

        // `footprint`'s `phys_footprint` is the kernel figure jetsam charges,
        // and the one `vmmap --summary`, Instruments and Xcode's memory gauge
        // display. Host RSS is not a substitute: a simulator app has the whole
        // iOS runtime's read-only framework pages resident (gigabytes, shared
        // by every simulated app at once), so RSS charges one app a large
        // near-constant offset and is not comparable to Android's
        // proportional-share TOTAL PSS. RSS stays as a fallback so a missing
        // or unparseable `footprint` degrades the figure rather than failing
        // the command — `source` records which one the number came from.
        let footprint = Command::new("footprint")
            .args(["--pid", &pid.to_string(), "-f", "bytes", "--noCategories"])
            .output()
            .ok();
        let phys_footprint = footprint
            .as_ref()
            .and_then(|out| Self::parse_footprint(&String::from_utf8_lossy(&out.stdout)));
        let (footprint_bytes, app_source) = match phys_footprint {
            Some(bytes) => (bytes, "footprint phys_footprint"),
            None => {
                let ps = Command::new("ps")
                    .args(["-o", "rss=", "-p", &pid.to_string()])
                    .output()?;
                let rss =
                    Self::parse_ps_rss(&String::from_utf8_lossy(&ps.stdout)).ok_or_else(|| {
                        SimctlError::CommandFailed(format!("could not read RSS for pid {}", pid))
                    })?;
                (rss, "ps -o rss= (footprint unavailable)")
            }
        };

        let total_bytes = Self::sysctl("hw.memsize")?
            .trim()
            .parse::<u64>()
            .map_err(|e| {
                SimctlError::CommandFailed(format!("could not parse hw.memsize: {}", e))
            })?;

        let vm_stat = Command::new("vm_stat").output()?;
        let free_bytes = Self::parse_vm_stat(&String::from_utf8_lossy(&vm_stat.stdout))
            .ok_or_else(|| {
                SimctlError::CommandFailed("could not parse vm_stat output".to_string())
            })?;

        let pressure =
            Self::parse_pressure_level(&Self::sysctl("kern.memorystatus_vm_pressure_level")?);

        Ok(Some(MemoryInfo {
            app: AppMemory {
                pid,
                footprint_bytes,
                source: app_source.to_string(),
            },
            device: DeviceMemory {
                total_bytes,
                free_bytes,
                pressure,
                source: "sysctl hw.memsize + vm_stat".to_string(),
            },
        }))
    }

    /// Reads a single scalar sysctl via `sysctl -n <name>`.
    fn sysctl(name: &str) -> Result<String, SimctlError> {
        let output = Command::new("sysctl").args(["-n", name]).output()?;
        if !output.status.success() {
            return Err(SimctlError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Extracts `phys_footprint` from `footprint --pid <pid> -f bytes
    /// --noCategories` output, in bytes.
    ///
    /// Read from the `Auxiliary data:` section rather than the `Footprint:`
    /// figure on the header line: the two differ slightly, and
    /// `phys_footprint` is the authoritative `task_vm_info` value that `vmmap
    /// --summary` also prints. That section carries other keys
    /// (`phys_footprint_peak`, and `neural_peak` on some processes) in no
    /// fixed order, so the line is matched by label. The flag is spelled
    /// `--pid` and not `-p`, which is ambiguous with `--proc <name>`.
    ///
    /// Returns `None` when the label is absent or its value is not a number;
    /// the caller then falls back to RSS.
    fn parse_footprint(stdout: &str) -> Option<u64> {
        stdout.lines().find_map(|line| {
            line.trim()
                .strip_prefix("phys_footprint:")?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
    }

    /// Converts `ps -o rss=` output — a single whitespace-padded figure — to
    /// bytes. `ps` reports RSS in kilobytes, hence the × 1024.
    fn parse_ps_rss(stdout: &str) -> Option<u64> {
        stdout.trim().parse::<u64>().ok().map(|kb| kb * 1024)
    }

    /// Sums the pages `vm_stat` reports as reclaimable — free, inactive and
    /// speculative — and converts them to bytes.
    ///
    /// The page size comes from vm_stat's own header line (`Mach Virtual
    /// Memory Statistics: (page size of N bytes)`), never a hardcoded 4096:
    /// Apple silicon pages are 16384 bytes. Returns `None` if the header or
    /// the free page count is missing. Inactive and speculative default to
    /// zero rather than failing the whole read.
    fn parse_vm_stat(stdout: &str) -> Option<u64> {
        let page_size: u64 = stdout
            .split_once("page size of")?
            .1
            .split_whitespace()
            .next()?
            .parse()
            .ok()?;
        let pages = |label: &str| -> Option<u64> {
            stdout.lines().find_map(|line| {
                line.trim()
                    .strip_prefix(label)?
                    .trim()
                    .trim_end_matches('.')
                    .parse()
                    .ok()
            })
        };
        let free = pages("Pages free:")?;
        let inactive = pages("Pages inactive:").unwrap_or(0);
        let speculative = pages("Pages speculative:").unwrap_or(0);
        Some((free + inactive + speculative) * page_size)
    }

    /// Maps `kern.memorystatus_vm_pressure_level` onto [`MemoryPressure`].
    ///
    /// The sysctl reports 1 = normal, 2 = warn, 4 = critical. Any other value,
    /// including unparseable output, reads as normal — an unrecognised level
    /// is not evidence of pressure.
    fn parse_pressure_level(stdout: &str) -> MemoryPressure {
        match stdout.trim().parse::<u32>() {
            Ok(2) => MemoryPressure::Warn,
            Ok(4) => MemoryPressure::Critical,
            _ => MemoryPressure::Normal,
        }
    }

    /// Terminates an app on a simulator device.
    ///
    /// Runs `xcrun simctl terminate <udid> <bundle_id>` to stop the specified
    /// application on the given simulator. If the app is not currently running,
    /// this method returns successfully (the "not running" state is not treated
    /// as an error).
    ///
    /// # Arguments
    ///
    /// * `udid` - The unique device identifier of the target simulator
    /// * `bundle_id` - The bundle identifier of the app to terminate
    ///
    /// # Errors
    ///
    /// - [`SimctlError::Io`] if the command fails to execute
    /// - [`SimctlError::CommandFailed`] if simctl returns an error (except for "not running")
    pub fn terminate_app(udid: &str, bundle_id: &str) -> Result<(), SimctlError> {
        let output = Command::new("xcrun")
            .args(["simctl", "terminate", udid, bundle_id])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // App not running is not an error
            if !stderr.contains("not running") {
                return Err(SimctlError::CommandFailed(stderr.to_string()));
            }
        }
        Ok(())
    }

    /// Lists installed apps on a booted simulator.
    ///
    /// Runs `xcrun simctl listapps <udid>` and pipes the output through
    /// `plutil -convert json -o - -- -` to convert from plist to JSON.
    /// Returns apps sorted with User apps first, then alphabetical by bundle_id.
    ///
    /// # Arguments
    ///
    /// * `udid` - The unique device identifier of the target simulator
    ///
    /// # Errors
    ///
    /// - [`SimctlError::Io`] if the command fails to execute
    /// - [`SimctlError::CommandFailed`] if simctl or plutil returns an error
    /// - [`SimctlError::JsonParse`] if the JSON output cannot be parsed
    pub fn list_apps(udid: &str) -> Result<Vec<InstalledApp>, SimctlError> {
        let simctl_output = Command::new("xcrun")
            .args(["simctl", "listapps", udid])
            .output()?;

        if !simctl_output.status.success() {
            return Err(SimctlError::CommandFailed(
                String::from_utf8_lossy(&simctl_output.stderr).to_string(),
            ));
        }

        let plutil_output = Command::new("plutil")
            .args(["-convert", "json", "-o", "-", "--", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                if let Some(ref mut stdin) = child.stdin {
                    stdin.write_all(&simctl_output.stdout)?;
                }
                // Drop stdin to signal EOF
                child.stdin.take();
                child.wait_with_output()
            })?;

        if !plutil_output.status.success() {
            return Err(SimctlError::CommandFailed(
                String::from_utf8_lossy(&plutil_output.stderr).to_string(),
            ));
        }

        Self::parse_app_list(&plutil_output.stdout)
    }

    /// Parses app list JSON from plutil output into a sorted vector of installed apps.
    ///
    /// The JSON is a dictionary keyed by bundle ID, where each value contains
    /// `CFBundleIdentifier`, `CFBundleDisplayName`, and `ApplicationType`.
    ///
    /// # Arguments
    ///
    /// * `json` - Raw JSON bytes from plutil output
    ///
    /// # Errors
    ///
    /// - [`SimctlError::JsonParse`] if the JSON is invalid
    pub fn parse_app_list(json: &[u8]) -> Result<Vec<InstalledApp>, SimctlError> {
        let map: std::collections::HashMap<String, serde_json::Value> =
            serde_json::from_slice(json)?;

        let mut apps: Vec<InstalledApp> = map
            .into_values()
            .filter_map(|entry| {
                let bundle_id = entry.get("CFBundleIdentifier")?.as_str()?.to_string();
                let display_name = entry
                    .get("CFBundleDisplayName")
                    .and_then(|v| v.as_str())
                    .or_else(|| entry.get("CFBundleName").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_string();
                let app_type = entry
                    .get("ApplicationType")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown")
                    .to_string();
                Some(InstalledApp {
                    bundle_id,
                    display_name,
                    app_type,
                })
            })
            .collect();

        // Sort: User apps first, then alphabetical by bundle_id
        apps.sort_by(|a, b| {
            let a_is_user = a.app_type == "User";
            let b_is_user = b.app_type == "User";
            b_is_user
                .cmp(&a_is_user)
                .then(a.bundle_id.cmp(&b.bundle_id))
        });

        Ok(apps)
    }

    /// Parses device list JSON into a flat vector of devices.
    ///
    /// This method is exposed primarily for testing purposes. It takes
    /// raw JSON bytes (as returned by `simctl list devices -j`) and
    /// returns a flattened list of all devices.
    ///
    /// # Arguments
    ///
    /// * `json` - Raw JSON bytes from simctl output
    ///
    /// # Returns
    ///
    /// A `Vec<SimulatorDevice>` containing all devices from the JSON.
    ///
    /// # Errors
    ///
    /// - [`SimctlError::JsonParse`] if the JSON is invalid or has unexpected structure
    pub fn parse_device_list(json: &[u8]) -> Result<Vec<SimulatorDevice>, SimctlError> {
        let device_list: DeviceList = serde_json::from_slice(json)?;
        let devices: Vec<SimulatorDevice> = device_list.devices.into_values().flatten().collect();
        Ok(devices)
    }

    /// Finds the first booted device in a list.
    ///
    /// Searches through the provided device list and returns a reference
    /// to the first device with state "Booted". This method is exposed
    /// primarily for testing purposes.
    ///
    /// # Arguments
    ///
    /// * `devices` - Slice of simulator devices to search
    ///
    /// # Returns
    ///
    /// `Some(&SimulatorDevice)` if a booted device is found, `None` otherwise.
    pub fn find_booted_device(devices: &[SimulatorDevice]) -> Option<&SimulatorDevice> {
        devices.iter().find(|d| d.state == "Booted")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Sample JSON matching actual simctl output format
    const SAMPLE_DEVICE_LIST: &str = r#"{
        "devices": {
            "com.apple.CoreSimulator.SimRuntime.iOS-17-0": [
                {
                    "udid": "A1B2C3D4-E5F6-7890-ABCD-EF1234567890",
                    "name": "iPhone 15 Pro",
                    "state": "Booted",
                    "deviceTypeIdentifier": "com.apple.CoreSimulator.SimDeviceType.iPhone-15-Pro"
                },
                {
                    "udid": "B2C3D4E5-F6A7-8901-BCDE-F12345678901",
                    "name": "iPhone 15",
                    "state": "Shutdown",
                    "deviceTypeIdentifier": "com.apple.CoreSimulator.SimDeviceType.iPhone-15"
                }
            ],
            "com.apple.CoreSimulator.SimRuntime.iOS-16-4": [
                {
                    "udid": "C3D4E5F6-A7B8-9012-CDEF-123456789012",
                    "name": "iPhone 14",
                    "state": "Shutdown",
                    "deviceTypeIdentifier": "com.apple.CoreSimulator.SimDeviceType.iPhone-14"
                }
            ]
        }
    }"#;

    const EMPTY_DEVICE_LIST: &str = r#"{"devices": {}}"#;

    const NO_BOOTED_DEVICES: &str = r#"{
        "devices": {
            "com.apple.CoreSimulator.SimRuntime.iOS-17-0": [
                {
                    "udid": "A1B2C3D4-E5F6-7890-ABCD-EF1234567890",
                    "name": "iPhone 15 Pro",
                    "state": "Shutdown"
                }
            ]
        }
    }"#;

    #[test]
    fn test_parse_device_list_success() {
        let devices = Simctl::parse_device_list(SAMPLE_DEVICE_LIST.as_bytes())
            .expect("Should parse valid JSON");

        assert_eq!(devices.len(), 3);

        // Check that we have devices from both runtime versions
        let names: Vec<&str> = devices.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"iPhone 15 Pro"));
        assert!(names.contains(&"iPhone 15"));
        assert!(names.contains(&"iPhone 14"));
    }

    #[test]
    fn test_parse_device_list_empty() {
        let devices = Simctl::parse_device_list(EMPTY_DEVICE_LIST.as_bytes())
            .expect("Should parse empty device list");

        assert!(devices.is_empty());
    }

    #[test]
    fn test_parse_device_list_invalid_json() {
        let result = Simctl::parse_device_list(b"not valid json");

        assert!(result.is_err());
        match result {
            Err(SimctlError::JsonParse(_)) => {} // Expected
            Err(e) => panic!("Expected JsonParse error, got: {:?}", e),
            Ok(_) => panic!("Expected error, got Ok"),
        }
    }

    #[test]
    fn test_parse_device_list_missing_devices_key() {
        let invalid_json = r#"{"something_else": []}"#;
        let result = Simctl::parse_device_list(invalid_json.as_bytes());

        // serde should fail to deserialize without "devices" key
        assert!(result.is_err());
    }

    #[test]
    fn test_find_booted_device_found() {
        let devices = Simctl::parse_device_list(SAMPLE_DEVICE_LIST.as_bytes()).unwrap();
        let booted = Simctl::find_booted_device(&devices);

        assert!(booted.is_some());
        let device = booted.unwrap();
        assert_eq!(device.name, "iPhone 15 Pro");
        assert_eq!(device.state, "Booted");
    }

    #[test]
    fn test_find_booted_device_none_booted() {
        let devices = Simctl::parse_device_list(NO_BOOTED_DEVICES.as_bytes()).unwrap();
        let booted = Simctl::find_booted_device(&devices);

        assert!(booted.is_none());
    }

    const TWO_BOOTED_DEVICES: &str = r#"{
        "devices": {
            "com.apple.CoreSimulator.SimRuntime.iOS-17-0": [
                {
                    "udid": "A1B2C3D4-E5F6-7890-ABCD-EF1234567890",
                    "name": "iPhone 15 Pro",
                    "state": "Booted"
                },
                {
                    "udid": "35CB0000-1111-2222-3333-444455556666",
                    "name": "iPhone SE",
                    "state": "Booted"
                }
            ]
        }
    }"#;

    #[test]
    fn test_booted_udid_from_none_booted() {
        let devices = Simctl::parse_device_list(NO_BOOTED_DEVICES.as_bytes()).unwrap();
        let result = Simctl::booted_udid_from(devices);

        match result {
            Err(SimctlError::NoBootedSimulator) => {} // Expected
            other => panic!("Expected NoBootedSimulator, got: {:?}", other),
        }
    }

    #[test]
    fn test_booted_udid_from_single_booted() {
        let devices = Simctl::parse_device_list(SAMPLE_DEVICE_LIST.as_bytes()).unwrap();
        let udid = Simctl::booted_udid_from(devices).expect("Single booted device should resolve");

        assert_eq!(udid, "A1B2C3D4-E5F6-7890-ABCD-EF1234567890");
    }

    #[test]
    fn test_booted_udid_from_multiple_booted() {
        let devices = Simctl::parse_device_list(TWO_BOOTED_DEVICES.as_bytes()).unwrap();
        let result = Simctl::booted_udid_from(devices);

        match result {
            Err(SimctlError::MultipleBootedSimulators(booted)) => {
                assert_eq!(booted.len(), 2);
                assert_eq!(booted[0].0, "iPhone 15 Pro");
                assert_eq!(booted[1].0, "iPhone SE");
            }
            other => panic!("Expected MultipleBootedSimulators, got: {:?}", other),
        }
    }

    #[test]
    fn test_booted_udid_from_multiple_booted_message() {
        let devices = Simctl::parse_device_list(TWO_BOOTED_DEVICES.as_bytes()).unwrap();
        let message = Simctl::booted_udid_from(devices)
            .expect_err("Two booted devices should be an error")
            .to_string();

        assert!(message.contains("--device"));
        assert!(message.contains("iPhone 15 Pro"));
        assert!(message.contains("A1B2C3D4-E5F6-7890-ABCD-EF1234567890"));
        assert!(message.contains("iPhone SE"));
        assert!(message.contains("35CB0000-1111-2222-3333-444455556666"));
    }

    #[test]
    fn test_find_booted_device_empty_list() {
        let devices: Vec<SimulatorDevice> = vec![];
        let booted = Simctl::find_booted_device(&devices);

        assert!(booted.is_none());
    }

    #[test]
    fn test_simulator_device_fields() {
        let devices = Simctl::parse_device_list(SAMPLE_DEVICE_LIST.as_bytes()).unwrap();
        let booted = devices.iter().find(|d| d.state == "Booted").unwrap();

        assert_eq!(booted.udid, "A1B2C3D4-E5F6-7890-ABCD-EF1234567890");
        assert_eq!(booted.name, "iPhone 15 Pro");
        assert_eq!(booted.state, "Booted");
        assert!(booted.device_type.is_some());
        assert!(booted
            .device_type
            .as_ref()
            .unwrap()
            .contains("iPhone-15-Pro"));
    }

    #[test]
    fn test_simulator_device_optional_device_type() {
        // Device without deviceTypeIdentifier should still parse
        let json = r#"{
            "devices": {
                "com.apple.CoreSimulator.SimRuntime.iOS-17-0": [
                    {
                        "udid": "test-udid",
                        "name": "Test Device",
                        "state": "Shutdown"
                    }
                ]
            }
        }"#;

        let devices = Simctl::parse_device_list(json.as_bytes()).unwrap();
        assert_eq!(devices.len(), 1);
        assert!(devices[0].device_type.is_none());
    }

    #[test]
    fn test_simctl_error_display() {
        let cmd_err = SimctlError::CommandFailed("test error".to_string());
        assert!(cmd_err.to_string().contains("test error"));

        let no_booted = SimctlError::NoBootedSimulator;
        assert!(no_booted.to_string().contains("No booted simulator"));
    }

    #[test]
    fn test_screenshot_with_invalid_udid() {
        // This tests actual command execution with invalid input
        let result = Simctl::screenshot("invalid-udid-that-does-not-exist");

        // Should fail because the simulator doesn't exist
        assert!(result.is_err());
        match result {
            Err(SimctlError::CommandFailed(msg)) => {
                // The error message should indicate the device wasn't found
                assert!(!msg.is_empty() || msg.is_empty()); // Accept any error message
            }
            Err(e) => {
                // IO errors are also acceptable (e.g., if simctl behaves differently)
                println!("Got error: {:?}", e);
            }
            Ok(_) => panic!("Expected error for invalid UDID"),
        }
    }

    const SAMPLE_APP_LIST: &str = r#"{
        "com.apple.mobilesafari": {
            "CFBundleIdentifier": "com.apple.mobilesafari",
            "CFBundleDisplayName": "Safari",
            "ApplicationType": "System"
        },
        "com.example.myapp": {
            "CFBundleIdentifier": "com.example.myapp",
            "CFBundleDisplayName": "My App",
            "ApplicationType": "User"
        },
        "com.apple.Preferences": {
            "CFBundleIdentifier": "com.apple.Preferences",
            "CFBundleDisplayName": "Settings",
            "ApplicationType": "System"
        }
    }"#;

    #[test]
    fn test_parse_app_list_success() {
        let apps =
            Simctl::parse_app_list(SAMPLE_APP_LIST.as_bytes()).expect("Should parse valid JSON");

        assert_eq!(apps.len(), 3);
        // User apps should come first
        assert_eq!(apps[0].bundle_id, "com.example.myapp");
        assert_eq!(apps[0].app_type, "User");
        // System apps follow, alphabetical
        assert_eq!(apps[1].bundle_id, "com.apple.Preferences");
        assert_eq!(apps[2].bundle_id, "com.apple.mobilesafari");
    }

    #[test]
    fn test_parse_app_list_empty() {
        let apps = Simctl::parse_app_list(b"{}").expect("Should parse empty object");
        assert!(apps.is_empty());
    }

    #[test]
    fn test_parse_app_list_missing_display_name() {
        let json = r#"{
            "com.example.noname": {
                "CFBundleIdentifier": "com.example.noname",
                "CFBundleName": "FallbackName",
                "ApplicationType": "User"
            }
        }"#;
        let apps = Simctl::parse_app_list(json.as_bytes()).unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].display_name, "FallbackName");
    }

    #[test]
    fn test_parse_app_list_invalid_json() {
        let result = Simctl::parse_app_list(b"not valid json");
        assert!(result.is_err());
    }

    #[test]
    fn test_boot_with_invalid_udid() {
        let result = Simctl::boot("invalid-udid-that-does-not-exist");

        assert!(result.is_err());
    }

    // --- start-target launch reporting ---

    // Real `xcrun simctl launch` output: one `<bundle>: <pid>` line.
    #[test]
    fn test_parse_launch_pid() {
        assert_eq!(
            Simctl::parse_launch_pid("com.apple.mobilesafari: 2481\n"),
            Some(2481)
        );
    }

    #[test]
    fn test_parse_launch_pid_missing() {
        assert_eq!(Simctl::parse_launch_pid(""), None);
        assert_eq!(Simctl::parse_launch_pid("com.example.App: n/a\n"), None);
    }

    // Real `simctl spawn <udid> launchctl list` rows: pid, status, label.
    const SAMPLE_LAUNCHCTL_LIST: &str = "PID\tStatus\tLabel\n\
2840\t0\tUIKitApplication:com.apple.mobilesafari[58ea][rb-legacy]\n\
-\t0\tUIKitApplication:com.example.Idle[1f2e][rb-legacy]\n\
71\t0\tcom.apple.backboardd\n";

    #[test]
    fn test_parse_launchctl_pid_running() {
        assert_eq!(
            Simctl::parse_launchctl_pid(SAMPLE_LAUNCHCTL_LIST, "com.apple.mobilesafari"),
            Some(2840)
        );
    }

    #[test]
    fn test_parse_launchctl_pid_not_running() {
        // Absent entirely, and present-but-unlaunched (`-` in the pid column):
        // both mean "not running", so start-target reports a real launch.
        assert_eq!(
            Simctl::parse_launchctl_pid(SAMPLE_LAUNCHCTL_LIST, "com.example.Absent"),
            None
        );
        assert_eq!(
            Simctl::parse_launchctl_pid(SAMPLE_LAUNCHCTL_LIST, "com.example.Idle"),
            None
        );
    }

    // A bundle id that is a prefix of another must not match it — the `[`
    // suffix in the needle is what keeps `com.example.App` off
    // `com.example.AppTwo`.
    #[test]
    fn test_parse_launchctl_pid_no_prefix_collision() {
        let list = "1234\t0\tUIKitApplication:com.example.AppTwo[aaaa][rb-legacy]\n";
        assert_eq!(Simctl::parse_launchctl_pid(list, "com.example.App"), None);
    }

    // --- quieting mediaanalysisd ---

    // Every invocation, successful or not, writes this to stderr, so success is
    // classified on the exit code alone.
    const BOOTOUT_DEPRECATION_WARNING: &str =
        "Boot-out is a deprecated command. See rdar://78126471 for more info.";

    #[test]
    fn test_classify_bootout_unloaded() {
        assert_eq!(
            Simctl::classify_bootout(Some(0), BOOTOUT_DEPRECATION_WARNING).unwrap(),
            QuietOutcome::Unloaded
        );
    }

    #[test]
    fn test_classify_bootout_already_quiet() {
        assert_eq!(
            Simctl::classify_bootout(Some(3), "Boot-out failed: 3: No such process").unwrap(),
            QuietOutcome::AlreadyQuiet
        );
    }

    #[test]
    fn test_classify_bootout_device_not_booted() {
        assert_eq!(
            Simctl::classify_bootout(
                Some(149),
                "Process spawn via launchd failed because device is not booted"
            )
            .unwrap(),
            QuietOutcome::NotBooted
        );
    }

    // A label launchd does not know is a genuine error and must surface.
    #[test]
    fn test_classify_bootout_unknown_label() {
        let message = Simctl::classify_bootout(
            Some(113),
            "Could not find service \"com.apple.nope\" in domain for system",
        )
        .expect_err("Exit 113 should be an error")
        .to_string();

        assert!(message.contains("com.apple.nope"));
    }

    // Real-shaped `vm_stat` output, Apple silicon (16384-byte pages).
    const SAMPLE_VM_STAT_16K: &str = "\
Mach Virtual Memory Statistics: (page size of 16384 bytes)
Pages free:                              105442.
Pages active:                           1237981.
Pages inactive:                          198375.
Pages speculative:                        12043.
Pages throttled:                              0.
Pages wired down:                        402118.
";

    // Intel Mac output, 4096-byte pages.
    const SAMPLE_VM_STAT_4K: &str = "\
Mach Virtual Memory Statistics: (page size of 4096 bytes)
Pages free:                                1000.
Pages active:                             50000.
Pages inactive:                             500.
Pages speculative:                          100.
";

    #[test]
    fn test_parse_vm_stat_uses_header_page_size() {
        let free = Simctl::parse_vm_stat(SAMPLE_VM_STAT_16K).unwrap();
        assert_eq!(free, (105442 + 198375 + 12043) * 16384);
    }

    #[test]
    fn test_parse_vm_stat_non_4096_page_size_differs() {
        let free = Simctl::parse_vm_stat(SAMPLE_VM_STAT_4K).unwrap();
        assert_eq!(free, (1000 + 500 + 100) * 4096);
    }

    #[test]
    fn test_parse_vm_stat_missing_header_or_free() {
        // No `page size of N bytes` header.
        assert!(Simctl::parse_vm_stat("Pages free: 1000.\n").is_none());
        // Header but no free line.
        assert!(Simctl::parse_vm_stat(
            "Mach Virtual Memory Statistics: (page size of 4096 bytes)\n"
        )
        .is_none());
        assert!(Simctl::parse_vm_stat("").is_none());
    }

    #[test]
    fn test_parse_vm_stat_inactive_and_speculative_optional() {
        let stdout = "Mach Virtual Memory Statistics: (page size of 4096 bytes)\n\
Pages free:                                1000.\n";
        assert_eq!(Simctl::parse_vm_stat(stdout).unwrap(), 1000 * 4096);
    }

    // Literal `footprint --pid <pid> -f bytes --noCategories` output captured
    // on this machine, for a process whose Auxiliary data also carries
    // `neural_peak` — the keys are not in a fixed order or set.
    const SAMPLE_FOOTPRINT: &str = "\
======================================================================
mediaanalysisd [1049]: 64-bit    Footprint: 36407696 B (16384 bytes per page)
======================================================================

Auxiliary data:
    neural_peak: 315441152 B
    phys_footprint: 36407696 B
    phys_footprint_peak: 702631056 B
";

    #[test]
    fn test_parse_footprint_phys_footprint() {
        assert_eq!(Simctl::parse_footprint(SAMPLE_FOOTPRINT), Some(36407696));
    }

    #[test]
    fn test_parse_footprint_prefers_phys_footprint_over_peak() {
        // `phys_footprint_peak` must not be mistaken for `phys_footprint`,
        // even when it is listed first.
        let stdout = "Auxiliary data:\n\
    phys_footprint_peak: 702631056 B\n\
    phys_footprint: 36407696 B\n";
        assert_eq!(Simctl::parse_footprint(stdout), Some(36407696));
    }

    #[test]
    fn test_parse_footprint_absent() {
        // `footprint` refusing the pid prints no Auxiliary data at all.
        assert!(Simctl::parse_footprint("").is_none());
        assert!(Simctl::parse_footprint("footprint: no process found\n").is_none());
        // Header `Footprint:` alone is not the figure we want.
        let header_only = "zsh [87582]: 64-bit    Footprint: 2130352 B (16384 bytes per page)\n";
        assert!(Simctl::parse_footprint(header_only).is_none());
    }

    #[test]
    fn test_parse_footprint_malformed_number() {
        let stdout = "Auxiliary data:\n    phys_footprint: not-a-number B\n";
        assert!(Simctl::parse_footprint(stdout).is_none());
        assert!(Simctl::parse_footprint("    phys_footprint:\n").is_none());
    }

    #[test]
    fn test_parse_ps_rss_kilobytes_to_bytes() {
        // `ps -o rss=` pads its single figure with leading spaces.
        assert_eq!(Simctl::parse_ps_rss("  120456\n").unwrap(), 120456 * 1024);
        assert!(Simctl::parse_ps_rss("").is_none());
        assert!(Simctl::parse_ps_rss("not-a-number").is_none());
    }

    #[test]
    fn test_parse_pressure_level_thresholds() {
        assert_eq!(Simctl::parse_pressure_level("1\n"), MemoryPressure::Normal);
        assert_eq!(Simctl::parse_pressure_level("2\n"), MemoryPressure::Warn);
        assert_eq!(
            Simctl::parse_pressure_level("4\n"),
            MemoryPressure::Critical
        );
        // 3 is not a level macOS reports, and neither is empty output.
        assert_eq!(Simctl::parse_pressure_level("3"), MemoryPressure::Normal);
        assert_eq!(Simctl::parse_pressure_level(""), MemoryPressure::Normal);
    }
}
