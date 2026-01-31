// simctl command interface
use std::process::Command;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SimctlError {
    #[error("Command execution failed: {0}")]
    CommandFailed(String),
    #[error("No booted simulator found")]
    NoBootedSimulator,
    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulatorDevice {
    pub udid: String,
    pub name: String,
    pub state: String,
    #[serde(rename = "deviceTypeIdentifier")]
    pub device_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeviceList {
    devices: std::collections::HashMap<String, Vec<SimulatorDevice>>,
}

pub struct Simctl;

impl Simctl {
    /// List all simulator devices
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

    /// Get the first booted simulator UDID
    pub fn get_booted_udid() -> Result<String, SimctlError> {
        let devices = Self::list_devices()?;
        devices.into_iter()
            .find(|d| d.state == "Booted")
            .map(|d| d.udid)
            .ok_or(SimctlError::NoBootedSimulator)
    }

    /// Take a screenshot, returns PNG bytes
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

    /// Boot a simulator
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

    /// Send keyboard input via axe CLI
    pub fn send_keys(udid: &str, text: &str) -> Result<(), SimctlError> {
        let output = Command::new("axe")
            .args(["type", text, "--udid", udid])
            .output()?;

        if !output.status.success() {
            return Err(SimctlError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string()
            ));
        }
        Ok(())
    }
}
