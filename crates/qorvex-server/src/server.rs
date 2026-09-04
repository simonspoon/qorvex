//! Core server state and request handling.
//!
//! This module extracts the backend logic from qorvex-repl's App into a
//! standalone `ServerState` that can be driven by an IPC socket server.

use std::path::PathBuf;
use std::sync::Arc;

use tracing::{debug, info, warn};

use qorvex_core::action::{ActionResult, ActionType};
use qorvex_core::adb_device::Adb;
use qorvex_core::adb_forward::AdbForward;
use qorvex_core::agent_driver::AgentDriver;
use qorvex_core::agent_lifecycle::{AgentLifecycle, AgentLifecycleConfig};
use qorvex_core::android_driver::AndroidDriver;
use qorvex_core::android_lifecycle::{AndroidLifecycle, AndroidLifecycleConfig};
use qorvex_core::config::QorvexConfig;
use qorvex_core::driver::{flatten_elements, AutomationDriver, DriverError};
use qorvex_core::executor::ActionExecutor;
use qorvex_core::ipc::{IpcRequest, IpcResponse, Platform};
use qorvex_core::session::Session;
use qorvex_core::simctl::{Simctl, SimctlError, SimulatorDevice};

/// Backend state for the automation server.
///
/// Holds all session, device, and executor state that was previously
/// embedded in the REPL's `App` struct.
pub struct ServerState {
    pub session_name: String,
    pub session: Option<Arc<Session>>,
    pub simulator_udid: Option<String>,
    pub shared_driver: Arc<tokio::sync::Mutex<Option<Arc<dyn AutomationDriver>>>>,
    pub executor: Option<ActionExecutor>,
    pub agent_lifecycle: Option<Arc<AgentLifecycle>>,
    pub cached_devices: Vec<SimulatorDevice>,
    /// Cached Android devices (adb serials) for completion and `use-device`
    /// resolution. Seeded at startup and refreshed on `list-devices
    /// --platform android`, mirroring `cached_devices` for iOS.
    pub cached_android_devices: Vec<qorvex_core::adb_device::AndroidDevice>,
    pub target_bundle_id: Option<String>,
    pub default_timeout_ms: u64,
    pub agent_port: u16,
    pub is_physical_device: bool,
    /// The tunnel address for CoreDevice devices (from tunneld), if available.
    pub tunnel_address: Option<String>,
    /// Whether the selected physical device should use the native CoreDevice tunnel.
    ///
    /// Set to `true` when a device is selected via the CoreDevice path (not usbmuxd,
    /// not tunneld). When `true` and `tunnel_address` is `None`, `AgentDriver::core_device()`
    /// is used instead of `AgentDriver::usb_device()`.
    pub use_core_device: bool,
    /// mDNS hostname for direct TCP connection to a WiFi (localNetwork) device.
    ///
    /// When set, `AgentDriver::direct(hostname, port)` is used instead of any
    /// tunnel approach. Typical value: `"Hillbilly.local"`.
    pub direct_host: Option<String>,

    // --- Android (additive; iOS path above is unchanged) ---
    /// The adb serial of the selected Android device, if a `BootDevice`
    /// targeted Android. `None` means no Android device is selected.
    pub android_serial: Option<String>,
    /// The managed Android agent lifecycle (Gradle build / install /
    /// `am instrument` process), if `StartAgent` targeted Android.
    pub android_lifecycle: Option<Arc<AndroidLifecycle>>,
    /// The server-owned `adb forward` rule for the Android session.
    ///
    /// `handle_start_agent_android` establishes this forward to pick a free
    /// loopback port and passes the bound port to both the lifecycle (for the
    /// health poll) and the [`AndroidDriver`]. The server keeps it for the
    /// session lifetime so the rule the driver and health-poll depend on is
    /// **not** torn down by [`AdbForward::drop`] when the start handler
    /// returns. The driver re-issues the same `tcp:<port>` rule idempotently on
    /// `connect`/recovery and owns its own forward; both removals (here on
    /// `stop-agent`, and the driver's on drop) are idempotent no-ops against
    /// adb, so there is no competing-removal hazard. Released in
    /// `handle_stop_agent`.
    pub android_forward: Option<AdbForward>,
}

impl ServerState {
    /// Create a new `ServerState`, pre-fetching devices and detecting a booted simulator.
    pub fn new(session_name: String) -> Self {
        let config = QorvexConfig::load();
        let agent_port = config.agent_port();
        let cached_devices = Simctl::list_devices().unwrap_or_default();
        let cached_android_devices = Adb::list_devices().unwrap_or_default();
        let simulator_udid = Simctl::get_booted_udid().ok();
        let executor = simulator_udid
            .as_ref()
            .map(|_| ActionExecutor::with_agent("localhost".to_string(), agent_port));

        info!(
            session = %session_name,
            device = ?simulator_udid,
            devices = cached_devices.len(),
            "ServerState initialised"
        );

        Self {
            session_name,
            session: None,
            simulator_udid,
            shared_driver: Arc::new(tokio::sync::Mutex::new(None)),
            executor,
            agent_lifecycle: None,
            cached_devices,
            cached_android_devices,
            target_bundle_id: None,
            default_timeout_ms: 5000,
            agent_port,
            is_physical_device: false,
            tunnel_address: None,
            use_core_device: false,
            direct_host: None,
            android_serial: None,
            android_lifecycle: None,
            android_forward: None,
        }
    }

    /// Handle a single IPC request and return a response.
    ///
    /// `Subscribe` is **not** handled here — it must be handled by the caller
    /// because it is a streaming operation.
    pub async fn handle_request(&mut self, request: IpcRequest) -> IpcResponse {
        match request {
            // ── Session Management ──────────────────────────────────────
            IpcRequest::StartSession => self.handle_start_session().await,
            IpcRequest::EndSession => self.handle_end_session(),

            // ── Device Management ───────────────────────────────────────
            IpcRequest::ListDevices { platform } => self.handle_list_devices(platform),
            IpcRequest::ListPhysicalDevices => self.handle_list_physical_devices().await,
            IpcRequest::UseDevice { udid } => self.handle_use_device(&udid).await,
            IpcRequest::BootDevice { udid, platform } => {
                self.handle_boot_device(&udid, platform).await
            }

            // ── Agent Management ────────────────────────────────────────
            IpcRequest::StartAgent {
                project_dir,
                platform,
                java_home,
            } => {
                self.handle_start_agent(project_dir, platform, java_home)
                    .await
            }
            IpcRequest::StopAgent => self.handle_stop_agent(),
            IpcRequest::Connect { host, port } => self.handle_connect(&host, port).await,

            // ── Target App Lifecycle ────────────────────────────────────
            IpcRequest::StartTarget { force } => self.handle_start_target(force).await,
            IpcRequest::StopTarget => self.handle_stop_target().await,

            // ── Target Info ─────────────────────────────────────────────
            IpcRequest::GetTargetInfo => self.handle_get_target_info().await,

            // ── Configuration ───────────────────────────────────────────
            IpcRequest::SetTarget { bundle_id } => self.handle_set_target(&bundle_id).await,
            IpcRequest::SetTimeout { timeout_ms } => {
                self.default_timeout_ms = timeout_ms;
                IpcResponse::CommandResult {
                    success: true,
                    message: format!("Default timeout set to {}ms", timeout_ms),
                }
            }
            IpcRequest::GetTimeout => IpcResponse::TimeoutValue {
                timeout_ms: self.default_timeout_ms,
            },

            // ── On-Demand Fetching ──────────────────────────────────────
            IpcRequest::FetchElements => self.handle_fetch_elements().await,
            IpcRequest::FetchApps => self.handle_fetch_apps().await,

            // ── Info ────────────────────────────────────────────────────
            IpcRequest::GetSessionInfo => self.handle_get_session_info().await,
            IpcRequest::GetCompletionData => IpcResponse::CompletionData {
                elements: Vec::new(),
                devices: self.cached_devices.clone(),
                android_devices: self.cached_android_devices.clone(),
            },

            // ── Execute ─────────────────────────────────────────────────
            IpcRequest::Execute { action, tag } => self.handle_execute(action, tag).await,

            // ── State / Log (forwarded from session) ────────────────────
            IpcRequest::GetState => self.handle_get_state().await,
            IpcRequest::GetLog => self.handle_get_log().await,

            // ── Subscribe — should not reach here ───────────────────────
            IpcRequest::Subscribe => IpcResponse::Error {
                message: "Subscribe must be handled by the server loop, not handle_request"
                    .to_string(),
            },

            // ── Shutdown — should not reach here ────────────────────────
            IpcRequest::Shutdown => IpcResponse::Error {
                message: "Shutdown is handled by the server loop".to_string(),
            },
        }
    }

    // ── Session ─────────────────────────────────────────────────────────

    async fn handle_start_session(&mut self) -> IpcResponse {
        // Refuse to start against an ambiguous boot state: with no device chosen
        // and several simulators booted, the caller must name one.
        if self.simulator_udid.is_none() && self.android_serial.is_none() {
            if let Err(err @ SimctlError::MultipleBootedSimulators(_)) = Simctl::get_booted_udid() {
                return IpcResponse::CommandResult {
                    success: false,
                    message: err.to_string(),
                };
            }
        }

        let session = Session::new(self.simulator_udid.clone(), &self.session_name);
        self.session = Some(session.clone());
        self.shared_driver = Arc::new(tokio::sync::Mutex::new(None));

        info!(session_name = %self.session_name, "Session started");

        // Auto-start agent if no executor or agent not reachable
        let needs_agent = match &self.executor {
            Some(executor) => executor.driver().screenshot().await.is_err(),
            None => true,
        };

        if !needs_agent {
            return IpcResponse::CommandResult {
                success: true,
                message: "Session started".to_string(),
            };
        }

        let udid = match self.simulator_udid.clone() {
            Some(udid) => udid,
            None => {
                return IpcResponse::CommandResult {
                    success: true,
                    message: "Session started (no device selected)".to_string(),
                };
            }
        };

        let config = QorvexConfig::load();
        let agent_source_dir = match config.effective_agent_source_dir() {
            Some(dir) => dir,
            None => {
                return IpcResponse::CommandResult {
                    success: false,
                    message: "No agent source found. Install via Homebrew or set agent_source_dir in ~/.qorvex/config.json".to_string(),
                };
            }
        };

        info!("Auto-starting agent");
        let mut lc_config = AgentLifecycleConfig::new(agent_source_dir);
        lc_config.agent_port = self.agent_port;
        if self.is_physical_device {
            lc_config.is_physical = true;
            lc_config.startup_timeout = std::time::Duration::from_secs(120);
            lc_config.tunnel_address = self.tunnel_address.clone();
            lc_config.direct_host = self.direct_host.clone();
            lc_config.development_team = config.development_team.clone();
            lc_config.agent_bundle_id = config.agent_bundle_id.clone();
        }
        let lifecycle = Arc::new(AgentLifecycle::new(udid.clone(), lc_config));

        match lifecycle.ensure_agent_ready().await {
            Ok(()) => {
                let mut driver = if self.is_physical_device {
                    if let Some(ref addr) = self.tunnel_address {
                        AgentDriver::tunneld(addr.clone(), self.agent_port)
                            .with_lifecycle(lifecycle.clone())
                    } else if let Some(ref host) = self.direct_host {
                        AgentDriver::direct(host.clone(), self.agent_port)
                            .with_lifecycle(lifecycle.clone())
                    } else if self.use_core_device {
                        AgentDriver::core_device(udid.clone(), self.agent_port)
                            .with_lifecycle(lifecycle.clone())
                    } else {
                        AgentDriver::usb_device(udid.clone(), self.agent_port)
                            .with_lifecycle(lifecycle.clone())
                    }
                } else {
                    AgentDriver::direct("127.0.0.1", self.agent_port)
                        .with_lifecycle(lifecycle.clone())
                };
                self.agent_lifecycle = Some(lifecycle);
                match driver.connect().await {
                    Ok(()) => {
                        self.set_executor_with_driver(Arc::new(driver)).await;
                        info!("Agent started and connected");
                        IpcResponse::CommandResult {
                            success: true,
                            message: "Session started".to_string(),
                        }
                    }
                    Err(e) => {
                        info!(error = %e, "Agent started but connection failed");
                        IpcResponse::CommandResult {
                            success: false,
                            message: format!("Agent started but connection failed: {}", e),
                        }
                    }
                }
            }
            Err(e) => {
                info!(error = %e, "Auto-start agent failed");
                IpcResponse::CommandResult {
                    success: false,
                    message: format!("Failed to start agent: {}", e),
                }
            }
        }
    }

    fn handle_end_session(&mut self) -> IpcResponse {
        self.session = None;

        IpcResponse::CommandResult {
            success: true,
            message: "Session ended".to_string(),
        }
    }

    // ── Devices ─────────────────────────────────────────────────────────

    fn handle_list_devices(&mut self, platform: Platform) -> IpcResponse {
        match platform {
            Platform::Ios => match Simctl::list_devices() {
                Ok(devices) => {
                    self.cached_devices = devices.clone();
                    IpcResponse::DeviceList { devices }
                }
                Err(e) => IpcResponse::Error {
                    message: e.to_string(),
                },
            },
            Platform::Android => match Adb::list_devices() {
                Ok(devices) => {
                    self.cached_android_devices = devices.clone();
                    IpcResponse::AndroidDeviceList { devices }
                }
                Err(e) => IpcResponse::Error {
                    message: e.to_string(),
                },
            },
        }
    }

    async fn handle_list_physical_devices(&mut self) -> IpcResponse {
        let mut devices = Vec::new();
        let mut seen_udids = std::collections::HashSet::new();

        // 1. Try usbmuxd
        if let Ok(usb_devices) = qorvex_core::usb_tunnel::list_devices().await {
            for d in usb_devices {
                seen_udids.insert(d.udid.clone());
                devices.push(qorvex_core::ipc::PhysicalDeviceInfo {
                    udid: d.udid,
                    name: None,
                    connection: d.connection.to_string(),
                });
            }
        }

        // 2. Try CoreDevice
        if let Ok(cd_devices) = qorvex_core::coredevice::list_devices().await {
            for d in cd_devices {
                let udid = d.udid.clone().unwrap_or(d.identifier.clone());
                if seen_udids.contains(&udid) {
                    continue;
                }
                seen_udids.insert(udid.clone());
                devices.push(qorvex_core::ipc::PhysicalDeviceInfo {
                    udid,
                    name: Some(d.name),
                    connection: d.transport_type,
                });
            }
        }

        IpcResponse::PhysicalDeviceList { devices }
    }

    /// Switch the session to an Android device addressed by `serial`.
    ///
    /// Device/agent routing is mutually exclusive between platforms, so this
    /// retires all iOS selection state (symmetric with the Android boot path,
    /// which clears `simulator_udid`): a stale `simulator_udid`/executor would
    /// misroute actions to a now-inactive simulator agent, and stale physical
    /// iOS fields would mislead the connection paths.
    fn select_android_device(&mut self, serial: &str) -> IpcResponse {
        self.android_serial = Some(serial.to_string());
        self.simulator_udid = None;
        self.executor = None;
        self.is_physical_device = false;
        self.use_core_device = false;
        self.direct_host = None;
        IpcResponse::CommandResult {
            success: true,
            message: format!("Using Android device {}", serial),
        }
    }

    async fn handle_use_device(&mut self, udid: &str) -> IpcResponse {
        let udid = strip_quotes(udid);
        if udid.is_empty() {
            return IpcResponse::CommandResult {
                success: false,
                message: "use_device requires a UDID".to_string(),
            };
        }
        // An adb serial (e.g. `emulator-5554`, `192.168.1.10:5555`, or a
        // hardware serial) selects an Android device. The Android cache — seeded
        // at startup, refreshed on `list-devices --platform android`, and the
        // source of the completion dropdown — is the fast path and spawns no
        // process. Selecting from the dropdown (or any iOS UDID) hits here.
        if self
            .cached_android_devices
            .iter()
            .any(|d| d.serial == udid && d.is_ready())
        {
            return self.select_android_device(udid);
        }
        // adb serials do not match the iOS UDID format, so a non-UDID miss may
        // be an Android device connected since the last cache refresh; confirm
        // it against live adb. iOS UDIDs skip this and never spawn `adb`.
        if !is_valid_udid(udid) {
            if matches!(Adb::list_devices(), Ok(devices)
                if devices.iter().any(|d| d.serial == udid && d.is_ready()))
            {
                return self.select_android_device(udid);
            }
            return IpcResponse::CommandResult {
                success: false,
                message: format!("Invalid UDID format: {}", udid),
            };
        }
        // Selecting an iOS device retires any Android selection so device/agent
        // routing stays mutually exclusive (symmetric with the Android boot path
        // clearing `simulator_udid`); otherwise a stale `android_serial` would
        // misroute start/stop-target to adb on an iOS session.
        self.android_serial = None;
        // Check if the UDID belongs to a known simulator.
        if self.cached_devices.iter().any(|d| d.udid == udid) {
            self.is_physical_device = false;
            self.use_core_device = false;
            self.direct_host = None;
            self.simulator_udid = Some(udid.to_string());
            self.executor = Some(ActionExecutor::with_agent(
                "localhost".to_string(),
                self.agent_port,
            ));
            return IpcResponse::CommandResult {
                success: true,
                message: format!("Using simulator {}", udid),
            };
        }
        // Not a simulator — check physical devices via USB tunnel.
        if let Ok(physical_devices) = qorvex_core::usb_tunnel::list_devices().await {
            if physical_devices.iter().any(|d| d.udid == udid) {
                self.is_physical_device = true;
                self.use_core_device = false;
                self.simulator_udid = Some(udid.to_string());
                self.tunnel_address = None;
                self.executor = None;
                // Also look up CoreDevice info for the mDNS hostname so
                // we can connect via simple TCP (Bonjour) instead of
                // relying on the USB tunnel or CoreDevice tunnel.
                self.direct_host = None;
                if let Ok(cd_devices) = qorvex_core::coredevice::list_devices().await {
                    if let Some(cd) = cd_devices
                        .iter()
                        .find(|d| d.udid.as_deref() == Some(udid) || d.identifier == udid)
                    {
                        self.direct_host = cd
                            .hostname
                            .clone()
                            .or_else(|| Some(format!("{}.local", cd.name)));
                    }
                }
                let name = self.direct_host.as_deref().unwrap_or(udid);
                return IpcResponse::CommandResult {
                    success: true,
                    message: format!("Using physical device {}", name),
                };
            }
        }

        // Not in usbmuxd — check CoreDevice
        if let Ok(cd_devices) = qorvex_core::coredevice::list_devices().await {
            // Find by traditional UDID or CoreDevice identifier
            if let Some(cd) = cd_devices
                .iter()
                .find(|d| d.udid.as_deref() == Some(udid) || d.identifier == udid)
            {
                self.is_physical_device = true;
                // Prefer the traditional UDID (e.g. 00008140-…) over the CoreDevice
                // UUID because xcodebuild -destination id=… requires the former.
                self.simulator_udid = Some(cd.udid.clone().unwrap_or_else(|| udid.to_string()));
                self.executor = None;

                // Always set direct_host from CoreDevice info — Bonjour
                // mDNS (`Name.local`) works for both WiFi and USB-connected
                // devices, and avoids the native CoreDevice tunnel which
                // requires pairing files that modern macOS no longer stores
                // in the expected locations.
                self.direct_host = cd
                    .hostname
                    .clone()
                    .or_else(|| Some(format!("{}.local", cd.name)));
                self.tunnel_address = None;
                self.use_core_device = false;

                return IpcResponse::CommandResult {
                    success: true,
                    message: format!("Using physical device {} ({})", cd.name, udid),
                };
            }
        }

        // None found
        IpcResponse::CommandResult {
            success: false,
            message: format!(
                "Device {} not found (not a simulator, not USB, and not a CoreDevice)",
                udid
            ),
        }
    }

    async fn handle_boot_device(&mut self, udid: &str, platform: Platform) -> IpcResponse {
        match platform {
            Platform::Ios => self.handle_boot_device_ios(udid),
            Platform::Android => self.handle_boot_device_android(udid).await,
        }
    }

    fn handle_boot_device_ios(&mut self, udid: &str) -> IpcResponse {
        let udid = strip_quotes(udid);
        if udid.is_empty() {
            return IpcResponse::CommandResult {
                success: false,
                message: "boot_device requires a UDID".to_string(),
            };
        }
        if !is_valid_udid(udid) {
            return IpcResponse::CommandResult {
                success: false,
                message: format!("Invalid UDID format: {}", udid),
            };
        }
        match Simctl::boot(udid) {
            Ok(_) => {
                self.simulator_udid = Some(udid.to_string());
                self.executor = Some(ActionExecutor::with_agent(
                    "localhost".to_string(),
                    self.agent_port,
                ));
                // Switching to iOS retires any active Android selection so
                // device/agent selection is mutually exclusive. Terminate the
                // Android agent and release its forward to avoid orphaned
                // resources, then clear the serial.
                if let Some(lifecycle) = self.android_lifecycle.take() {
                    let _ = lifecycle.terminate_agent();
                }
                if let Some(mut forward) = self.android_forward.take() {
                    let _ = forward.remove();
                }
                self.android_serial = None;
                IpcResponse::CommandResult {
                    success: true,
                    message: format!("Booted and using device {}", udid),
                }
            }
            Err(e) => IpcResponse::CommandResult {
                success: false,
                message: e.to_string(),
            },
        }
    }

    /// Boot/select an Android target.
    ///
    /// The `target` is an AVD name to boot via `emulator -avd`, or an already-
    /// running adb serial (e.g. `emulator-5554`, `host:port`) to select
    /// directly. Booting an emulator runs the blocking `emulator` CLI, so it is
    /// dispatched to a blocking thread.
    async fn handle_boot_device_android(&mut self, target: &str) -> IpcResponse {
        let target = strip_quotes(target).to_string();
        if target.is_empty() {
            return IpcResponse::CommandResult {
                success: false,
                message: "boot-device --platform android requires an AVD name or adb serial"
                    .to_string(),
            };
        }

        // If the target already names a running adb device, select it directly.
        let already_running = matches!(Adb::list_devices(), Ok(devices) if devices
            .iter()
            .any(|d| d.serial == target && d.is_ready()));

        let serial_result: Result<String, String> = if already_running {
            Ok(target.clone())
        } else {
            // Treat the target as an AVD name and boot it.
            let avd = target.clone();
            tokio::task::spawn_blocking(move || {
                Adb::boot_emulator(&avd, std::time::Duration::from_secs(120))
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r.map_err(|e| e.to_string()))
        };

        match serial_result {
            Ok(serial) => {
                self.android_serial = Some(serial.clone());
                // Switch off any iOS device selection so session/agent routing
                // targets Android, and retire the stale iOS executor so a
                // subsequent action does not route to a now-inactive simulator
                // agent (device/agent selection is mutually exclusive).
                self.simulator_udid = None;
                self.executor = None;
                IpcResponse::CommandResult {
                    success: true,
                    message: format!("Booted and using Android device {serial}"),
                }
            }
            Err(e) => IpcResponse::CommandResult {
                success: false,
                message: format!("Failed to boot Android device: {e}"),
            },
        }
    }

    // ── Agent ───────────────────────────────────────────────────────────

    async fn handle_start_agent(
        &mut self,
        project_dir: Option<String>,
        platform: Platform,
        java_home: Option<String>,
    ) -> IpcResponse {
        // The REPL defaults `--platform` to iOS when omitted, so an explicit-iOS
        // request is indistinguishable from an unspecified one. Device selection
        // is mutually exclusive between platforms (`use-device`/`boot-device`
        // clear the other side), so when an Android device is the active
        // selection, route there — otherwise `start-agent` after
        // `use-device <androidSerial>` would fall through to the iOS path and
        // fail with "No simulator selected" despite a device being connected.
        let platform = if platform == Platform::Ios
            && self.simulator_udid.is_none()
            && self.android_serial.is_some()
        {
            Platform::Android
        } else {
            platform
        };
        match platform {
            Platform::Ios => self.handle_start_agent_ios(project_dir).await,
            Platform::Android => {
                self.handle_start_agent_android(project_dir, java_home)
                    .await
            }
        }
    }

    /// Start the Android (Kotlin) agent: validate config, build/install/launch
    /// via Gradle + `am instrument`, establish the `adb forward` tunnel, and
    /// connect an [`AndroidDriver`].
    ///
    /// Missing or invalid Android config yields a clear validation error here
    /// (spec F3) rather than a downstream Gradle/adb crash.
    async fn handle_start_agent_android(
        &mut self,
        project_dir: Option<String>,
        java_home: Option<String>,
    ) -> IpcResponse {
        let serial = match self.android_serial.clone() {
            Some(s) => s,
            None => {
                return IpcResponse::CommandResult {
                    success: false,
                    message: "No Android device selected. Run `boot-device --platform android \
                              <avd-or-serial>` first."
                        .to_string(),
                };
            }
        };

        let config = QorvexConfig::load();

        // Resolve the project dir: explicit arg wins, else config-validated dir.
        let project_dir = match project_dir {
            Some(p) => {
                let p = PathBuf::from(strip_quotes(&p));
                if !p.join("gradlew").exists() {
                    return IpcResponse::CommandResult {
                        success: false,
                        message: format!(
                            "Android agent project directory is missing its Gradle wrapper \
                             (expected `gradlew` at {})",
                            p.display()
                        ),
                    };
                }
                p
            }
            None => match config.validate_android() {
                Ok(dir) => dir,
                Err(e) => {
                    // Clear, actionable validation error (F3).
                    return IpcResponse::CommandResult {
                        success: false,
                        message: e.to_string(),
                    };
                }
            },
        };

        let device_port = config.android_device_port();

        // Build a fresh adb forward to pick a free local port.
        let serial_for_fwd = serial.clone();
        let forward = match tokio::task::spawn_blocking(move || {
            AdbForward::establish(&serial_for_fwd, None, device_port)
        })
        .await
        {
            Ok(Ok(fwd)) => fwd,
            Ok(Err(e)) => {
                return IpcResponse::CommandResult {
                    success: false,
                    message: format!("Failed to establish adb forward: {e}"),
                };
            }
            Err(e) => {
                return IpcResponse::CommandResult {
                    success: false,
                    message: format!("adb forward task failed: {e}"),
                };
            }
        };
        let local_port = forward.local_port();

        let mut lc_config = AndroidLifecycleConfig::new(project_dir);
        lc_config.device_port = device_port;
        // Honor the JDK the *client* shell resolved: the daemon's own env was
        // frozen when it was spawned, so this is the only path by which a
        // freshly-exported QORVEX_ANDROID_JAVA_HOME reaches the Gradle build.
        lc_config.java_home = java_home.map(PathBuf::from);
        let lifecycle = Arc::new(AndroidLifecycle::new(serial.clone(), lc_config));

        // Fast-path: skip the Gradle build/install/spawn cycle when the agent
        // is already healthy on the forwarded port; `ensure_agent_ready`
        // delegates to `ensure_running` (preserving its distinct error
        // reporting) only when the agent is not reachable.
        match lifecycle.ensure_agent_ready(local_port).await {
            Ok(()) => {
                self.android_lifecycle = Some(lifecycle);
                // Keep the forward alive for the session — dropping it here
                // would `adb forward --remove` the rule the driver and health
                // poll depend on. Switching to Android also retires any stale
                // iOS executor so device/agent selection is mutually exclusive.
                self.android_forward = Some(forward);
                self.executor = None;
                let mut driver = AndroidDriver::new(serial.clone(), Some(local_port), device_port);
                match driver.connect().await {
                    Ok(()) => {
                        self.set_executor_with_driver(Arc::new(driver)).await;
                        IpcResponse::CommandResult {
                            success: true,
                            message: "Android agent started and connected".to_string(),
                        }
                    }
                    Err(e) => IpcResponse::CommandResult {
                        success: false,
                        message: format!("Android agent started but connection failed: {e}"),
                    },
                }
            }
            Err(e) => IpcResponse::CommandResult {
                success: false,
                message: format!("Failed to start Android agent: {e}"),
            },
        }
    }

    async fn handle_start_agent_ios(&mut self, project_dir: Option<String>) -> IpcResponse {
        if self.simulator_udid.is_none() {
            return IpcResponse::CommandResult {
                success: false,
                message: "No simulator selected. Use UseDevice or BootDevice first.".to_string(),
            };
        }
        let udid = self.simulator_udid.clone().unwrap();

        let config = QorvexConfig::load();

        if let Some(project_dir_str) = project_dir {
            // With path: build, spawn, wait, store lifecycle
            let project_dir = PathBuf::from(strip_quotes(&project_dir_str));
            let mut lc_config = AgentLifecycleConfig::new(project_dir);
            lc_config.agent_port = self.agent_port;
            if self.is_physical_device {
                lc_config.is_physical = true;
                lc_config.startup_timeout = std::time::Duration::from_secs(120);
                lc_config.tunnel_address = self.tunnel_address.clone();
                lc_config.direct_host = self.direct_host.clone();
                lc_config.development_team = config.development_team.clone();
                lc_config.agent_bundle_id = config.agent_bundle_id.clone();
            }
            let lifecycle = Arc::new(AgentLifecycle::new(udid.clone(), lc_config));

            match lifecycle.ensure_running().await {
                Ok(()) => {
                    let mut driver = if self.is_physical_device {
                        if let Some(ref addr) = self.tunnel_address {
                            AgentDriver::tunneld(addr.clone(), self.agent_port)
                                .with_lifecycle(lifecycle.clone())
                        } else if let Some(ref host) = self.direct_host {
                            AgentDriver::direct(host.clone(), self.agent_port)
                                .with_lifecycle(lifecycle.clone())
                        } else if self.use_core_device {
                            AgentDriver::core_device(udid.clone(), self.agent_port)
                                .with_lifecycle(lifecycle.clone())
                        } else {
                            AgentDriver::usb_device(udid.clone(), self.agent_port)
                                .with_lifecycle(lifecycle.clone())
                        }
                    } else {
                        AgentDriver::direct("127.0.0.1", self.agent_port)
                            .with_lifecycle(lifecycle.clone())
                    };
                    self.agent_lifecycle = Some(lifecycle);
                    match driver.connect().await {
                        Ok(()) => {
                            self.set_executor_with_driver(Arc::new(driver)).await;
                            IpcResponse::CommandResult {
                                success: true,
                                message: "Agent started and connected".to_string(),
                            }
                        }
                        Err(e) => IpcResponse::CommandResult {
                            success: false,
                            message: format!("Agent started but connection failed: {}", e),
                        },
                    }
                }
                Err(e) => IpcResponse::CommandResult {
                    success: false,
                    message: format!("Failed to start agent: {}", e),
                },
            }
        } else {
            // No path argument: try config, then fall back to external agent
            let config = QorvexConfig::load();
            if let Some(project_dir) = config.effective_agent_source_dir() {
                let mut lc_config = AgentLifecycleConfig::new(project_dir);
                lc_config.agent_port = self.agent_port;
                if self.is_physical_device {
                    lc_config.is_physical = true;
                    lc_config.startup_timeout = std::time::Duration::from_secs(120);
                    lc_config.tunnel_address = self.tunnel_address.clone();
                    lc_config.direct_host = self.direct_host.clone();
                    lc_config.development_team = config.development_team.clone();
                    lc_config.agent_bundle_id = config.agent_bundle_id.clone();
                }
                let lifecycle = Arc::new(AgentLifecycle::new(udid.clone(), lc_config));

                match lifecycle.ensure_agent_ready().await {
                    Ok(()) => {
                        let mut driver = if self.is_physical_device {
                            if let Some(ref addr) = self.tunnel_address {
                                AgentDriver::tunneld(addr.clone(), self.agent_port)
                                    .with_lifecycle(lifecycle.clone())
                            } else if let Some(ref host) = self.direct_host {
                                AgentDriver::direct(host.clone(), self.agent_port)
                                    .with_lifecycle(lifecycle.clone())
                            } else if self.use_core_device {
                                AgentDriver::core_device(udid.clone(), self.agent_port)
                                    .with_lifecycle(lifecycle.clone())
                            } else {
                                AgentDriver::usb_device(udid.clone(), self.agent_port)
                                    .with_lifecycle(lifecycle.clone())
                            }
                        } else {
                            AgentDriver::direct("127.0.0.1", self.agent_port)
                                .with_lifecycle(lifecycle.clone())
                        };
                        self.agent_lifecycle = Some(lifecycle);
                        match driver.connect().await {
                            Ok(()) => {
                                self.set_executor_with_driver(Arc::new(driver)).await;
                                IpcResponse::CommandResult {
                                    success: true,
                                    message: "Agent started and connected".to_string(),
                                }
                            }
                            Err(e) => IpcResponse::CommandResult {
                                success: false,
                                message: format!("Agent started but connection failed: {}", e),
                            },
                        }
                    }
                    Err(e) => IpcResponse::CommandResult {
                        success: false,
                        message: format!("Failed to start agent: {}", e),
                    },
                }
            } else {
                // No config: connect to externally-started agent
                let mut lc_config = AgentLifecycleConfig::new(PathBuf::new());
                lc_config.agent_port = self.agent_port;
                if self.is_physical_device {
                    lc_config.is_physical = true;
                    lc_config.startup_timeout = std::time::Duration::from_secs(120);
                    lc_config.tunnel_address = self.tunnel_address.clone();
                    lc_config.direct_host = self.direct_host.clone();
                }
                let lifecycle = AgentLifecycle::new(udid.clone(), lc_config);

                match lifecycle.wait_for_ready().await {
                    Ok(()) => {
                        let mut driver = if self.is_physical_device {
                            if let Some(ref addr) = self.tunnel_address {
                                AgentDriver::tunneld(addr.clone(), self.agent_port)
                            } else if let Some(ref host) = self.direct_host {
                                AgentDriver::direct(host.clone(), self.agent_port)
                            } else if self.use_core_device {
                                AgentDriver::core_device(udid.clone(), self.agent_port)
                            } else {
                                AgentDriver::usb_device(udid.clone(), self.agent_port)
                            }
                        } else {
                            AgentDriver::direct("127.0.0.1", self.agent_port)
                        };
                        match driver.connect().await {
                            Ok(()) => {
                                self.set_executor_with_driver(Arc::new(driver)).await;
                                IpcResponse::CommandResult {
                                    success: true,
                                    message: "Agent connected".to_string(),
                                }
                            }
                            Err(e) => IpcResponse::CommandResult {
                                success: false,
                                message: format!("Connection failed: {}", e),
                            },
                        }
                    }
                    Err(e) => IpcResponse::CommandResult {
                        success: false,
                        message: format!("Agent not reachable: {}", e),
                    },
                }
            }
        }
    }

    fn handle_stop_agent(&mut self) -> IpcResponse {
        let mut stopped = false;

        // iOS agent lifecycle.
        if let Some(lifecycle) = self.agent_lifecycle.take() {
            let _ = lifecycle.terminate_agent();
            stopped = true;
        }

        // Android agent lifecycle: terminate the `am instrument` child and
        // release the server-owned `adb forward` rule.
        if let Some(lifecycle) = self.android_lifecycle.take() {
            let _ = lifecycle.terminate_agent();
            stopped = true;
        }
        if let Some(mut forward) = self.android_forward.take() {
            let _ = forward.remove();
            stopped = true;
        }

        if stopped {
            IpcResponse::CommandResult {
                success: true,
                message: "Agent stopped".to_string(),
            }
        } else {
            IpcResponse::CommandResult {
                success: false,
                message: "No managed agent to stop".to_string(),
            }
        }
    }

    async fn handle_connect(&mut self, host: &str, port: u16) -> IpcResponse {
        let mut driver = AgentDriver::direct(host, port);
        match driver.connect().await {
            Ok(()) => {
                self.set_executor_with_driver(Arc::new(driver)).await;
                IpcResponse::CommandResult {
                    success: true,
                    message: format!("Connected to {}:{}", host, port),
                }
            }
            Err(e) => IpcResponse::CommandResult {
                success: false,
                message: format!("Connection failed: {}", e),
            },
        }
    }

    // ── Configuration ───────────────────────────────────────────────────

    async fn handle_set_target(&mut self, bundle_id: &str) -> IpcResponse {
        let bundle_id = strip_quotes(bundle_id);
        if bundle_id.is_empty() {
            return IpcResponse::CommandResult {
                success: false,
                message: "set_target requires a bundle_id".to_string(),
            };
        }
        let (response, action_result) = match self.record_target(bundle_id).await {
            Ok(message) => (
                IpcResponse::CommandResult {
                    success: true,
                    message,
                },
                ActionResult::Success,
            ),
            Err(msg) => (
                IpcResponse::CommandResult {
                    success: false,
                    message: msg.clone(),
                },
                ActionResult::Failure(msg),
            ),
        };
        self.log_action(
            ActionType::SetTarget {
                bundle_id: bundle_id.to_string(),
            },
            action_result,
            None,
            None,
        )
        .await;
        response
    }

    /// Launches the target app, reporting whether it *actually* launched or
    /// was already running.
    ///
    /// Neither backend relaunches a running app on its own, and both report
    /// success either way — a silent no-op that makes a caller relying on
    /// launch-time setup (a suite that reseeds its database on start) assert
    /// against the previous run's state. So the running process id is probed
    /// before launching, and the outcome is surfaced in both the message and
    /// the structured `data` payload. `force` terminates the running copy
    /// first, making the relaunch real.
    async fn handle_start_target(&self, force: bool) -> IpcResponse {
        let Some(ref bundle_id) = self.target_bundle_id else {
            return IpcResponse::CommandResult {
                success: false,
                message: "No target set. Use set-target first.".to_string(),
            };
        };
        // Launch is platform-specific and agent-independent (mirrors iOS
        // Simctl): route to adb when an Android device is selected, else
        // Simctl. Android target selection clears `simulator_udid`, so the
        // serial check distinguishes the two.
        let launch_result: Result<(bool, Option<u32>), String> =
            if let Some(ref serial) = self.android_serial {
                Self::start_target_android(serial, bundle_id, force).map_err(|e| e.to_string())
            } else if let Some(ref udid) = self.simulator_udid {
                Self::start_target_ios(udid, bundle_id, force).map_err(|e| e.to_string())
            } else {
                return IpcResponse::CommandResult {
                    success: false,
                    message: "No device selected.".to_string(),
                };
            };

        let (response, action_result) = match launch_result {
            Ok((was_running, pid)) => {
                // The agent's app handle is bound to the process that was
                // running when the target was last set. A relaunch leaves it
                // pointing at a dead process: reads still work (they resolve
                // from a fresh snapshot) but element queries find nothing, so
                // every tap fails until the user restarts the session. Pushing
                // the bundle id again rebuilds the handle against the new
                // process.
                self.refresh_agent_target(bundle_id).await;
                let (message, data) = start_target_report(bundle_id, was_running, pid, force);
                (
                    IpcResponse::ActionResult {
                        success: true,
                        message,
                        screenshot: None,
                        data: Some(data.to_string()),
                    },
                    ActionResult::Success,
                )
            }
            Err(e) => {
                let msg = format!("Failed to launch app: {}", e);
                (
                    IpcResponse::CommandResult {
                        success: false,
                        message: msg.clone(),
                    },
                    ActionResult::Failure(msg),
                )
            }
        };
        self.log_action(ActionType::StartTarget { force }, action_result, None, None)
            .await;
        response
    }

    /// iOS leg of [`Self::handle_start_target`]: returns
    /// `(was_already_running, pid_after_launch)`. `simctl launch` reports the
    /// pid itself, so only the pre-launch probe is extra.
    fn start_target_ios(
        udid: &str,
        bundle_id: &str,
        force: bool,
    ) -> Result<(bool, Option<u32>), qorvex_core::simctl::SimctlError> {
        let was_running = Simctl::app_pid(udid, bundle_id)?.is_some();
        let pid = Simctl::launch_app(udid, bundle_id, force)?;
        Ok((was_running, pid))
    }

    /// Android leg of [`Self::handle_start_target`]. `monkey` prints no pid, so
    /// the process is probed on both sides of the launch; `force` force-stops
    /// the running copy first, since a LAUNCHER intent only brings an existing
    /// process to the front.
    fn start_target_android(
        serial: &str,
        package: &str,
        force: bool,
    ) -> Result<(bool, Option<u32>), qorvex_core::adb_device::AdbError> {
        let was_running = Adb::app_pid(serial, package)?.is_some();
        if force && was_running {
            Adb::force_stop(serial, package)?;
        }
        Adb::launch_app(serial, package)?;
        let pid = Adb::app_pid(serial, package)?;
        Ok((was_running, pid))
    }

    async fn handle_stop_target(&self) -> IpcResponse {
        let Some(ref bundle_id) = self.target_bundle_id else {
            return IpcResponse::CommandResult {
                success: false,
                message: "No target set. Use set-target first.".to_string(),
            };
        };
        let terminate_result = if let Some(ref serial) = self.android_serial {
            Adb::force_stop(serial, bundle_id).map_err(|e| e.to_string())
        } else if let Some(ref udid) = self.simulator_udid {
            Simctl::terminate_app(udid, bundle_id).map_err(|e| e.to_string())
        } else {
            return IpcResponse::CommandResult {
                success: false,
                message: "No device selected.".to_string(),
            };
        };
        let (response, action_result) = match terminate_result {
            Ok(()) => (
                IpcResponse::CommandResult {
                    success: true,
                    message: format!("Terminated {}", bundle_id),
                },
                ActionResult::Success,
            ),
            Err(e) => {
                let msg = format!("Failed to terminate app: {}", e);
                (
                    IpcResponse::CommandResult {
                        success: false,
                        message: msg.clone(),
                    },
                    ActionResult::Failure(msg),
                )
            }
        };
        self.log_action(ActionType::StopTarget, action_result, None, None)
            .await;
        response
    }

    // ── Target Info ──────────────────────────────────────────────────────

    async fn handle_get_target_info(&self) -> IpcResponse {
        let driver = if let Some(guard) = self.shared_driver.lock().await.as_ref() {
            guard.clone()
        } else if let Some(executor) = &self.executor {
            executor.driver().clone()
        } else {
            return IpcResponse::CommandResult {
                success: false,
                message: "No automation backend connected.".to_string(),
            };
        };
        match driver.get_target_info().await {
            Ok(mut info) => {
                // Enrich with the bundle_id from server state if the driver
                // didn't provide it.
                if info.bundle_id.is_empty() {
                    if let Some(ref bid) = self.target_bundle_id {
                        info.bundle_id = bid.clone();
                    }
                }
                let json = serde_json::to_string(&info).unwrap_or_default();
                IpcResponse::ActionResult {
                    success: true,
                    message: format!("{} ({})", info.display_name, info.bundle_id),
                    screenshot: None,
                    data: Some(json),
                }
            }
            Err(e) => IpcResponse::CommandResult {
                success: false,
                message: format!("get-target-info failed: {}", e),
            },
        }
    }

    // ── On-Demand Fetching ──────────────────────────────────────────────

    async fn handle_fetch_elements(&self) -> IpcResponse {
        let driver = if let Some(guard) = self.shared_driver.lock().await.as_ref() {
            guard.clone()
        } else if let Some(executor) = &self.executor {
            executor.driver().clone()
        } else {
            return IpcResponse::CompletionData {
                elements: Vec::new(),
                devices: Vec::new(),
                android_devices: Vec::new(),
            };
        };

        match driver.dump_tree().await {
            Ok(hierarchy) => {
                let elements = flatten_elements(&hierarchy);
                IpcResponse::CompletionData {
                    elements,
                    devices: Vec::new(),
                    android_devices: Vec::new(),
                }
            }
            Err(_) => IpcResponse::CompletionData {
                elements: Vec::new(),
                devices: Vec::new(),
                android_devices: Vec::new(),
            },
        }
    }

    /// Fetch installed apps for `set-target` completion, picking the source by
    /// active platform. Android selection clears `simulator_udid`, so an active
    /// `android_serial` means the device is Android and we enumerate packages
    /// via `adb`; otherwise fall back to the booted simulator via `simctl`.
    /// A failed enumeration yields an empty list — completion stays silent
    /// rather than erroring.
    async fn handle_fetch_apps(&self) -> IpcResponse {
        let apps = if let Some(serial) = self.android_serial.clone() {
            tokio::task::spawn_blocking(move || Adb::list_packages(&serial))
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or_default()
        } else if let Some(udid) = self.simulator_udid.clone() {
            tokio::task::spawn_blocking(move || Simctl::list_apps(&udid))
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        IpcResponse::AppList { apps }
    }

    // ── Info ─────────────────────────────────────────────────────────────

    async fn handle_get_session_info(&self) -> IpcResponse {
        match &self.session {
            Some(session) => {
                let action_log = session.get_action_log().await;
                IpcResponse::SessionInfo {
                    session_name: self.session_name.clone(),
                    active: true,
                    device_udid: self.simulator_udid.clone(),
                    action_count: action_log.len(),
                }
            }
            None => IpcResponse::SessionInfo {
                session_name: self.session_name.clone(),
                active: false,
                device_udid: self.simulator_udid.clone(),
                action_count: 0,
            },
        }
    }

    // ── Execute ──────────────────────────────────────────────────────────

    async fn handle_execute(&mut self, action: ActionType, tag: Option<String>) -> IpcResponse {
        debug!(action = %action.name(), "executing action");

        // LogComment doesn't require a driver
        if let ActionType::LogComment { ref message } = action {
            let msg = format!("Logged: {}", message);
            self.log_action(action, ActionResult::Success, None, tag)
                .await;
            return IpcResponse::ActionResult {
                success: true,
                message: msg,
                screenshot: None,
                data: None,
            };
        }

        // Nor does `set-target`: it writes server state, and the agent's copy
        // is a cache pushed opportunistically (see `record_target`). Routing it
        // through the executor below would fail the whole command whenever the
        // agent is absent or unreachable — even though `start-target` /
        // `stop-target`, the only readers of that state, drive simctl/adb and
        // need no agent at all.
        if let ActionType::SetTarget { ref bundle_id } = action {
            let bundle_id = strip_quotes(bundle_id).to_string();
            if bundle_id.is_empty() {
                return IpcResponse::Error {
                    message: "set_target requires a bundle_id".to_string(),
                };
            }
            let (response, action_result) = match self.record_target(&bundle_id).await {
                Ok(message) => (
                    IpcResponse::ActionResult {
                        success: true,
                        message,
                        screenshot: None,
                        data: None,
                    },
                    ActionResult::Success,
                ),
                Err(msg) => (
                    IpcResponse::ActionResult {
                        success: false,
                        message: msg.clone(),
                        screenshot: None,
                        data: None,
                    },
                    ActionResult::Failure(msg),
                ),
            };
            self.log_action(action, action_result, None, tag).await;
            return response;
        }

        let driver_guard = self.shared_driver.lock().await;
        let driver_opt = driver_guard.clone();
        drop(driver_guard);

        // Prefer the shared driver (set when agent connects); fall back to executor's driver.
        let executor = if let Some(driver) = driver_opt {
            Some(ActionExecutor::new(driver))
        } else {
            self.executor
                .as_ref()
                .map(|e| ActionExecutor::new(e.driver().clone()))
        };

        match executor {
            Some(executor) => {
                let result = executor.execute(action.clone()).await;

                // Log to session
                let action_result = if result.success {
                    ActionResult::Success
                } else {
                    ActionResult::Failure(result.message.clone())
                };
                let duration_ms = result
                    .data
                    .as_ref()
                    .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
                    .and_then(|v| v.get("elapsed_ms").and_then(|e| e.as_u64()));
                self.log_action(action, action_result, duration_ms, tag)
                    .await;

                IpcResponse::ActionResult {
                    success: result.success,
                    message: result.message,
                    screenshot: result.screenshot.map(Arc::new),
                    data: result.data,
                }
            }
            None => IpcResponse::Error {
                message: "No automation backend connected".to_string(),
            },
        }
    }

    // ── State / Log ──────────────────────────────────────────────────────

    async fn handle_get_state(&self) -> IpcResponse {
        match &self.session {
            Some(session) => IpcResponse::State {
                session_id: session.id.to_string(),
                screenshot: session.get_screenshot().await,
            },
            None => IpcResponse::Error {
                message: "No active session".to_string(),
            },
        }
    }

    async fn handle_get_log(&self) -> IpcResponse {
        match &self.session {
            Some(session) => IpcResponse::Log {
                entries: session.get_action_log().await,
            },
            None => IpcResponse::Error {
                message: "No active session".to_string(),
            },
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────

    /// Set the executor and update the shared driver so IPC clients reuse the same connection.
    pub async fn set_executor_with_driver(&mut self, driver: Arc<dyn AutomationDriver>) {
        self.executor = Some(ActionExecutor::new(driver.clone()));
        *self.shared_driver.lock().await = Some(driver.clone());
        // A target recorded while no agent was connected lives only in server
        // state, so hand it to the fresh driver — otherwise automation would
        // drive nothing until a second set-target. This is also where a bundle
        // id recorded blind gets validated for the first time; a bad one fails
        // loudly here rather than silently doing nothing later.
        if let Some(bundle_id) = self.target_bundle_id.clone() {
            if let Err(e) = driver.set_target(&bundle_id).await {
                warn!(bundle_id = %bundle_id, error = %e, "failed to apply recorded target to agent");
            }
        }
    }

    /// Re-pushes the recorded target to the agent so its app handle points at
    /// the currently running process.
    ///
    /// Unlike [`Self::record_target`] this changes no server state and never
    /// fails the caller: the launch itself succeeded, and an unreachable agent
    /// is the connection's problem, not the launch's.
    async fn refresh_agent_target(&self, bundle_id: &str) {
        let driver = match self.shared_driver.lock().await.clone() {
            Some(driver) => Some(driver),
            None => self.executor.as_ref().map(|e| e.driver().clone()),
        };
        if let Some(driver) = driver {
            if let Err(e) = driver.set_target(bundle_id).await {
                warn!(bundle_id = %bundle_id, error = %e, "failed to refresh agent target after launch");
            }
        }
    }

    /// Pushes the target bundle id to the agent if one is reachable, then
    /// records it. `Ok` carries the message to report, `Err` the failure.
    ///
    /// The record is what `start-target` / `stop-target` read, and those drive
    /// simctl/adb directly — they never need an agent. Recording used to happen
    /// only on a successful driver round-trip, so choosing which app to launch
    /// meant standing up a whole agent connection first.
    ///
    /// Which failures are fatal turns on *who* failed. An unreachable agent is
    /// the connection's problem, not the id's: the id is recorded anyway and
    /// the push retried when an agent (re)connects
    /// ([`Self::set_executor_with_driver`]). A [`DriverError::CommandFailed`]
    /// is the agent answering — the Android agent refuses a package that isn't
    /// installed — so that stays fatal and nothing is recorded, or a typo'd id
    /// would sit in server state waiting to fail again at `start-target`.
    async fn record_target(&mut self, bundle_id: &str) -> Result<String, String> {
        // Prefer the shared driver (set when an agent connects); fall back to
        // the executor's, mirroring `handle_execute`.
        let driver = match self.shared_driver.lock().await.clone() {
            Some(driver) => Some(driver),
            None => self.executor.as_ref().map(|e| e.driver().clone()),
        };
        let note = match driver {
            Some(driver) => match driver.set_target(bundle_id).await {
                Ok(()) => String::new(),
                Err(e) if is_agent_rejection(&e) => {
                    return Err(format!("Failed to set target: {}", e))
                }
                Err(e) => format!(" (recorded; agent not updated: {})", e),
            },
            None => " (recorded; no agent connected)".to_string(),
        };
        self.target_bundle_id = Some(bundle_id.to_string());
        Ok(format!("Target set to {}{}", bundle_id, note))
    }

    /// Log an action to the current session.
    pub async fn log_action(
        &self,
        action: ActionType,
        result: ActionResult,
        duration_ms: Option<u64>,
        tag: Option<String>,
    ) {
        if let Some(session) = &self.session {
            let screenshot = None;
            session
                .log_action(action, result, screenshot, duration_ms, tag)
                .await;
        }
    }
}

/// Validates a UDID format.
///
/// Accepts:
/// - Simulator UDIDs: 8-4-4-4-12 hex groups (36 chars, e.g., `A1B2C3D4-…`)
/// - Legacy physical UDIDs: 40 contiguous hex characters
/// - Modern physical UDIDs (ECID-based): 8 hex + dash + 16 hex (25 chars,
///   e.g., `00008140-000A15911AE3001C`)
/// - CoreDevice identifiers: UUID format (same as simulator format)
fn is_valid_udid(udid: &str) -> bool {
    // Legacy physical device UDID: 40 contiguous hex characters.
    if udid.len() == 40 && udid.chars().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }
    // Modern ECID-based physical UDID: 8 hex digits, dash, 16 hex digits (25 chars).
    if udid.len() == 25 {
        let parts: Vec<&str> = udid.split('-').collect();
        if parts.len() == 2
            && parts[0].len() == 8
            && parts[1].len() == 16
            && parts[0].chars().all(|c| c.is_ascii_hexdigit())
            && parts[1].chars().all(|c| c.is_ascii_hexdigit())
        {
            return true;
        }
    }
    // Simulator / CoreDevice UUID: 8-4-4-4-12 hex groups (36 chars).
    if udid.len() != 36 {
        return false;
    }
    let parts: Vec<&str> = udid.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    let expected_lengths = [8, 4, 4, 4, 12];
    for (part, &expected_len) in parts.iter().zip(expected_lengths.iter()) {
        if part.len() != expected_len {
            return false;
        }
        if !part.chars().all(|c| c.is_ascii_hexdigit()) {
            return false;
        }
    }
    true
}

/// Strip surrounding quotes from a string if present.
/// Whether a failed `set_target` push was the agent *rejecting the id* rather
/// than the agent being unreachable.
///
/// Only `CommandFailed` carries an answer from the agent — the Android agent
/// returns one for a package that isn't installed. Everything else
/// (`NotConnected`, `ConnectionLost`, `Timeout`, IO, parse) describes the
/// connection, and says nothing about whether the bundle id is good.
fn is_agent_rejection(e: &DriverError) -> bool {
    matches!(e, DriverError::CommandFailed(_))
}

fn strip_quotes(s: &str) -> &str {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// Builds `start-target`'s human message and structured payload from the
/// launch outcome.
///
/// Kept separate from the handler so the three-way branch — launched / already
/// running / relaunched — is testable without a device. `launched` is the field
/// a script branches on: it is false *only* for the attach case, which is the
/// outcome the caller previously could not detect.
fn start_target_report(
    bundle_id: &str,
    was_running: bool,
    pid: Option<u32>,
    force: bool,
) -> (String, serde_json::Value) {
    let pid_note = match pid {
        Some(p) => format!(" (pid {})", p),
        None => String::new(),
    };
    let message = if !was_running {
        format!("Launched {}{}", bundle_id, pid_note)
    } else if force {
        format!("Relaunched {}{}", bundle_id, pid_note)
    } else {
        format!(
            "{} already running{} — not relaunched; pass --force to restart it",
            bundle_id, pid_note
        )
    };
    let data = serde_json::json!({
        "bundle_id": bundle_id,
        "launched": force || !was_running,
        "already_running": was_running,
        "pid": pid,
    });
    (message, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- start-target outcome reporting ---

    #[test]
    fn start_target_report_fresh_launch() {
        let (msg, data) = start_target_report("com.example.App", false, Some(42), false);
        assert_eq!(msg, "Launched com.example.App (pid 42)");
        assert_eq!(data["launched"], true);
        assert_eq!(data["already_running"], false);
        assert_eq!(data["pid"], 42);
    }

    #[test]
    fn start_target_report_already_running_is_not_a_launch() {
        // The case the caller could not previously detect: success, but
        // nothing actually started.
        let (msg, data) = start_target_report("com.example.App", true, Some(42), false);
        assert!(msg.contains("already running (pid 42)"), "{msg}");
        assert!(msg.contains("--force"), "{msg}");
        assert_eq!(data["launched"], false);
        assert_eq!(data["already_running"], true);
    }

    #[test]
    fn start_target_report_force_relaunch_counts_as_launched() {
        let (msg, data) = start_target_report("com.example.App", true, Some(99), true);
        assert_eq!(msg, "Relaunched com.example.App (pid 99)");
        assert_eq!(data["launched"], true);
        assert_eq!(data["already_running"], true);
    }

    #[test]
    fn start_target_report_omits_pid_when_unknown() {
        // An unparseable/absent pid degrades the report; it must not fabricate
        // one or drop the outcome.
        let (msg, data) = start_target_report("com.example.App", false, None, false);
        assert_eq!(msg, "Launched com.example.App");
        assert_eq!(data["pid"], serde_json::Value::Null);
        assert_eq!(data["launched"], true);
    }

    /// Build an `AndroidLifecycle` pointing at a dummy project — `new` does no
    /// device I/O, so this is safe with no emulator/adb present. `terminate_agent`
    /// (called by the handlers under test) is best-effort and never errors with
    /// no child process.
    fn dummy_android_lifecycle() -> Arc<AndroidLifecycle> {
        let config = AndroidLifecycleConfig::new(PathBuf::from("/tmp/agent-android"));
        Arc::new(AndroidLifecycle::new("emulator-5554".into(), config))
    }

    /// `stop-agent` must terminate the Android lifecycle (not just iOS) and
    /// clear the stored forward, returning success when an Android agent was
    /// present (finding #2).
    #[test]
    fn stop_agent_clears_android_lifecycle() {
        let mut state = ServerState::new("test".into());
        // Simulate a running Android agent (no forward — building a real
        // `AdbForward` needs adb; the forward field default is `None` and the
        // handler's `.take()` is exercised regardless).
        state.android_serial = Some("emulator-5554".into());
        state.android_lifecycle = Some(dummy_android_lifecycle());

        let resp = state.handle_stop_agent();
        match resp {
            IpcResponse::CommandResult { success, .. } => assert!(success),
            other => panic!("expected CommandResult, got {other:?}"),
        }
        // Lifecycle and forward are released.
        assert!(state.android_lifecycle.is_none());
        assert!(state.android_forward.is_none());
    }

    /// `stop-agent` with nothing running returns failure (no managed agent).
    #[test]
    fn stop_agent_no_agent_is_failure() {
        let mut state = ServerState::new("test".into());
        state.agent_lifecycle = None;
        state.android_lifecycle = None;
        state.android_forward = None;
        match state.handle_stop_agent() {
            IpcResponse::CommandResult { success, .. } => assert!(!success),
            other => panic!("expected CommandResult, got {other:?}"),
        }
    }

    /// Booting an iOS device must retire any active Android selection so
    /// device/agent state is mutually exclusive (finding #3). Uses a real
    /// simulator UDID format; `Simctl::boot` may fail with no simulator, in
    /// which case the clearing is not asserted (the success path is the one the
    /// finding targets, exercised when a simulator exists).
    #[test]
    fn boot_ios_clears_android_state_on_success() {
        let mut state = ServerState::new("test".into());
        state.android_serial = Some("emulator-5554".into());
        state.android_lifecycle = Some(dummy_android_lifecycle());

        // A syntactically valid simulator UDID. If boot succeeds (a sim with
        // this UDID exists), the Android state must be cleared; if it fails (no
        // such sim — the common CI case), state is left as-is and we skip the
        // assertion since the mutual-exclusion lives on the success branch.
        let resp = state.handle_boot_device_ios("00000000-0000-0000-0000-000000000000");
        if let IpcResponse::CommandResult { success: true, .. } = resp {
            assert!(state.android_serial.is_none());
            assert!(state.android_lifecycle.is_none());
            assert!(state.android_forward.is_none());
            assert!(state.simulator_udid.is_some());
        }
    }

    /// Starting the Android agent with no device selected returns a clear
    /// error and does not touch iOS executor state.
    #[tokio::test]
    async fn start_agent_android_without_device_errors() {
        let mut state = ServerState::new("test".into());
        state.android_serial = None;
        let resp = state.handle_start_agent_android(None, None).await;
        match resp {
            IpcResponse::CommandResult { success, message } => {
                assert!(!success);
                assert!(message.contains("No Android device selected"));
            }
            other => panic!("expected CommandResult, got {other:?}"),
        }
    }

    /// `start-agent` with no explicit platform (which the REPL sends as the
    /// default iOS) routes to the Android path when an Android device is the
    /// active selection, instead of failing with "No simulator selected".
    /// We assert via the error message: the Android path reports a build/adb
    /// failure (no real device in CI), never the iOS "No simulator selected".
    #[tokio::test]
    async fn start_agent_infers_android_from_selected_device() {
        let mut state = ServerState::new("test".into());
        state.simulator_udid = None;
        state.android_serial = Some("emulator-5554".into());
        let resp = state.handle_start_agent(None, Platform::Ios, None).await;
        match resp {
            IpcResponse::CommandResult { message, .. } => {
                assert!(
                    !message.contains("No simulator selected"),
                    "expected Android routing, got iOS error: {message}"
                );
            }
            other => panic!("expected CommandResult, got {other:?}"),
        }
    }

    /// The Android forward field defaults to `None` and is independent of the
    /// iOS forward-less path (finding #1 wiring: the field exists and is part
    /// of `ServerState`).
    #[test]
    fn android_forward_defaults_to_none() {
        let state = ServerState::new("test".into());
        assert!(state.android_forward.is_none());
    }

    // --- set-target without an agent ---

    /// `ServerState::new` seeds an executor whenever a simulator happens to be
    /// booted on the host, which would mask the agentless path under test.
    async fn agentless_state() -> ServerState {
        let mut state = ServerState::new("test".into());
        state.executor = None;
        *state.shared_driver.lock().await = None;
        state
    }

    /// The reported failure mode: an executor exists (a simulator was booted,
    /// so `ServerState::new` seeded one) but nothing is connected behind it, so
    /// the driver push fails with `NotConnected`. The id must still be recorded
    /// — that is what made `set-target` demand a full agent connection.
    async fn unreachable_agent_state() -> ServerState {
        let mut state = agentless_state().await;
        // Port 1 is privileged and unbound: `connect` is never called, so every
        // request fails at the "no client" check with `NotConnected`.
        state.executor = Some(ActionExecutor::with_agent("127.0.0.1".to_string(), 1));
        state
    }

    /// The REPL's `set-target` records the bundle id with no agent connected,
    /// so `start-target` — which drives simctl/adb directly — is usable without
    /// standing up an agent connection just to store a string.
    #[tokio::test]
    async fn set_target_without_agent_records_bundle_id() {
        let mut state = agentless_state().await;
        match state.handle_set_target("com.example.App").await {
            IpcResponse::CommandResult { success, message } => {
                assert!(success, "{message}");
                assert!(message.contains("com.example.App"), "{message}");
            }
            other => panic!("expected CommandResult, got {other:?}"),
        }
        assert_eq!(state.target_bundle_id.as_deref(), Some("com.example.App"));
    }

    /// The CLI routes `set-target` through `Execute`, which used to reject it
    /// with "No automation backend connected" before the handler ever ran.
    #[tokio::test]
    async fn execute_set_target_without_agent_records_bundle_id() {
        let mut state = agentless_state().await;
        let action = ActionType::SetTarget {
            bundle_id: "com.example.App".to_string(),
        };
        match state.handle_execute(action, None).await {
            IpcResponse::ActionResult {
                success, message, ..
            } => {
                assert!(success, "{message}");
            }
            other => panic!("expected ActionResult, got {other:?}"),
        }
        assert_eq!(state.target_bundle_id.as_deref(), Some("com.example.App"));
    }

    /// An unreachable agent is reported, not fatal: the id is recorded and the
    /// caller is told the agent did not get it.
    #[tokio::test]
    async fn set_target_with_unreachable_agent_still_records() {
        let mut state = unreachable_agent_state().await;
        match state.handle_set_target("com.example.App").await {
            IpcResponse::CommandResult { success, message } => {
                assert!(success, "{message}");
                assert!(message.contains("agent not updated"), "{message}");
            }
            other => panic!("expected CommandResult, got {other:?}"),
        }
        assert_eq!(state.target_bundle_id.as_deref(), Some("com.example.App"));
    }

    /// Same for the CLI's `Execute` path — this is the exact command that
    /// failed with "Not connected to automation backend".
    #[tokio::test]
    async fn execute_set_target_with_unreachable_agent_still_records() {
        let mut state = unreachable_agent_state().await;
        let action = ActionType::SetTarget {
            bundle_id: "com.example.App".to_string(),
        };
        match state.handle_execute(action, None).await {
            IpcResponse::ActionResult {
                success, message, ..
            } => {
                assert!(success, "{message}");
                assert!(message.contains("agent not updated"), "{message}");
            }
            other => panic!("expected ActionResult, got {other:?}"),
        }
        assert_eq!(state.target_bundle_id.as_deref(), Some("com.example.App"));
    }

    /// An agent that *answers* with a rejection is not the same as one that
    /// cannot be reached. The Android agent refuses a package that isn't
    /// installed (`CommandFailed`), and that must stay fatal — otherwise a
    /// typo'd id sits in server state waiting to fail again at `start-target`.
    /// Every other variant is the connection, not the id.
    #[test]
    fn only_an_agent_answer_counts_as_rejecting_the_target() {
        assert!(is_agent_rejection(&DriverError::CommandFailed(
            "Target application is not installed".into()
        )));
        assert!(!is_agent_rejection(&DriverError::NotConnected));
        assert!(!is_agent_rejection(&DriverError::ConnectionLost(
            "reset by peer".into()
        )));
        assert!(!is_agent_rejection(&DriverError::Timeout));
    }

    /// Connecting an agent must not clobber a target recorded while none was
    /// around — `set_executor_with_driver` re-pushes it, and that push must be
    /// non-fatal when it fails. (The push itself is verified end-to-end against
    /// a real simulator; with no agent listening here, only its harmlessness
    /// and the surviving state are asserted.)
    #[tokio::test]
    async fn connecting_an_agent_preserves_the_recorded_target() {
        let mut state = agentless_state().await;
        state.handle_set_target("com.example.App").await;

        let driver = Arc::new(AndroidDriver::new("emulator-5554", Some(1), 1));
        state.set_executor_with_driver(driver).await;

        assert_eq!(state.target_bundle_id.as_deref(), Some("com.example.App"));
        assert!(state.shared_driver.lock().await.is_some());
    }

    /// Only `set-target` is exempt: everything else still needs a driver, so a
    /// missing agent must not be silently swallowed.
    #[tokio::test]
    async fn execute_other_action_without_agent_still_errors() {
        let mut state = agentless_state().await;
        let action = ActionType::Tap {
            selector: "ok".to_string(),
            by_label: false,
            element_type: None,
            timeout_ms: None,
        };
        match state.handle_execute(action, None).await {
            IpcResponse::Error { message } => {
                assert!(
                    message.contains("No automation backend connected"),
                    "{message}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
        assert!(state.target_bundle_id.is_none());
    }

    /// An empty bundle id records nothing — the agentless path must not turn
    /// `set-target ""` into a stored target that later launches nothing.
    #[tokio::test]
    async fn execute_set_target_empty_bundle_id_records_nothing() {
        let mut state = agentless_state().await;
        let action = ActionType::SetTarget {
            bundle_id: "  ".to_string(),
        };
        match state.handle_execute(action, None).await {
            IpcResponse::Error { .. } => {}
            other => panic!("expected Error, got {other:?}"),
        }
        assert!(state.target_bundle_id.is_none());
    }
}
