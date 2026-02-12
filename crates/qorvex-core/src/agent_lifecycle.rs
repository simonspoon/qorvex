//! Lifecycle management for the Swift accessibility agent on iOS Simulators.
//!
//! This module handles building, launching, health-checking, and stopping the
//! native Swift agent that runs as a UI Testing target on the simulator. The
//! agent listens on a TCP port and accepts binary protocol commands (see
//! [`crate::protocol`]).
//!
//! # Overview
//!
//! [`AgentLifecycle`] orchestrates the full agent startup sequence:
//!
//! 1. **Build** the XCTest bundle via `xcodebuild build-for-testing`
//! 2. **Spawn** the agent via `xcodebuild test-without-building`
//! 3. **Wait for ready** by polling the TCP port with heartbeat requests
//! 4. **Retry** on failure (terminate + respawn) up to a configurable limit
//!
//! # Example
//!
//! ```no_run
//! use std::path::PathBuf;
//! use qorvex_core::agent_lifecycle::{AgentLifecycle, AgentLifecycleConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = AgentLifecycleConfig::new(PathBuf::from("qorvex-agent"));
//! let lifecycle = AgentLifecycle::new("DEVICE-UDID".into(), config);
//! lifecycle.ensure_running().await?;
//! # Ok(())
//! # }
//! ```

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use thiserror::Error;

use crate::agent_client::AgentClient;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const XCODEPROJ: &str = "QorvexAgent.xcodeproj";
const SCHEME: &str = "QorvexAgentUITests";
const TEST_CLASS: &str = "QorvexAgentUITests/QorvexAgentTests/testRunAgent";
const DERIVED_DATA_DIR: &str = ".build";
const AGENT_BUNDLE_ID: &str = "com.qorvex.agent";

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the agent lifecycle manager.
pub struct AgentLifecycleConfig {
    /// Path to the Swift agent project directory (containing the `.xcodeproj`).
    pub project_dir: PathBuf,
    /// TCP port the agent listens on.
    pub agent_port: u16,
    /// Maximum time to wait for the agent to become ready.
    pub startup_timeout: Duration,
    /// Maximum number of launch retries before giving up.
    pub max_retries: u32,
}

impl AgentLifecycleConfig {
    /// Create a new config pointing at the given project directory.
    pub fn new(project_dir: PathBuf) -> Self {
        Self {
            project_dir,
            agent_port: 8080,
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
    /// The agent project directory or `.xcodeproj` was not found.
    #[error("Agent project not found: {0}")]
    ProjectNotFound(PathBuf),

    /// `xcodebuild build-for-testing` failed.
    #[error("Failed to build agent: {0}")]
    BuildFailed(String),

    /// `xcodebuild test-without-building` failed to spawn.
    #[error("Failed to launch agent: {0}")]
    LaunchFailed(String),

    /// The agent did not respond to heartbeat within the startup timeout.
    #[error("Agent failed to become ready within timeout")]
    StartupTimeout,

    /// An operation was attempted that requires the agent to be running.
    #[error("Agent is not running")]
    NotRunning,

    /// An I/O error occurred.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------
// AgentLifecycle
// ---------------------------------------------------------------------------

/// Manages the full lifecycle of the Swift accessibility agent on a simulator.
///
/// Provides methods to build, spawn, health-check, and terminate the agent.
/// The synchronous methods (`build_agent`, `spawn_agent`, `terminate_agent`)
/// use `std::process::Command` and can be wrapped with `tokio::task::spawn_blocking`
/// by callers. The async methods (`wait_for_ready`, `ensure_running`,
/// `is_agent_reachable`) use [`AgentClient`] for TCP communication.
pub struct AgentLifecycle {
    config: AgentLifecycleConfig,
    udid: String,
    child: Mutex<Option<std::process::Child>>,
}

impl Drop for AgentLifecycle {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.child.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

impl AgentLifecycle {
    /// Create a new lifecycle manager for the given simulator device.
    pub fn new(udid: String, config: AgentLifecycleConfig) -> Self {
        Self {
            config,
            udid,
            child: Mutex::new(None),
        }
    }

    /// Returns the `SocketAddr` used to reach the agent on localhost.
    fn agent_addr(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.config.agent_port))
    }

    // -----------------------------------------------------------------------
    // Synchronous xcodebuild operations
    // -----------------------------------------------------------------------

    /// Build the XCTest bundle via `xcodebuild build-for-testing`.
    ///
    /// Verifies the project directory and `.xcodeproj` exist, then runs the
    /// build. Stdout is suppressed and stderr is captured for error reporting.
    ///
    /// # Errors
    ///
    /// - [`AgentLifecycleError::ProjectNotFound`] if the project dir or xcodeproj does not exist
    /// - [`AgentLifecycleError::BuildFailed`] if xcodebuild returns a non-zero exit code
    /// - [`AgentLifecycleError::Io`] if the command fails to execute
    pub fn build_agent(&self) -> Result<(), AgentLifecycleError> {
        if !self.config.project_dir.exists() {
            return Err(AgentLifecycleError::ProjectNotFound(
                self.config.project_dir.clone(),
            ));
        }

        let xcodeproj = self.config.project_dir.join(XCODEPROJ);
        if !xcodeproj.exists() {
            return Err(AgentLifecycleError::ProjectNotFound(xcodeproj));
        }

        let output = Command::new("xcodebuild")
            .args([
                "build-for-testing",
                "-project",
                &xcodeproj.to_string_lossy(),
                "-scheme",
                SCHEME,
                "-destination",
                &format!("id={}", self.udid),
                "-derivedDataPath",
                &self.config
                    .project_dir
                    .join(DERIVED_DATA_DIR)
                    .to_string_lossy(),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AgentLifecycleError::BuildFailed(stderr.to_string()));
        }

        Ok(())
    }

    /// Spawn the agent via `xcodebuild test-without-building`.
    ///
    /// Launches xcodebuild as a child process and stores the handle for later
    /// cleanup. Stdout and stderr are suppressed to avoid TUI interference.
    ///
    /// # Errors
    ///
    /// - [`AgentLifecycleError::LaunchFailed`] if the command fails to spawn
    pub fn spawn_agent(&self) -> Result<(), AgentLifecycleError> {
        let xcodeproj = self.config.project_dir.join(XCODEPROJ);

        let child = Command::new("xcodebuild")
            .args([
                "test-without-building",
                "-project",
                &xcodeproj.to_string_lossy(),
                "-scheme",
                SCHEME,
                "-destination",
                &format!("id={}", self.udid),
                "-derivedDataPath",
                &self.config
                    .project_dir
                    .join(DERIVED_DATA_DIR)
                    .to_string_lossy(),
                "-only-testing",
                TEST_CLASS,
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| AgentLifecycleError::LaunchFailed(e.to_string()))?;

        let mut guard = self.child.lock().unwrap();
        *guard = Some(child);

        Ok(())
    }

    /// Terminate the agent process.
    ///
    /// Kills the stored child process (if any), then falls back to
    /// `xcrun simctl terminate` in case the agent is still running.
    pub fn terminate_agent(&self) -> Result<(), AgentLifecycleError> {
        let mut guard = self.child.lock().unwrap();
        if let Some(mut child) = guard.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        drop(guard);

        // Fallback: simctl terminate in case the agent process is still around.
        let _ = Command::new("xcrun")
            .args(["simctl", "terminate", &self.udid, AGENT_BUNDLE_ID])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();

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

    /// Orchestrate the full agent startup: build, spawn, and wait for ready.
    ///
    /// If [`wait_for_ready`](Self::wait_for_ready) fails, the agent is terminated
    /// and respawned up to [`AgentLifecycleConfig::max_retries`] times.
    ///
    /// # Errors
    ///
    /// - Any error from [`build_agent`](Self::build_agent)
    /// - Any error from [`spawn_agent`](Self::spawn_agent)
    /// - [`AgentLifecycleError::StartupTimeout`] if all retries are exhausted
    pub async fn ensure_running(&self) -> Result<(), AgentLifecycleError> {
        self.build_agent()?;
        self.spawn_agent()?;

        for attempt in 0..=self.config.max_retries {
            match self.wait_for_ready().await {
                Ok(()) => return Ok(()),
                Err(AgentLifecycleError::StartupTimeout) if attempt < self.config.max_retries => {
                    // Terminate and respawn for the next attempt.
                    let _ = self.terminate_agent();
                    self.spawn_agent()?;
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

    /// Ensure the agent is running, starting it only if not already reachable.
    ///
    /// Unlike [`ensure_running`](Self::ensure_running) which always rebuilds,
    /// this method first checks whether the agent is already listening and
    /// skips the build/spawn cycle if it is.
    pub async fn ensure_agent_ready(&self) -> Result<(), AgentLifecycleError> {
        if self.is_agent_reachable().await {
            return Ok(());
        }
        self.ensure_running().await
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
    fn config_new_defaults() {
        let config = AgentLifecycleConfig::new(PathBuf::from("/tmp/agent"));

        assert_eq!(config.project_dir, PathBuf::from("/tmp/agent"));
        assert_eq!(config.agent_port, 8080);
        assert_eq!(config.startup_timeout, Duration::from_secs(30));
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn config_custom_values() {
        let config = AgentLifecycleConfig {
            project_dir: PathBuf::from("/tmp/custom"),
            agent_port: 12345,
            startup_timeout: Duration::from_secs(10),
            max_retries: 5,
        };

        assert_eq!(config.project_dir, PathBuf::from("/tmp/custom"));
        assert_eq!(config.agent_port, 12345);
        assert_eq!(config.startup_timeout, Duration::from_secs(10));
        assert_eq!(config.max_retries, 5);
    }

    // -- Error display tests ------------------------------------------------

    #[test]
    fn error_display_project_not_found() {
        let err = AgentLifecycleError::ProjectNotFound(PathBuf::from("/missing/project"));
        assert_eq!(
            err.to_string(),
            "Agent project not found: /missing/project"
        );
    }

    #[test]
    fn error_display_build_failed() {
        let err = AgentLifecycleError::BuildFailed("scheme not found".to_string());
        assert_eq!(err.to_string(), "Failed to build agent: scheme not found");
    }

    #[test]
    fn error_display_launch_failed() {
        let err = AgentLifecycleError::LaunchFailed("spawn failed".to_string());
        assert_eq!(err.to_string(), "Failed to launch agent: spawn failed");
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
    fn error_display_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err = AgentLifecycleError::Io(io_err);
        assert!(err.to_string().contains("IO error"));
        assert!(err.to_string().contains("file not found"));
    }

    // -- build_agent tests --------------------------------------------------

    #[test]
    fn build_agent_project_dir_not_found() {
        let config = AgentLifecycleConfig::new(PathBuf::from("/nonexistent/project"));
        let lifecycle = AgentLifecycle::new("test-udid".to_string(), config);

        let result = lifecycle.build_agent();
        assert!(result.is_err());
        match result {
            Err(AgentLifecycleError::ProjectNotFound(path)) => {
                assert_eq!(path, PathBuf::from("/nonexistent/project"));
            }
            other => panic!("Expected ProjectNotFound, got: {:?}", other),
        }
    }

    #[test]
    fn build_agent_xcodeproj_not_found() {
        // Use temp dir as project dir (exists but has no .xcodeproj)
        let config = AgentLifecycleConfig::new(std::env::temp_dir());
        let lifecycle = AgentLifecycle::new("test-udid".to_string(), config);

        let result = lifecycle.build_agent();
        assert!(result.is_err());
        match result {
            Err(AgentLifecycleError::ProjectNotFound(path)) => {
                assert!(path.to_string_lossy().contains(XCODEPROJ));
            }
            other => panic!("Expected ProjectNotFound, got: {:?}", other),
        }
    }

    // -- terminate_agent tests ----------------------------------------------

    #[test]
    fn terminate_agent_no_child() {
        let config = AgentLifecycleConfig::new(PathBuf::from("/tmp/agent"));
        let lifecycle = AgentLifecycle::new("test-udid".to_string(), config);

        // Should succeed even with no child process.
        let result = lifecycle.terminate_agent();
        assert!(result.is_ok());
    }

    // -- lifecycle construction tests ---------------------------------------

    #[test]
    fn lifecycle_construction() {
        let config = AgentLifecycleConfig {
            project_dir: PathBuf::from("/tmp/agent"),
            agent_port: 5555,
            startup_timeout: Duration::from_secs(15),
            max_retries: 2,
        };
        let lifecycle = AgentLifecycle::new("ABCD-1234".to_string(), config);

        assert_eq!(lifecycle.udid, "ABCD-1234");
        assert_eq!(lifecycle.config.agent_port, 5555);
        assert_eq!(
            lifecycle.agent_addr(),
            "127.0.0.1:5555".parse::<SocketAddr>().unwrap()
        );
        assert!(lifecycle.child.lock().unwrap().is_none());
    }

    // -- Async tests --------------------------------------------------------

    #[tokio::test]
    async fn is_agent_reachable_returns_false_when_nothing_listening() {
        let config = AgentLifecycleConfig {
            project_dir: PathBuf::from("/tmp/agent"),
            // Use a port that (almost certainly) has nothing listening.
            agent_port: 19999,
            startup_timeout: Duration::from_secs(30),
            max_retries: 3,
        };
        let lifecycle = AgentLifecycle::new("test-udid".to_string(), config);

        assert!(!lifecycle.is_agent_reachable().await);
    }

    #[tokio::test]
    async fn wait_for_ready_times_out_when_nothing_listening() {
        let config = AgentLifecycleConfig {
            project_dir: PathBuf::from("/tmp/agent"),
            agent_port: 19998,
            startup_timeout: Duration::from_secs(1),
            max_retries: 3,
        };
        let lifecycle = AgentLifecycle::new("test-udid".to_string(), config);

        let result = lifecycle.wait_for_ready().await;
        assert!(matches!(result, Err(AgentLifecycleError::StartupTimeout)));
    }
}
