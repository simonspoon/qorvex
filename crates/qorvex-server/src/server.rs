//! Core server state and request handling.
//!
//! This module extracts the backend logic from qorvex-repl's App into a
//! standalone `ServerState` that can be driven by an IPC socket server.

use std::path::PathBuf;
use std::sync::Arc;

use tracing::{debug, info};

use qorvex_core::action::{ActionResult, ActionType};
use qorvex_core::agent_driver::AgentDriver;
use qorvex_core::agent_lifecycle::{AgentLifecycle, AgentLifecycleConfig};
use qorvex_core::config::QorvexConfig;
use qorvex_core::driver::{flatten_elements, AutomationDriver};
use qorvex_core::executor::ActionExecutor;
use qorvex_core::ipc::{IpcRequest, IpcResponse};
use qorvex_core::session::Session;
use qorvex_core::simctl::{Simctl, SimulatorDevice};

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
}

impl ServerState {
    /// Create a new `ServerState`, pre-fetching devices and detecting a booted simulator.
    pub fn new(session_name: String) -> Self {
        let config = QorvexConfig::load();
        let agent_port = config.agent_port();
        let cached_devices = Simctl::list_devices().unwrap_or_default();
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
            target_bundle_id: None,
            default_timeout_ms: 5000,
            agent_port,
            is_physical_device: false,
            tunnel_address: None,
            use_core_device: false,
            direct_host: None,
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
            IpcRequest::ListDevices => self.handle_list_devices(),
            IpcRequest::ListPhysicalDevices => self.handle_list_physical_devices().await,
            IpcRequest::UseDevice { udid } => self.handle_use_device(&udid).await,
            IpcRequest::BootDevice { udid } => self.handle_boot_device(&udid),

            // ── Agent Management ────────────────────────────────────────
            IpcRequest::StartAgent { project_dir } => self.handle_start_agent(project_dir).await,
            IpcRequest::StopAgent => self.handle_stop_agent(),
            IpcRequest::Connect { host, port } => self.handle_connect(&host, port).await,

            // ── Target App Lifecycle ────────────────────────────────────
            IpcRequest::StartTarget => self.handle_start_target().await,
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

            // ── Info ────────────────────────────────────────────────────
            IpcRequest::GetSessionInfo => self.handle_get_session_info().await,
            IpcRequest::GetCompletionData => IpcResponse::CompletionData {
                elements: Vec::new(),
                devices: self.cached_devices.clone(),
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
        let session = Session::new(self.simulator_udid.clone(), &self.session_name);
        self.session = Some(session.clone());
        self.shared_driver = Arc::new(tokio::sync::Mutex::new(None));

        info!(session_name = %self.session_name, "Session started");

        // Auto-start agent if no executor or agent not reachable
        let needs_agent = match &self.executor {
            Some(executor) => executor.driver().screenshot().await.is_err(),
            None => true,
        };

        if needs_agent {
            if let Some(ref udid) = self.simulator_udid.clone() {
                let config = QorvexConfig::load();
                if let Some(agent_source_dir) = config.effective_agent_source_dir() {
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
                                }
                                Err(e) => {
                                    info!(error = %e, "Agent started but connection failed");
                                }
                            }
                        }
                        Err(e) => {
                            info!(error = %e, "Auto-start agent failed");
                        }
                    }
                }
            }
        }

        IpcResponse::CommandResult {
            success: true,
            message: "Session started".to_string(),
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

    fn handle_list_devices(&mut self) -> IpcResponse {
        match Simctl::list_devices() {
            Ok(devices) => {
                self.cached_devices = devices.clone();
                IpcResponse::DeviceList { devices }
            }
            Err(e) => IpcResponse::Error {
                message: e.to_string(),
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

    async fn handle_use_device(&mut self, udid: &str) -> IpcResponse {
        let udid = strip_quotes(udid);
        if udid.is_empty() {
            return IpcResponse::CommandResult {
                success: false,
                message: "use_device requires a UDID".to_string(),
            };
        }
        if !is_valid_udid(udid) {
            return IpcResponse::CommandResult {
                success: false,
                message: format!("Invalid UDID format: {}", udid),
            };
        }
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

    fn handle_boot_device(&mut self, udid: &str) -> IpcResponse {
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

    // ── Agent ───────────────────────────────────────────────────────────

    async fn handle_start_agent(&mut self, project_dir: Option<String>) -> IpcResponse {
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
        if let Some(lifecycle) = self.agent_lifecycle.take() {
            let _ = lifecycle.terminate_agent();
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
        let (response, action_result) = match &self.executor {
            Some(executor) => match executor.driver().set_target(bundle_id).await {
                Ok(()) => {
                    self.target_bundle_id = Some(bundle_id.to_string());
                    (
                        IpcResponse::CommandResult {
                            success: true,
                            message: format!("Target set to {}", bundle_id),
                        },
                        ActionResult::Success,
                    )
                }
                Err(e) => {
                    let msg = format!("Failed to set target: {}", e);
                    (
                        IpcResponse::CommandResult {
                            success: false,
                            message: msg.clone(),
                        },
                        ActionResult::Failure(msg),
                    )
                }
            },
            None => {
                let msg = "No agent connected".to_string();
                (
                    IpcResponse::CommandResult {
                        success: false,
                        message: msg.clone(),
                    },
                    ActionResult::Failure(msg),
                )
            }
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

    async fn handle_start_target(&self) -> IpcResponse {
        let Some(ref bundle_id) = self.target_bundle_id else {
            return IpcResponse::CommandResult {
                success: false,
                message: "No target set. Use set-target first.".to_string(),
            };
        };
        let Some(ref udid) = self.simulator_udid else {
            return IpcResponse::CommandResult {
                success: false,
                message: "No simulator selected.".to_string(),
            };
        };
        let (response, action_result) = match Simctl::launch_app(udid, bundle_id) {
            Ok(()) => (
                IpcResponse::CommandResult {
                    success: true,
                    message: format!("Launched {}", bundle_id),
                },
                ActionResult::Success,
            ),
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
        self.log_action(ActionType::StartTarget, action_result, None, None)
            .await;
        response
    }

    async fn handle_stop_target(&self) -> IpcResponse {
        let Some(ref bundle_id) = self.target_bundle_id else {
            return IpcResponse::CommandResult {
                success: false,
                message: "No target set. Use set-target first.".to_string(),
            };
        };
        let Some(ref udid) = self.simulator_udid else {
            return IpcResponse::CommandResult {
                success: false,
                message: "No simulator selected.".to_string(),
            };
        };
        let (response, action_result) = match Simctl::terminate_app(udid, bundle_id) {
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
            };
        };

        match driver.dump_tree().await {
            Ok(hierarchy) => {
                let elements = flatten_elements(&hierarchy);
                IpcResponse::CompletionData {
                    elements,
                    devices: Vec::new(),
                }
            }
            Err(_) => IpcResponse::CompletionData {
                elements: Vec::new(),
                devices: Vec::new(),
            },
        }
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
                // Sync server state when target is set via Execute path
                if result.success {
                    if let ActionType::SetTarget { ref bundle_id } = action {
                        self.target_bundle_id = Some(bundle_id.clone());
                    }
                }

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
        *self.shared_driver.lock().await = Some(driver);
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
fn strip_quotes(s: &str) -> &str {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        &s[1..s.len() - 1]
    } else {
        s
    }
}
