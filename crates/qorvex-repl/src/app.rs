//! Application state and event handling.

use std::collections::VecDeque;

use ratatui::layout::Rect;
use ratatui::text::Line;
use tokio::sync::mpsc;
use tui_input::Input;

use qorvex_core::action::ActionType;
use qorvex_core::element::UIElement;
use qorvex_core::ipc::{socket_path, IpcClient, IpcRequest, IpcResponse};
use qorvex_core::session::SessionEvent;
use qorvex_core::simctl::SimulatorDevice;

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
    // --- TUI state (kept as-is) ---
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
    /// Whether the app should quit.
    pub should_quit: bool,

    // --- IPC client ---
    /// Session name.
    pub session_name: String,
    /// IPC client connection to qorvex-server.
    client: Option<IpcClient>,

    // --- Completion caches (populated via IPC) ---
    /// Cached UI elements from last screen info.
    pub cached_elements: Vec<UIElement>,
    /// Cached simulator devices.
    pub cached_devices: Vec<SimulatorDevice>,

    // --- Background subscriber channel ---
    /// Channel receiver for element updates from session events.
    element_update_rx: Option<mpsc::Receiver<Vec<UIElement>>>,
}

/// Ensure the qorvex-server process is running for the given session.
///
/// If the socket already exists, assumes the server is running.
/// Otherwise, spawns `qorvex-server` as a detached background process
/// and waits briefly for the socket to appear.
fn ensure_server_running(session_name: &str) {
    let sock = socket_path(session_name);
    if sock.exists() {
        return;
    }
    // Spawn qorvex-server as a detached background process
    let log_dir = dirs::home_dir()
        .expect("Could not determine home directory")
        .join(".qorvex")
        .join("logs");
    std::fs::create_dir_all(&log_dir).ok();
    let log_file = std::fs::File::create(log_dir.join("qorvex-server-launch.log")).ok();

    let mut cmd = std::process::Command::new("qorvex-server");
    cmd.args(["-s", session_name]);
    if let Some(f) = log_file {
        cmd.stdout(f.try_clone().unwrap_or_else(|_| std::fs::File::create("/dev/null").unwrap()));
        cmd.stderr(f);
    }
    let _ = cmd.spawn();

    // Wait briefly for socket to appear
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if sock.exists() {
            break;
        }
    }
}

impl App {
    /// Create a new App instance.
    pub async fn new(session_name: String) -> Self {
        // Ensure server is running
        ensure_server_running(&session_name);

        // Connect IPC client
        let client = match IpcClient::connect(&session_name).await {
            Ok(c) => Some(c),
            Err(_) => None,
        };

        let mut app = Self {
            input: Input::default(),
            completion: CompletionState::default(),
            output_history: VecDeque::new(),
            output_scroll_position: 0,
            selection: SelectionState::default(),
            output_area: None,
            should_quit: false,
            session_name: session_name.clone(),
            client,
            cached_elements: Vec::new(),
            cached_devices: Vec::new(),
            element_update_rx: None,
        };

        // Show connection status
        let sock = socket_path(&session_name);
        if app.client.is_some() {
            app.add_output(Line::from(format!(
                "Connected to server | Session: {} | Socket: {:?}",
                session_name, sock
            )));

            // Send StartSession
            if let Some(ref mut client) = app.client {
                match client.send(&IpcRequest::StartSession).await {
                    Ok(IpcResponse::CommandResult { success, message }) => {
                        app.add_output(format_result(success, &message));
                    }
                    Ok(IpcResponse::Error { message }) => {
                        app.add_output(format_result(false, &message));
                    }
                    Err(e) => {
                        app.add_output(format_result(false, &format!("StartSession error: {}", e)));
                    }
                    _ => {}
                }
            }

            // Fetch initial completion data
            if let Some(ref mut client) = app.client {
                match client.send(&IpcRequest::GetCompletionData).await {
                    Ok(IpcResponse::CompletionData { elements, devices }) => {
                        app.cached_elements = elements;
                        app.cached_devices = devices;
                    }
                    _ => {}
                }
            }
        } else {
            app.add_output(Line::from(format!(
                "Failed to connect to server | Session: {} | Socket: {:?}",
                session_name, sock
            )));
            app.add_output(Line::from("Is qorvex-server running? Try: qorvex-server -s <session>"));
        }

        app.add_output(Line::from("Type 'help' for available commands."));
        app.add_output(Line::from(""));

        // Background subscriber for element updates
        let (tx, rx) = mpsc::channel::<Vec<UIElement>>(16);
        app.element_update_rx = Some(rx);
        let sub_session = session_name.clone();
        tokio::spawn(async move {
            // Keep trying to subscribe
            loop {
                if let Ok(mut sub_client) = IpcClient::connect(&sub_session).await {
                    if sub_client.subscribe().await.is_ok() {
                        loop {
                            match sub_client.read_event().await {
                                Ok(IpcResponse::Event { event: SessionEvent::ScreenInfoUpdated { elements, .. } }) => {
                                    let _ = tx.send((*elements).clone()).await;
                                }
                                Err(_) => break,
                                _ => {}
                            }
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        });

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

    async fn process_command(&mut self, input: &str) {
        let (cmd, args) = parse_command(input);

        // Local-only commands
        match cmd.as_str() {
            "help" => {
                self.show_help();
                return;
            }
            "quit" => {
                // Tell server to end session, then quit
                if let Some(ref mut client) = self.client {
                    let _ = client.send(&IpcRequest::EndSession).await;
                }
                self.should_quit = true;
                return;
            }
            _ => {}
        }

        // Map command to IPC request
        let request = match cmd.as_str() {
            "start_session" => IpcRequest::StartSession,
            "end_session" => IpcRequest::EndSession,
            "list_devices" => IpcRequest::ListDevices,
            "use_device" => IpcRequest::UseDevice {
                udid: args.first().map(|s| strip_quotes(s).to_string()).unwrap_or_default(),
            },
            "boot_device" => IpcRequest::BootDevice {
                udid: args.first().map(|s| strip_quotes(s).to_string()).unwrap_or_default(),
            },
            "start_agent" => IpcRequest::StartAgent {
                project_dir: args.first().map(|s| strip_quotes(s).to_string()),
            },
            "stop_agent" => IpcRequest::StopAgent,
            "set_target" => IpcRequest::SetTarget {
                bundle_id: args.first().map(|s| strip_quotes(s).to_string()).unwrap_or_default(),
            },
            "set_timeout" => {
                let ms_str = args.first().map(|s| s.trim()).unwrap_or("");
                if ms_str.is_empty() {
                    IpcRequest::GetTimeout
                } else {
                    match ms_str.parse::<u64>() {
                        Ok(ms) => IpcRequest::SetTimeout { timeout_ms: ms },
                        Err(_) => {
                            self.add_output(format_result(false, "set_timeout requires a number in milliseconds"));
                            return;
                        }
                    }
                }
            },
            "start_watcher" => IpcRequest::StartWatcher {
                interval_ms: args.first().and_then(|s| s.parse().ok()),
            },
            "stop_watcher" => IpcRequest::StopWatcher,
            "get_session_info" => IpcRequest::GetSessionInfo,
            "get_screenshot" => IpcRequest::Execute {
                action: ActionType::GetScreenshot,
            },
            "list_elements" | "get_screen_info" => IpcRequest::Execute {
                action: ActionType::GetScreenInfo,
            },
            "tap" => {
                let no_wait = args.iter().any(|s| s.trim() == "--no-wait");
                let filtered: Vec<String> = args.iter().filter(|s| s.trim() != "--no-wait").cloned().collect();
                let selector = filtered.first().map(|s| strip_quotes(s).to_string()).unwrap_or_default();
                if selector.is_empty() {
                    self.add_output(format_result(false, "tap requires at least 1 argument: tap(selector)"));
                    return;
                }
                let by_label = filtered.get(1).map(|s| s.trim().to_lowercase() == "label").unwrap_or(false);
                let element_type = filtered.get(2).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                if !no_wait {
                    if let Some(ref mut client) = self.client {
                        let wait_req = IpcRequest::Execute {
                            action: ActionType::WaitFor {
                                selector: selector.clone(),
                                by_label,
                                element_type: element_type.clone(),
                                timeout_ms: 5000,
                                require_stable: false,
                            },
                        };
                        match client.send(&wait_req).await {
                            Ok(IpcResponse::ActionResult { success: false, message, .. }) => {
                                self.add_output(format_result(false, &message));
                                return;
                            }
                            Ok(IpcResponse::Error { message }) => {
                                self.add_output(format_result(false, &message));
                                return;
                            }
                            Err(e) => {
                                self.add_output(format_result(false, &format!("IPC error: {}", e)));
                                return;
                            }
                            _ => {} // success, continue to tap
                        }
                    }
                }
                IpcRequest::Execute {
                    action: ActionType::Tap { selector, by_label, element_type },
                }
            },
            "swipe" => IpcRequest::Execute {
                action: ActionType::Swipe {
                    direction: args.first().map(|s| s.trim().to_lowercase()).unwrap_or_else(|| "up".to_string()),
                },
            },
            "tap_location" => {
                if args.len() < 2 {
                    self.add_output(format_result(false, "tap_location requires 2 arguments: tap_location(x, y)"));
                    return;
                }
                match (args[0].parse::<i32>(), args[1].parse::<i32>()) {
                    (Ok(x), Ok(y)) if x >= 0 && y >= 0 => IpcRequest::Execute {
                        action: ActionType::TapLocation { x, y },
                    },
                    _ => {
                        self.add_output(format_result(false, "Invalid coordinates"));
                        return;
                    }
                }
            },
            "wait_for" => {
                let selector = args.first().map(|s| strip_quotes(s).to_string()).unwrap_or_default();
                if selector.is_empty() {
                    self.add_output(format_result(false, "wait_for requires at least 1 argument"));
                    return;
                }
                let timeout_ms = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5000);
                let by_label = args.get(2).map(|s| s.trim().to_lowercase() == "label").unwrap_or(false);
                let element_type = args.get(3).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                IpcRequest::Execute {
                    action: ActionType::WaitFor { selector, by_label, element_type, timeout_ms, require_stable: true },
                }
            },
            "wait_for_not" => {
                let selector = args.first().map(|s| strip_quotes(s).to_string()).unwrap_or_default();
                if selector.is_empty() {
                    self.add_output(format_result(false, "wait_for_not requires at least 1 argument"));
                    return;
                }
                let timeout_ms = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5000);
                let by_label = args.get(2).map(|s| s.trim().to_lowercase() == "label").unwrap_or(false);
                let element_type = args.get(3).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                IpcRequest::Execute {
                    action: ActionType::WaitForNot { selector, by_label, element_type, timeout_ms },
                }
            },
            "send_keys" => {
                let text = args.join(" ");
                if text.is_empty() {
                    self.add_output(format_result(false, "send_keys requires text"));
                    return;
                }
                IpcRequest::Execute {
                    action: ActionType::SendKeys { text },
                }
            },
            "get_value" => {
                let no_wait = args.iter().any(|s| s.trim() == "--no-wait");
                let filtered: Vec<String> = args.iter().filter(|s| s.trim() != "--no-wait").cloned().collect();
                let selector = filtered.first().map(|s| strip_quotes(s).to_string()).unwrap_or_default();
                if selector.is_empty() {
                    self.add_output(format_result(false, "get_value requires at least 1 argument"));
                    return;
                }
                let by_label = filtered.get(1).map(|s| s.trim().to_lowercase() == "label").unwrap_or(false);
                let element_type = filtered.get(2).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                if !no_wait {
                    if let Some(ref mut client) = self.client {
                        let wait_req = IpcRequest::Execute {
                            action: ActionType::WaitFor {
                                selector: selector.clone(),
                                by_label,
                                element_type: element_type.clone(),
                                timeout_ms: 5000,
                                require_stable: false,
                            },
                        };
                        match client.send(&wait_req).await {
                            Ok(IpcResponse::ActionResult { success: false, message, .. }) => {
                                self.add_output(format_result(false, &message));
                                return;
                            }
                            Ok(IpcResponse::Error { message }) => {
                                self.add_output(format_result(false, &message));
                                return;
                            }
                            Err(e) => {
                                self.add_output(format_result(false, &format!("IPC error: {}", e)));
                                return;
                            }
                            _ => {}
                        }
                    }
                }
                IpcRequest::Execute {
                    action: ActionType::GetValue { selector, by_label, element_type },
                }
            },
            "log_comment" => {
                let message = args.join(" ");
                if message.is_empty() {
                    self.add_output(format_result(false, "log_comment requires a message"));
                    return;
                }
                IpcRequest::Execute {
                    action: ActionType::LogComment { message },
                }
            },
            _ => {
                self.add_output(format_result(false, &format!("Unknown command: {}", cmd)));
                return;
            }
        };

        // Send request and display response
        let Some(ref mut client) = self.client else {
            self.add_output(format_result(false, "Not connected to server"));
            return;
        };

        match client.send(&request).await {
            Ok(response) => self.display_response(&cmd, response),
            Err(e) => self.add_output(format_result(false, &format!("IPC error: {}", e))),
        }
    }

    fn display_response(&mut self, cmd: &str, response: IpcResponse) {
        match response {
            IpcResponse::CommandResult { success, message } => {
                self.add_output(format_result(success, &message));
            }
            IpcResponse::ActionResult { success, message, data, .. } => {
                match cmd {
                    "list_elements" | "get_screen_info" => {
                        if success {
                            if let Some(ref data) = data {
                                if let Ok(elements) = serde_json::from_str::<Vec<UIElement>>(data) {
                                    self.cached_elements = elements.clone();
                                    for elem in &elements {
                                        self.add_output(format_element(elem));
                                    }
                                    self.add_output(format_result(true, &format!("{} elements", elements.len())));
                                    return;
                                }
                            }
                        }
                        self.add_output(format_result(success, &message));
                    }
                    "get_value" => {
                        if success {
                            let value = data.unwrap_or_else(|| "(null)".to_string());
                            self.add_output(format_result(true, &format!("Value: {}", value)));
                        } else {
                            self.add_output(format_result(false, &message));
                        }
                    }
                    "get_screenshot" => {
                        if success {
                            let byte_count = data.as_ref().map(|d| d.len() * 3 / 4).unwrap_or(0);
                            self.add_output(format_result(true, &format!("{} bytes (base64 logged)", byte_count)));
                        } else {
                            self.add_output(format_result(false, &message));
                        }
                    }
                    "wait_for" | "wait_for_not" => {
                        if success {
                            self.add_output(format_result(true, &format!("{} ({})", message, data.unwrap_or_default())));
                        } else {
                            self.add_output(format_result(false, &message));
                        }
                    }
                    _ => {
                        self.add_output(format_result(success, &message));
                    }
                }
            }
            IpcResponse::DeviceList { devices } => {
                self.cached_devices = devices.clone();
                for device in &devices {
                    self.add_output(format_device(device));
                }
                self.add_output(format_result(true, &format!("{} devices", devices.len())));
            }
            IpcResponse::SessionInfo { session_name, active, device_udid, action_count } => {
                if active {
                    self.add_output(Line::from(format!("Session: {} (active)", session_name)));
                    self.add_output(Line::from(format!("Device: {:?}", device_udid)));
                    self.add_output(Line::from(format!("Actions: {}", action_count)));
                } else {
                    self.add_output(Line::from(format!("Session: {} (inactive)", session_name)));
                }
            }
            IpcResponse::TimeoutValue { timeout_ms } => {
                self.add_output(format_result(true, &format!("Current default timeout: {}ms", timeout_ms)));
            }
            IpcResponse::CompletionData { elements, devices } => {
                self.cached_elements = elements;
                self.cached_devices = devices;
            }
            IpcResponse::Error { message } => {
                self.add_output(format_result(false, &message));
            }
            _ => {}
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
            "  set_timeout(ms)        Set default wait timeout (ms)",
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
            "  wait_for_not(selector) Wait for element to disappear (5s)",
            "  wait_for_not(sel, ms)  Wait for disappearance with timeout",
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
