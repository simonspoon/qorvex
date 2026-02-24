//! Core server state and request handling.
//!
//! This module extracts the backend logic from qorvex-repl's App into a
//! standalone `ServerState` that can be driven by an IPC socket server.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, info};

use qorvex_core::action::{ActionResult, ActionType};
use qorvex_core::agent_driver::AgentDriver;
use qorvex_core::agent_lifecycle::{AgentLifecycle, AgentLifecycleConfig};
use qorvex_core::config::QorvexConfig;
use qorvex_core::driver::AutomationDriver;
use qorvex_core::element::UIElement;
use qorvex_core::executor::ActionExecutor;
use qorvex_core::ipc::{IpcRequest, IpcResponse};
use qorvex_core::session::{Session, SessionEvent};
use qorvex_core::simctl::{Simctl, SimulatorDevice};
use qorvex_core::watcher::{ScreenWatcher, WatcherConfig, WatcherHandle};

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
    pub watcher_handle: Option<WatcherHandle>,
    pub element_update_rx: Option<mpsc::Receiver<Vec<UIElement>>>,
    pub cached_elements: Vec<UIElement>,
    pub cached_devices: Vec<SimulatorDevice>,
    pub target_bundle_id: Option<String>,
    pub default_timeout_ms: u64,
}

impl ServerState {
    /// Create a new `ServerState`, pre-fetching devices and detecting a booted simulator.
    pub fn new(session_name: String) -> Self {
        let cached_devices = Simctl::list_devices().unwrap_or_default();
        let simulator_udid = Simctl::get_booted_udid().ok();
        let executor = simulator_udid
            .as_ref()
            .map(|_| ActionExecutor::with_agent("localhost".to_string(), 8080));

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
            watcher_handle: None,
            element_update_rx: None,
            cached_elements: Vec::new(),
            cached_devices,
            target_bundle_id: None,
            default_timeout_ms: 5000,
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
            IpcRequest::UseDevice { udid } => self.handle_use_device(&udid),
            IpcRequest::BootDevice { udid } => self.handle_boot_device(&udid),

            // ── Agent Management ────────────────────────────────────────
            IpcRequest::StartAgent { project_dir } => self.handle_start_agent(project_dir).await,
            IpcRequest::StopAgent => self.handle_stop_agent(),
            IpcRequest::Connect { host, port } => self.handle_connect(&host, port).await,

            // ── Target App Lifecycle ────────────────────────────────────
            IpcRequest::StartTarget => self.handle_start_target(),
            IpcRequest::StopTarget => self.handle_stop_target(),

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

            // ── Watcher ─────────────────────────────────────────────────
            IpcRequest::StartWatcher { interval_ms } => {
                self.handle_start_watcher(interval_ms).await
            }
            IpcRequest::StopWatcher => self.handle_stop_watcher(),

            // ── Info ────────────────────────────────────────────────────
            IpcRequest::GetSessionInfo => self.handle_get_session_info().await,
            IpcRequest::GetCompletionData => IpcResponse::CompletionData {
                elements: self.cached_elements.clone(),
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
                if let Some(agent_source_dir) = config.agent_source_dir {
                    info!("Auto-starting agent");
                    let lc_config = AgentLifecycleConfig::new(agent_source_dir);
                    let lifecycle = Arc::new(AgentLifecycle::new(udid.clone(), lc_config));

                    match lifecycle.ensure_agent_ready().await {
                        Ok(()) => {
                            let mut driver = AgentDriver::direct("127.0.0.1", 8080)
                                .with_lifecycle(lifecycle.clone());
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
        if let Some(handle) = self.watcher_handle.take() {
            handle.cancel();
        }
        self.element_update_rx = None;
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

    fn handle_use_device(&mut self, udid: &str) -> IpcResponse {
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
        self.simulator_udid = Some(udid.to_string());
        self.executor = Some(ActionExecutor::with_agent("localhost".to_string(), 8080));
        IpcResponse::CommandResult {
            success: true,
            message: format!("Using device {}", udid),
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
                self.executor = Some(ActionExecutor::with_agent("localhost".to_string(), 8080));
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

        if let Some(project_dir_str) = project_dir {
            // With path: build, spawn, wait, store lifecycle
            let project_dir = PathBuf::from(strip_quotes(&project_dir_str));
            let config = AgentLifecycleConfig::new(project_dir);
            let lifecycle = Arc::new(AgentLifecycle::new(udid, config));

            match lifecycle.ensure_running().await {
                Ok(()) => {
                    let mut driver = AgentDriver::direct("127.0.0.1", 8080)
                        .with_lifecycle(lifecycle.clone());
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
            if let Some(project_dir) = config.agent_source_dir {
                let lc_config = AgentLifecycleConfig::new(project_dir);
                let lifecycle = Arc::new(AgentLifecycle::new(udid, lc_config));

                match lifecycle.ensure_agent_ready().await {
                    Ok(()) => {
                        let mut driver = AgentDriver::direct("127.0.0.1", 8080)
                            .with_lifecycle(lifecycle.clone());
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
                let lc_config = AgentLifecycleConfig::new(PathBuf::new());
                let lifecycle = AgentLifecycle::new(udid, lc_config);

                match lifecycle.wait_for_ready().await {
                    Ok(()) => {
                        let mut driver = AgentDriver::direct("127.0.0.1", 8080);
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
        match &self.executor {
            Some(executor) => match executor.driver().set_target(bundle_id).await {
                Ok(()) => {
                    self.target_bundle_id = Some(bundle_id.to_string());
                    IpcResponse::CommandResult {
                        success: true,
                        message: format!("Target set to {}", bundle_id),
                    }
                }
                Err(e) => IpcResponse::CommandResult {
                    success: false,
                    message: format!("Failed to set target: {}", e),
                },
            },
            None => IpcResponse::CommandResult {
                success: false,
                message: "No agent connected".to_string(),
            },
        }
    }

    fn handle_start_target(&self) -> IpcResponse {
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
        match Simctl::launch_app(udid, bundle_id) {
            Ok(()) => IpcResponse::CommandResult {
                success: true,
                message: format!("Launched {}", bundle_id),
            },
            Err(e) => IpcResponse::CommandResult {
                success: false,
                message: format!("Failed to launch app: {}", e),
            },
        }
    }

    fn handle_stop_target(&self) -> IpcResponse {
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
        match Simctl::terminate_app(udid, bundle_id) {
            Ok(()) => IpcResponse::CommandResult {
                success: true,
                message: format!("Terminated {}", bundle_id),
            },
            Err(e) => IpcResponse::CommandResult {
                success: false,
                message: format!("Failed to terminate app: {}", e),
            },
        }
    }

    // ── Watcher ─────────────────────────────────────────────────────────

    async fn handle_start_watcher(&mut self, interval_ms: Option<u64>) -> IpcResponse {
        if self.watcher_handle.is_some() {
            return IpcResponse::CommandResult {
                success: false,
                message: "Watcher already running".to_string(),
            };
        }
        if self.session.is_none() {
            return IpcResponse::CommandResult {
                success: false,
                message: "No active session. Send StartSession first.".to_string(),
            };
        }
        if self.executor.is_none() {
            return IpcResponse::CommandResult {
                success: false,
                message: "No simulator selected".to_string(),
            };
        }

        let interval = interval_ms.unwrap_or(500);
        let config = WatcherConfig {
            interval_ms: interval,
            capture_screenshots: true,
            visual_change_threshold: 5,
        };

        let session = self.session.as_ref().unwrap().clone();

        // Create channel for element updates
        let (tx, rx) = mpsc::channel::<Vec<UIElement>>(16);
        self.element_update_rx = Some(rx);

        // Spawn task to forward session events to the channel
        let mut event_rx = session.subscribe();
        tokio::spawn(async move {
            while let Ok(event) = event_rx.recv().await {
                if let SessionEvent::ScreenInfoUpdated { elements, .. } = event {
                    let elements_vec = (*elements).clone();
                    if tx.send(elements_vec).await.is_err() {
                        break;
                    }
                }
            }
        });

        let driver = self.executor.as_ref().unwrap().driver().clone();
        let handle = ScreenWatcher::spawn(session, driver, config);
        self.watcher_handle = Some(handle);

        IpcResponse::CommandResult {
            success: true,
            message: format!("Watcher started ({}ms interval)", interval),
        }
    }

    fn handle_stop_watcher(&mut self) -> IpcResponse {
        if let Some(handle) = self.watcher_handle.take() {
            handle.cancel();
            self.element_update_rx = None;
            IpcResponse::CommandResult {
                success: true,
                message: "Watcher stopped".to_string(),
            }
        } else {
            IpcResponse::CommandResult {
                success: false,
                message: "No watcher running".to_string(),
            }
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
            self.log_action(action, ActionResult::Success, None, tag).await;
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
            self.executor.as_ref().map(|e| ActionExecutor::new(e.driver().clone()))
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
                self.log_action(action, action_result, duration_ms, tag).await;

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

    /// Non-blocking poll for element updates from the watcher.
    pub fn check_element_updates(&mut self) {
        if let Some(ref mut rx) = self.element_update_rx {
            while let Ok(elements) = rx.try_recv() {
                self.cached_elements = elements;
            }
        }
    }
}

/// Validates a simulator UDID format (8-4-4-4-12 hex).
fn is_valid_udid(udid: &str) -> bool {
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
