//! Application state and event handling.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use ratatui::layout::Rect;
use ratatui::text::Line;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tui_input::Input;

use qorvex_core::action::{ActionResult, ActionType};
use qorvex_core::agent_driver::AgentDriver;
use qorvex_core::agent_lifecycle::{AgentLifecycle, AgentLifecycleConfig};
use qorvex_core::driver::AutomationDriver;
use qorvex_core::element::UIElement;
use qorvex_core::executor::ActionExecutor;
use qorvex_core::ipc::{socket_path, IpcServer};
use qorvex_core::session::{Session, SessionEvent};
use qorvex_core::simctl::{Simctl, SimulatorDevice};
use qorvex_core::watcher::{ScreenWatcher, WatcherConfig, WatcherHandle};

use crate::completion::{CandidateKind, CompletionState};
use crate::format::{format_command, format_device, format_element, format_result};

/// Maximum number of lines to keep in output history.
const MAX_OUTPUT_HISTORY: usize = 1000;

/// A position in the output text (logical line index + column offset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextPosition {
    /// Index into output_history.
    pub line: usize,
    /// Character column within the line.
    pub col: usize,
}

impl TextPosition {
    pub fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

impl PartialOrd for TextPosition {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TextPosition {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.line.cmp(&other.line).then(self.col.cmp(&other.col))
    }
}

/// Text selection state for the output area.
#[derive(Debug, Default)]
pub struct SelectionState {
    /// Anchor point (where the mouse was pressed).
    pub anchor: Option<TextPosition>,
    /// Current end point (where the mouse is now).
    pub endpoint: Option<TextPosition>,
    /// Whether a drag is currently in progress.
    pub dragging: bool,
}

impl SelectionState {
    /// Returns the ordered (start, end) positions if a selection exists.
    pub fn range(&self) -> Option<(TextPosition, TextPosition)> {
        match (self.anchor, self.endpoint) {
            (Some(a), Some(b)) if a != b => {
                if a <= b { Some((a, b)) } else { Some((b, a)) }
            }
            _ => None,
        }
    }

    /// Whether there is an active selection.
    pub fn has_selection(&self) -> bool {
        self.range().is_some()
    }

    /// Clear the selection.
    pub fn clear(&mut self) {
        self.anchor = None;
        self.endpoint = None;
        self.dragging = false;
    }
}

/// Application state.
pub struct App {
    /// Text input widget state.
    pub input: Input,
    /// Completion state.
    pub completion: CompletionState,
    /// Output history lines.
    pub output_history: VecDeque<Line<'static>>,
    /// Scroll offset from bottom (0 = at bottom).
    pub output_scroll_position: usize,
    /// Text selection state for copy support.
    pub selection: SelectionState,
    /// Cached output area rect from last render (for mouse hit-testing).
    pub output_area: Option<Rect>,
    /// Cached UI elements from last screen info.
    pub cached_elements: Vec<UIElement>,
    /// Cached simulator devices.
    pub cached_devices: Vec<SimulatorDevice>,
    /// Session name.
    pub session_name: String,
    /// Current session.
    pub session: Option<Arc<Session>>,
    /// Current simulator UDID.
    pub simulator_udid: Option<String>,
    /// IPC server handle.
    pub ipc_server_handle: Option<JoinHandle<()>>,
    /// Action executor for automation commands.
    pub executor: Option<ActionExecutor>,
    /// Managed agent lifecycle (set when agent is started via start_agent with a path).
    pub agent_lifecycle: Option<AgentLifecycle>,
    /// Screen watcher handle.
    pub watcher_handle: Option<WatcherHandle>,
    /// Channel receiver for element updates from session events.
    pub element_update_rx: Option<mpsc::Receiver<Vec<UIElement>>>,
    /// Whether the app should quit.
    pub should_quit: bool,
}

impl App {
    /// Create a new App instance.
    pub fn new(session_name: String) -> Self {
        // Pre-fetch devices
        let cached_devices = Simctl::list_devices().unwrap_or_default();

        // Try to get booted simulator
        let simulator_udid = Simctl::get_booted_udid().ok();

        // Create executor if we have a simulator
        let executor = simulator_udid.as_ref().map(|_| ActionExecutor::with_agent("localhost".to_string(), 8080));

        let mut app = Self {
            input: Input::default(),
            completion: CompletionState::default(),
            output_history: VecDeque::new(),
            output_scroll_position: 0,
            selection: SelectionState::default(),
            output_area: None,
            cached_elements: Vec::new(),
            cached_devices,
            session_name,
            session: None,
            simulator_udid,
            executor,
            agent_lifecycle: None,
            ipc_server_handle: None,
            watcher_handle: None,
            element_update_rx: None,
            should_quit: false,
        };

        // Show initial info
        app.add_output(Line::from(format!(
            "Session: {} | Socket: {:?}",
            app.session_name,
            socket_path(&app.session_name)
        )));

        if let Some(udid) = &app.simulator_udid {
            app.add_output(Line::from(format!("Using booted simulator: {}", udid)));
        } else {
            app.add_output(Line::from("No booted simulator found. Use list_devices and use_device."));
        }

        app.add_output(Line::from("Type 'help' for available commands."));
        app.add_output(Line::from(""));

        app
    }

    /// Add a line to output history.
    pub fn add_output(&mut self, line: Line<'static>) {
        self.output_history.push_back(line);
        if self.output_history.len() > MAX_OUTPUT_HISTORY {
            self.output_history.pop_front();
        }
        // Auto-scroll to bottom
        self.output_scroll_position = 0;
    }

    /// Update completion state based on current input.
    pub fn update_completion(&mut self) {
        let input = self.input.value().to_string();
        self.completion.update(&input, &self.cached_elements, &self.cached_devices);
    }

    /// Accept the current completion.
    pub fn accept_completion(&mut self) {
        if let Some(candidate) = self.completion.selected_candidate() {
            let text = candidate.text.clone();
            let kind = candidate.kind;
            let current = self.input.value().to_string();

            // Find where to insert the completion
            let new_value = if let Some(paren_idx) = current.rfind('(') {
                let before_paren = &current[..=paren_idx];
                let args_part = &current[paren_idx + 1..];

                // For ElementSelector kinds, replace ALL arguments (the composed text contains everything)
                if matches!(kind, CandidateKind::ElementSelectorById | CandidateKind::ElementSelectorByLabel) {
                    format!("{}{}", before_paren, text)
                } else {
                    // Find the last comma or start of args
                    if let Some(comma_idx) = args_part.rfind(',') {
                        format!("{}{}, {}", before_paren, &args_part[..comma_idx], text)
                    } else {
                        format!("{}{}", before_paren, text)
                    }
                }
            } else {
                // Completing a command name
                text
            };

            self.input = Input::new(new_value);
            self.completion.hide();
        }
    }

    /// Execute the current input as a command.
    pub async fn execute_command(&mut self) {
        let input = self.input.value().trim().to_string();
        if input.is_empty() {
            return;
        }

        // Add command to output
        self.add_output(format_command(&input));

        // Process the command
        self.process_command(&input).await;

        // Clear input and completion
        self.input = Input::default();
        self.completion.hide();
    }

    /// Scroll output up (away from bottom).
    pub fn scroll_up(&mut self) {
        self.output_scroll_position = self.output_scroll_position.saturating_add(1);
    }

    /// Scroll output down (toward bottom).
    pub fn scroll_down(&mut self) {
        self.output_scroll_position = self.output_scroll_position.saturating_sub(1);
    }

    /// Extract the selected text from output history.
    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection.range()?;
        let mut result = String::new();

        for i in start.line..=end.line.min(self.output_history.len().saturating_sub(1)) {
            let line = &self.output_history[i];
            let line_str: String = line.spans.iter().map(|s| s.content.as_ref()).collect();

            let start_col = if i == start.line { start.col } else { 0 };
            let end_col = if i == end.line { end.col } else { line_str.len() };

            let start_col = start_col.min(line_str.len());
            let end_col = end_col.min(line_str.len());

            if start_col < end_col {
                result.push_str(&line_str[start_col..end_col]);
            } else if i == start.line && i == end.line {
                // Single line, but cols might be equal — skip
            } else {
                // Full line selected but empty
            }

            if i < end.line {
                result.push('\n');
            }
        }

        if result.is_empty() { None } else { Some(result) }
    }

    /// Copy the current selection to the system clipboard.
    pub fn copy_selection_to_clipboard(&mut self) -> bool {
        if let Some(text) = self.selected_text() {
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                if clipboard.set_text(&text).is_ok() {
                    self.selection.clear();
                    return true;
                }
            }
        }
        false
    }

    /// Check for element updates from the watcher (non-blocking).
    pub fn check_element_updates(&mut self) {
        if let Some(ref mut rx) = self.element_update_rx {
            while let Ok(elements) = rx.try_recv() {
                self.cached_elements = elements;
            }
        }
    }

    async fn capture_screenshot(&self) -> Option<String> {
        if let Some(ref executor) = self.executor {
            executor.driver().screenshot().await.ok().map(|bytes| {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.encode(&bytes)
            })
        } else {
            None
        }
    }

    async fn log_action(&self, action: ActionType, result: ActionResult, duration_ms: Option<u64>) {
        if let Some(session) = &self.session {
            let screenshot = self.capture_screenshot().await;
            session.log_action(action, result, screenshot, duration_ms).await;
        }
    }

    async fn process_command(&mut self, input: &str) {
        let (cmd, args) = parse_command(input);

        match cmd.as_str() {
            "start_session" => {
                let session = Session::new(self.simulator_udid.clone(), &self.session_name);
                self.session = Some(session.clone());

                let server = IpcServer::new(session, &self.session_name);
                let handle = tokio::spawn(async move {
                    let _ = server.run().await;
                });
                self.ipc_server_handle = Some(handle);

                self.add_output(format_result(true, "Session started"));
            }
            "end_session" => {
                // Stop watcher first
                if let Some(handle) = self.watcher_handle.take() {
                    handle.cancel();
                }
                self.element_update_rx = None;
                if let Some(handle) = self.ipc_server_handle.take() {
                    handle.abort();
                }
                self.session = None;
                self.add_output(format_result(true, "Session ended"));
            }
            "quit" => {
                // Stop watcher first
                if let Some(handle) = self.watcher_handle.take() {
                    handle.cancel();
                }
                self.element_update_rx = None;
                if let Some(handle) = self.ipc_server_handle.take() {
                    handle.abort();
                }
                // Stop managed agent
                if let Some(lifecycle) = self.agent_lifecycle.take() {
                    let _ = lifecycle.terminate_agent();
                }
                self.session = None;
                self.should_quit = true;
            }
            "start_watcher" => {
                if self.watcher_handle.is_some() {
                    self.add_output(format_result(false, "Watcher already running"));
                } else if self.session.is_none() {
                    self.add_output(format_result(false, "No active session. Run start_session first."));
                } else if self.executor.is_none() {
                    self.add_output(format_result(false, "No simulator selected"));
                } else {
                    let interval_ms = args.first()
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(500);

                    let config = WatcherConfig {
                        interval_ms,
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

                    self.add_output(format_result(true, &format!("Watcher started ({}ms interval)", interval_ms)));
                }
            }
            "stop_watcher" => {
                if let Some(handle) = self.watcher_handle.take() {
                    handle.cancel();
                    self.element_update_rx = None;
                    self.add_output(format_result(true, "Watcher stopped"));
                } else {
                    self.add_output(format_result(false, "No watcher running"));
                }
            }
            "help" => {
                self.show_help();
            }
            "list_devices" => {
                match Simctl::list_devices() {
                    Ok(devices) => {
                        self.cached_devices = devices.clone();
                        for device in &devices {
                            self.add_output(format_device(device));
                        }
                        self.add_output(format_result(true, &format!("{} devices", devices.len())));
                    }
                    Err(e) => {
                        self.add_output(format_result(false, &e.to_string()));
                    }
                }
            }
            "use_device" => {
                let udid = args.first().map(|s| s.as_str()).unwrap_or("");
                if udid.is_empty() {
                    self.add_output(format_result(false, "use_device requires 1 argument: use_device(udid)"));
                } else if !is_valid_udid(udid) {
                    self.add_output(format_result(false, &format!("Invalid UDID format: {}", udid)));
                } else {
                    self.simulator_udid = Some(udid.to_string());
                    self.executor = Some(ActionExecutor::with_agent("localhost".to_string(), 8080));
                    self.add_output(format_result(true, &format!("Using device {}", udid)));
                }
            }
            "boot_device" => {
                let udid = args.first().map(|s| s.as_str()).unwrap_or("");
                if udid.is_empty() {
                    self.add_output(format_result(false, "boot_device requires 1 argument: boot_device(udid)"));
                } else if !is_valid_udid(udid) {
                    self.add_output(format_result(false, &format!("Invalid UDID format: {}", udid)));
                } else {
                    match Simctl::boot(udid) {
                        Ok(_) => {
                            self.simulator_udid = Some(udid.to_string());
                            self.executor = Some(ActionExecutor::with_agent("localhost".to_string(), 8080));
                            self.add_output(format_result(true, &format!("Booted and using device {}", udid)));
                        }
                        Err(e) => {
                            self.add_output(format_result(false, &e.to_string()));
                        }
                    }
                }
            }
            "start_agent" => {
                if self.simulator_udid.is_none() {
                    self.add_output(format_result(false, "No simulator selected. Use use_device or boot_device first."));
                } else {
                    let udid = self.simulator_udid.clone().unwrap();

                    if let Some(project_dir_str) = args.first() {
                        // With path: build, spawn, wait, store lifecycle
                        let project_dir = PathBuf::from(strip_quotes(project_dir_str));
                        let config = AgentLifecycleConfig::new(project_dir);
                        let lifecycle = AgentLifecycle::new(udid, config);

                        self.add_output(Line::from("Building and starting agent..."));

                        match lifecycle.ensure_running().await {
                            Ok(()) => {
                                self.agent_lifecycle = Some(lifecycle);
                                let mut driver = AgentDriver::direct("127.0.0.1", 8080);
                                match driver.connect().await {
                                    Ok(()) => {
                                        self.executor = Some(ActionExecutor::new(Arc::new(driver)));
                                        self.add_output(format_result(true, "Agent started and connected"));
                                    }
                                    Err(e) => {
                                        self.add_output(format_result(false, &format!("Agent started but connection failed: {}", e)));
                                    }
                                }
                            }
                            Err(e) => {
                                self.add_output(format_result(false, &format!("Failed to start agent: {}", e)));
                            }
                        }
                    } else {
                        // No path: connect to externally-started agent
                        self.add_output(Line::from("Connecting to agent..."));
                        let config = AgentLifecycleConfig::new(PathBuf::new());
                        let lifecycle = AgentLifecycle::new(udid, config);

                        match lifecycle.wait_for_ready().await {
                            Ok(()) => {
                                let mut driver = AgentDriver::direct("127.0.0.1", 8080);
                                match driver.connect().await {
                                    Ok(()) => {
                                        self.executor = Some(ActionExecutor::new(Arc::new(driver)));
                                        self.add_output(format_result(true, "Agent connected"));
                                    }
                                    Err(e) => {
                                        self.add_output(format_result(false, &format!("Connection failed: {}", e)));
                                    }
                                }
                            }
                            Err(e) => {
                                self.add_output(format_result(false, &format!("Agent not reachable: {}", e)));
                            }
                        }
                    }
                }
            }
            "stop_agent" => {
                if let Some(lifecycle) = self.agent_lifecycle.take() {
                    let _ = lifecycle.terminate_agent();
                    self.add_output(format_result(true, "Agent stopped"));
                } else {
                    self.add_output(format_result(false, "No managed agent to stop"));
                }
            }
            "set_target" => {
                let bundle_id = args.first().map(|s| strip_quotes(s)).unwrap_or("");
                if bundle_id.is_empty() {
                    self.add_output(format_result(false, "set_target requires 1 argument: set_target(bundle_id)"));
                } else {
                    match &self.executor {
                        Some(executor) => {
                            match executor.driver().set_target(bundle_id).await {
                                Ok(()) => {
                                    self.add_output(format_result(true, &format!("Target set to {}", bundle_id)));
                                }
                                Err(e) => {
                                    self.add_output(format_result(false, &format!("Failed to set target: {}", e)));
                                }
                            }
                        }
                        None => {
                            self.add_output(format_result(false, "No agent connected"));
                        }
                    }
                }
            }
            "list_elements" | "get_screen_info" => {
                match &self.executor {
                    Some(executor) => {
                        let result = executor.execute(ActionType::GetScreenInfo).await;
                        if result.success {
                            if let Some(ref data) = result.data {
                                if let Ok(elements) = serde_json::from_str::<Vec<UIElement>>(data) {
                                    self.cached_elements = elements.clone();
                                    for elem in &elements {
                                        self.add_output(format_element(elem));
                                    }
                                    self.add_output(format_result(true, &format!("{} elements", elements.len())));
                                }
                            }
                            if cmd == "get_screen_info" {
                                self.log_action(ActionType::GetScreenInfo, ActionResult::Success, None).await;
                            }
                        } else {
                            self.add_output(format_result(false, &result.message));
                            if cmd == "get_screen_info" {
                                self.log_action(ActionType::GetScreenInfo, ActionResult::Failure(result.message), None).await;
                            }
                        }
                    }
                    None => {
                        self.add_output(format_result(false, "No simulator selected"));
                    }
                }
            }
            "tap" => {
                let no_wait = args.iter().any(|s| s.trim() == "--no-wait");
                let args: Vec<String> = args.iter().filter(|s| s.trim() != "--no-wait").cloned().collect();
                let selector = args.first().map(|s| strip_quotes(s)).unwrap_or("");
                if selector.is_empty() {
                    self.add_output(format_result(false, "tap requires at least 1 argument: tap(selector) or tap(selector, label) or tap(selector, label, type)"));
                } else {
                    // Check for 'label' flag in second argument
                    let by_label = args.get(1).map(|s| s.trim().to_lowercase() == "label").unwrap_or(false);
                    // Third argument is element type (if present and not empty)
                    let element_type = args.get(2).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

                    match &self.executor {
                        Some(executor) => {
                            // Wait for element first (default behavior, skip with --no-wait)
                            if !no_wait {
                                let wait_result = executor.execute(ActionType::WaitFor {
                                    selector: selector.to_string(),
                                    by_label,
                                    element_type: element_type.clone(),
                                    timeout_ms: 5000,
                                }).await;
                                if !wait_result.success {
                                    self.log_action(
                                        ActionType::Tap {
                                            selector: selector.to_string(),
                                            by_label,
                                            element_type,
                                        },
                                        ActionResult::Failure(wait_result.message.clone()),
                                        None,
                                    ).await;
                                    self.add_output(format_result(false, &wait_result.message));
                                    return;
                                }
                            }

                            let result = executor.execute(ActionType::Tap {
                                selector: selector.to_string(),
                                by_label,
                                element_type: element_type.clone(),
                            }).await;

                            let action_result = if result.success {
                                ActionResult::Success
                            } else {
                                ActionResult::Failure(result.message.clone())
                            };

                            self.log_action(
                                ActionType::Tap {
                                    selector: selector.to_string(),
                                    by_label,
                                    element_type,
                                },
                                action_result,
                                None,
                            ).await;

                            if result.success {
                                let msg = if by_label {
                                    format!("Tapped element with label \"{}\"", selector)
                                } else {
                                    format!("Tapped {}", selector)
                                };
                                self.add_output(format_result(true, &msg));
                            } else {
                                self.add_output(format_result(false, &result.message));
                            }
                        }
                        None => {
                            self.add_output(format_result(false, "No simulator selected"));
                        }
                    }
                }
            }
            "swipe" => {
                let direction = args.first().map(|s| s.trim().to_lowercase()).unwrap_or_else(|| "up".to_string());
                match &self.executor {
                    Some(executor) => {
                        let result = executor.execute(ActionType::Swipe {
                            direction: direction.clone(),
                        }).await;

                        let action_result = if result.success {
                            ActionResult::Success
                        } else {
                            ActionResult::Failure(result.message.clone())
                        };

                        self.log_action(
                            ActionType::Swipe { direction },
                            action_result,
                            None,
                        ).await;

                        if result.success {
                            self.add_output(format_result(true, &result.message));
                        } else {
                            self.add_output(format_result(false, &result.message));
                        }
                    }
                    None => {
                        self.add_output(format_result(false, "No simulator selected"));
                    }
                }
            }
            "tap_location" => {
                if args.len() < 2 {
                    self.add_output(format_result(false, "tap_location requires 2 arguments: tap_location(x, y)"));
                } else {
                    let x: Result<i32, _> = args[0].parse();
                    let y: Result<i32, _> = args[1].parse();

                    match (x, y) {
                        (Ok(x), Ok(y)) if x >= 0 && y >= 0 => {
                            match &self.executor {
                                Some(executor) => {
                                    let result = executor.execute(ActionType::TapLocation { x, y }).await;

                                    let action_result = if result.success {
                                        ActionResult::Success
                                    } else {
                                        ActionResult::Failure(result.message.clone())
                                    };

                                    self.log_action(
                                        ActionType::TapLocation { x, y },
                                        action_result,
                                        None,
                                    ).await;

                                    if result.success {
                                        self.add_output(format_result(true, &format!("Tapped ({}, {})", x, y)));
                                    } else {
                                        self.add_output(format_result(false, &result.message));
                                    }
                                }
                                None => {
                                    self.add_output(format_result(false, "No simulator selected"));
                                }
                            }
                        }
                        _ => {
                            self.add_output(format_result(false, "Invalid coordinates - must be non-negative integers"));
                        }
                    }
                }
            }
            "wait_for" => {
                let selector = args.first().map(|s| strip_quotes(s)).unwrap_or("");
                if selector.is_empty() {
                    self.add_output(format_result(false, "wait_for requires at least 1 argument: wait_for(selector) or wait_for(selector, timeout_ms) or wait_for(selector, timeout_ms, label) or wait_for(selector, timeout_ms, label, type)"));
                } else {
                    // Second arg is timeout (default 5000)
                    let timeout_ms: u64 = args.get(1)
                        .map(|s| s.parse().unwrap_or(5000))
                        .unwrap_or(5000);
                    // Third arg is 'label' flag
                    let by_label = args.get(2).map(|s| s.trim().to_lowercase() == "label").unwrap_or(false);
                    // Fourth arg is element type
                    let element_type = args.get(3).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

                    match &self.executor {
                        Some(executor) => {
                            let result = executor.execute(ActionType::WaitFor {
                                selector: selector.to_string(),
                                by_label,
                                element_type: element_type.clone(),
                                timeout_ms,
                            }).await;

                            let action_result = if result.success {
                                ActionResult::Success
                            } else {
                                ActionResult::Failure(result.message.clone())
                            };

                            let duration_ms = result.data.as_ref()
                                .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
                                .and_then(|v| v.get("elapsed_ms").and_then(|e| e.as_u64()));

                            self.log_action(
                                ActionType::WaitFor {
                                    selector: selector.to_string(),
                                    by_label,
                                    element_type,
                                    timeout_ms,
                                },
                                action_result,
                                duration_ms,
                            ).await;

                            if result.success {
                                self.add_output(format_result(true, &format!("{} ({})", result.message, result.data.unwrap_or_default())));
                            } else {
                                self.add_output(format_result(false, &result.message));
                            }
                        }
                        None => {
                            self.add_output(format_result(false, "No simulator selected"));
                        }
                    }
                }
            }
            "send_keys" => {
                let text = args.join(" ");
                if text.is_empty() {
                    self.add_output(format_result(false, "send_keys requires text: send_keys(text)"));
                } else {
                    match &self.executor {
                        Some(executor) => {
                            let result = executor.execute(ActionType::SendKeys { text: text.clone() }).await;

                            let action_result = if result.success {
                                ActionResult::Success
                            } else {
                                ActionResult::Failure(result.message.clone())
                            };

                            self.log_action(
                                ActionType::SendKeys { text: text.clone() },
                                action_result,
                                None,
                            ).await;

                            if result.success {
                                self.add_output(format_result(true, &format!("Sent: {}", text)));
                            } else {
                                self.add_output(format_result(false, &result.message));
                            }
                        }
                        None => {
                            self.add_output(format_result(false, "No simulator selected"));
                        }
                    }
                }
            }
            "get_value" => {
                let no_wait = args.iter().any(|s| s.trim() == "--no-wait");
                let args: Vec<String> = args.iter().filter(|s| s.trim() != "--no-wait").cloned().collect();
                let selector = args.first().map(|s| strip_quotes(s)).unwrap_or("");
                if selector.is_empty() {
                    self.add_output(format_result(false, "get_value requires at least 1 argument: get_value(selector) or get_value(selector, label) or get_value(selector, label, type)"));
                } else {
                    // Check for 'label' flag in second argument
                    let by_label = args.get(1).map(|s| s.trim().to_lowercase() == "label").unwrap_or(false);
                    // Third argument is element type (if present and not empty)
                    let element_type = args.get(2).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

                    match &self.executor {
                        Some(executor) => {
                            // Wait for element first (default behavior, skip with --no-wait)
                            if !no_wait {
                                let wait_result = executor.execute(ActionType::WaitFor {
                                    selector: selector.to_string(),
                                    by_label,
                                    element_type: element_type.clone(),
                                    timeout_ms: 5000,
                                }).await;
                                if !wait_result.success {
                                    self.log_action(
                                        ActionType::GetValue {
                                            selector: selector.to_string(),
                                            by_label,
                                            element_type,
                                        },
                                        ActionResult::Failure(wait_result.message.clone()),
                                        None,
                                    ).await;
                                    self.add_output(format_result(false, &wait_result.message));
                                    return;
                                }
                            }

                            let result = executor.execute(ActionType::GetValue {
                                selector: selector.to_string(),
                                by_label,
                                element_type: element_type.clone(),
                            }).await;

                            let action_result = if result.success {
                                ActionResult::Success
                            } else {
                                ActionResult::Failure(result.message.clone())
                            };

                            self.log_action(
                                ActionType::GetValue {
                                    selector: selector.to_string(),
                                    by_label,
                                    element_type,
                                },
                                action_result,
                                None,
                            ).await;

                            if result.success {
                                let value = result.data.unwrap_or_else(|| "(null)".to_string());
                                self.add_output(format_result(true, &format!("Value: {}", value)));
                            } else {
                                self.add_output(format_result(false, &result.message));
                            }
                        }
                        None => {
                            self.add_output(format_result(false, "No simulator selected"));
                        }
                    }
                }
            }
            "get_screenshot" => {
                match &self.executor {
                    Some(executor) => {
                        let result = executor.execute(ActionType::GetScreenshot).await;
                        if result.success {
                            if let Some(ref screenshot) = result.screenshot {
                                if let Some(session) = &self.session {
                                    session.log_action(
                                        ActionType::GetScreenshot,
                                        ActionResult::Success,
                                        Some(screenshot.clone()),
                                        None,
                                    ).await;
                                }
                            }
                            // Estimate byte count from base64 data length
                            let byte_count = result.data.as_ref()
                                .map(|d| d.len() * 3 / 4)
                                .unwrap_or(0);
                            self.add_output(format_result(true, &format!("{} bytes (base64 logged)", byte_count)));
                        } else {
                            self.log_action(ActionType::GetScreenshot, ActionResult::Failure(result.message.clone()), None).await;
                            self.add_output(format_result(false, &result.message));
                        }
                    }
                    None => {
                        self.add_output(format_result(false, "No simulator selected"));
                    }
                }
            }
            "log_comment" => {
                let message = args.join(" ");
                if message.is_empty() {
                    self.add_output(format_result(false, "log_comment requires a message"));
                } else {
                    self.log_action(
                        ActionType::LogComment { message: message.clone() },
                        ActionResult::Success,
                        None,
                    ).await;
                    self.add_output(format_result(true, &format!("Logged: {}", message)));
                }
            }
            "get_session_info" => {
                match &self.session {
                    Some(session) => {
                        let action_log = session.get_action_log().await;
                        self.add_output(Line::from(format!("Session: {} (active)", self.session_name)));
                        self.add_output(Line::from(format!("Device: {:?}", self.simulator_udid)));
                        self.add_output(Line::from(format!("Actions: {}", action_log.len())));
                    }
                    None => {
                        self.add_output(Line::from(format!("Session: {} (inactive)", self.session_name)));
                        self.add_output(Line::from(format!("Device: {:?}", self.simulator_udid)));
                    }
                }
            }
            _ => {
                self.add_output(format_result(false, &format!("Unknown command: {}", cmd)));
            }
        }
    }

    fn show_help(&mut self) {
        let help_lines = [
            "",
            "Session:",
            "  start_session          Start a new session",
            "  end_session            End the current session",
            "  get_session_info       Get current session information",
            "",
            "Device:",
            "  list_devices           List available simulators",
            "  use_device(udid)       Select a simulator by UDID",
            "  boot_device(udid)      Boot a simulator",
            "  start_agent            Connect to externally-started agent",
            "  start_agent(path)      Build and launch agent from project dir",
            "  stop_agent             Stop managed agent process",
            "  set_target(bundle_id)  Set target app for automation",
            "",
            "Screen:",
            "  get_screenshot         Capture a screenshot (base64 PNG)",
            "  get_screen_info        Get UI hierarchy",
            "  start_watcher          Auto-detect screen changes (500ms)",
            "  start_watcher(ms)      Auto-detect with custom interval",
            "  stop_watcher           Stop screen change detection",
            "",
            "UI:",
            "  list_elements          List all UI elements",
            "  tap(selector)          Tap element by ID (waits 5s)",
            "  tap(sel, label)        Tap element by label (waits 5s)",
            "  tap(sel, label, type)  Tap by label + type (waits 5s)",
            "  tap(sel, --no-wait)    Tap without waiting",
            "  swipe()                Swipe up (default)",
            "  swipe(direction)       Swipe: up, down, left, right",
            "  tap_location(x, y)     Tap at screen coordinates",
            "  get_value(selector)    Get element value by ID (waits 5s)",
            "  get_value(sel, label)  Get element value by label (waits 5s)",
            "  get_value(s, --no-wait) Get value without waiting",
            "  wait_for(selector)     Wait for element (5s timeout)",
            "  wait_for(sel, ms)      Wait with custom timeout",
            "  wait_for(sel,ms,label) Wait by label with timeout",
            "",
            "Input:",
            "  send_keys(text)        Send keyboard input",
            "  log_comment(message)   Log a comment to the session",
            "",
            "General:",
            "  help                   Show this help message",
            "  quit                   Exit the REPL",
            "",
        ];

        for line in help_lines {
            self.add_output(Line::from(line.to_string()));
        }
    }
}

/// Validates a simulator UDID format.
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

/// Parse a command string into command name and arguments.
fn parse_command(input: &str) -> (String, Vec<String>) {
    let Some(paren_idx) = input.find('(') else {
        return (input.to_string(), vec![]);
    };

    let cmd = input[..paren_idx].trim().to_string();
    let after_paren = &input[paren_idx + 1..];
    let args_str = find_matching_paren_content(after_paren);
    let args_str = args_str.trim();

    if args_str.is_empty() {
        return (cmd, vec![]);
    }

    let args = split_args(args_str);
    (cmd, args)
}

fn find_matching_paren_content(s: &str) -> &str {
    let mut depth = 1;
    let mut in_double_quote = false;
    let mut in_single_quote = false;
    let mut prev_was_escape = false;

    for (i, c) in s.char_indices() {
        if prev_was_escape {
            prev_was_escape = false;
            continue;
        }

        match c {
            '\\' if in_double_quote || in_single_quote => {
                prev_was_escape = true;
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
            }
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
            }
            '(' if !in_double_quote && !in_single_quote => {
                depth += 1;
            }
            ')' if !in_double_quote && !in_single_quote => {
                depth -= 1;
                if depth == 0 {
                    return &s[..i];
                }
            }
            _ => {}
        }
    }

    s.trim_end_matches(')')
}

fn split_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    let mut in_double_quote = false;
    let mut in_single_quote = false;
    let mut prev_was_escape = false;

    for c in s.chars() {
        if prev_was_escape {
            current.push(c);
            prev_was_escape = false;
            continue;
        }

        match c {
            '\\' if in_double_quote || in_single_quote => {
                prev_was_escape = true;
                current.push(c);
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
                current.push(c);
            }
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
                current.push(c);
            }
            '(' if !in_double_quote && !in_single_quote => {
                depth += 1;
                current.push(c);
            }
            ')' if !in_double_quote && !in_single_quote => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 && !in_double_quote && !in_single_quote => {
                args.push(current.trim().to_string());
                current = String::new();
            }
            _ => {
                current.push(c);
            }
        }
    }

    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        args.push(trimmed);
    }

    args
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
