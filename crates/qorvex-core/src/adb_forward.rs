//! Single `adb forward` TCP tunnel to the on-device Kotlin agent.
//!
//! This module collapses the three iOS transport modules
//! ([`usb_tunnel`](crate::usb_tunnel), [`coredevice`](crate::coredevice),
//! [`core_device_tunnel`](crate::core_device_tunnel)) into one. On Android,
//! `adb` already unifies USB, emulator, and network (`adb connect`) transports
//! — a device is addressed by its adb `serial` and `adb` owns the underlying
//! link — so a single `adb forward` rule reaches the agent uniformly for every
//! device kind (ADR-3).
//!
//! [`AdbForward::establish`] runs
//! `adb -s <serial> forward tcp:<local_port> tcp:<device_port>` and the host
//! then talks to `127.0.0.1:<local_port>` — identical to the simulator
//! `Direct` connection path, so the driver reuses
//! [`AgentClient::new`](crate::agent_client::AgentClient::new) with no
//! `from_stream` plumbing.
//!
//! The forward rule lives in the adb server, independent of the agent process
//! and of any one TCP connection, so it can be torn down explicitly with
//! [`AdbForward::remove`] (and as a safety net in [`Drop`]), leaving no
//! orphaned `adb forward` entries (D2), and re-issued idempotently on a
//! forward-level failure with [`AdbForward::ensure`] (the reconnect strategy,
//! ADR-3 §4).
//!
//! # Example
//!
//! ```no_run
//! use qorvex_core::adb_forward::AdbForward;
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Open a tunnel to the agent's device-side port 8080, letting the host
//! // pick a free local port.
//! let mut forward = AdbForward::establish("emulator-5554", None, 8080)?;
//! let addr = forward.local_addr(); // e.g. "127.0.0.1:43217"
//! // ... connect AgentClient::new(addr) through the forward ...
//!
//! // On session end the forward is released (also done on drop).
//! forward.remove()?;
//! # Ok(())
//! # }
//! ```

use std::process::Command;
use thiserror::Error;

/// The loopback host the forwarded port is bound on. The driver connects here.
const LOCALHOST: &str = "127.0.0.1";

// ---------------------------------------------------------------------------
// Error type (mirrors usb_tunnel::UsbTunnelError in shape and intent)
// ---------------------------------------------------------------------------

/// Errors that can occur during `adb forward` tunnel operations.
#[derive(Error, Debug)]
pub enum AdbForwardError {
    /// The `adb forward` command could not be executed (adb missing from PATH,
    /// or an I/O error spawning the process).
    #[error("failed to execute adb: {0}")]
    AdbUnavailable(String),

    /// `adb forward` returned a non-zero exit status (e.g. device not found,
    /// device offline, or the port could not be bound).
    #[error("failed to establish adb forward: {0}")]
    ForwardFailed(String),

    /// adb did not report a usable local port for a `tcp:0` (auto-assign)
    /// forward request.
    #[error("adb forward returned no local port")]
    NoLocalPort,

    /// Removing the forward rule failed.
    #[error("failed to remove adb forward: {0}")]
    RemoveFailed(String),
}

// ---------------------------------------------------------------------------
// AdbForward
// ---------------------------------------------------------------------------

/// An established `adb forward` tunnel from a host loopback port to the agent's
/// device-side TCP port.
///
/// The host connects to `127.0.0.1:<local_port>` (see [`AdbForward::local_addr`])
/// and adb bridges the bytes to `<device_port>` on the device identified by
/// `serial`. The forward is released by [`AdbForward::remove`] or, as a safety
/// net, when the value is dropped — so no orphaned `adb forward` entries remain
/// after a session (D2).
#[derive(Debug)]
pub struct AdbForward {
    /// The adb serial of the target device (emulator / USB / network).
    serial: String,
    /// The host-side loopback port adb bound for this forward.
    local_port: u16,
    /// The agent's TCP port inside the device.
    device_port: u16,
    /// Set once [`remove`](AdbForward::remove) has released the rule, so the
    /// [`Drop`] safety net does not attempt a redundant removal.
    removed: bool,
}

impl AdbForward {
    /// Establish an `adb forward` tunnel to the device-side agent port.
    ///
    /// Runs `adb -s <serial> forward tcp:<local_port> tcp:<device_port>`. The
    /// same command works for emulators, USB-physical, and `adb connect`
    /// network devices because the device is addressed by its adb `serial`
    /// (ADR-3). On success the host can reach the agent at
    /// `127.0.0.1:<local_port>` (see [`local_addr`](AdbForward::local_addr)).
    ///
    /// # Arguments
    ///
    /// * `serial` - The adb serial of the target device.
    /// * `local_port` - The host loopback port to bind. Pass `None` (or
    ///   `Some(0)`) to let adb pick a free port (`tcp:0`); the assigned port is
    ///   read back from adb's stdout and exposed via
    ///   [`local_port`](AdbForward::local_port).
    /// * `device_port` - The agent's TCP port inside the device (e.g. `8080`).
    ///
    /// # Errors
    ///
    /// - [`AdbForwardError::AdbUnavailable`] if `adb` cannot be executed.
    /// - [`AdbForwardError::ForwardFailed`] if adb returns a non-zero status.
    /// - [`AdbForwardError::NoLocalPort`] if a `tcp:0` request yields no port.
    pub fn establish(
        serial: &str,
        local_port: Option<u16>,
        device_port: u16,
    ) -> Result<Self, AdbForwardError> {
        let requested = local_port.unwrap_or(0);
        let output = Command::new("adb")
            .args([
                "-s",
                serial,
                "forward",
                &format!("tcp:{requested}"),
                &format!("tcp:{device_port}"),
            ])
            .output()
            .map_err(|e| AdbForwardError::AdbUnavailable(e.to_string()))?;

        if !output.status.success() {
            return Err(AdbForwardError::ForwardFailed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }

        // When a fixed port was requested, adb prints nothing on success and we
        // keep the requested port. When `tcp:0` was requested, adb prints the
        // assigned port on stdout.
        let bound_port = if requested == 0 {
            Self::parse_assigned_port(&String::from_utf8_lossy(&output.stdout))
                .ok_or(AdbForwardError::NoLocalPort)?
        } else {
            requested
        };

        Ok(Self {
            serial: serial.to_string(),
            local_port: bound_port,
            device_port,
            removed: false,
        })
    }

    /// Re-issue the `adb forward` rule, re-establishing the tunnel in place.
    ///
    /// This is the forward-level reconnect step (ADR-3 §4): on a forward-level
    /// failure (device re-plugged, emulator reboot, `adb` server bounce) the
    /// rule is re-issued before the driver reconnects its TCP socket. Re-issuing
    /// the same rule is safe and idempotent — adb overwrites the existing rule
    /// for that local port — so the bound `local_port` is preserved and the
    /// driver keeps connecting to the same `127.0.0.1:<local_port>`.
    ///
    /// A dropped *TCP* connection (agent restart, transient stall) does **not**
    /// remove the forward, so the driver's plain socket reconnect generally
    /// succeeds without calling this; `ensure` is the fallback when that fails.
    ///
    /// # Errors
    ///
    /// - [`AdbForwardError::AdbUnavailable`] if `adb` cannot be executed.
    /// - [`AdbForwardError::ForwardFailed`] if adb returns a non-zero status.
    pub fn ensure(&mut self) -> Result<(), AdbForwardError> {
        let output = Command::new("adb")
            .args([
                "-s",
                &self.serial,
                "forward",
                &format!("tcp:{}", self.local_port),
                &format!("tcp:{}", self.device_port),
            ])
            .output()
            .map_err(|e| AdbForwardError::AdbUnavailable(e.to_string()))?;

        if !output.status.success() {
            return Err(AdbForwardError::ForwardFailed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        // Re-establishing a fixed port keeps the same `local_port`; nothing to
        // read back.
        self.removed = false;
        Ok(())
    }

    /// Release the forward rule:
    /// `adb -s <serial> forward --remove tcp:<local_port>`.
    ///
    /// Called on session end (D2). Idempotent: a second call (including the
    /// [`Drop`] safety net) is a no-op once the rule has been removed.
    ///
    /// # Errors
    ///
    /// - [`AdbForwardError::AdbUnavailable`] if `adb` cannot be executed.
    /// - [`AdbForwardError::RemoveFailed`] if adb returns a non-zero status.
    pub fn remove(&mut self) -> Result<(), AdbForwardError> {
        if self.removed {
            return Ok(());
        }
        let output = Command::new("adb")
            .args([
                "-s",
                &self.serial,
                "forward",
                "--remove",
                &format!("tcp:{}", self.local_port),
            ])
            .output()
            .map_err(|e| AdbForwardError::AdbUnavailable(e.to_string()))?;

        if !output.status.success() {
            return Err(AdbForwardError::RemoveFailed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        self.removed = true;
        Ok(())
    }

    /// The host loopback port adb bound for this forward.
    ///
    /// This is what the driver connects to (the `Direct`/simulator path reused
    /// over `127.0.0.1`).
    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    /// The agent's device-side TCP port this forward bridges to.
    pub fn device_port(&self) -> u16 {
        self.device_port
    }

    /// The adb serial of the device this forward targets.
    pub fn serial(&self) -> &str {
        &self.serial
    }

    /// The `127.0.0.1:<local_port>` address the driver connects to.
    ///
    /// Hand this straight to
    /// [`AgentClient::new`](crate::agent_client::AgentClient::new).
    pub fn local_addr(&self) -> String {
        format!("{LOCALHOST}:{}", self.local_port)
    }

    /// Parse the local port adb prints to stdout after a `tcp:0` (auto-assign)
    /// forward request.
    ///
    /// adb prints the assigned port number alone on a line (e.g. `43217`).
    /// Exposed for unit testing. Returns the first all-digit token that parses
    /// as a `u16`, or `None` if the output carries no port.
    fn parse_assigned_port(stdout: &str) -> Option<u16> {
        stdout.split_whitespace().find_map(|tok| tok.parse().ok())
    }

    /// Parse the output of `adb forward --list` into `(serial, local, remote)`
    /// triples.
    ///
    /// Each line is `"<serial> tcp:<local> tcp:<remote>"`. Exposed for unit
    /// testing the teardown / no-orphan-entries logic without a live device.
    pub fn parse_forward_list(output: &str) -> Vec<(String, String, String)> {
        output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let serial = fields.next()?.to_string();
                let local = fields.next()?.to_string();
                let remote = fields.next()?.to_string();
                Some((serial, local, remote))
            })
            .collect()
    }
}

impl Drop for AdbForward {
    /// Safety net: release the forward rule if it was not removed explicitly,
    /// so a session that ends without calling [`remove`](AdbForward::remove)
    /// (panic, early return) still leaves no orphaned `adb forward` entries
    /// (D2). Errors are ignored — drop cannot fail and the explicit
    /// [`remove`](AdbForward::remove) is the path that surfaces failures.
    fn drop(&mut self) {
        if !self.removed {
            let _ = self.remove();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_mirrors_usb_tunnel_style() {
        let err = AdbForwardError::AdbUnavailable("not found".into());
        assert!(err.to_string().contains("adb"));

        let err = AdbForwardError::ForwardFailed("device offline".into());
        assert!(err.to_string().contains("device offline"));

        let err = AdbForwardError::NoLocalPort;
        assert!(err.to_string().contains("no local port"));

        let err = AdbForwardError::RemoveFailed("rule not found".into());
        assert!(err.to_string().contains("rule not found"));
    }

    #[test]
    fn parse_assigned_port_reads_bare_number() {
        assert_eq!(AdbForward::parse_assigned_port("43217\n"), Some(43217));
        assert_eq!(AdbForward::parse_assigned_port("  5037 "), Some(5037));
    }

    #[test]
    fn parse_assigned_port_none_when_absent() {
        assert_eq!(AdbForward::parse_assigned_port(""), None);
        assert_eq!(AdbForward::parse_assigned_port("\n"), None);
    }

    #[test]
    fn parse_assigned_port_rejects_out_of_range() {
        // 70000 does not fit in a u16 → no usable port parsed.
        assert_eq!(AdbForward::parse_assigned_port("70000"), None);
    }

    #[test]
    fn local_addr_is_loopback_with_bound_port() {
        let forward = AdbForward {
            serial: "emulator-5554".into(),
            local_port: 43217,
            device_port: 8080,
            removed: false,
        };
        assert_eq!(forward.local_addr(), "127.0.0.1:43217");
        assert_eq!(forward.local_port(), 43217);
        assert_eq!(forward.device_port(), 8080);
        assert_eq!(forward.serial(), "emulator-5554");
    }

    #[test]
    fn remove_is_idempotent_once_removed() {
        // With `removed` already set, `remove` short-circuits without spawning
        // adb, so it succeeds even when adb is absent — proving the teardown /
        // Drop safety-net path does not double-remove.
        let mut forward = AdbForward {
            serial: "emulator-5554".into(),
            local_port: 43217,
            device_port: 8080,
            removed: true,
        };
        assert!(forward.remove().is_ok());
    }

    #[test]
    fn parse_forward_list_extracts_rules() {
        let output = "emulator-5554 tcp:43217 tcp:8080\n\
192.168.1.42:5555 tcp:43218 tcp:8080\n";
        let rules = AdbForward::parse_forward_list(output);
        assert_eq!(rules.len(), 2);
        assert_eq!(
            rules[0],
            (
                "emulator-5554".into(),
                "tcp:43217".into(),
                "tcp:8080".into()
            )
        );
        assert_eq!(rules[1].0, "192.168.1.42:5555");
    }

    #[test]
    fn parse_forward_list_no_orphans_after_remove() {
        // Models the no-orphaned-entries check (D2): after teardown a device's
        // local-port rule must be absent from `adb forward --list`.
        let before = "emulator-5554 tcp:43217 tcp:8080\n";
        let after = ""; // rule removed
        assert_eq!(AdbForward::parse_forward_list(before).len(), 1);
        let remaining = AdbForward::parse_forward_list(after);
        assert!(!remaining
            .iter()
            .any(|(_, local, _)| local == "tcp:43217"));
    }

    #[test]
    fn parse_forward_list_empty() {
        assert!(AdbForward::parse_forward_list("").is_empty());
        assert!(AdbForward::parse_forward_list("\n\n").is_empty());
    }
}
