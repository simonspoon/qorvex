//! Lifecycle management for the Kotlin UiAutomator agent on Android devices.
//!
//! This module is the Android counterpart of
//! [`agent_lifecycle`](crate::agent_lifecycle). It brings the Kotlin
//! instrumentation agent (story #84, in `qorvex-agent-android/`) from source to
//! a healthy, reachable TCP server, mirroring the iOS lifecycle shape exactly
//! (ADR-2 in `.scratch/arch.md`).
//!
//! # Overview
//!
//! [`AndroidLifecycle`] orchestrates the full agent startup sequence:
//!
//! 1. **Build** the agent + instrumentation APKs via `gradlew assembleDebug
//!    assembleDebugAndroidTest` (the Android analog of
//!    `xcodebuild build-for-testing`).
//! 2. **Install** both APKs via `adb install -r`.
//! 3. **Spawn** the long-lived agent via
//!    `adb shell am instrument -w -e qorvex_port <port> -e class <entry>`
//!    as a child process whose handle the lifecycle owns (the `-w` flag keeps
//!    the host `adb` call attached — ADR-2). This blocks in a serve loop on the
//!    device, holding a `ServerSocket` open until killed.
//! 4. **Wait for ready** by polling the agent's heartbeat over the **forwarded**
//!    localhost port every 500 ms until ready or the startup timeout. On each
//!    poll it also `try_wait()`s the `am instrument` child so an early exit
//!    (build products missing, instrumentation crash) surfaces as a distinct,
//!    actionable error instead of silently polling until timeout.
//! 5. **Retry** on failure (terminate + respawn) up to a configurable limit.
//!
//! Each failure mode produces a **distinct** error (story #88 / spec E2):
//!
//! | Failure | Error variant |
//! |---|---|
//! | Gradle build failure | [`AndroidLifecycleError::BuildFailed`] |
//! | `adb install` failure | [`AndroidLifecycleError::InstallFailed`] |
//! | `am instrument` failed to spawn | [`AndroidLifecycleError::LaunchFailed`] |
//! | `am instrument` exited early | [`AndroidLifecycleError::InstrumentFailed`] |
//! | port never opens (agent never heartbeats) | [`AndroidLifecycleError::StartupTimeout`] |
//!
//! # Connection path (ADR-2 / ADR-3)
//!
//! The health poll runs **through** the `adb forward` tunnel (story #86): the
//! agent serves on `device_port` inside the device, `adb forward` binds a host
//! loopback `local_port`, and the lifecycle heartbeats `127.0.0.1:<local_port>`
//! — identical to the simulator `Direct` path, reusing
//! [`AgentClient`](crate::agent_client::AgentClient) unchanged.
//!
//! # Entry point for story #89
//!
//! [`AndroidLifecycle::ensure_running`] is the clean entry point #89 calls to
//! start the Android agent (mirroring `AgentLifecycle::ensure_running`). It
//! takes the host-side `local_port` that an [`AdbForward`](crate::adb_forward)
//! has already bound, polls health through it, and returns once the agent is
//! reachable. #89 wires this into the `start-agent` frontend command / driver
//! startup; this module deliberately does **not** establish the forward itself
//! (that is `AdbForward`'s job, owned by `AndroidDriver`) so the forward and the
//! agent process have independent lifetimes (ADR-3 §4).
//!
//! # Example
//!
//! ```no_run
//! use std::path::PathBuf;
//! use qorvex_core::android_lifecycle::{AndroidLifecycle, AndroidLifecycleConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = AndroidLifecycleConfig::new(PathBuf::from("qorvex-agent-android"));
//! let lifecycle = AndroidLifecycle::new("emulator-5554".into(), config);
//! // `local_port` is the host port an `AdbForward` already bound to the agent.
//! lifecycle.ensure_running(43217).await?;
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

/// The Gradle wrapper script at the agent project root.
const GRADLEW: &str = "gradlew";
/// The host app APK produced by `assembleDebug`.
const APP_APK: &str = "build/outputs/apk/debug/qorvex-agent-android-debug.apk";
/// The instrumentation APK produced by `assembleDebugAndroidTest`. This is the
/// one `am instrument` launches.
const TEST_APK: &str = "build/outputs/apk/androidTest/debug/qorvex-agent-android-debug-androidTest.apk";
/// The agent (host app) package; force-stopped on teardown as a fallback.
const AGENT_PACKAGE: &str = "com.qorvex.agent";
/// The instrumentation target `<package>/<runner>` for `am instrument`.
const INSTRUMENT_TARGET: &str = "com.qorvex.agent.test/androidx.test.runner.AndroidJUnitRunner";
/// The fully-qualified entry-point test method (the long-lived serve loop).
const INSTRUMENT_CLASS: &str = "com.qorvex.agent.QorvexAgentTest#runAgent";
/// Instrumentation arg name carrying the device-side port (mirrors iOS
/// `TEST_RUNNER_QORVEX_PORT`); see ADR-2.
const PORT_ARG: &str = "qorvex_port";

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the Android agent lifecycle manager.
pub struct AndroidLifecycleConfig {
    /// Path to the Kotlin agent project directory (containing `gradlew`).
    pub project_dir: PathBuf,
    /// Device-side TCP port the agent's `ServerSocket` binds (passed as the
    /// `qorvex_port` instrumentation arg).
    pub device_port: u16,
    /// Maximum time to wait for the agent to become ready.
    pub startup_timeout: Duration,
    /// Maximum number of launch retries before giving up.
    pub max_retries: u32,
}

impl AndroidLifecycleConfig {
    /// Create a new config pointing at the given project directory, with
    /// defaults matching the iOS lifecycle (port 8080, 60 s timeout — Gradle
    /// builds and emulator boots are slower than simulators — 3 retries).
    pub fn new(project_dir: PathBuf) -> Self {
        Self {
            project_dir,
            device_port: crate::android_driver::DEFAULT_ANDROID_AGENT_PORT,
            startup_timeout: Duration::from_secs(60),
            max_retries: 3,
        }
    }
}

// ---------------------------------------------------------------------------
// Errors — one distinct variant per failure mode (spec E2)
// ---------------------------------------------------------------------------

/// Errors specific to Android agent lifecycle operations.
///
/// Each failure mode named in the story acceptance maps to a distinct variant
/// so callers (and the frontend in #89) can surface an actionable message.
#[derive(Error, Debug)]
pub enum AndroidLifecycleError {
    /// The agent project directory or `gradlew` was not found.
    #[error("Android agent project not found: {0}")]
    ProjectNotFound(PathBuf),

    /// `gradlew assemble...` failed (Gradle build failure).
    #[error("Failed to build Android agent (Gradle): {0}")]
    BuildFailed(String),

    /// `adb install` rejected one of the APKs.
    #[error("Failed to install Android agent APK: {0}")]
    InstallFailed(String),

    /// `adb shell am instrument` failed to spawn as a child process.
    #[error("Failed to launch Android agent (am instrument): {0}")]
    LaunchFailed(String),

    /// `am instrument` exited early — the instrumentation process died before
    /// the agent became reachable (e.g. instrumentation not found, runner
    /// crash, agent threw on startup). Distinct from `StartupTimeout`.
    #[error("Android agent instrumentation exited: {0}")]
    InstrumentFailed(String),

    /// The agent never responded to a heartbeat within the startup timeout —
    /// the forwarded TCP port never opened. Distinct from `InstrumentFailed`
    /// (the process is still alive but not serving / not reachable).
    #[error("Android agent failed to become ready within timeout (port never opened)")]
    StartupTimeout,

    /// An operation was attempted that requires the agent to be running.
    #[error("Android agent is not running")]
    NotRunning,

    /// An I/O error occurred.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------
// AndroidLifecycle
// ---------------------------------------------------------------------------

/// Manages the full lifecycle of the Kotlin UiAutomator agent on a device.
///
/// Provides methods to build, install, spawn, health-check, and terminate the
/// agent. The synchronous methods (`build_agent`, `install_agent`,
/// `spawn_agent`, `terminate_agent`) use [`std::process::Command`] and can be
/// wrapped with `tokio::task::spawn_blocking` by callers. The async methods
/// (`wait_for_ready`, `ensure_running`, `is_agent_reachable`) use
/// [`AgentClient`] for TCP communication over the forwarded port.
pub struct AndroidLifecycle {
    config: AndroidLifecycleConfig,
    /// adb serial of the target device (emulator-5554 | host:port | USB serial).
    serial: String,
    /// The `am instrument -w` child process (the long-lived agent handle).
    child: Mutex<Option<std::process::Child>>,
}

impl Drop for AndroidLifecycle {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.child.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

impl AndroidLifecycle {
    /// Create a new lifecycle manager for the given adb `serial`.
    pub fn new(serial: String, config: AndroidLifecycleConfig) -> Self {
        Self {
            config,
            serial,
            child: Mutex::new(None),
        }
    }

    /// The adb serial this lifecycle targets.
    pub fn serial(&self) -> &str {
        &self.serial
    }

    /// The device-side port the agent serves on.
    pub fn device_port(&self) -> u16 {
        self.config.device_port
    }

    /// Returns the loopback `SocketAddr` used to reach the agent through the
    /// `adb forward` tunnel.
    fn agent_addr(local_port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], local_port))
    }

    /// Path to the Gradle wrapper at the project root.
    fn gradlew_path(&self) -> PathBuf {
        self.config.project_dir.join(GRADLEW)
    }

    /// Absolute path to the produced APK relative path.
    fn apk_path(&self, rel: &str) -> PathBuf {
        self.config.project_dir.join(rel)
    }

    // -----------------------------------------------------------------------
    // Synchronous build / install / spawn operations
    // -----------------------------------------------------------------------

    /// Build the agent + instrumentation APKs via Gradle.
    ///
    /// Runs `./gradlew assembleDebug assembleDebugAndroidTest` in the project
    /// directory. Stdout is suppressed and stderr is captured for diagnostics.
    /// This is the Android analog of `xcodebuild build-for-testing`.
    ///
    /// # Errors
    ///
    /// - [`AndroidLifecycleError::ProjectNotFound`] if the project dir or
    ///   `gradlew` does not exist
    /// - [`AndroidLifecycleError::BuildFailed`] if Gradle returns a non-zero
    ///   exit code
    /// - [`AndroidLifecycleError::Io`] if the command fails to execute
    #[instrument(skip(self))]
    pub fn build_agent(&self) -> Result<(), AndroidLifecycleError> {
        if !self.config.project_dir.exists() {
            return Err(AndroidLifecycleError::ProjectNotFound(
                self.config.project_dir.clone(),
            ));
        }
        let gradlew = self.gradlew_path();
        if !gradlew.exists() {
            return Err(AndroidLifecycleError::ProjectNotFound(gradlew));
        }

        let output = Command::new(&gradlew)
            .current_dir(&self.config.project_dir)
            .args(["assembleDebug", "assembleDebugAndroidTest"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AndroidLifecycleError::BuildFailed(tail_diagnostic(
                &stderr,
                output.status.to_string(),
            )));
        }

        info!("android agent build complete");
        Ok(())
    }

    /// Install both APKs onto the device via `adb install -r`.
    ///
    /// Installs the host app APK first, then the instrumentation APK. A reused
    /// (`-r`) install overwrites any prior copy. `adb` reports install failures
    /// in stdout (`Failure [...]`) even with a zero exit code, so both the exit
    /// status and stdout text are checked.
    ///
    /// # Errors
    ///
    /// - [`AndroidLifecycleError::InstallFailed`] if either APK is missing or
    ///   `adb install` rejects it.
    #[instrument(skip(self))]
    pub fn install_agent(&self) -> Result<(), AndroidLifecycleError> {
        for rel in [APP_APK, TEST_APK] {
            let apk = self.apk_path(rel);
            if !apk.exists() {
                return Err(AndroidLifecycleError::InstallFailed(format!(
                    "APK not found (run build first): {}",
                    apk.display()
                )));
            }
            self.adb_install(&apk.to_string_lossy())?;
        }
        info!("android agent APKs installed");
        Ok(())
    }

    /// Run a single `adb -s <serial> install -r <apk>` and classify the result.
    fn adb_install(&self, apk: &str) -> Result<(), AndroidLifecycleError> {
        let output = Command::new("adb")
            .args(["-s", &self.serial, "install", "-r", apk])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        // adb sometimes exits 0 on a `Failure [...]` line, so check the text too.
        if !output.status.success() || stdout.contains("Failure") || stderr.contains("Failure") {
            let detail = if !stderr.trim().is_empty() {
                stderr.trim().to_string()
            } else {
                stdout.trim().to_string()
            };
            return Err(AndroidLifecycleError::InstallFailed(format!(
                "{apk}: {detail}"
            )));
        }
        Ok(())
    }

    /// Spawn the long-lived agent via `adb shell am instrument -w`.
    ///
    /// Launches `adb -s <serial> shell am instrument -w -e qorvex_port <port>
    /// -e class <entry> <target>` as a child process and stores the handle for
    /// health-checking and cleanup. `-w` keeps the host `adb` call attached so
    /// this lifecycle owns the agent process handle (the `Child` the recovery
    /// ladder depends on — ADR-2). Stdout is suppressed; stderr is captured so
    /// an early instrumentation exit can be diagnosed.
    ///
    /// # Errors
    ///
    /// - [`AndroidLifecycleError::LaunchFailed`] if the command fails to spawn.
    #[instrument(skip(self))]
    pub fn spawn_agent(&self) -> Result<(), AndroidLifecycleError> {
        let port = self.config.device_port.to_string();
        let child = Command::new("adb")
            .args([
                "-s",
                &self.serial,
                "shell",
                "am",
                "instrument",
                "-w",
                "-e",
                PORT_ARG,
                &port,
                "-e",
                "class",
                INSTRUMENT_CLASS,
                INSTRUMENT_TARGET,
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| AndroidLifecycleError::LaunchFailed(e.to_string()))?;

        let mut guard = self.child.lock().unwrap();
        *guard = Some(child);

        debug!(
            port = self.config.device_port,
            "passing device port to agent via {PORT_ARG} instrumentation arg"
        );
        info!("android agent process spawned (am instrument -w)");
        Ok(())
    }

    /// Terminate the agent process.
    ///
    /// Kills the stored `am instrument` child (if any), then falls back to
    /// `adb shell am force-stop <agent package>` in case the agent process is
    /// still around (the `simctl terminate` analog — ADR-2).
    pub fn terminate_agent(&self) -> Result<(), AndroidLifecycleError> {
        let mut guard = self.child.lock().unwrap();
        if let Some(mut child) = guard.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        drop(guard);

        let _ = Command::new("adb")
            .args([
                "-s",
                &self.serial,
                "shell",
                "am",
                "force-stop",
                AGENT_PACKAGE,
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Async health-check and orchestration
    // -----------------------------------------------------------------------

    /// Wait for the agent to become ready by polling its forwarded TCP port.
    ///
    /// Attempts to connect via [`AgentClient`] to `127.0.0.1:<local_port>` (the
    /// host side of the `adb forward` tunnel) and send a heartbeat every 500 ms
    /// until either a successful response is received or
    /// [`AndroidLifecycleConfig::startup_timeout`] is exceeded.
    ///
    /// On each poll the `am instrument` child is `try_wait()`ed: if it has
    /// exited, a distinct [`AndroidLifecycleError::InstrumentFailed`] is
    /// returned (with stderr tail) instead of silently polling until timeout —
    /// this is what separates "instrument failed" from "port never opened"
    /// (spec E2).
    ///
    /// # Errors
    ///
    /// - [`AndroidLifecycleError::InstrumentFailed`] if `am instrument` exited
    ///   early
    /// - [`AndroidLifecycleError::StartupTimeout`] if the agent never
    ///   heartbeats within the timeout
    #[instrument(skip(self))]
    pub async fn wait_for_ready(&self, local_port: u16) -> Result<(), AndroidLifecycleError> {
        let deadline = tokio::time::Instant::now() + self.config.startup_timeout;
        let addr = Self::agent_addr(local_port);

        loop {
            let mut client = AgentClient::new(addr);
            if client.connect().await.is_ok() {
                if client.heartbeat().await.is_ok() {
                    client.disconnect();
                    info!("android agent ready");
                    return Ok(());
                }
                client.disconnect();
            }

            // Check if `am instrument` exited early (e.g. instrumentation not
            // installed, runner crash, agent threw on startup). Without this
            // check we would silently poll until timeout while the process is
            // already dead — and conflate it with "port never opened".
            if let Some(detail) = self.poll_child_exit() {
                return Err(AndroidLifecycleError::InstrumentFailed(detail));
            }

            if tokio::time::Instant::now() >= deadline {
                return Err(AndroidLifecycleError::StartupTimeout);
            }

            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// If the `am instrument` child has exited, return a diagnostic string
    /// (exit status + stderr tail); otherwise `None` (still running / no child).
    fn poll_child_exit(&self) -> Option<String> {
        let mut guard = self.child.lock().unwrap();
        let child = guard.as_mut()?;
        let status = child.try_wait().ok().flatten()?;
        let stderr = child
            .stderr
            .take()
            .and_then(|mut s| {
                use std::io::Read;
                let mut buf = String::new();
                s.read_to_string(&mut buf).ok()?;
                Some(buf)
            })
            .unwrap_or_default();
        Some(tail_diagnostic(&stderr, status.to_string()))
    }

    /// Orchestrate the full agent startup: build, install, spawn, wait-for-ready.
    ///
    /// This is the clean entry point story #89 calls to start the Android agent
    /// (the analog of `AgentLifecycle::ensure_running`). The caller passes the
    /// host-side `local_port` an [`AdbForward`](crate::adb_forward) has already
    /// bound to the agent's `device_port`; the health poll runs through it.
    ///
    /// If [`wait_for_ready`](Self::wait_for_ready) fails with a respawnable
    /// error (instrument exited / port never opened), the agent is terminated
    /// and respawned up to [`AndroidLifecycleConfig::max_retries`] times. Build
    /// and install run once (they are not retried — a Gradle/install failure is
    /// surfaced immediately and is not a transient startup race).
    ///
    /// # Errors
    ///
    /// - Any error from [`build_agent`](Self::build_agent) /
    ///   [`install_agent`](Self::install_agent) / [`spawn_agent`](Self::spawn_agent)
    /// - [`AndroidLifecycleError::StartupTimeout`] /
    ///   [`AndroidLifecycleError::InstrumentFailed`] if all retries are exhausted
    #[instrument(skip(self))]
    pub async fn ensure_running(&self, local_port: u16) -> Result<(), AndroidLifecycleError> {
        self.build_agent()?;
        self.install_agent()?;
        self.spawn_agent()?;

        let mut last_err = AndroidLifecycleError::StartupTimeout;
        for attempt in 0..=self.config.max_retries {
            match self.wait_for_ready(local_port).await {
                Ok(()) => {
                    info!("android agent running after attempt {attempt}");
                    return Ok(());
                }
                Err(
                    e @ (AndroidLifecycleError::StartupTimeout
                    | AndroidLifecycleError::InstrumentFailed(_)),
                ) if attempt < self.config.max_retries => {
                    // Terminate and respawn for the next attempt.
                    last_err = e;
                    let _ = self.terminate_agent();
                    self.spawn_agent()?;
                }
                Err(e) => return Err(e),
            }
        }

        Err(last_err)
    }

    /// Quick reachability check: try to connect and heartbeat with a short
    /// timeout over the forwarded port.
    ///
    /// Returns `true` if the agent responds to a heartbeat within 2 seconds,
    /// `false` otherwise.
    pub async fn is_agent_reachable(&self, local_port: u16) -> bool {
        let addr = Self::agent_addr(local_port);
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
    /// this first checks whether the agent is already serving on the forwarded
    /// port and skips the build/install/spawn cycle if it is. This is the entry
    /// point #89 should prefer when a forward may already point at a live agent.
    #[instrument(skip(self))]
    pub async fn ensure_agent_ready(&self, local_port: u16) -> Result<(), AndroidLifecycleError> {
        if self.is_agent_reachable(local_port).await {
            debug!("android agent already reachable, skipping build");
            return Ok(());
        }
        self.ensure_running(local_port).await
    }
}

/// Reduce a captured stderr blob to an actionable one-line(-ish) diagnostic:
/// exit status plus the last few non-empty lines.
fn tail_diagnostic(stderr: &str, status: String) -> String {
    let tail: String = stderr
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .rev()
        .take(20)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    if tail.is_empty() {
        status
    } else {
        format!("{status} — {}", tail.trim())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
//
// These exercise the state-machine / error-classification logic without a live
// device: config defaults, the distinct error variants and their Display
// strings, project/APK-not-found classification (the synchronous guards that
// run before any device contact), the tail_diagnostic reducer, and the
// readiness poll's "port never opens" → StartupTimeout path over a loopback
// (no adb, no emulator). The live build→install→instrument→health flow is
// deferred to integration story #90.

#[cfg(test)]
mod tests {
    use super::*;

    // --- config ---

    #[test]
    fn config_new_defaults() {
        let config = AndroidLifecycleConfig::new(PathBuf::from("/tmp/agent-android"));
        assert_eq!(config.project_dir, PathBuf::from("/tmp/agent-android"));
        assert_eq!(config.device_port, 8080);
        assert_eq!(config.startup_timeout, Duration::from_secs(60));
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn config_custom_values() {
        let config = AndroidLifecycleConfig {
            project_dir: PathBuf::from("/tmp/custom"),
            device_port: 9999,
            startup_timeout: Duration::from_secs(10),
            max_retries: 5,
        };
        assert_eq!(config.device_port, 9999);
        assert_eq!(config.startup_timeout, Duration::from_secs(10));
        assert_eq!(config.max_retries, 5);
    }

    // --- distinct error Display (one per failure mode — spec E2) ---

    #[test]
    fn error_display_project_not_found() {
        let err = AndroidLifecycleError::ProjectNotFound(PathBuf::from("/missing"));
        assert_eq!(err.to_string(), "Android agent project not found: /missing");
    }

    #[test]
    fn error_display_build_failed() {
        let err = AndroidLifecycleError::BuildFailed("task assembleDebug FAILED".into());
        assert_eq!(
            err.to_string(),
            "Failed to build Android agent (Gradle): task assembleDebug FAILED"
        );
    }

    #[test]
    fn error_display_install_failed() {
        let err = AndroidLifecycleError::InstallFailed("INSTALL_FAILED_INVALID_APK".into());
        assert_eq!(
            err.to_string(),
            "Failed to install Android agent APK: INSTALL_FAILED_INVALID_APK"
        );
    }

    #[test]
    fn error_display_launch_failed() {
        let err = AndroidLifecycleError::LaunchFailed("adb not found".into());
        assert_eq!(
            err.to_string(),
            "Failed to launch Android agent (am instrument): adb not found"
        );
    }

    #[test]
    fn error_display_instrument_failed() {
        let err = AndroidLifecycleError::InstrumentFailed("exit status: 1 — crash".into());
        assert_eq!(
            err.to_string(),
            "Android agent instrumentation exited: exit status: 1 — crash"
        );
    }

    #[test]
    fn error_display_startup_timeout() {
        let err = AndroidLifecycleError::StartupTimeout;
        assert_eq!(
            err.to_string(),
            "Android agent failed to become ready within timeout (port never opened)"
        );
    }

    #[test]
    fn error_display_not_running() {
        let err = AndroidLifecycleError::NotRunning;
        assert_eq!(err.to_string(), "Android agent is not running");
    }

    #[test]
    fn error_display_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err = AndroidLifecycleError::Io(io_err);
        assert!(err.to_string().contains("IO error"));
        assert!(err.to_string().contains("file not found"));
    }

    /// The three acceptance failure modes must be *distinct* variants — a caller
    /// can branch on them. This guards against collapsing them into one error.
    #[test]
    fn three_failure_modes_are_distinct() {
        let build = AndroidLifecycleError::BuildFailed("x".into());
        let instrument = AndroidLifecycleError::InstrumentFailed("y".into());
        let timeout = AndroidLifecycleError::StartupTimeout;
        assert!(matches!(build, AndroidLifecycleError::BuildFailed(_)));
        assert!(matches!(
            instrument,
            AndroidLifecycleError::InstrumentFailed(_)
        ));
        assert!(matches!(timeout, AndroidLifecycleError::StartupTimeout));
        // And their messages are mutually distinct.
        assert_ne!(build.to_string(), instrument.to_string());
        assert_ne!(instrument.to_string(), timeout.to_string());
        assert_ne!(build.to_string(), timeout.to_string());
    }

    // --- build_agent guards (run before any device contact) ---

    #[test]
    fn build_agent_project_dir_not_found() {
        let config = AndroidLifecycleConfig::new(PathBuf::from("/nonexistent/android-project"));
        let lifecycle = AndroidLifecycle::new("emulator-5554".into(), config);
        match lifecycle.build_agent() {
            Err(AndroidLifecycleError::ProjectNotFound(path)) => {
                assert_eq!(path, PathBuf::from("/nonexistent/android-project"));
            }
            other => panic!("expected ProjectNotFound, got {other:?}"),
        }
    }

    #[test]
    fn build_agent_gradlew_not_found() {
        // temp dir exists but has no gradlew → ProjectNotFound(gradlew path).
        let config = AndroidLifecycleConfig::new(std::env::temp_dir());
        let lifecycle = AndroidLifecycle::new("emulator-5554".into(), config);
        match lifecycle.build_agent() {
            Err(AndroidLifecycleError::ProjectNotFound(path)) => {
                assert!(path.to_string_lossy().ends_with(GRADLEW));
            }
            other => panic!("expected ProjectNotFound, got {other:?}"),
        }
    }

    // --- install_agent guards: missing APK → InstallFailed (not a panic) ---

    #[test]
    fn install_agent_missing_apk() {
        // A real, empty temp project dir: gradlew check is bypassed (install
        // does not call build), APKs are absent → InstallFailed with a path hint.
        let config = AndroidLifecycleConfig::new(std::env::temp_dir());
        let lifecycle = AndroidLifecycle::new("emulator-5554".into(), config);
        match lifecycle.install_agent() {
            Err(AndroidLifecycleError::InstallFailed(msg)) => {
                assert!(msg.contains("APK not found"));
            }
            other => panic!("expected InstallFailed, got {other:?}"),
        }
    }

    // --- construction & accessors ---

    #[test]
    fn lifecycle_construction() {
        let config = AndroidLifecycleConfig {
            project_dir: PathBuf::from("/tmp/agent-android"),
            device_port: 5555,
            startup_timeout: Duration::from_secs(15),
            max_retries: 2,
        };
        let lifecycle = AndroidLifecycle::new("emulator-5554".into(), config);
        assert_eq!(lifecycle.serial(), "emulator-5554");
        assert_eq!(lifecycle.device_port(), 5555);
        assert_eq!(
            AndroidLifecycle::agent_addr(5555),
            "127.0.0.1:5555".parse::<SocketAddr>().unwrap()
        );
        assert!(lifecycle.child.lock().unwrap().is_none());
    }

    // --- terminate is safe with no child ---

    #[test]
    fn terminate_agent_no_child() {
        let config = AndroidLifecycleConfig::new(PathBuf::from("/tmp/agent-android"));
        let lifecycle = AndroidLifecycle::new("emulator-5554".into(), config);
        // Should succeed even with no child; the adb force-stop fallback is
        // best-effort (its failure is ignored), so this never errors.
        assert!(lifecycle.terminate_agent().is_ok());
    }

    // --- poll_child_exit returns None when there is no child ---

    #[test]
    fn poll_child_exit_none_without_child() {
        let config = AndroidLifecycleConfig::new(PathBuf::from("/tmp/agent-android"));
        let lifecycle = AndroidLifecycle::new("emulator-5554".into(), config);
        assert!(lifecycle.poll_child_exit().is_none());
    }

    // --- tail_diagnostic reducer ---

    #[test]
    fn tail_diagnostic_empty_stderr_returns_status() {
        assert_eq!(tail_diagnostic("", "exit status: 1".into()), "exit status: 1");
        assert_eq!(
            tail_diagnostic("   \n  \n", "exit status: 2".into()),
            "exit status: 2"
        );
    }

    #[test]
    fn tail_diagnostic_appends_stderr_tail() {
        let out = tail_diagnostic("noise\nFAILURE: build failed\n", "exit status: 1".into());
        assert!(out.starts_with("exit status: 1 — "));
        assert!(out.contains("FAILURE: build failed"));
    }

    #[test]
    fn tail_diagnostic_keeps_last_lines_only() {
        let many: String = (0..50).map(|i| format!("line{i}\n")).collect();
        let out = tail_diagnostic(&many, "exit status: 1".into());
        // Keeps the last lines (tail), drops the earliest.
        assert!(out.contains("line49"));
        assert!(!out.contains("line0\n"));
    }

    // --- async readiness: port never opens → StartupTimeout (no child) ---

    #[tokio::test]
    async fn is_agent_reachable_false_when_nothing_listening() {
        let config = AndroidLifecycleConfig::new(PathBuf::from("/tmp/agent-android"));
        let lifecycle = AndroidLifecycle::new("emulator-5554".into(), config);
        // Port with (almost certainly) nothing listening on loopback.
        assert!(!lifecycle.is_agent_reachable(19897).await);
    }

    #[tokio::test]
    async fn wait_for_ready_times_out_when_port_never_opens() {
        let config = AndroidLifecycleConfig {
            project_dir: PathBuf::from("/tmp/agent-android"),
            device_port: 8080,
            startup_timeout: Duration::from_secs(1),
            max_retries: 3,
        };
        let lifecycle = AndroidLifecycle::new("emulator-5554".into(), config);
        // No child process and nothing listening → the loop falls through to
        // the deadline and returns StartupTimeout (the "port never opens" mode,
        // distinct from InstrumentFailed which requires an exited child).
        let result = lifecycle.wait_for_ready(19896).await;
        assert!(matches!(result, Err(AndroidLifecycleError::StartupTimeout)));
    }

    #[tokio::test]
    async fn wait_for_ready_ok_when_agent_heartbeats() {
        use crate::protocol::{encode_response, read_frame_length, Response};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        // A loopback mock standing in for the forwarded agent port: accept one
        // connection, read the heartbeat frame, reply Ok. This proves the
        // ready path over the *same* AgentClient a production caller uses.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut header = [0u8; 4];
            stream.read_exact(&mut header).await.unwrap();
            let len = read_frame_length(&header) as usize;
            let mut payload = vec![0u8; len];
            stream.read_exact(&mut payload).await.unwrap();
            let bytes = encode_response(&Response::Ok);
            stream.write_all(&bytes).await.unwrap();
            stream.flush().await.unwrap();
        });

        let config = AndroidLifecycleConfig {
            project_dir: PathBuf::from("/tmp/agent-android"),
            device_port: 8080,
            startup_timeout: Duration::from_secs(5),
            max_retries: 3,
        };
        let lifecycle = AndroidLifecycle::new("emulator-5554".into(), config);
        assert!(lifecycle.wait_for_ready(port).await.is_ok());
    }
}
