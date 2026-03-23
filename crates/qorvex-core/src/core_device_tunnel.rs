//! CoreDevice tunnel for physical iOS devices (iOS 17+).
//!
//! This module connects to physical iOS devices via the idevice crate's
//! [`CoreDeviceProxy`] service and a userspace TCP stack. No root or sudo is
//! required — the tunnel is established entirely in user space over a normal
//! TCP connection to the device's `{UDID}.coredevice.local` mDNS hostname.
//!
//! # Overview
//!
//! 1. Locate the device's pairing file on disk.
//! 2. Resolve the device IP via `{UDID}.coredevice.local` mDNS lookup.
//! 3. Connect through lockdownd → `CoreDeviceProxy` service.
//! 4. Create a software TCP tunnel (`create_software_tunnel`).
//! 5. Open a TCP stream to the agent port inside the tunnel.
//!
//! # Example
//!
//! ```no_run
//! use qorvex_core::core_device_tunnel::connect_coredevice;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let stream = connect_coredevice("00008140-000A15911AE3001C", 8080).await?;
//! // stream implements AgentStream — pass to AgentClient::from_stream
//! # Ok(())
//! # }
//! ```

use std::path::PathBuf;

use idevice::pairing_file::PairingFile;
use idevice::provider::TcpProvider;
use idevice::services::core_device_proxy::CoreDeviceProxy;
use idevice::IdeviceService;
use thiserror::Error;
use tokio::net::lookup_host;

use crate::agent_client::AgentStream;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur while connecting to a physical iOS device via the
/// CoreDevice tunnel.
#[derive(Debug, Error)]
pub enum CoreDeviceTunnelError {
    /// No pairing file was found for the given UDID.
    ///
    /// The device must be trusted on this Mac before a tunnel can be opened.
    #[error("pairing file not found for device {0}")]
    PairingFileNotFound(String),

    /// The pairing file existed but could not be parsed.
    #[error("failed to read pairing file: {0}")]
    PairingFileRead(#[from] idevice::IdeviceError),

    /// The device's `{UDID}.coredevice.local` hostname could not be resolved.
    #[error("failed to resolve device address for {0}")]
    AddressResolution(String),

    /// Connecting to the `CoreDeviceProxy` lockdown service failed.
    #[error("CoreDeviceProxy connection failed: {0}")]
    ProxyConnection(String),

    /// Creating the userspace software tunnel failed.
    #[error("software tunnel creation failed: {0}")]
    TunnelCreation(String),

    /// Connecting to the agent port inside the tunnel failed.
    #[error("TCP connection to agent port failed: {0}")]
    AgentConnection(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Return the path to the pairing file for `udid`, or `None` if neither
/// candidate location exists.
///
/// macOS stores pairing records at:
/// - `~/Library/Lockdown/PairRecords/{UDID}.plist`  (try first)
/// - `/var/db/lockdown/{UDID}.plist`                (fallback)
fn find_pairing_file(udid: &str) -> Option<PathBuf> {
    // Primary: user-level lockdown directory (no root required).
    if let Some(home) = dirs::home_dir() {
        let user_path = home
            .join("Library")
            .join("Lockdown")
            .join("PairRecords")
            .join(format!("{udid}.plist"));
        if user_path.exists() {
            return Some(user_path);
        }
    }

    // Fallback: system-level lockdown directory (requires root in practice,
    // but check anyway in case the file is accessible).
    let system_path = PathBuf::from(format!("/var/db/lockdown/{udid}.plist"));
    if system_path.exists() {
        return Some(system_path);
    }

    None
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Connect to a physical iOS device via the CoreDevice tunnel and return a
/// stream to the agent port.
///
/// The returned stream implements [`AgentStream`] and can be handed directly to
/// [`AgentClient::from_stream`](crate::agent_client::AgentClient::from_stream).
///
/// # Arguments
///
/// * `udid`       – The device's UDID (e.g. `"00008140-000A15911AE3001C"`).
/// * `agent_port` – The TCP port the Swift agent is listening on inside the
///   tunnel (typically 8080).
///
/// # Errors
///
/// Returns a [`CoreDeviceTunnelError`] if any step of the connection sequence
/// fails (pairing file missing, mDNS resolution, lockdown, TLS, tunnel, or TCP).
pub async fn connect_coredevice(
    udid: &str,
    agent_port: u16,
) -> Result<Box<dyn AgentStream>, CoreDeviceTunnelError> {
    // -----------------------------------------------------------------------
    // Step 1: Locate and load the pairing file.
    // -----------------------------------------------------------------------
    let pairing_path = find_pairing_file(udid)
        .ok_or_else(|| CoreDeviceTunnelError::PairingFileNotFound(udid.to_string()))?;

    let pairing_file = PairingFile::read_from_file(&pairing_path)?;

    // -----------------------------------------------------------------------
    // Step 2: Resolve the device IP via mDNS.
    //
    // iOS 17+ devices advertise themselves as `{UDID}.coredevice.local`.
    // We pass port 62078 (lockdownd) so `lookup_host` returns SocketAddrs;
    // we only care about the IP part.
    // -----------------------------------------------------------------------
    let hostname = format!("{udid}.coredevice.local:62078");
    let mut addrs = lookup_host(&hostname)
        .await
        .map_err(|_| CoreDeviceTunnelError::AddressResolution(udid.to_string()))?;

    let ip = addrs
        .next()
        .map(|s| s.ip())
        .ok_or_else(|| CoreDeviceTunnelError::AddressResolution(udid.to_string()))?;

    // -----------------------------------------------------------------------
    // Step 3: Connect via CoreDeviceProxy (lockdown → TLS → start_service).
    // -----------------------------------------------------------------------
    let provider = TcpProvider {
        addr: ip,
        pairing_file,
        label: "qorvex".to_string(),
    };

    let proxy = CoreDeviceProxy::connect(&provider)
        .await
        .map_err(|e| CoreDeviceTunnelError::ProxyConnection(e.to_string()))?;

    // -----------------------------------------------------------------------
    // Step 4: Create the userspace software TCP tunnel.
    // -----------------------------------------------------------------------
    let adapter = proxy
        .create_software_tunnel()
        .map_err(|e| CoreDeviceTunnelError::TunnelCreation(e.to_string()))?;

    // Wrap the adapter in a background task. `AdapterHandle` is dropped after
    // `connect()` returns; the background task keeps running because
    // `StreamHandle` holds a channel sender clone.
    let mut handle = adapter.to_async_handle();

    // -----------------------------------------------------------------------
    // Step 5: Open a TCP stream to the agent port inside the tunnel.
    // -----------------------------------------------------------------------
    let stream = handle.connect(agent_port).await?;

    Ok(Box::new(stream))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_pairing_file_returns_none_for_unknown_udid() {
        // A fabricated UDID that should never have a pairing file on the CI host.
        let result = find_pairing_file("00000000-0000000000000000");
        assert!(result.is_none());
    }

    #[test]
    fn error_display_pairing_file_not_found() {
        let err = CoreDeviceTunnelError::PairingFileNotFound("MYUDID".into());
        assert!(err.to_string().contains("MYUDID"));
    }

    #[test]
    fn error_display_address_resolution() {
        let err = CoreDeviceTunnelError::AddressResolution("MYUDID".into());
        assert!(err.to_string().contains("MYUDID"));
    }

    #[test]
    fn error_display_proxy_connection() {
        let err = CoreDeviceTunnelError::ProxyConnection("timeout".into());
        assert!(err.to_string().contains("CoreDeviceProxy"));
        assert!(err.to_string().contains("timeout"));
    }

    #[test]
    fn error_display_tunnel_creation() {
        let err = CoreDeviceTunnelError::TunnelCreation("ip parse error".into());
        assert!(err.to_string().contains("tunnel"));
    }

    #[test]
    fn error_display_agent_connection() {
        let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
        let err = CoreDeviceTunnelError::AgentConnection(io_err);
        assert!(err.to_string().contains("agent port"));
    }
}
