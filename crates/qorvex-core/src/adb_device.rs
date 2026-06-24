//! Interface to Android's `adb` (Android Debug Bridge) command-line tool.
//!
//! This module provides a Rust wrapper around `adb`, the Android analogue of
//! the iOS [`simctl`](crate::simctl) module. It enables device listing
//! (emulators, USB-connected physical devices, and network devices joined via
//! `adb connect`), AVD enumeration, emulator boot with a boot-complete
//! readiness poll, APK installation, and an agent-independent screenshot
//! fallback via `screencap`.
//!
//! # Requirements
//!
//! The Android SDK platform-tools must be installed so that `adb` is on the
//! `PATH`. AVD enumeration additionally requires the `emulator` binary
//! (Android SDK emulator package).
//!
//! # Example
//!
//! ```no_run
//! use qorvex_core::adb_device::Adb;
//!
//! // List all connected devices and running emulators
//! let devices = Adb::list_devices().unwrap();
//! for device in &devices {
//!     println!("{}: {} ({:?})", device.serial, device.state, device.kind);
//! }
//!
//! // Capture a screenshot independent of the agent
//! if let Some(device) = devices.iter().find(|d| d.is_ready()) {
//!     let png_bytes = Adb::screencap(&device.serial).unwrap();
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::process::Command;
use std::time::{Duration, Instant};
use thiserror::Error;

/// Errors that can occur when interacting with `adb`.
///
/// Mirrors [`crate::simctl::SimctlError`] in shape and intent.
#[derive(Error, Debug)]
pub enum AdbError {
    /// An adb command failed to execute successfully (non-zero exit code).
    #[error("Command execution failed: {0}")]
    CommandFailed(String),

    /// No device matching the requested serial was found.
    #[error("Device not found: {0}")]
    DeviceNotFound(String),

    /// The device did not reach the requested state before the timeout elapsed.
    #[error("Timed out waiting for device to be ready: {0}")]
    BootTimeout(String),

    /// An I/O error occurred while executing the command.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// The transport kind of a device reported by `adb devices`.
///
/// Distinguishes emulators (local adb endpoints), USB-connected physical
/// devices, and devices joined over the network via `adb connect <host:port>`.
/// All three share the same `adb` command surface; the kind is inferred from
/// the serial's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    /// A running emulator (serial like `emulator-5554`).
    Emulator,
    /// A device reachable over the network (serial like `192.168.1.10:5555`).
    Network,
    /// A USB-connected physical device (any other serial form).
    Physical,
}

/// Represents an Android device as reported by `adb devices -l`.
///
/// A device may be a running emulator, a USB-connected physical device, or a
/// network device joined via `adb connect`. The `serial` is the stable
/// identifier adb uses to address the device (`-s <serial>`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AndroidDevice {
    /// The adb serial that uniquely identifies and addresses this device
    /// (e.g. `emulator-5554`, `192.168.1.10:5555`, or a hardware serial).
    pub serial: String,

    /// The adb connection state (`device`, `offline`, `unauthorized`, etc.).
    /// Only `device` indicates the target is usable.
    pub state: String,

    /// The inferred transport kind (emulator / network / physical).
    pub kind: DeviceKind,

    /// The `model:` attribute from `adb devices -l`, if present
    /// (e.g. `Pixel_7`).
    pub model: Option<String>,

    /// The `product:` attribute from `adb devices -l`, if present.
    pub product: Option<String>,

    /// The `device:` (hardware/board) attribute from `adb devices -l`, if present.
    pub device: Option<String>,

    /// The `transport_id:` attribute from `adb devices -l`, if present.
    pub transport_id: Option<String>,
}

impl AndroidDevice {
    /// Returns true when the device is in the usable `device` state
    /// (online, authorized, and ready for install/instrument).
    pub fn is_ready(&self) -> bool {
        self.state == "device"
    }
}

/// An Android Virtual Device (AVD) as reported by `emulator -list-avds`.
///
/// An AVD is a bootable emulator configuration; it is distinct from a
/// *running* emulator (which appears in [`Adb::list_devices`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Avd {
    /// The AVD name, used to boot it via `emulator -avd <name>`.
    pub name: String,
}

/// Wrapper for `adb` (Android Debug Bridge) commands.
///
/// Provides static methods for discovering, booting, and controlling Android
/// devices and emulators. All methods are synchronous and execute shell
/// commands, mirroring [`crate::simctl::Simctl`].
pub struct Adb;

impl Adb {
    /// Lists all devices currently visible to adb.
    ///
    /// Queries `adb devices -l` and parses the output into a list of
    /// [`AndroidDevice`]s covering running emulators, USB physical devices,
    /// and network devices joined via [`Adb::connect`]. Stable identifiers
    /// are the adb serials.
    ///
    /// # Errors
    ///
    /// - [`AdbError::Io`] if `adb` cannot be executed
    /// - [`AdbError::CommandFailed`] if adb returns a non-zero exit code
    pub fn list_devices() -> Result<Vec<AndroidDevice>, AdbError> {
        let output = Command::new("adb").args(["devices", "-l"]).output()?;

        if !output.status.success() {
            return Err(AdbError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(Self::parse_devices(&String::from_utf8_lossy(
            &output.stdout,
        )))
    }

    /// Enumerates available Android Virtual Devices (AVDs).
    ///
    /// Runs `emulator -list-avds` and parses one AVD name per line. AVDs are
    /// bootable emulator configurations; use [`Adb::boot_emulator`] to start
    /// one. Requires the Android SDK `emulator` binary on the `PATH`.
    ///
    /// # Errors
    ///
    /// - [`AdbError::Io`] if `emulator` cannot be executed
    /// - [`AdbError::CommandFailed`] if the command returns a non-zero exit code
    pub fn list_avds() -> Result<Vec<Avd>, AdbError> {
        let output = Command::new("emulator").arg("-list-avds").output()?;

        if !output.status.success() {
            return Err(AdbError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(Self::parse_avds(&String::from_utf8_lossy(&output.stdout)))
    }

    /// Joins a network device into the adb device list.
    ///
    /// Runs `adb connect <host:port>`. After a successful connect the device
    /// appears in [`Adb::list_devices`] addressable by its `host:port` serial,
    /// using the same `adb` command surface as emulators and USB devices
    /// (ADR-3: adb is the single Android transport).
    ///
    /// # Arguments
    ///
    /// * `host_port` - The device endpoint, e.g. `192.168.1.10:5555`
    ///
    /// # Errors
    ///
    /// - [`AdbError::Io`] if `adb` cannot be executed
    /// - [`AdbError::CommandFailed`] if adb reports the connection failed
    ///   (adb exits 0 even on a failed connect, so the stdout text is checked)
    pub fn connect(host_port: &str) -> Result<(), AdbError> {
        let output = Command::new("adb").args(["connect", host_port]).output()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        // `adb connect` exits 0 even when it cannot reach the host; success is
        // signalled in the stdout text ("connected to" / "already connected").
        if !output.status.success() || !Self::connect_succeeded(&stdout) {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AdbError::CommandFailed(format!(
                "{}{}",
                stdout.trim(),
                stderr.trim()
            )));
        }
        Ok(())
    }

    /// Disconnects a previously connected network device.
    ///
    /// Runs `adb disconnect <host:port>`. The inverse of [`Adb::connect`].
    ///
    /// # Errors
    ///
    /// - [`AdbError::Io`] if `adb` cannot be executed
    /// - [`AdbError::CommandFailed`] if adb returns a non-zero exit code
    pub fn disconnect(host_port: &str) -> Result<(), AdbError> {
        let output = Command::new("adb")
            .args(["disconnect", host_port])
            .output()?;

        if !output.status.success() {
            return Err(AdbError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(())
    }

    /// Boots an emulator AVD and waits until it reports boot-complete.
    ///
    /// Spawns `emulator -avd <avd_name>` detached, then polls until a device
    /// for that emulator appears, comes online, and reports
    /// `getprop sys.boot_completed == 1` (see [`Adb::wait_for_boot`]). Returns
    /// the adb serial of the booted emulator only once it is ready for
    /// install/instrument.
    ///
    /// Because a single host can run multiple emulators, the new emulator is
    /// identified as the emulator serial that was not present before the boot.
    ///
    /// # Arguments
    ///
    /// * `avd_name` - The AVD to boot (from [`Adb::list_avds`])
    /// * `timeout` - Maximum time to wait for boot-complete
    ///
    /// # Errors
    ///
    /// - [`AdbError::Io`] if `emulator` cannot be spawned
    /// - [`AdbError::BootTimeout`] if no new emulator reports ready in time
    pub fn boot_emulator(avd_name: &str, timeout: Duration) -> Result<String, AdbError> {
        let before: std::collections::HashSet<String> = Self::list_devices()?
            .into_iter()
            .filter(|d| d.kind == DeviceKind::Emulator)
            .map(|d| d.serial)
            .collect();

        // Spawn detached; the emulator process is long-lived and is not owned
        // by this call (lifecycle ownership is out of scope — story #88).
        Command::new("emulator")
            .args(["-avd", avd_name])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;

        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                return Err(AdbError::BootTimeout(format!(
                    "emulator '{}' did not appear/boot within {:?}",
                    avd_name, timeout
                )));
            }

            // Find an emulator serial that wasn't present before the boot and
            // is now online.
            let new_serial = Self::list_devices()
                .ok()
                .into_iter()
                .flatten()
                .find(|d| {
                    d.kind == DeviceKind::Emulator && d.is_ready() && !before.contains(&d.serial)
                })
                .map(|d| d.serial);

            if let Some(serial) = new_serial {
                let remaining = deadline.saturating_duration_since(Instant::now());
                Self::wait_for_boot(&serial, remaining)?;
                return Ok(serial);
            }

            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// Polls a device until it reports boot-complete and is ready.
    ///
    /// Mirrors `agent_lifecycle::wait_for_ready`: polls
    /// `adb -s <serial> shell getprop sys.boot_completed` every 500 ms until it
    /// returns `1` (the device is ready for install/instrument) or the timeout
    /// elapses. The device must already be online (`device` state).
    ///
    /// # Arguments
    ///
    /// * `serial` - The adb serial of the device to poll
    /// * `timeout` - Maximum time to wait for boot-complete
    ///
    /// # Errors
    ///
    /// - [`AdbError::BootTimeout`] if `sys.boot_completed` is not `1` in time
    pub fn wait_for_boot(serial: &str, timeout: Duration) -> Result<(), AdbError> {
        let deadline = Instant::now() + timeout;
        loop {
            if Self::is_boot_completed(serial) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(AdbError::BootTimeout(format!(
                    "device '{}' did not report sys.boot_completed=1 within {:?}",
                    serial, timeout
                )));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// Returns true if the device reports `sys.boot_completed == 1`.
    ///
    /// A single best-effort probe (no retry); any command failure is treated as
    /// "not yet booted" so the caller's poll loop can continue.
    fn is_boot_completed(serial: &str) -> bool {
        Command::new("adb")
            .args(["-s", serial, "shell", "getprop", "sys.boot_completed"])
            .output()
            .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "1")
            .unwrap_or(false)
    }

    /// Installs an APK onto a device.
    ///
    /// Runs `adb -s <serial> install -r <apk_path>` (`-r` reinstalls, keeping
    /// data, the common case for iterating). The Android analogue of an app
    /// install on the simulator.
    ///
    /// # Arguments
    ///
    /// * `serial` - The adb serial of the target device
    /// * `apk_path` - Filesystem path to the APK to install
    ///
    /// # Errors
    ///
    /// - [`AdbError::Io`] if `adb` cannot be executed
    /// - [`AdbError::CommandFailed`] if the install fails. adb may exit 0 while
    ///   printing `Failure [...]` to stdout, so the output text is inspected.
    pub fn install(serial: &str, apk_path: &str) -> Result<(), AdbError> {
        let output = Command::new("adb")
            .args(["-s", serial, "install", "-r", apk_path])
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() || stdout.contains("Failure") || stderr.contains("Failure") {
            return Err(AdbError::CommandFailed(format!(
                "{}{}",
                stdout.trim(),
                stderr.trim()
            )));
        }
        Ok(())
    }

    /// Captures a screenshot of the device screen, independent of the agent.
    ///
    /// Runs `adb -s <serial> exec-out screencap -p` and returns the PNG bytes
    /// straight from stdout. This is the agent-independent screenshot fallback
    /// (C3): it works whenever adb can reach the device, regardless of whether
    /// the Kotlin agent is running. The iOS analogue is
    /// [`crate::simctl::Simctl::screenshot`].
    ///
    /// `exec-out` is used (not `shell`) so the binary PNG stream is not
    /// corrupted by line-ending translation.
    ///
    /// # Arguments
    ///
    /// * `serial` - The adb serial of the target device
    ///
    /// # Returns
    ///
    /// A `Vec<u8>` containing PNG image data.
    ///
    /// # Errors
    ///
    /// - [`AdbError::Io`] if `adb` cannot be executed
    /// - [`AdbError::CommandFailed`] if screencap fails or returns no data
    pub fn screencap(serial: &str) -> Result<Vec<u8>, AdbError> {
        let output = Command::new("adb")
            .args(["-s", serial, "exec-out", "screencap", "-p"])
            .output()?;

        if !output.status.success() || output.stdout.is_empty() {
            return Err(AdbError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(output.stdout)
    }

    // ----- Pure parsing/classification helpers (exposed for unit testing) -----

    /// Parses the output of `adb devices -l` into a list of devices.
    ///
    /// Exposed primarily for testing. The first line (`List of devices
    /// attached`) and any blank/`*`-prefixed daemon lines are ignored. Each
    /// device line is `"<serial> <state> [key:value ...]"`.
    pub fn parse_devices(output: &str) -> Vec<AndroidDevice> {
        output
            .lines()
            .map(str::trim)
            .filter(|line| {
                !line.is_empty()
                    && !line.starts_with("List of devices")
                    && !line.starts_with('*')
            })
            .filter_map(Self::parse_device_line)
            .collect()
    }

    /// Parses a single `adb devices -l` line into an [`AndroidDevice`].
    fn parse_device_line(line: &str) -> Option<AndroidDevice> {
        let mut fields = line.split_whitespace();
        let serial = fields.next()?.to_string();
        let state = fields.next()?.to_string();

        let mut model = None;
        let mut product = None;
        let mut device = None;
        let mut transport_id = None;
        for attr in fields {
            if let Some((key, value)) = attr.split_once(':') {
                let value = value.to_string();
                match key {
                    "model" => model = Some(value),
                    "product" => product = Some(value),
                    "device" => device = Some(value),
                    "transport_id" => transport_id = Some(value),
                    _ => {}
                }
            }
        }

        Some(AndroidDevice {
            kind: Self::classify_serial(&serial),
            serial,
            state,
            model,
            product,
            device,
            transport_id,
        })
    }

    /// Classifies a device by the shape of its adb serial.
    ///
    /// - `emulator-<port>` → [`DeviceKind::Emulator`]
    /// - `<host>:<port>` (network endpoint) → [`DeviceKind::Network`]
    /// - anything else (hardware serial) → [`DeviceKind::Physical`]
    pub fn classify_serial(serial: &str) -> DeviceKind {
        if serial.starts_with("emulator-") {
            DeviceKind::Emulator
        } else if Self::looks_like_host_port(serial) {
            DeviceKind::Network
        } else {
            DeviceKind::Physical
        }
    }

    /// Returns true if the serial looks like a `host:port` network endpoint.
    ///
    /// Requires a single `:` separating a non-empty host from an all-numeric
    /// port (covers both IPv4 and hostnames; bare hardware serials never carry
    /// a `:`-delimited numeric port).
    fn looks_like_host_port(serial: &str) -> bool {
        match serial.rsplit_once(':') {
            Some((host, port)) => {
                !host.is_empty() && !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())
            }
            None => false,
        }
    }

    /// Returns true if `adb connect` stdout indicates a successful connection.
    fn connect_succeeded(stdout: &str) -> bool {
        stdout.contains("connected to") || stdout.contains("already connected")
    }

    /// Parses the output of `emulator -list-avds` into a list of AVDs.
    ///
    /// Exposed primarily for testing. One AVD name per non-empty line; warning
    /// lines emitted by the emulator binary (prefixed `INFO`/`WARNING`) are
    /// skipped.
    pub fn parse_avds(output: &str) -> Vec<Avd> {
        output
            .lines()
            .map(str::trim)
            .filter(|line| {
                !line.is_empty()
                    && !line.starts_with("INFO")
                    && !line.starts_with("WARNING")
                    && !line.starts_with("ERROR")
            })
            .map(|name| Avd {
                name: name.to_string(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Representative `adb devices -l` output: a running emulator, a USB
    // physical device, a network (`adb connect`) device, and an offline one.
    const SAMPLE_DEVICES: &str = "List of devices attached\n\
emulator-5554          device product:sdk_gphone64_arm64 model:sdk_gphone64_arm64 device:emu64a transport_id:1\n\
1A2B3C4D5E6F           device product:redfin model:Pixel_5 device:redfin transport_id:2\n\
192.168.1.42:5555      device product:lineage_oriole model:Pixel_6_Pro device:oriole transport_id:3\n\
emulator-5556          offline\n";

    #[test]
    fn test_parse_devices_counts_and_serials() {
        let devices = Adb::parse_devices(SAMPLE_DEVICES);
        assert_eq!(devices.len(), 4);

        let serials: Vec<&str> = devices.iter().map(|d| d.serial.as_str()).collect();
        assert!(serials.contains(&"emulator-5554"));
        assert!(serials.contains(&"1A2B3C4D5E6F"));
        assert!(serials.contains(&"192.168.1.42:5555"));
        assert!(serials.contains(&"emulator-5556"));
    }

    #[test]
    fn test_parse_devices_classifies_kinds() {
        let devices = Adb::parse_devices(SAMPLE_DEVICES);
        let by_serial = |s: &str| devices.iter().find(|d| d.serial == s).unwrap();

        assert_eq!(by_serial("emulator-5554").kind, DeviceKind::Emulator);
        assert_eq!(by_serial("1A2B3C4D5E6F").kind, DeviceKind::Physical);
        assert_eq!(by_serial("192.168.1.42:5555").kind, DeviceKind::Network);
        assert_eq!(by_serial("emulator-5556").kind, DeviceKind::Emulator);
    }

    #[test]
    fn test_parse_devices_attributes() {
        let devices = Adb::parse_devices(SAMPLE_DEVICES);
        let phys = devices.iter().find(|d| d.serial == "1A2B3C4D5E6F").unwrap();

        assert_eq!(phys.state, "device");
        assert_eq!(phys.model.as_deref(), Some("Pixel_5"));
        assert_eq!(phys.product.as_deref(), Some("redfin"));
        assert_eq!(phys.device.as_deref(), Some("redfin"));
        assert_eq!(phys.transport_id.as_deref(), Some("2"));
        assert!(phys.is_ready());
    }

    #[test]
    fn test_parse_devices_offline_not_ready() {
        let devices = Adb::parse_devices(SAMPLE_DEVICES);
        let offline = devices.iter().find(|d| d.serial == "emulator-5556").unwrap();

        assert_eq!(offline.state, "offline");
        assert!(!offline.is_ready());
        // No -l attributes on an offline line.
        assert!(offline.model.is_none());
        assert!(offline.transport_id.is_none());
    }

    #[test]
    fn test_parse_devices_empty() {
        let devices = Adb::parse_devices("List of devices attached\n\n");
        assert!(devices.is_empty());
    }

    #[test]
    fn test_parse_devices_skips_daemon_lines() {
        let output = "* daemon not running; starting now at tcp:5037\n\
* daemon started successfully\n\
List of devices attached\n\
emulator-5554 device\n";
        let devices = Adb::parse_devices(output);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].serial, "emulator-5554");
    }

    #[test]
    fn test_classify_serial() {
        assert_eq!(Adb::classify_serial("emulator-5554"), DeviceKind::Emulator);
        assert_eq!(
            Adb::classify_serial("192.168.1.42:5555"),
            DeviceKind::Network
        );
        assert_eq!(
            Adb::classify_serial("myhost.local:5555"),
            DeviceKind::Network
        );
        assert_eq!(Adb::classify_serial("1A2B3C4D5E6F"), DeviceKind::Physical);
        // A hardware serial that happens to contain a colon but no numeric port
        // stays physical.
        assert_eq!(Adb::classify_serial("ABC:notaport"), DeviceKind::Physical);
    }

    #[test]
    fn test_parse_avds() {
        let output = "Pixel_7_API_34\nPixel_Tablet_API_33\nMedium_Phone_API_35\n";
        let avds = Adb::parse_avds(output);
        assert_eq!(avds.len(), 3);
        assert_eq!(avds[0].name, "Pixel_7_API_34");
        assert_eq!(avds[2].name, "Medium_Phone_API_35");
    }

    #[test]
    fn test_parse_avds_skips_info_lines() {
        let output = "INFO    | Storing crashdata in: /tmp/foo\nPixel_7_API_34\n";
        let avds = Adb::parse_avds(output);
        assert_eq!(avds.len(), 1);
        assert_eq!(avds[0].name, "Pixel_7_API_34");
    }

    #[test]
    fn test_parse_avds_empty() {
        assert!(Adb::parse_avds("").is_empty());
    }

    #[test]
    fn test_connect_succeeded() {
        assert!(Adb::connect_succeeded("connected to 192.168.1.42:5555"));
        assert!(Adb::connect_succeeded(
            "already connected to 192.168.1.42:5555"
        ));
        assert!(!Adb::connect_succeeded(
            "failed to connect to 192.168.1.42:5555"
        ));
        assert!(!Adb::connect_succeeded(
            "cannot connect to 192.168.1.42:5555: Connection refused"
        ));
    }

    #[test]
    fn test_adb_error_display() {
        let cmd_err = AdbError::CommandFailed("boom".to_string());
        assert!(cmd_err.to_string().contains("boom"));

        let not_found = AdbError::DeviceNotFound("emulator-5554".to_string());
        assert!(not_found.to_string().contains("emulator-5554"));

        let timeout = AdbError::BootTimeout("slow".to_string());
        assert!(timeout.to_string().contains("slow"));
    }

    #[test]
    fn test_device_kind_serde_roundtrip() {
        let json = serde_json::to_string(&DeviceKind::Emulator).unwrap();
        assert_eq!(json, "\"emulator\"");
        let back: DeviceKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, DeviceKind::Emulator);
    }
}
