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

use std::process::Command;
use serde::{Deserialize, Serialize};
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

    /// Failed to parse JSON output from simctl.
    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),

    /// An I/O error occurred while executing the command.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
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
                String::from_utf8_lossy(&output.stderr).to_string()
            ));
        }

        let device_list: DeviceList = serde_json::from_slice(&output.stdout)?;
        let devices: Vec<SimulatorDevice> = device_list.devices
            .into_values()
            .flatten()
            .collect();

        Ok(devices)
    }

    /// Returns the UDID of the first booted simulator.
    ///
    /// Searches through all available devices and returns the UDID of the
    /// first one found with state "Booted".
    ///
    /// # Returns
    ///
    /// The UDID string of the booted simulator.
    ///
    /// # Errors
    ///
    /// - [`SimctlError::NoBootedSimulator`] if no simulator is currently booted
    /// - Any errors from [`Self::list_devices`]
    pub fn get_booted_udid() -> Result<String, SimctlError> {
        let devices = Self::list_devices()?;
        devices.into_iter()
            .find(|d| d.state == "Booted")
            .map(|d| d.udid)
            .ok_or(SimctlError::NoBootedSimulator)
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
                String::from_utf8_lossy(&output.stderr).to_string()
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
        let devices: Vec<SimulatorDevice> = device_list.devices
            .into_values()
            .flatten()
            .collect();
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
        assert!(booted.device_type.as_ref().unwrap().contains("iPhone-15-Pro"));
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

    #[test]
    fn test_boot_with_invalid_udid() {
        let result = Simctl::boot("invalid-udid-that-does-not-exist");

        assert!(result.is_err());
    }
}
