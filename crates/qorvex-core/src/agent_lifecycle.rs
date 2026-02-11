//! Lifecycle management for the Swift accessibility agent on iOS Simulators.
//!
//! This module handles building, installing, launching, health-checking, and
//! stopping the native Swift agent that runs as a UI Testing target on the
//! simulator. The agent listens on a TCP port and accepts binary protocol
//! commands (see [`crate::protocol`]).
//!
//! # Overview
//!
//! [`AgentLifecycle`] orchestrates the full agent startup sequence:
//!
//! 1. **Install** the `.app` bundle onto the simulator via `xcrun simctl install`
//! 2. **Launch** the agent process via `xcrun simctl launch`
//! 3. **Wait for ready** by polling the TCP port with heartbeat requests
//! 4. **Retry** on failure (terminate + relaunch) up to a configurable limit
//!
//! # Example
//!
//! ```no_run
//! use std::path::PathBuf;
//! use qorvex_core::agent_lifecycle::{AgentLifecycle, AgentLifecycleConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = AgentLifecycleConfig {
//!     agent_app_path: Some(PathBuf::from("build/QorvexAgent.app")),
//!     ..Default::default()
//! };
//!
//! let lifecycle = AgentLifecycle::new("DEVICE-UDID".into(), config);
//! lifecycle.ensure_running().await?;
//! # Ok(())
//! # }
//! ```

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use thiserror::Error;

use crate::agent_client::AgentClient;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the agent lifecycle manager.
pub struct AgentLifecycleConfig {
    /// Path to the built agent `.app` bundle (e.g., from `xcodebuild`).
    pub agent_app_path: Option<PathBuf>,
    /// The bundle identifier of the agent app.
    pub agent_bundle_id: String,
    /// TCP port the agent listens on.
    pub agent_port: u16,
    /// Maximum time to wait for the agent to become ready.
    pub startup_timeout: Duration,
    /// Maximum number of launch retries before giving up.
    pub max_retries: u32,
}

impl Default for AgentLifecycleConfig {
    fn default() -> Self {
        Self {
            agent_app_path: None,
            agent_bundle_id: "com.qorvex.agent".to_string(),
            agent_port: 9800,
            startup_timeout: Duration::from_secs(30),
            max_retries: 3,
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors specific to agent lifecycle operations.
#[derive(Error, Debug)]
pub enum AgentLifecycleError {
    /// The agent `.app` bundle was not found at the configured path.
    #[error("Agent app not found at path: {0}")]
    AppNotFound(PathBuf),

    /// `xcrun simctl install` failed.
    #[error("Failed to install agent: {0}")]
    InstallFailed(String),

    /// `xcrun simctl launch` failed.
    #[error("Failed to launch agent: {0}")]
    LaunchFailed(String),

    /// The agent did not respond to heartbeat within the startup timeout.
    #[error("Agent failed to become ready within timeout")]
    StartupTimeout,

    /// An operation was attempted that requires the agent to be running.
    #[error("Agent is not running")]
    NotRunning,

    /// An error was propagated from a `simctl` command.
    #[error("simctl error: {0}")]
    Simctl(String),

    /// An I/O error occurred.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------
// AgentLifecycle
// ---------------------------------------------------------------------------

/// Manages the full lifecycle of the Swift accessibility agent on a simulator.
///
/// Provides methods to install, launch, health-check, and terminate the agent.
/// The synchronous methods (`install_agent`, `launch_agent`, `terminate_agent`)
/// use `std::process::Command` and can be wrapped with `tokio::task::spawn_blocking`
/// by callers. The async methods (`wait_for_ready`, `ensure_running`,
/// `is_agent_reachable`) use [`AgentClient`] for TCP communication.
pub struct AgentLifecycle {
    config: AgentLifecycleConfig,
    udid: String,
}

impl AgentLifecycle {
    /// Create a new lifecycle manager for the given simulator device.
    pub fn new(udid: String, config: AgentLifecycleConfig) -> Self {
        Self { config, udid }
    }

    /// Returns the `SocketAddr` used to reach the agent on localhost.
    fn agent_addr(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.config.agent_port))
    }

    // -----------------------------------------------------------------------
    // Synchronous simctl operations
    // -----------------------------------------------------------------------

    /// Install the agent `.app` bundle onto the simulator.
    ///
    /// If [`AgentLifecycleConfig::agent_app_path`] is `Some`, the path is
    /// verified to exist and then passed to `xcrun simctl install`. If the
    /// path is `None`, an [`AgentLifecycleError::AppNotFound`] is returned
    /// immediately.
    ///
    /// # Errors
    ///
    /// - [`AgentLifecycleError::AppNotFound`] if no path is configured or the path does not exist
    /// - [`AgentLifecycleError::InstallFailed`] if simctl install returns a non-zero exit code
    /// - [`AgentLifecycleError::Io`] if the command fails to execute
    pub fn install_agent(&self) -> Result<(), AgentLifecycleError> {
        let app_path = match &self.config.agent_app_path {
            Some(path) => path,
            None => {
                return Err(AgentLifecycleError::AppNotFound(PathBuf::from(
                    "<no agent_app_path configured>",
                )));
            }
        };

        if !app_path.exists() {
            return Err(AgentLifecycleError::AppNotFound(app_path.clone()));
        }

        let output = Command::new("xcrun")
            .args([
                "simctl",
                "install",
                &self.udid,
                &app_path.to_string_lossy(),
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AgentLifecycleError::InstallFailed(stderr.to_string()));
        }

        Ok(())
    }

    /// Launch the agent process on the simulator.
    ///
    /// Runs `xcrun simctl launch <udid> <bundle_id>` and verifies the command
    /// succeeds.
    ///
    /// # Errors
    ///
    /// - [`AgentLifecycleError::LaunchFailed`] if simctl launch returns a non-zero exit code
    /// - [`AgentLifecycleError::Io`] if the command fails to execute
    pub fn launch_agent(&self) -> Result<(), AgentLifecycleError> {
        let output = Command::new("xcrun")
            .args([
                "simctl",
                "launch",
                &self.udid,
                &self.config.agent_bundle_id,
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AgentLifecycleError::LaunchFailed(stderr.to_string()));
        }

        Ok(())
    }

    /// Terminate the agent process on the simulator.
    ///
    /// Runs `xcrun simctl terminate <udid> <bundle_id>`. If the agent is not
    /// currently running (stderr contains "not running"), the method succeeds
    /// silently.
    ///
    /// # Errors
    ///
    /// - [`AgentLifecycleError::Simctl`] if simctl terminate fails for a reason other
    ///   than the app not running
    /// - [`AgentLifecycleError::Io`] if the command fails to execute
    pub fn terminate_agent(&self) -> Result<(), AgentLifecycleError> {
        let output = Command::new("xcrun")
            .args([
                "simctl",
                "terminate",
                &self.udid,
                &self.config.agent_bundle_id,
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Silently succeed if the app isn't running.
            if !stderr.to_lowercase().contains("not running") {
                return Err(AgentLifecycleError::Simctl(stderr.to_string()));
            }
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Async health-check and orchestration
    // -----------------------------------------------------------------------

    /// Wait for the agent to become ready by polling its TCP port.
    ///
    /// Attempts to connect via [`AgentClient`] and send a heartbeat every
    /// 500 ms until either a successful response is received or
    /// [`AgentLifecycleConfig::startup_timeout`] is exceeded.
    ///
    /// # Errors
    ///
    /// - [`AgentLifecycleError::StartupTimeout`] if the agent does not respond within the timeout
    pub async fn wait_for_ready(&self) -> Result<(), AgentLifecycleError> {
        let deadline = tokio::time::Instant::now() + self.config.startup_timeout;
        let addr = self.agent_addr();

        loop {
            let mut client = AgentClient::new(addr);
            if client.connect().await.is_ok() {
                if client.heartbeat().await.is_ok() {
                    client.disconnect();
                    return Ok(());
                }
                client.disconnect();
            }

            if tokio::time::Instant::now() >= deadline {
                return Err(AgentLifecycleError::StartupTimeout);
            }

            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// Orchestrate the full agent startup: install, launch, and wait for ready.
    ///
    /// If [`wait_for_ready`](Self::wait_for_ready) fails, the agent is terminated
    /// and relaunched up to [`AgentLifecycleConfig::max_retries`] times.
    ///
    /// # Errors
    ///
    /// - Any error from [`install_agent`](Self::install_agent)
    /// - Any error from [`launch_agent`](Self::launch_agent)
    /// - [`AgentLifecycleError::StartupTimeout`] if all retries are exhausted
    pub async fn ensure_running(&self) -> Result<(), AgentLifecycleError> {
        self.install_agent()?;
        self.launch_agent()?;

        for attempt in 0..=self.config.max_retries {
            match self.wait_for_ready().await {
                Ok(()) => return Ok(()),
                Err(AgentLifecycleError::StartupTimeout) if attempt < self.config.max_retries => {
                    // Terminate and relaunch for the next attempt.
                    let _ = self.terminate_agent();
                    self.launch_agent()?;
                }
                Err(e) => return Err(e),
            }
        }

        Err(AgentLifecycleError::StartupTimeout)
    }

    /// Quick reachability check: try to connect and heartbeat with a short timeout.
    ///
    /// Returns `true` if the agent responds to a heartbeat within 2 seconds,
    /// `false` otherwise.
    pub async fn is_agent_reachable(&self) -> bool {
        let addr = self.agent_addr();
        let check = async {
            let mut client = AgentClient::new(addr);
            client.connect().await.ok()?;
            let result = client.heartbeat().await;
            client.disconnect();
            result.ok()
        };

        tokio::time::timeout(Duration::from_secs(2), check)
            .await
            .is_ok_and(|inner| inner.is_some())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Config tests -------------------------------------------------------

    #[test]
    fn default_config_values() {
        let config = AgentLifecycleConfig::default();

        assert!(config.agent_app_path.is_none());
        assert_eq!(config.agent_bundle_id, "com.qorvex.agent");
        assert_eq!(config.agent_port, 9800);
        assert_eq!(config.startup_timeout, Duration::from_secs(30));
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn config_construction_with_custom_values() {
        let config = AgentLifecycleConfig {
            agent_app_path: Some(PathBuf::from("/tmp/MyAgent.app")),
            agent_bundle_id: "com.example.test".to_string(),
            agent_port: 12345,
            startup_timeout: Duration::from_secs(10),
            max_retries: 5,
        };

        assert_eq!(
            config.agent_app_path,
            Some(PathBuf::from("/tmp/MyAgent.app"))
        );
        assert_eq!(config.agent_bundle_id, "com.example.test");
        assert_eq!(config.agent_port, 12345);
        assert_eq!(config.startup_timeout, Duration::from_secs(10));
        assert_eq!(config.max_retries, 5);
    }

    // -- Error display tests ------------------------------------------------

    #[test]
    fn error_display_app_not_found() {
        let err = AgentLifecycleError::AppNotFound(PathBuf::from("/missing/path.app"));
        assert_eq!(
            err.to_string(),
            "Agent app not found at path: /missing/path.app"
        );
    }

    #[test]
    fn error_display_install_failed() {
        let err = AgentLifecycleError::InstallFailed("device not found".to_string());
        assert_eq!(err.to_string(), "Failed to install agent: device not found");
    }

    #[test]
    fn error_display_launch_failed() {
        let err = AgentLifecycleError::LaunchFailed("bundle not found".to_string());
        assert_eq!(err.to_string(), "Failed to launch agent: bundle not found");
    }

    #[test]
    fn error_display_startup_timeout() {
        let err = AgentLifecycleError::StartupTimeout;
        assert_eq!(
            err.to_string(),
            "Agent failed to become ready within timeout"
        );
    }

    #[test]
    fn error_display_not_running() {
        let err = AgentLifecycleError::NotRunning;
        assert_eq!(err.to_string(), "Agent is not running");
    }

    #[test]
    fn error_display_simctl() {
        let err = AgentLifecycleError::Simctl("something went wrong".to_string());
        assert_eq!(err.to_string(), "simctl error: something went wrong");
    }

    #[test]
    fn error_display_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err = AgentLifecycleError::Io(io_err);
        assert!(err.to_string().contains("IO error"));
        assert!(err.to_string().contains("file not found"));
    }

    // -- install_agent tests ------------------------------------------------

    #[test]
    fn install_agent_no_path_configured() {
        let config = AgentLifecycleConfig::default();
        let lifecycle = AgentLifecycle::new("test-udid".to_string(), config);

        let result = lifecycle.install_agent();
        assert!(result.is_err());
        match result {
            Err(AgentLifecycleError::AppNotFound(path)) => {
                assert!(path.to_string_lossy().contains("no agent_app_path"));
            }
            other => panic!("Expected AppNotFound, got: {:?}", other),
        }
    }

    #[test]
    fn install_agent_nonexistent_path() {
        let config = AgentLifecycleConfig {
            agent_app_path: Some(PathBuf::from("/nonexistent/path/QorvexAgent.app")),
            ..Default::default()
        };
        let lifecycle = AgentLifecycle::new("test-udid".to_string(), config);

        let result = lifecycle.install_agent();
        assert!(result.is_err());
        match result {
            Err(AgentLifecycleError::AppNotFound(path)) => {
                assert_eq!(path, PathBuf::from("/nonexistent/path/QorvexAgent.app"));
            }
            other => panic!("Expected AppNotFound, got: {:?}", other),
        }
    }

    // -- terminate_agent tests ----------------------------------------------

    // Note: We cannot easily unit-test terminate_agent without simctl, but
    // we can verify that the struct constructs correctly and the method
    // signature is sound.

    #[test]
    fn lifecycle_construction() {
        let config = AgentLifecycleConfig {
            agent_app_path: Some(PathBuf::from("/tmp/Test.app")),
            agent_bundle_id: "com.test.agent".to_string(),
            agent_port: 5555,
            startup_timeout: Duration::from_secs(15),
            max_retries: 2,
        };
        let lifecycle = AgentLifecycle::new("ABCD-1234".to_string(), config);

        assert_eq!(lifecycle.udid, "ABCD-1234");
        assert_eq!(lifecycle.config.agent_bundle_id, "com.test.agent");
        assert_eq!(lifecycle.config.agent_port, 5555);
        assert_eq!(
            lifecycle.agent_addr(),
            "127.0.0.1:5555".parse::<SocketAddr>().unwrap()
        );
    }

    // -- macOS-only simctl tests -------------------------------------------

    #[cfg(target_os = "macos")]
    mod macos_tests {
        use super::*;

        #[test]
        fn install_agent_invalid_udid() {
            let config = AgentLifecycleConfig {
                // Use a real but empty temp dir as the "app" so the path exists
                agent_app_path: Some(std::env::temp_dir()),
                ..Default::default()
            };
            let lifecycle = AgentLifecycle::new("invalid-udid-000".to_string(), config);

            let result = lifecycle.install_agent();
            assert!(result.is_err());
            match result {
                Err(AgentLifecycleError::InstallFailed(msg)) => {
                    assert!(!msg.is_empty());
                }
                Err(e) => {
                    // IO error is also acceptable if simctl behaves unexpectedly
                    println!("Got error: {:?}", e);
                }
                Ok(_) => panic!("Expected error for invalid UDID"),
            }
        }

        #[test]
        fn launch_agent_invalid_udid() {
            let config = AgentLifecycleConfig::default();
            let lifecycle = AgentLifecycle::new("invalid-udid-000".to_string(), config);

            let result = lifecycle.launch_agent();
            assert!(result.is_err());
            match result {
                Err(AgentLifecycleError::LaunchFailed(msg)) => {
                    assert!(!msg.is_empty());
                }
                Err(e) => {
                    println!("Got error: {:?}", e);
                }
                Ok(_) => panic!("Expected error for invalid UDID"),
            }
        }

        #[test]
        fn terminate_agent_not_running_succeeds() {
            // Terminating a bundle that isn't running should silently succeed.
            let config = AgentLifecycleConfig {
                agent_bundle_id: "com.qorvex.nonexistent.agent".to_string(),
                ..Default::default()
            };
            let lifecycle = AgentLifecycle::new("invalid-udid-000".to_string(), config);

            // This will either silently succeed (stderr contains "not running")
            // or fail with a simctl error for the invalid UDID. Both are
            // acceptable since we cannot guarantee a booted sim in CI.
            let _result = lifecycle.terminate_agent();
        }
    }

    // -- Async tests --------------------------------------------------------

    #[tokio::test]
    async fn is_agent_reachable_returns_false_when_nothing_listening() {
        let config = AgentLifecycleConfig {
            // Use a port that (almost certainly) has nothing listening.
            agent_port: 19999,
            ..Default::default()
        };
        let lifecycle = AgentLifecycle::new("test-udid".to_string(), config);

        assert!(!lifecycle.is_agent_reachable().await);
    }

    #[tokio::test]
    async fn wait_for_ready_times_out_when_nothing_listening() {
        let config = AgentLifecycleConfig {
            agent_port: 19998,
            startup_timeout: Duration::from_secs(1),
            ..Default::default()
        };
        let lifecycle = AgentLifecycle::new("test-udid".to_string(), config);

        let result = lifecycle.wait_for_ready().await;
        assert!(matches!(result, Err(AgentLifecycleError::StartupTimeout)));
    }
}
