//! Discovery of physical iOS devices via Apple's CoreDevice framework.
//!
//! This module wraps `xcrun devicectl list devices` to enumerate paired
//! physical devices and extract their identifiers, models, and connection
//! properties.
//!
//! # Requirements
//!
//! Xcode 15+ must be installed for `xcrun devicectl` to be available.
//!
//! # Example
//!
//! ```no_run
//! # async fn example() -> Result<(), qorvex_core::coredevice::CoreDeviceError> {
//! use qorvex_core::coredevice::list_devices;
//!
//! let devices = list_devices().await?;
//! for d in &devices {
//!     println!("{}: {} ({})", d.name, d.identifier, d.transport_type);
//! }
//! # Ok(())
//! # }
//! ```

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors that can occur when discovering devices via CoreDevice.
#[derive(Error, Debug)]
pub enum CoreDeviceError {
    /// `xcrun devicectl` is not available (e.g., older Xcode).
    #[error("devicectl not available: {0}")]
    NotAvailable(String),

    /// Failed to parse the JSON output from devicectl.
    #[error("failed to parse devicectl output: {0}")]
    ParseError(String),

    /// The devicectl command exited with an error.
    #[error("devicectl command failed: {0}")]
    CommandFailed(String),
}

/// A physical device discovered via CoreDevice (`xcrun devicectl`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreDeviceInfo {
    /// CoreDevice identifier (UUID format).
    pub identifier: String,
    /// The device's traditional UDID (e.g., `00008140-…`), if extractable
    /// from `potentialHostnames`.
    pub udid: Option<String>,
    /// Human-readable name (e.g., "Hillbilly").
    pub name: String,
    /// Device model description (e.g., "iPhone17,2" or a marketing name).
    pub model: String,
    /// OS version string (e.g., "26.4").
    pub os_version: String,
    /// Connection transport type (e.g., "localNetwork", "wired").
    pub transport_type: String,
    /// Whether the device is paired with this Mac.
    pub is_paired: bool,
    /// Whether developer mode is enabled on the device.
    pub developer_mode: bool,
    /// mDNS hostname for direct TCP connection (e.g. `Hillbilly.local`).
    ///
    /// Set for all paired devices. For `localNetwork` devices this hostname can be
    /// used to connect directly without a CoreDevice tunnel.
    pub hostname: Option<String>,
}

/// Discover paired physical devices via `xcrun devicectl list devices`.
///
/// Only devices with `pairingState == "paired"` are returned. The blocking
/// subprocess is run on a Tokio blocking thread to avoid stalling the
/// async runtime.
pub async fn list_devices() -> Result<Vec<CoreDeviceInfo>, CoreDeviceError> {
    tokio::task::spawn_blocking(list_devices_blocking)
        .await
        .map_err(|e| CoreDeviceError::CommandFailed(format!("task join error: {e}")))?
}

/// Synchronous implementation of device discovery.
fn list_devices_blocking() -> Result<Vec<CoreDeviceInfo>, CoreDeviceError> {
    use std::io::Read;
    use std::process::Command;

    // Create a temp file for JSON output.
    let tmp_path =
        std::env::temp_dir().join(format!("qorvex_devicectl_{}.json", std::process::id()));
    let tmp_str = tmp_path.display().to_string();

    // Verify devicectl is available.
    let status = Command::new("xcrun")
        .args(["devicectl", "--version"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| CoreDeviceError::NotAvailable(e.to_string()))?;

    if !status.success() {
        return Err(CoreDeviceError::NotAvailable(
            "xcrun devicectl returned non-zero exit code".into(),
        ));
    }

    // Run the actual device listing.
    let output = Command::new("xcrun")
        .args(["devicectl", "list", "devices", "--json-output", &tmp_str])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| CoreDeviceError::CommandFailed(e.to_string()))?;

    if !output.success() {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(CoreDeviceError::CommandFailed(
            "devicectl list devices exited with non-zero status".into(),
        ));
    }

    // Read the JSON output file.
    let mut json_string = String::new();
    let read_result =
        std::fs::File::open(&tmp_path).and_then(|mut f| f.read_to_string(&mut json_string));

    // Clean up temp file regardless of read outcome.
    let _ = std::fs::remove_file(&tmp_path);

    read_result.map_err(|e| {
        CoreDeviceError::ParseError(format!("failed to read JSON output file: {e}"))
    })?;

    parse_devicectl_json(&json_string)
}

// ---------------------------------------------------------------------------
// JSON deserialization types (internal)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct DevicectlOutput {
    result: DevicectlResult,
}

#[derive(Deserialize)]
struct DevicectlResult {
    devices: Vec<RawDevice>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDevice {
    identifier: String,
    connection_properties: ConnectionProperties,
    device_properties: DeviceProperties,
    hardware_properties: HardwareProperties,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionProperties {
    pairing_state: Option<String>,
    transport_type: Option<String>,
    #[serde(default)]
    potential_hostnames: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceProperties {
    developer_mode_status: Option<String>,
    name: Option<String>,
    os_version_number: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HardwareProperties {
    product_type: Option<String>,
    device_type: Option<String>,
    marketing_name: Option<String>,
}

// ---------------------------------------------------------------------------
// Parsing logic
// ---------------------------------------------------------------------------

/// Parse the raw JSON string from devicectl into a list of [`CoreDeviceInfo`].
fn parse_devicectl_json(json: &str) -> Result<Vec<CoreDeviceInfo>, CoreDeviceError> {
    let output: DevicectlOutput =
        serde_json::from_str(json).map_err(|e| CoreDeviceError::ParseError(e.to_string()))?;

    let mut devices = Vec::new();

    for raw in output.result.devices {
        // Only include paired devices.
        let paired = raw.connection_properties.pairing_state.as_deref() == Some("paired");

        if !paired {
            continue;
        }

        let name = raw.device_properties.name.clone().unwrap_or_default();

        let udid = extract_traditional_udid(
            &raw.connection_properties.potential_hostnames,
            &raw.identifier,
            &name,
        );

        let model = raw
            .hardware_properties
            .marketing_name
            .clone()
            .or_else(|| {
                // Fallback: "deviceType productType" or just productType.
                match (
                    &raw.hardware_properties.device_type,
                    &raw.hardware_properties.product_type,
                ) {
                    (Some(dt), Some(pt)) => Some(format!("{dt} ({pt})")),
                    (None, Some(pt)) => Some(pt.clone()),
                    (Some(dt), None) => Some(dt.clone()),
                    (None, None) => None,
                }
            })
            .unwrap_or_else(|| "Unknown".into());

        let hostname = Some(format!("{}.local", name));
        devices.push(CoreDeviceInfo {
            identifier: raw.identifier,
            udid,
            hostname,
            name,
            model,
            os_version: raw.device_properties.os_version_number.unwrap_or_default(),
            transport_type: raw
                .connection_properties
                .transport_type
                .unwrap_or_else(|| "unknown".into()),
            is_paired: true,
            developer_mode: raw.device_properties.developer_mode_status.as_deref()
                == Some("enabled"),
        });
    }

    Ok(devices)
}

/// Extract the traditional UDID from `potentialHostnames`.
///
/// Each hostname has the form `<value>.coredevice.local`. We strip the suffix
/// and discard entries that match the CoreDevice UUID identifier or the
/// human-readable device name. The remaining entry is the traditional UDID.
fn extract_traditional_udid(
    hostnames: &[String],
    coredevice_id: &str,
    device_name: &str,
) -> Option<String> {
    let suffix = ".coredevice.local";

    for hostname in hostnames {
        let stripped = hostname.strip_suffix(suffix).unwrap_or(hostname);

        // Skip if it matches the CoreDevice UUID identifier.
        if stripped.eq_ignore_ascii_case(coredevice_id) {
            continue;
        }

        // Skip if it matches the device name.
        if stripped.eq_ignore_ascii_case(device_name) {
            continue;
        }

        // Skip if it looks like a UUID (8-4-4-4-12 hex pattern).
        if is_uuid_format(stripped) {
            continue;
        }

        // What remains should be the traditional UDID.
        return Some(stripped.to_string());
    }

    None
}

/// Check whether a string matches the UUID format: 8-4-4-4-12 hex digits.
fn is_uuid_format(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    let expected_lens = [8, 4, 4, 4, 12];
    parts
        .iter()
        .zip(expected_lens.iter())
        .all(|(part, &len)| part.len() == len && part.chars().all(|c| c.is_ascii_hexdigit()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_JSON: &str = r#"{
  "result": {
    "devices": [
      {
        "connectionProperties": {
          "pairingState": "paired",
          "transportType": "localNetwork",
          "tunnelState": "disconnected",
          "potentialHostnames": [
            "Hillbilly.coredevice.local",
            "44E36C64-57C2-5D8B-B934-199A60F6F809.coredevice.local",
            "00008140-000A15911AE3001C.coredevice.local"
          ]
        },
        "deviceProperties": {
          "developerModeStatus": "enabled",
          "name": "Hillbilly",
          "osVersionNumber": "26.4"
        },
        "hardwareProperties": {
          "deviceType": "iPhone",
          "productType": "iPhone17,2"
        },
        "identifier": "44E36C64-57C2-5D8B-B934-199A60F6F809",
        "visibilityClass": "default"
      }
    ]
  },
  "info": {
    "arguments": [],
    "commandType": "devicectl.list.devices",
    "environment": {},
    "outcome": "success",
    "version": "1"
  }
}"#;

    #[test]
    fn parse_sample_json() {
        let devices = parse_devicectl_json(SAMPLE_JSON).unwrap();
        assert_eq!(devices.len(), 1);

        let d = &devices[0];
        assert_eq!(d.identifier, "44E36C64-57C2-5D8B-B934-199A60F6F809");
        assert_eq!(d.udid.as_deref(), Some("00008140-000A15911AE3001C"));
        assert_eq!(d.name, "Hillbilly");
        assert_eq!(d.hostname.as_deref(), Some("Hillbilly.local"));
        assert_eq!(d.model, "iPhone (iPhone17,2)");
        assert_eq!(d.os_version, "26.4");
        assert_eq!(d.transport_type, "localNetwork");
        assert!(d.is_paired);
        assert!(d.developer_mode);
    }

    #[test]
    fn unpaired_devices_are_excluded() {
        let json = r#"{
  "result": {
    "devices": [
      {
        "connectionProperties": {
          "pairingState": "unpaired",
          "transportType": "wired",
          "potentialHostnames": []
        },
        "deviceProperties": {
          "developerModeStatus": "disabled",
          "name": "SomeDevice",
          "osVersionNumber": "18.0"
        },
        "hardwareProperties": {
          "deviceType": "iPhone",
          "productType": "iPhone15,1"
        },
        "identifier": "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE",
        "visibilityClass": "default"
      }
    ]
  },
  "info": {}
}"#;
        let devices = parse_devicectl_json(json).unwrap();
        assert!(devices.is_empty());
    }

    #[test]
    fn empty_device_list() {
        let json = r#"{"result": {"devices": []}, "info": {}}"#;
        let devices = parse_devicectl_json(json).unwrap();
        assert!(devices.is_empty());
    }

    #[test]
    fn udid_none_when_no_hostnames() {
        let json = r#"{
  "result": {
    "devices": [
      {
        "connectionProperties": {
          "pairingState": "paired",
          "transportType": "wired",
          "potentialHostnames": []
        },
        "deviceProperties": {
          "developerModeStatus": "enabled",
          "name": "TestDevice",
          "osVersionNumber": "18.0"
        },
        "hardwareProperties": {
          "deviceType": "iPad",
          "productType": "iPad14,1"
        },
        "identifier": "11111111-2222-3333-4444-555555555555",
        "visibilityClass": "default"
      }
    ]
  },
  "info": {}
}"#;
        let devices = parse_devicectl_json(json).unwrap();
        assert_eq!(devices.len(), 1);
        assert!(devices[0].udid.is_none());
    }

    #[test]
    fn marketing_name_preferred_over_product_type() {
        let json = r#"{
  "result": {
    "devices": [
      {
        "connectionProperties": {
          "pairingState": "paired",
          "transportType": "wired",
          "potentialHostnames": []
        },
        "deviceProperties": {
          "developerModeStatus": "enabled",
          "name": "TestDevice",
          "osVersionNumber": "18.0"
        },
        "hardwareProperties": {
          "deviceType": "iPhone",
          "productType": "iPhone17,2",
          "marketingName": "iPhone 16 Pro Max"
        },
        "identifier": "11111111-2222-3333-4444-555555555555",
        "visibilityClass": "default"
      }
    ]
  },
  "info": {}
}"#;
        let devices = parse_devicectl_json(json).unwrap();
        assert_eq!(devices[0].model, "iPhone 16 Pro Max");
    }

    #[test]
    fn is_uuid_format_works() {
        assert!(is_uuid_format("44E36C64-57C2-5D8B-B934-199A60F6F809"));
        assert!(!is_uuid_format("00008140-000A15911AE3001C"));
        assert!(!is_uuid_format("Hillbilly"));
        assert!(!is_uuid_format(""));
    }
}
