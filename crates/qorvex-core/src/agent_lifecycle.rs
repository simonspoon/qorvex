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
//! 4. **Retry** up to a configurable limit: respawn if the runner died, keep
//!    waiting on it if it is merely slow to come up, and restart it once in the
//!    last window if waiting has not paid off
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

use tracing::{debug, info, instrument};

use crate::agent_client::AgentClient;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const XCODEPROJ: &str = "QorvexAgent.xcodeproj";
const SCHEME: &str = "QorvexAgentUITests";
const TEST_CLASS: &str = "QorvexAgentUITests/QorvexAgentTests/testRunAgent";
const DERIVED_DATA_DIR: &str = ".build";
const AGENT_BUNDLE_ID: &str = "com.qorvex.agent";
/// Build setting the agent project derives both target bundle IDs from
/// (see `qorvex-agent/project.yml`). Overriding this one variable renames the
/// app and its UI-test runner together, keeping them distinct.
const AGENT_BUNDLE_ID_VAR: &str = "QORVEX_AGENT_BUNDLE_ID";

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
    /// Whether the target is a physical device (`true`) or a simulator (`false`).
    pub is_physical: bool,
    /// Tunnel address for reaching the device (tunneld or direct network).
    ///
    /// When set, the lifecycle uses this address instead of usbmuxd for
    /// readiness polling and connectivity checks on physical devices.
    pub tunnel_address: Option<String>,
    /// mDNS hostname for direct TCP connection to a WiFi (localNetwork) device.
    ///
    /// When set, the lifecycle connects directly to `{direct_host}:{agent_port}`
    /// instead of going through usbmuxd or the CoreDevice tunnel.
    pub direct_host: Option<String>,
    /// Apple Development Team ID for code-signing on physical devices.
    /// Required when `is_physical` is set: xcodebuild overrides
    /// `DEVELOPMENT_TEAM`, `CODE_SIGN_IDENTITY`, and `CODE_SIGN_STYLE` so the
    /// agent can be deployed without modifying `project.yml` (important for
    /// open-source repos). Absent, the build would produce a runner signed with
    /// whatever identity the packaged project resolves to — not the user's — so
    /// [`AgentLifecycle::build_agent`] fails fast instead.
    pub development_team: Option<String>,
    /// Override bundle ID for the agent when the default is claimed by another team.
    pub agent_bundle_id: Option<String>,
}

impl AgentLifecycleConfig {
    /// Create a new config pointing at the given project directory.
    pub fn new(project_dir: PathBuf) -> Self {
        Self {
            project_dir,
            agent_port: 8080,
            // Sized from measurement, not intuition. Across 26 sides of
            // simultaneous cold starts, healthy ones reached ready in 101-131s,
            // drifting with machine load. `ensure_running` spends the first
            // `max_retries` windows waiting on one progressing launch, so
            // patience runs to 3 x 60s = 180s — clear of the top of that range,
            // which matters because the rescue respawn at the end of that
            // stretch would otherwise kill a healthy, nearly-ready runner. The
            // final window is then 60s against the 22-28s a warm relaunch
            // needs. A runner that *dies* is still caught by `try_wait` on the
            // next 500ms poll, so the longer window costs nothing there.
            // Physical devices override this to 120s.
            startup_timeout: Duration::from_secs(60),
            max_retries: 3,
            is_physical: false,
            tunnel_address: None,
            direct_host: None,
            development_team: None,
            agent_bundle_id: None,
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

    /// A physical-device build was requested but no Apple Development Team is
    /// configured, so the agent cannot be signed for the user's team.
    #[error(
        "no Apple Development Team configured — the agent cannot be code-signed for this device. \
         Set `development_team` in ~/.qorvex/config.json to your 10-character Team ID (Xcode ▸ \
         Settings ▸ Accounts, or https://developer.apple.com/account under Membership details). \
         If `com.qorvex.agent` is already registered to another team, also set `agent_bundle_id` \
         to an identifier your team owns (e.g. \"com.example.qorvex.agent\")."
    )]
    SigningNotConfigured,

    /// `xcodebuild test-without-building` failed to spawn.
    #[error("Failed to launch agent: {0}")]
    LaunchFailed(String),

    /// `xcodebuild test-without-building` exited early with an error.
    #[error("Agent process exited: {0}")]
    SpawnFailed(String),

    /// The agent did not respond to heartbeat within the startup timeout.
    #[error("Agent failed to become ready within timeout")]
    StartupTimeout,

    /// An operation was attempted that requires the agent to be running.
    #[error("Agent is not running")]
    NotRunning,

    /// The agent already listening on the shared port is bound to a different
    /// device than the one requested. Reusing it would silently drive the wrong
    /// simulator, so the lifecycle fails fast instead.
    #[error(
        "another agent is already running on 127.0.0.1:{port} for a different simulator \
         ({holder}), but {requested} was requested — run `qorvex stop-agent` to release the \
         port, or target the device already running"
    )]
    DeviceMismatch {
        /// UDID of the simulator the listening agent is driving.
        holder: String,
        /// UDID of the simulator the caller asked for.
        requested: String,
        /// The shared TCP port the conflict is on.
        port: u16,
    },

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

    /// Build the argv for `xcodebuild build-for-testing`.
    ///
    /// Split out from [`build_agent`](Self::build_agent) so the signing
    /// overrides — the part that decides which team the physical-device runner
    /// is signed for — are testable without invoking xcodebuild.
    ///
    /// # Errors
    ///
    /// - [`AgentLifecycleError::SigningNotConfigured`] when building for a
    ///   physical device with no `development_team`
    fn build_args(&self, xcodeproj: &std::path::Path) -> Result<Vec<String>, AgentLifecycleError> {
        let destination = if self.config.is_physical {
            "generic/platform=iOS"
        } else {
            "generic/platform=iOS Simulator"
        };

        let mut args = vec![
            "build-for-testing".to_string(),
            "-project".to_string(),
            xcodeproj.to_string_lossy().to_string(),
            "-scheme".to_string(),
            SCHEME.to_string(),
            "-destination".to_string(),
            destination.to_string(),
            "-derivedDataPath".to_string(),
            self.config
                .project_dir
                .join(DERIVED_DATA_DIR)
                .to_string_lossy()
                .to_string(),
        ];

        // Physical devices must be signed for the user's own team. The packaged
        // project builds unsigned (`CODE_SIGNING_ALLOWED = NO`), so without an
        // explicit team the runner is not signed for the caller at all —
        // refuse rather than deploy someone else's identity.
        if self.config.is_physical {
            let team = self
                .config
                .development_team
                .as_ref()
                .ok_or(AgentLifecycleError::SigningNotConfigured)?;
            args.push(format!("DEVELOPMENT_TEAM={}", team));
            args.push("CODE_SIGN_STYLE=Automatic".to_string());
            args.push("CODE_SIGN_IDENTITY=Apple Development".to_string());
            args.push("CODE_SIGNING_ALLOWED=YES".to_string());
            args.push("CODE_SIGNING_REQUIRED=YES".to_string());
            args.push("-allowProvisioningUpdates".to_string());
            if let Some(ref bid) = self.config.agent_bundle_id {
                // Override the project's `QORVEX_AGENT_BUNDLE_ID` variable, not
                // `PRODUCT_BUNDLE_IDENTIFIER` directly: a command-line build
                // setting applies to every target, which would give the app and
                // the UI-test bundle the same identifier. The project derives
                // both ids from this variable so they stay distinct.
                args.push(format!("{}={}", AGENT_BUNDLE_ID_VAR, bid));
            }
        }

        Ok(args)
    }

    /// Build the XCTest bundle via `xcodebuild build-for-testing`.
    ///
    /// Verifies the project directory and `.xcodeproj` exist, then runs the
    /// build. Stdout is suppressed and stderr is captured for error reporting.
    ///
    /// # Errors
    ///
    /// - [`AgentLifecycleError::ProjectNotFound`] if the project dir or xcodeproj does not exist
    /// - [`AgentLifecycleError::SigningNotConfigured`] if a physical-device build has no team
    /// - [`AgentLifecycleError::BuildFailed`] if xcodebuild returns a non-zero exit code
    /// - [`AgentLifecycleError::Io`] if the command fails to execute
    #[instrument(skip(self))]
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

        let args = self.build_args(&xcodeproj)?;

        let output = Command::new("xcodebuild")
            .args(&args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AgentLifecycleError::BuildFailed(stderr.to_string()));
        }

        info!("agent build complete");
        Ok(())
    }

    /// Spawn the agent via `xcodebuild test-without-building`.
    ///
    /// Launches xcodebuild as a child process and stores the handle for later
    /// cleanup. Stdout is suppressed to avoid TUI interference; stderr is
    /// captured so that failures can be diagnosed.
    ///
    /// # Errors
    ///
    /// - [`AgentLifecycleError::LaunchFailed`] if the command fails to spawn
    #[instrument(skip(self))]
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
                &self
                    .config
                    .project_dir
                    .join(DERIVED_DATA_DIR)
                    .to_string_lossy(),
                "-only-testing",
                TEST_CLASS,
            ])
            .env(
                "TEST_RUNNER_QORVEX_PORT",
                self.config.agent_port.to_string(),
            )
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| AgentLifecycleError::LaunchFailed(e.to_string()))?;

        let mut guard = self.child.lock().unwrap();
        *guard = Some(child);

        debug!(
            port = self.config.agent_port,
            "passing port to agent via TEST_RUNNER_QORVEX_PORT"
        );
        info!("agent process spawned");
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
        // This only applies to simulators; physical devices do not support simctl.
        if !self.config.is_physical {
            let _ = Command::new("xcrun")
                .args(["simctl", "terminate", &self.udid, AGENT_BUNDLE_ID])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output();
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
    #[instrument(skip(self))]
    pub async fn wait_for_ready(&self) -> Result<(), AgentLifecycleError> {
        let deadline = tokio::time::Instant::now() + self.config.startup_timeout;
        let addr = self.agent_addr();

        loop {
            if self.config.is_physical {
                // For physical devices, try connection methods in order:
                // 1. Direct host (WiFi/localNetwork — mDNS hostname)
                // 2. Tunnel address (tunneld)
                // 3. USB tunnel (usbmuxd) → CoreDevice tunnel (iOS 17+)
                let reachable = if let Some(ref host) = self.config.direct_host {
                    let host_port = format!("{}:{}", host, self.config.agent_port);
                    tokio::net::TcpStream::connect(host_port.as_str())
                        .await
                        .is_ok()
                } else if let Some(ref tunnel_addr) = self.config.tunnel_address {
                    crate::usb_tunnel::connect_tunneld(tunnel_addr, self.config.agent_port)
                        .await
                        .is_ok()
                } else {
                    // Try usbmuxd first, then fall back to native CoreDevice tunnel (iOS 17+).
                    let via_usb = crate::usb_tunnel::connect(&self.udid, self.config.agent_port)
                        .await
                        .is_ok();
                    if via_usb {
                        true
                    } else {
                        crate::core_device_tunnel::connect_coredevice(
                            &self.udid,
                            self.config.agent_port,
                        )
                        .await
                        .is_ok()
                    }
                };
                if reachable {
                    info!("agent ready");
                    return Ok(());
                }
            } else {
                let mut client = AgentClient::new(addr);
                if client.connect().await.is_ok() {
                    if client.heartbeat().await.is_ok() {
                        client.disconnect();
                        info!("agent ready");
                        return Ok(());
                    }
                    client.disconnect();
                }
            }

            // Check if xcodebuild exited early (e.g. build products missing,
            // simulator not booted, signing error). Without this check we
            // silently poll until timeout while the process is already dead.
            {
                let mut guard = self.child.lock().unwrap();
                if let Some(ref mut child) = *guard {
                    if let Some(status) = child.try_wait().ok().flatten() {
                        // Collect stderr for diagnostics.
                        let stderr = child
                            .stderr
                            .take()
                            .and_then(|mut s| {
                                let mut buf = String::new();
                                use std::io::Read;
                                s.read_to_string(&mut buf).ok()?;
                                Some(buf)
                            })
                            .unwrap_or_default();
                        let detail = if stderr.is_empty() {
                            format!("exit code {}", status)
                        } else {
                            // Truncate to last meaningful lines.
                            let tail: String = stderr
                                .lines()
                                .rev()
                                .take(20)
                                .collect::<Vec<_>>()
                                .into_iter()
                                .rev()
                                .collect::<Vec<_>>()
                                .join("\n");
                            format!("exit code {} — {}", status, tail.trim())
                        };
                        return Err(AgentLifecycleError::SpawnFailed(detail));
                    }
                }
            }

            if tokio::time::Instant::now() >= deadline {
                return Err(AgentLifecycleError::StartupTimeout);
            }

            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// Check whether the agent XCTest bundle has already been built.
    ///
    /// Looks for a `.xctestrun` file in the derived-data `Build/Products`
    /// directory. Returns `true` when pre-built products exist (e.g. from
    /// `install.sh`), allowing [`ensure_running`](Self::ensure_running) to
    /// skip the build step. Only consulted for simulators: physical-device
    /// products must be rebuilt so they carry the caller's signing identity.
    fn is_agent_built(&self) -> bool {
        let products_dir = self
            .config
            .project_dir
            .join(DERIVED_DATA_DIR)
            .join("Build/Products");
        if !products_dir.exists() {
            return false;
        }
        // The xctestrun filename encodes the platform, e.g.:
        //   QorvexAgentUITests_iphonesimulator18.0-arm64.xctestrun  (simulator)
        //   QorvexAgentUITests_iphoneos18.0-arm64.xctestrun          (physical)
        // Only match the platform that matches is_physical to avoid using
        // a simulator build on a physical device (or vice versa).
        let platform_prefix = if self.config.is_physical {
            "iphoneos"
        } else {
            "iphonesimulator"
        };
        std::fs::read_dir(&products_dir)
            .map(|entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    let name = e.file_name();
                    let name = name.to_string_lossy();
                    name.ends_with(".xctestrun") && name.contains(platform_prefix)
                })
            })
            .unwrap_or(false)
    }

    /// Orchestrate the full agent startup: build (if needed), spawn, and wait for ready.
    ///
    /// On simulators the build step is skipped when pre-built products are
    /// detected (see [`is_agent_built`](Self::is_agent_built)). Physical devices
    /// always rebuild — see below. If [`wait_for_ready`](Self::wait_for_ready)
    /// reports the runner died, the agent is terminated and respawned up to
    /// [`AgentLifecycleConfig::max_retries`] times. If it merely timed out while
    /// the runner is still alive, the same launch is waited on again — except in
    /// the last window, where the runner is restarted once in case it is wedged
    /// rather than slow.
    ///
    /// # Errors
    ///
    /// - Any error from [`build_agent`](Self::build_agent)
    /// - Any error from [`spawn_agent`](Self::spawn_agent)
    /// - [`AgentLifecycleError::StartupTimeout`] if all retries are exhausted
    #[instrument(skip(self))]
    pub async fn ensure_running(&self) -> Result<(), AgentLifecycleError> {
        // Pre-built `iphoneos` products carry whatever signing identity the
        // build that produced them resolved — typically none, since they come
        // from a packaging step that never saw the user's team. Reusing them
        // deploys a runner that is not signed for this device's developer, so
        // physical devices always run the signing-aware build. `build-for-testing`
        // is incremental: when the products already match, this is a no-op.
        if !self.config.is_physical && self.is_agent_built() {
            info!("using pre-built agent");
        } else {
            info!("building agent");
            self.build_agent()?;
        }
        self.spawn_agent()?;

        for attempt in 0..=self.config.max_retries {
            match self.wait_for_ready().await {
                Ok(()) => {
                    info!("agent running after attempt {}", attempt);
                    return Ok(());
                }
                Err(AgentLifecycleError::SpawnFailed(_)) if attempt < self.config.max_retries => {
                    // The runner process exited: whatever it was doing is over,
                    // so terminate (to clear the dead child and any stray app)
                    // and respawn for the next attempt.
                    let _ = self.terminate_agent();
                    self.spawn_agent()?;
                }
                Err(AgentLifecycleError::StartupTimeout) if attempt < self.config.max_retries => {
                    // The deadline passed but the runner is *still alive* —
                    // `wait_for_ready` would have reported `SpawnFailed` had the
                    // child exited. It is slow, not dead: a cold xcodebuild plus
                    // a booting simulator regularly needs longer than one
                    // `startup_timeout` when two sessions contend for the CPU.
                    // Killing it would throw away that progress and start the
                    // same slow launch from zero — that impatience was the
                    // original bug, where a launch never got more than one
                    // window and two contending sessions never converged.
                    // So: wait on the launch we already have, and only give up
                    // on it once. `attempt + 1 == max_retries` is the last
                    // window in which a respawn can still be waited on, so a
                    // slow launch has had every earlier window to itself (three
                    // of the four at the defaults — 180s, clear of the 101-131s
                    // a healthy contended start needs) before anything is
                    // killed. A runner that is not slow but *wedged* — alive,
                    // never binding the port — has by then demonstrated that
                    // patience will not pay off, and the alternative to
                    // restarting it is certain failure, so spend the final
                    // window on a fresh launch.
                    if attempt + 1 == self.config.max_retries {
                        info!(
                            "agent still not ready after attempt {}; restarting it once for the final window",
                            attempt
                        );
                        let _ = self.terminate_agent();
                        self.spawn_agent()?;
                    } else {
                        debug!(
                            "agent still starting after attempt {}; waiting again",
                            attempt
                        );
                    }
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
        if self.config.is_physical {
            let check = async {
                if let Some(ref host) = self.config.direct_host {
                    let host_port = format!("{}:{}", host, self.config.agent_port);
                    tokio::net::TcpStream::connect(host_port.as_str())
                        .await
                        .is_ok()
                } else if let Some(ref tunnel_addr) = self.config.tunnel_address {
                    crate::usb_tunnel::connect_tunneld(tunnel_addr, self.config.agent_port)
                        .await
                        .is_ok()
                } else {
                    // Try usbmuxd first, then fall back to native CoreDevice tunnel (iOS 17+).
                    let via_usb = crate::usb_tunnel::connect(&self.udid, self.config.agent_port)
                        .await
                        .is_ok();
                    if via_usb {
                        true
                    } else {
                        crate::core_device_tunnel::connect_coredevice(
                            &self.udid,
                            self.config.agent_port,
                        )
                        .await
                        .is_ok()
                    }
                }
            };
            tokio::time::timeout(Duration::from_secs(2), check)
                .await
                .unwrap_or(false)
        } else {
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

    /// Ask the agent currently listening on the local port which simulator it is
    /// driving.
    ///
    /// Returns `Some(udid)` only when an agent answers and reports a UDID; returns
    /// `None` when nothing answers in time or the agent cannot report its identity
    /// (e.g. an older agent that predates the `DeviceUdid` opcode). Simulator-only:
    /// a physical-device agent has no `SIMULATOR_UDID` and is reached over a
    /// device-specific tunnel, so cross-attach cannot occur there.
    async fn reachable_agent_udid(&self) -> Option<String> {
        let addr = self.agent_addr();
        let check = async {
            let mut client = AgentClient::new(addr);
            client.connect().await.ok()?;
            let udid = client.device_udid().await.ok().flatten();
            client.disconnect();
            udid
        };
        tokio::time::timeout(Duration::from_secs(2), check)
            .await
            .ok()
            .flatten()
    }

    /// Ensure the agent is running, starting it only if not already reachable.
    ///
    /// Unlike [`ensure_running`](Self::ensure_running) which always rebuilds,
    /// this method first checks whether the agent is already listening and
    /// skips the build/spawn cycle if it is.
    ///
    /// On simulators the agent is a singleton on the shared loopback port: every
    /// booted simulator answers on `127.0.0.1:{agent_port}`, so a bare
    /// reachability probe cannot tell whether the listening agent is driving the
    /// requested device. Before reusing it, this confirms its reported UDID
    /// matches [`self.udid`]; a mismatch returns [`AgentLifecycleError::DeviceMismatch`]
    /// rather than silently attaching to another session's simulator.
    #[instrument(skip(self))]
    pub async fn ensure_agent_ready(&self) -> Result<(), AgentLifecycleError> {
        if self.is_agent_reachable().await {
            if !self.config.is_physical {
                if let Some(holder) = self.reachable_agent_udid().await {
                    if !holder.eq_ignore_ascii_case(&self.udid) {
                        return Err(AgentLifecycleError::DeviceMismatch {
                            holder,
                            requested: self.udid.clone(),
                            port: self.config.agent_port,
                        });
                    }
                }
            }
            debug!("agent already reachable, skipping build");
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
        assert_eq!(config.startup_timeout, Duration::from_secs(60));
        assert_eq!(config.max_retries, 3);
        assert!(!config.is_physical);
    }

    #[test]
    fn config_custom_values() {
        let config = AgentLifecycleConfig {
            project_dir: PathBuf::from("/tmp/custom"),
            agent_port: 12345,
            startup_timeout: Duration::from_secs(10),
            max_retries: 5,
            is_physical: false,
            tunnel_address: None,
            direct_host: None,
            development_team: None,
            agent_bundle_id: None,
        };

        assert_eq!(config.project_dir, PathBuf::from("/tmp/custom"));
        assert_eq!(config.agent_port, 12345);
        assert_eq!(config.startup_timeout, Duration::from_secs(10));
        assert_eq!(config.max_retries, 5);
    }

    // -- Build argument tests -----------------------------------------------

    /// Build a lifecycle for a physical device with the given signing config.
    fn physical(team: Option<&str>, bundle_id: Option<&str>) -> AgentLifecycle {
        let mut config = AgentLifecycleConfig::new(PathBuf::from("/tmp/agent"));
        config.is_physical = true;
        config.development_team = team.map(str::to_string);
        config.agent_bundle_id = bundle_id.map(str::to_string);
        AgentLifecycle::new("DEVICE-UDID".to_string(), config)
    }

    fn args_of(lifecycle: &AgentLifecycle) -> Vec<String> {
        lifecycle
            .build_args(&PathBuf::from("/tmp/agent/QorvexAgent.xcodeproj"))
            .expect("build args")
    }

    #[test]
    fn physical_build_signs_for_the_configured_team() {
        let args = args_of(&physical(Some("ABCDE12345"), None));

        assert!(args.contains(&"DEVELOPMENT_TEAM=ABCDE12345".to_string()));
        assert!(args.contains(&"CODE_SIGNING_REQUIRED=YES".to_string()));
        assert!(args.contains(&"-allowProvisioningUpdates".to_string()));
        assert!(args.contains(&"generic/platform=iOS".to_string()));
    }

    #[test]
    fn physical_build_without_a_team_is_refused() {
        // Regression: the build used to silently omit the signing settings,
        // producing a runner signed for whatever identity the packaged project
        // resolved to rather than the user's development team.
        let err = physical(None, None)
            .build_args(&PathBuf::from("/tmp/agent/QorvexAgent.xcodeproj"))
            .expect_err("a physical build with no team must fail");
        assert!(matches!(err, AgentLifecycleError::SigningNotConfigured));
        assert!(err.to_string().contains("development_team"));
    }

    #[test]
    fn bundle_id_override_renames_both_targets_via_one_variable() {
        // Overriding PRODUCT_BUNDLE_IDENTIFIER on the command line would apply
        // to every target and collide the app with its UI-test runner; the
        // project derives both ids from QORVEX_AGENT_BUNDLE_ID instead.
        let args = args_of(&physical(Some("ABCDE12345"), Some("com.example.agent")));

        assert!(args.contains(&"QORVEX_AGENT_BUNDLE_ID=com.example.agent".to_string()));
        assert!(!args
            .iter()
            .any(|a| a.starts_with("PRODUCT_BUNDLE_IDENTIFIER=")));
    }

    #[test]
    fn simulator_build_is_unsigned_and_needs_no_team() {
        let config = AgentLifecycleConfig::new(PathBuf::from("/tmp/agent"));
        let lifecycle = AgentLifecycle::new("SIM-UDID".to_string(), config);
        let args = args_of(&lifecycle);

        assert!(args.contains(&"generic/platform=iOS Simulator".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("DEVELOPMENT_TEAM=")));
    }

    // -- Error display tests ------------------------------------------------

    #[test]
    fn error_display_project_not_found() {
        let err = AgentLifecycleError::ProjectNotFound(PathBuf::from("/missing/project"));
        assert_eq!(err.to_string(), "Agent project not found: /missing/project");
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
            is_physical: false,
            tunnel_address: None,
            direct_host: None,
            development_team: None,
            agent_bundle_id: None,
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
            is_physical: false,
            tunnel_address: None,
            direct_host: None,
            development_team: None,
            agent_bundle_id: None,
        };
        let lifecycle = AgentLifecycle::new("test-udid".to_string(), config);

        assert!(!lifecycle.is_agent_reachable().await);
    }

    /// Start a mock simulator agent that answers heartbeats with `Ok` and
    /// `DeviceUdid` probes with the given UDID, serving connections until the
    /// test ends. Returns the loopback port it bound.
    async fn mock_sim_agent(holder_udid: &str) -> u16 {
        use crate::protocol::{
            decode_request, encode_response, read_frame_length, Request, Response,
        };
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let holder = holder_udid.to_string();

        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let holder = holder.clone();
                tokio::spawn(async move {
                    loop {
                        let mut header = [0u8; 4];
                        if stream.read_exact(&mut header).await.is_err() {
                            break; // client disconnected
                        }
                        let len = read_frame_length(&header) as usize;
                        let mut payload = vec![0u8; len];
                        if stream.read_exact(&mut payload).await.is_err() {
                            break;
                        }
                        let resp = match decode_request(&payload) {
                            Ok(Request::DeviceUdid) => Response::Value {
                                value: Some(holder.clone()),
                            },
                            _ => Response::Ok,
                        };
                        if stream.write_all(&encode_response(&resp)).await.is_err() {
                            break;
                        }
                        let _ = stream.flush().await;
                    }
                });
            }
        });

        port
    }

    fn sim_lifecycle_on_port(udid: &str, port: u16) -> AgentLifecycle {
        let mut config = AgentLifecycleConfig::new(PathBuf::from("/tmp/agent"));
        config.agent_port = port;
        AgentLifecycle::new(udid.to_string(), config)
    }

    #[tokio::test]
    async fn ensure_agent_ready_rejects_cross_device_attach() {
        // An agent for "OTHER-SIM" already holds the shared port; asking for
        // "WANTED-SIM" must fail fast instead of silently driving OTHER-SIM.
        let port = mock_sim_agent("OTHER-SIM").await;
        let lifecycle = sim_lifecycle_on_port("WANTED-SIM", port);

        let result = lifecycle.ensure_agent_ready().await;
        match result {
            Err(AgentLifecycleError::DeviceMismatch {
                holder,
                requested,
                port: p,
            }) => {
                assert_eq!(holder, "OTHER-SIM");
                assert_eq!(requested, "WANTED-SIM");
                assert_eq!(p, port);
            }
            other => panic!("expected DeviceMismatch, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn ensure_agent_ready_reuses_matching_device() {
        // The reachable agent is driving the requested simulator (case-insensitive
        // match), so it is reused without spawning a new one.
        let port = mock_sim_agent("same-sim").await;
        let lifecycle = sim_lifecycle_on_port("SAME-SIM", port);

        lifecycle.ensure_agent_ready().await.unwrap();
    }

    #[tokio::test]
    async fn wait_for_ready_times_out_when_nothing_listening() {
        let config = AgentLifecycleConfig {
            project_dir: PathBuf::from("/tmp/agent"),
            agent_port: 19998,
            startup_timeout: Duration::from_secs(1),
            max_retries: 3,
            is_physical: false,
            tunnel_address: None,
            direct_host: None,
            development_team: None,
            agent_bundle_id: None,
        };
        let lifecycle = AgentLifecycle::new("test-udid".to_string(), config);

        let result = lifecycle.wait_for_ready().await;
        assert!(matches!(result, Err(AgentLifecycleError::StartupTimeout)));
    }
}
