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
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use thiserror::Error;

use tracing::{debug, info, instrument, warn};

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
const TEST_APK: &str =
    "build/outputs/apk/androidTest/debug/qorvex-agent-android-debug-androidTest.apk";
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
    /// JDK home forwarded by the client to pin the Gradle build's Java, taking
    /// precedence over this (daemon) process's ambient environment. `None` lets
    /// resolution fall back to the daemon's own `QORVEX_ANDROID_JAVA_HOME` /
    /// `JAVA_HOME` / `java_home -v 17`. See [`resolve_build_java_home`].
    pub java_home: Option<PathBuf>,
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
            java_home: None,
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

        let mut cmd = Command::new(&gradlew);
        cmd.current_dir(&self.config.project_dir)
            .args(["assembleDebug", "assembleDebugAndroidTest"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());

        // Pin a Gradle-compatible JDK (17) for the daemon when we can find one,
        // so the build does not depend on the host's ambient JAVA_HOME pointing
        // at a supported Java version (Gradle 8.10.2 rejects Java > 23). A
        // client-forwarded `java_home` wins over this daemon process's frozen
        // environment (the whole point of forwarding it).
        let pinned_jdk = resolve_build_java_home(self.config.java_home.as_deref());
        if let Some(ref jh) = pinned_jdk {
            debug!(java_home = %jh.display(), "pinning JDK for Gradle build");
            cmd.env("JAVA_HOME", jh);
        }

        let output = cmd.output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let mut diag = tail_diagnostic(&stderr, output.status.to_string());
            if pinned_jdk.is_none() {
                diag.push_str(
                    "\n(hint: no JDK 17 found — Gradle 8.10.2 needs Java 17–23. \
                     Install a JDK 17, then set QORVEX_ANDROID_JAVA_HOME (or \
                     JAVA_HOME) to it in the shell you run qorvex from — it is \
                     forwarded to the server, so no server restart is needed.)",
                );
            }
            return Err(AndroidLifecycleError::BuildFailed(diag));
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

    /// Quick bridge-health check: confirm the agent's device-side UiAutomation
    /// connection is alive, independent of what is on screen.
    ///
    /// [`is_agent_reachable`](Self::is_agent_reachable) only proves the TCP server
    /// answers a heartbeat — which a stale agent *orphaned by a prior server*
    /// (killed without teardown) keeps doing while its accessibility connection is
    /// dead, persistently returning "no active window" for every UI command. This
    /// sends a [`Request::BridgeHealth`]: the agent reports healthy only when its
    /// bridge can reach the live window list, the signal that distinguishes a
    /// working agent (even one on a locked or app-less screen — which still has
    /// system windows) from a dead orphan (which sees no windows at all). The
    /// probe deliberately does not require an *active/app* window and never passes
    /// the agent's screen-gate, so a merely-locked device is not mistaken for a
    /// dead bridge and needlessly rebuilt.
    ///
    /// Returns `true` if the agent reports a healthy bridge within
    /// [`BRIDGE_HEALTH_TIMEOUT`], `false` otherwise.
    pub async fn is_agent_bridge_healthy(&self, local_port: u16) -> bool {
        let addr = Self::agent_addr(local_port);
        let check = async {
            let mut client = AgentClient::new(addr);
            client.connect().await.ok()?;
            let result = client.bridge_health().await;
            client.disconnect();
            result.ok()
        };
        tokio::time::timeout(BRIDGE_HEALTH_TIMEOUT, check)
            .await
            .is_ok_and(|inner| inner.is_some())
    }

    /// Ensure the agent is running, starting it only if not already reachable
    /// **and healthy**.
    ///
    /// Unlike [`ensure_running`](Self::ensure_running) which always rebuilds,
    /// this first checks whether the agent is already serving on the forwarded
    /// port and skips the build/install/spawn cycle if it is. This is the entry
    /// point #89 should prefer when a forward may already point at a live agent.
    ///
    /// Reachability alone is not enough to reuse an agent: a stale orphan left by
    /// a prior server keeps heartbeating with a dead UiAutomation bridge, so the
    /// session would be stranded in "no active window". When the agent is
    /// reachable but its bridge fails [`is_agent_bridge_healthy`](Self::is_agent_bridge_healthy),
    /// the orphan is torn down ([`ensure_running`](Self::ensure_running) does not
    /// stop a pre-existing process, so this teardown frees the device port for the
    /// fresh spawn) and the full build/install/spawn cycle runs.
    #[instrument(skip(self))]
    pub async fn ensure_agent_ready(&self, local_port: u16) -> Result<(), AndroidLifecycleError> {
        if self.is_agent_reachable(local_port).await {
            if self.is_agent_bridge_healthy(local_port).await {
                debug!("android agent already reachable and bridge healthy, skipping build");
                return Ok(());
            }
            warn!(
                "android agent reachable but UiAutomation bridge unhealthy \
                 (stale orphan from a prior server); relaunching"
            );
            let _ = self.terminate_agent();
        }
        self.ensure_running(local_port).await
    }
}

/// Upper bound on the bridge-health probe (connect + [`Request::BridgeHealth`]
/// round-trip). The agent answers in well under a second when healthy and
/// returns promptly when its bridge is dead; this only caps a hung/half-open
/// connection so the fast-path can't stall. Comfortably above the agent's own
/// ~1s internal liveness poll.
const BRIDGE_HEALTH_TIMEOUT: Duration = Duration::from_secs(5);

/// Lowest Java major version Gradle 8.10.2 + AGP 8.5.2 will run on.
const MIN_BUILD_JAVA: u32 = 17;
/// Highest Java major version Gradle 8.10.2 supports running on.
const MAX_BUILD_JAVA: u32 = 23;

/// Locate a JDK the Android Gradle build can actually run under.
///
/// The build uses Gradle 8.10.2 (wrapper) + AGP 8.5.2, which only run on
/// Java 17–23. The host's ambient `JAVA_HOME`/PATH may point at a too-new JDK
/// (e.g. 26) that Gradle refuses to launch on, producing the cryptic
/// `What went wrong: 26.0.1` build failure seen on fresh machines. Pinning a
/// supported JDK for the Gradle daemon (via `JAVA_HOME` on the spawned process)
/// makes the build deterministic regardless of what else is installed.
///
/// Each candidate is validated by running its `bin/java -version` and checking
/// the major version is in `[MIN_BUILD_JAVA, MAX_BUILD_JAVA]` — `java_home -v 17`
/// is NOT trusted blindly, since it falls back to the newest installed JDK
/// (even an older one like 11) when no exact match exists.
///
/// Resolution order (first that validates wins):
/// 1. `client_override` — the JDK the client shell forwarded over IPC. The
///    server is a persistent daemon whose environment is frozen at spawn, so a
///    freshly-exported `QORVEX_ANDROID_JAVA_HOME` can only reach it this way;
///    honoring it first is what makes the hint's advice actually work.
/// 2. `QORVEX_ANDROID_JAVA_HOME` — explicit override in the daemon's own env.
/// 3. The ambient `JAVA_HOME` — what the build already relied on.
/// 4. Homebrew `openjdk@17` kegs (`/opt/homebrew` and `/usr/local` prefixes) —
///    so a brew-installed 17 is found without a manual override.
/// 5. macOS `/usr/libexec/java_home -v 17` — the canonical JDK-17 locator.
///
/// Returns `None` if no compatible JDK can be pinned, in which case the build
/// falls back to the ambient `JAVA_HOME` (and `build_agent` adds a hint on
/// failure).
fn resolve_build_java_home(client_override: Option<&Path>) -> Option<PathBuf> {
    let candidates = build_java_home_candidates(
        client_override,
        std::env::var_os("QORVEX_ANDROID_JAVA_HOME").map(PathBuf::from),
        std::env::var_os("JAVA_HOME").map(PathBuf::from),
        &BREW_OPENJDK17_PREFIXES,
        system_java_home_17(),
    );
    candidates.into_iter().find(|home| {
        java_major_version(home)
            .is_some_and(|major| (MIN_BUILD_JAVA..=MAX_BUILD_JAVA).contains(&major))
    })
}

/// Homebrew `openjdk@17` keg prefixes probed as JDK candidates — Apple-silicon
/// (`/opt/homebrew`) then Intel (`/usr/local`). Each is validated by its
/// `bin/java -version` like any other candidate, so a missing keg is harmless.
const BREW_OPENJDK17_PREFIXES: [&str; 2] =
    ["/opt/homebrew/opt/openjdk@17", "/usr/local/opt/openjdk@17"];

/// Assemble the ordered list of JDK candidates (no validation). Kept pure in its
/// inputs so the resolution-order contract — in particular that the Homebrew
/// kegs are probed after the env overrides but before the unreliable
/// `/usr/libexec/java_home` — is unit-testable without touching the process env.
fn build_java_home_candidates(
    client_override: Option<&Path>,
    explicit_env: Option<PathBuf>,
    ambient_env: Option<PathBuf>,
    brew_prefixes: &[&str],
    system_java_home: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(client) = client_override {
        candidates.push(client.to_path_buf());
    }
    if let Some(explicit) = explicit_env {
        candidates.push(explicit);
    }
    if let Some(ambient) = ambient_env {
        candidates.push(ambient);
    }
    // Homebrew's `openjdk@17` keg — the common way to get a JDK 17 on macOS, and
    // preferred over `/usr/libexec/java_home`, which can return an incompatible
    // JDK (e.g. 11) when no exact 17 is registered with the system — exactly the
    // case on a machine whose only 17 is the brew keg.
    candidates.extend(brew_prefixes.iter().map(PathBuf::from));
    if let Some(system) = system_java_home {
        candidates.push(system);
    }
    candidates
}

/// macOS `/usr/libexec/java_home -v 17` — the canonical JDK-17 locator. Returns
/// `None` off-macOS (binary absent) or when the system has no JDK 17 home.
fn system_java_home_17() -> Option<PathBuf> {
    let output = Command::new("/usr/libexec/java_home")
        .args(["-v", "17"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let home = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if home.is_empty() {
        None
    } else {
        Some(PathBuf::from(home))
    }
}

/// The JDK home a *client* (REPL/CLI) should forward to the server for the
/// Android Gradle build, read from the client's own environment.
///
/// The server runs as a persistent, detached daemon whose environment is frozen
/// at spawn time, so a `QORVEX_ANDROID_JAVA_HOME` (or `JAVA_HOME`) exported in a
/// later shell never reaches it. Clients read it here and send it on the
/// `StartAgent` request so `start-agent` honors it without a server restart.
/// Prefers the explicit `QORVEX_ANDROID_JAVA_HOME`, falling back to the ambient
/// `JAVA_HOME`; the server still validates the chosen path's Java version.
pub fn client_java_home_override() -> Option<String> {
    // Treat an explicitly-empty value as unset, so a stray `QORVEX_ANDROID_JAVA_HOME=`
    // does not suppress the `JAVA_HOME` fallback (and an empty path is never forwarded).
    fn non_empty(var: &str) -> Option<String> {
        std::env::var(var).ok().filter(|s| !s.is_empty())
    }
    non_empty("QORVEX_ANDROID_JAVA_HOME").or_else(|| non_empty("JAVA_HOME"))
}

/// Run `<java_home>/bin/java -version` and parse the major version (e.g. 17, 23,
/// or 8 from a legacy `1.8.0` string). Returns `None` if the binary is missing
/// or its output can't be parsed.
fn java_major_version(java_home: &Path) -> Option<u32> {
    let java_bin = java_home.join("bin").join("java");
    if !java_bin.exists() {
        return None;
    }
    // `java -version` writes to stderr: `openjdk version "17.0.19" ...`.
    let output = Command::new(&java_bin).arg("-version").output().ok()?;
    parse_java_major(&String::from_utf8_lossy(&output.stderr))
}

/// Parse the major version out of `java -version` output. Handles the modern
/// scheme (`openjdk version "17.0.19"` → 17) and the legacy one
/// (`java version "1.8.0_302"` → 8).
fn parse_java_major(version_output: &str) -> Option<u32> {
    let version = version_output.split('"').nth(1)?; // the quoted version string
    let mut parts = version.split('.');
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
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
            java_home: None,
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
            java_home: None,
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

    // --- java version parsing (build JDK resolution) ---

    #[test]
    fn parse_java_major_modern_scheme() {
        let out = "openjdk version \"17.0.19\" 2026-04-21\nOpenJDK Runtime Environment";
        assert_eq!(parse_java_major(out), Some(17));
    }

    #[test]
    fn parse_java_major_unsupported_new_jdk() {
        // The exact failure mode from the bug report: a JDK 26 host.
        assert_eq!(
            parse_java_major("openjdk version \"26.0.1\" 2026-09-16"),
            Some(26)
        );
    }

    #[test]
    fn parse_java_major_legacy_scheme() {
        assert_eq!(parse_java_major("java version \"1.8.0_302\""), Some(8));
    }

    #[test]
    fn parse_java_major_garbage_is_none() {
        assert_eq!(parse_java_major("no version here"), None);
        assert_eq!(parse_java_major(""), None);
    }

    // --- build JDK resolution: client override (the QORVEX_ANDROID_JAVA_HOME
    //     daemon-env fix) ---

    /// Build a fake JDK home whose `bin/java -version` reports `version_line`.
    #[cfg(unix)]
    fn fake_jdk(tag: &str, version_line: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("qorvex-fake-jdk-{tag}-{}", std::process::id()));
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let java = bin.join("java");
        std::fs::write(&java, format!("#!/bin/sh\necho '{version_line}' 1>&2\n")).unwrap();
        std::fs::set_permissions(&java, std::fs::Permissions::from_mode(0o755)).unwrap();
        dir
    }

    /// A client-forwarded JDK that validates is pinned ahead of the daemon's own
    /// (frozen) environment — the core of the QORVEX_ANDROID_JAVA_HOME fix.
    #[cfg(unix)]
    #[test]
    fn resolve_build_java_home_honors_validated_client_override() {
        let dir = fake_jdk("ok", "openjdk version \"17.0.19\" 2026-04-21");
        assert_eq!(
            resolve_build_java_home(Some(&dir)).as_deref(),
            Some(dir.as_path())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A too-new client JDK is validated and skipped, never blindly pinned —
    /// resolution falls through to the other candidates instead.
    #[cfg(unix)]
    #[test]
    fn resolve_build_java_home_rejects_unsupported_client_override() {
        let dir = fake_jdk("new", "openjdk version \"26.0.1\" 2026-09-16");
        assert_ne!(
            resolve_build_java_home(Some(&dir)).as_deref(),
            Some(dir.as_path())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Homebrew `openjdk@17` kegs are probed after the env overrides and
    /// before the (unreliable) system `java_home`, so a brew-only machine can
    /// self-resolve a compatible 17 without a manual override.
    #[test]
    fn build_java_home_candidates_probes_brew_between_overrides_and_system() {
        let client = PathBuf::from("/client/jdk");
        let candidates = build_java_home_candidates(
            Some(&client),
            Some(PathBuf::from("/explicit/jdk")),
            Some(PathBuf::from("/ambient/jdk")),
            &BREW_OPENJDK17_PREFIXES,
            Some(PathBuf::from("/system/jdk17")),
        );
        assert_eq!(
            candidates,
            vec![
                PathBuf::from("/client/jdk"),
                PathBuf::from("/explicit/jdk"),
                PathBuf::from("/ambient/jdk"),
                PathBuf::from("/opt/homebrew/opt/openjdk@17"),
                PathBuf::from("/usr/local/opt/openjdk@17"),
                PathBuf::from("/system/jdk17"),
            ]
        );
    }

    /// With no env overrides and no system JDK (the brew-only / `java_home`
    /// returns-11 case), the brew kegs are still the candidates that get probed.
    #[test]
    fn build_java_home_candidates_brew_only() {
        let candidates =
            build_java_home_candidates(None, None, None, &BREW_OPENJDK17_PREFIXES, None);
        assert_eq!(
            candidates,
            vec![
                PathBuf::from("/opt/homebrew/opt/openjdk@17"),
                PathBuf::from("/usr/local/opt/openjdk@17"),
            ]
        );
    }

    // --- tail_diagnostic reducer ---

    #[test]
    fn tail_diagnostic_empty_stderr_returns_status() {
        assert_eq!(
            tail_diagnostic("", "exit status: 1".into()),
            "exit status: 1"
        );
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
    async fn is_agent_bridge_healthy_false_when_nothing_listening() {
        let config = AndroidLifecycleConfig::new(PathBuf::from("/tmp/agent-android"));
        let lifecycle = AndroidLifecycle::new("emulator-5554".into(), config);
        assert!(!lifecycle.is_agent_bridge_healthy(19898).await);
    }

    /// Spawn a loopback mock agent that reads one request frame, asserts it is a
    /// `BridgeHealth`, and replies with `response`. Returns the bound port.
    async fn spawn_bridge_health_mock(response: crate::protocol::Response) -> u16 {
        use crate::protocol::{decode_request, encode_response, read_frame_length, Request};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut header = [0u8; 4];
            stream.read_exact(&mut header).await.unwrap();
            let len = read_frame_length(&header) as usize;
            let mut payload = vec![0u8; len];
            stream.read_exact(&mut payload).await.unwrap();
            // The probe must send BridgeHealth, never a heartbeat or window-gated
            // request — that is the whole point of a window-independent signal.
            assert_eq!(decode_request(&payload).unwrap(), Request::BridgeHealth);
            let bytes = encode_response(&response);
            stream.write_all(&bytes).await.unwrap();
            stream.flush().await.unwrap();
        });
        port
    }

    #[tokio::test]
    async fn is_agent_bridge_healthy_true_when_agent_reports_ok() {
        let port = spawn_bridge_health_mock(crate::protocol::Response::Ok).await;
        let config = AndroidLifecycleConfig::new(PathBuf::from("/tmp/agent-android"));
        let lifecycle = AndroidLifecycle::new("emulator-5554".into(), config);
        assert!(lifecycle.is_agent_bridge_healthy(port).await);
    }

    #[tokio::test]
    async fn is_agent_bridge_healthy_false_when_agent_reports_error() {
        // A reachable orphan whose bridge is dead answers the probe with an
        // Error; the fast-path must read that as "not healthy" and not reuse it.
        let port = spawn_bridge_health_mock(crate::protocol::Response::Error {
            message: "Bridge unhealthy: no reachable windows".into(),
        })
        .await;
        let config = AndroidLifecycleConfig::new(PathBuf::from("/tmp/agent-android"));
        let lifecycle = AndroidLifecycle::new("emulator-5554".into(), config);
        assert!(!lifecycle.is_agent_bridge_healthy(port).await);
    }

    #[tokio::test]
    async fn wait_for_ready_times_out_when_port_never_opens() {
        let config = AndroidLifecycleConfig {
            project_dir: PathBuf::from("/tmp/agent-android"),
            device_port: 8080,
            startup_timeout: Duration::from_secs(1),
            max_retries: 3,
            java_home: None,
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
            java_home: None,
        };
        let lifecycle = AndroidLifecycle::new("emulator-5554".into(), config);
        assert!(lifecycle.wait_for_ready(port).await.is_ok());
    }
}
