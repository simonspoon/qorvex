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

            let new_value = if matches!(kind, CandidateKind::Command) {
                // Completing a command name — replace entire input
                text
            } else if matches!(kind, CandidateKind::ElementSelectorById | CandidateKind::ElementSelectorByLabel) {
                // Smart selector: replace everything after command name
                if let Some(first_space) = current.find(' ') {
                    format!("{} {}", &current[..first_space], text)
                } else {
                    text
                }
            } else {
                // Completing a single argument or flag: replace last space-separated token
                let quoted = if text.contains(' ') || text.contains('"') || text.contains('\'') {
                    format!("\"{}\"", text.replace('"', "\\\""))
                } else {
                    text
                };
                if let Some(last_space) = current.rfind(' ') {
                    format!("{} {}", &current[..last_space], quoted)
                } else {
                    quoted
                }
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

    pub(crate) async fn process_command(&mut self, input: &str) {
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
            "start-session" => IpcRequest::StartSession,
            "end-session" => IpcRequest::EndSession,
            "list-devices" => IpcRequest::ListDevices,
            "use-device" => IpcRequest::UseDevice {
                udid: args.positional.first().cloned().unwrap_or_default(),
            },
            "boot-device" => IpcRequest::BootDevice {
                udid: args.positional.first().cloned().unwrap_or_default(),
            },
            "start-agent" => IpcRequest::StartAgent {
                project_dir: args.positional.first().cloned(),
            },
            "stop-agent" => IpcRequest::StopAgent,
            "set-target" => IpcRequest::SetTarget {
                bundle_id: args.positional.first().cloned().unwrap_or_default(),
            },
            "set-timeout" => {
                let ms_str = args.positional.first().map(|s| s.as_str()).unwrap_or("");
                if ms_str.is_empty() {
                    IpcRequest::GetTimeout
                } else {
                    match ms_str.parse::<u64>() {
                        Ok(ms) => IpcRequest::SetTimeout { timeout_ms: ms },
                        Err(_) => {
                            self.add_output(format_result(false, "set-timeout requires a number in milliseconds"));
                            return;
                        }
                    }
                }
            },
            "start-watcher" => IpcRequest::StartWatcher {
                interval_ms: args.positional.first().and_then(|s| s.parse().ok()),
            },
            "stop-watcher" => IpcRequest::StopWatcher,
            "get-session-info" => IpcRequest::GetSessionInfo,
            "get-screenshot" => IpcRequest::Execute {
                action: ActionType::GetScreenshot,
            },
            "list-elements" | "get-screen-info" => IpcRequest::Execute {
                action: ActionType::GetScreenInfo,
            },
            "tap" => {
                let selector = args.positional.first().map(|s| s.to_string()).unwrap_or_default();
                if selector.is_empty() {
                    self.add_output(format_result(false, "tap requires a selector: tap <selector>"));
                    return;
                }
                let by_label = args.label;
                let element_type = args.element_type.clone();
                if !args.no_wait {
                    if let Some(ref mut client) = self.client {
                        let wait_req = IpcRequest::Execute {
                            action: ActionType::WaitFor {
                                selector: selector.clone(),
                                by_label,
                                element_type: element_type.clone(),
                                timeout_ms: args.timeout.unwrap_or(5000),
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
                    direction: args.positional.first().map(|s| s.to_lowercase()).unwrap_or_else(|| "up".to_string()),
                },
            },
            "tap-location" => {
                if args.positional.len() < 2 {
                    self.add_output(format_result(false, "tap-location requires 2 arguments: tap-location <x> <y>"));
                    return;
                }
                match (args.positional[0].parse::<i32>(), args.positional[1].parse::<i32>()) {
                    (Ok(x), Ok(y)) if x >= 0 && y >= 0 => IpcRequest::Execute {
                        action: ActionType::TapLocation { x, y },
                    },
                    _ => {
                        self.add_output(format_result(false, "Invalid coordinates"));
                        return;
                    }
                }
            },
            "wait-for" => {
                let selector = args.positional.first().map(|s| s.to_string()).unwrap_or_default();
                if selector.is_empty() {
                    self.add_output(format_result(false, "wait-for requires a selector: wait-for <selector>"));
                    return;
                }
                let timeout_ms = args.timeout.unwrap_or(5000);
                let by_label = args.label;
                let element_type = args.element_type.clone();
                IpcRequest::Execute {
                    action: ActionType::WaitFor { selector, by_label, element_type, timeout_ms, require_stable: true },
                }
            },
            "wait-for-not" => {
                let selector = args.positional.first().map(|s| s.to_string()).unwrap_or_default();
                if selector.is_empty() {
                    self.add_output(format_result(false, "wait-for-not requires a selector: wait-for-not <selector>"));
                    return;
                }
                let timeout_ms = args.timeout.unwrap_or(5000);
                let by_label = args.label;
                let element_type = args.element_type.clone();
                IpcRequest::Execute {
                    action: ActionType::WaitForNot { selector, by_label, element_type, timeout_ms },
                }
            },
            "send-keys" => {
                let text = args.positional.join(" ");
                if text.is_empty() {
                    self.add_output(format_result(false, "send-keys requires text: send-keys <text>"));
                    return;
                }
                IpcRequest::Execute {
                    action: ActionType::SendKeys { text },
                }
            },
            "get-value" => {
                let selector = args.positional.first().map(|s| s.to_string()).unwrap_or_default();
                if selector.is_empty() {
                    self.add_output(format_result(false, "get-value requires a selector: get-value <selector>"));
                    return;
                }
                let by_label = args.label;
                let element_type = args.element_type.clone();
                if !args.no_wait {
                    if let Some(ref mut client) = self.client {
                        let wait_req = IpcRequest::Execute {
                            action: ActionType::WaitFor {
                                selector: selector.clone(),
                                by_label,
                                element_type: element_type.clone(),
                                timeout_ms: args.timeout.unwrap_or(5000),
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
            "log-comment" => {
                let message = args.positional.join(" ");
                if message.is_empty() {
                    self.add_output(format_result(false, "log-comment requires a message: log-comment <message>"));
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
                    "list-elements" | "get-screen-info" => {
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
                    "get-value" => {
                        if success {
                            let value = data.unwrap_or_else(|| "(null)".to_string());
                            self.add_output(format_result(true, &format!("Value: {}", value)));
                        } else {
                            self.add_output(format_result(false, &message));
                        }
                    }
                    "get-screenshot" => {
                        if success {
                            let byte_count = data.as_ref().map(|d| d.len() * 3 / 4).unwrap_or(0);
                            self.add_output(format_result(true, &format!("{} bytes (base64 logged)", byte_count)));
                        } else {
                            self.add_output(format_result(false, &message));
                        }
                    }
                    "wait-for" | "wait-for-not" => {
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
            "  start-session            Start a new session",
            "  end-session              End the current session",
            "  get-session-info         Get current session information",
            "",
            "Device:",
            "  list-devices             List available simulators",
            "  use-device <udid>        Select a simulator by UDID",
            "  boot-device <udid>       Boot a simulator",
            "  start-agent [path]       Connect to / build+launch agent",
            "  stop-agent               Stop managed agent process",
            "  set-target <bundle_id>   Set target app for automation",
            "  set-timeout [ms]         Set/get default wait timeout",
            "",
            "Screen:",
            "  get-screenshot           Capture a screenshot (base64 PNG)",
            "  get-screen-info          Get UI hierarchy",
            "  start-watcher [ms]       Auto-detect screen changes (default 500ms)",
            "  stop-watcher             Stop screen change detection",
            "",
            "UI:",
            "  list-elements            List all UI elements",
            "  tap <sel> [--label] [--type T] [--no-wait] [--timeout ms]",
            "  swipe [direction]        Swipe: up, down, left, right",
            "  tap-location <x> <y>    Tap at screen coordinates",
            "  get-value <sel> [--label] [--type T] [--no-wait]",
            "  wait-for <sel> [--label] [--type T] [--timeout ms]",
            "  wait-for-not <sel> [--label] [--type T] [--timeout ms]",
            "",
            "Input:",
            "  send-keys <text>         Send keyboard input",
            "  log-comment <message>    Log a comment to the session",
            "",
            "General:",
            "  help                     Show this help message",
            "  quit                     Exit the REPL",
            "",
        ];

        for line in help_lines {
            self.add_output(Line::from(line.to_string()));
        }
    }
}

/// Parsed arguments from CLI-style command input.
pub(crate) struct ParsedArgs {
    pub positional: Vec<String>,
    pub label: bool,
    pub no_wait: bool,
    pub timeout: Option<u64>,
    pub element_type: Option<String>,
}

/// Tokenize input using shell-style rules: split on whitespace, respect double quotes.
pub(crate) fn shell_tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut prev_was_escape = false;

    for c in input.chars() {
        if prev_was_escape {
            current.push(c);
            prev_was_escape = false;
            continue;
        }

        match c {
            '\\' if in_quote => {
                prev_was_escape = true;
            }
            '"' => {
                in_quote = !in_quote;
            }
            c if c.is_whitespace() && !in_quote => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => {
                current.push(c);
            }
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

/// Parse a command string into command name and parsed arguments.
pub(crate) fn parse_command(input: &str) -> (String, ParsedArgs) {
    let tokens = shell_tokenize(input);
    let cmd = tokens.first().cloned().unwrap_or_default();

    let mut args = ParsedArgs {
        positional: Vec::new(),
        label: false,
        no_wait: false,
        timeout: None,
        element_type: None,
    };

    let mut iter = tokens.into_iter().skip(1);
    while let Some(tok) = iter.next() {
        match tok.as_str() {
            "--label" => args.label = true,
            "--no-wait" => args.no_wait = true,
            "--timeout" => {
                if let Some(val) = iter.next() {
                    args.timeout = val.parse().ok();
                }
            }
            "--type" => {
                args.element_type = iter.next();
            }
            _ => args.positional.push(tok),
        }
    }

    (cmd, args)
}

/// Strip surrounding quotes from a string if present.
pub(crate) fn strip_quotes(s: &str) -> &str {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_command tests (updated for new CLI-style syntax) ---

    #[test]
    fn test_parse_command_simple() {
        let (cmd, args) = parse_command("help");
        assert_eq!(cmd, "help");
        assert!(args.positional.is_empty());
    }

    #[test]
    fn test_parse_command_single_arg() {
        let (cmd, args) = parse_command("tap button1");
        assert_eq!(cmd, "tap");
        assert_eq!(args.positional, vec!["button1"]);
    }

    #[test]
    fn test_parse_command_multiple_args() {
        let (cmd, args) = parse_command("wait-for btn --timeout 5000 --label");
        assert_eq!(cmd, "wait-for");
        assert_eq!(args.positional, vec!["btn"]);
        assert_eq!(args.timeout, Some(5000));
        assert!(args.label);
    }

    #[test]
    fn test_parse_command_quoted_arg() {
        let (cmd, args) = parse_command(r#"tap "my button""#);
        assert_eq!(cmd, "tap");
        assert_eq!(args.positional, vec!["my button"]);
    }

    #[test]
    fn test_parse_command_whitespace_around_cmd() {
        let (cmd, args) = parse_command("  tap  button1");
        assert_eq!(cmd, "tap");
        assert_eq!(args.positional, vec!["button1"]);
    }

    // --- strip_quotes tests (kept as-is) ---

    #[test]
    fn test_strip_quotes_double() {
        assert_eq!(strip_quotes("\"hello\""), "hello");
    }

    #[test]
    fn test_strip_quotes_single() {
        assert_eq!(strip_quotes("'hello'"), "hello");
    }

    #[test]
    fn test_strip_quotes_none() {
        assert_eq!(strip_quotes("plain"), "plain");
    }

    #[test]
    fn test_strip_quotes_with_spaces() {
        assert_eq!(strip_quotes("  \"hello\"  "), "hello");
    }

    // --- New flag / argument tests ---

    #[test]
    fn test_parse_command_label_flag() {
        let (cmd, args) = parse_command("tap \"Sign In\" --label");
        assert_eq!(cmd, "tap");
        assert_eq!(args.positional, vec!["Sign In"]);
        assert!(args.label);
        assert!(!args.no_wait);
    }

    #[test]
    fn test_parse_command_all_flags() {
        let (cmd, args) = parse_command("tap \"Sign In\" --label --type Button --no-wait --timeout 3000");
        assert_eq!(cmd, "tap");
        assert_eq!(args.positional, vec!["Sign In"]);
        assert!(args.label);
        assert!(args.no_wait);
        assert_eq!(args.timeout, Some(3000));
        assert_eq!(args.element_type, Some("Button".to_string()));
    }

    #[test]
    fn test_parse_command_no_flags() {
        let (cmd, args) = parse_command("tap button1");
        assert_eq!(cmd, "tap");
        assert_eq!(args.positional, vec!["button1"]);
        assert!(!args.label);
        assert!(!args.no_wait);
        assert_eq!(args.timeout, None);
        assert_eq!(args.element_type, None);
    }

    #[test]
    fn test_parse_command_timeout_flag() {
        let (cmd, args) = parse_command("wait-for element --timeout 10000");
        assert_eq!(cmd, "wait-for");
        assert_eq!(args.positional, vec!["element"]);
        assert_eq!(args.timeout, Some(10000));
    }

    // --- shell_tokenize tests ---

    #[test]
    fn test_shell_tokenize_basic() {
        let tokens = shell_tokenize("tap button1");
        assert_eq!(tokens, vec!["tap", "button1"]);
    }

    #[test]
    fn test_shell_tokenize_quoted() {
        let tokens = shell_tokenize(r#"tap "my button" --label"#);
        assert_eq!(tokens, vec!["tap", "my button", "--label"]);
    }

    #[test]
    fn test_shell_tokenize_empty() {
        let tokens = shell_tokenize("");
        assert!(tokens.is_empty());
    }

    #[test]
    fn test_shell_tokenize_extra_spaces() {
        let tokens = shell_tokenize("  tap   button1  ");
        assert_eq!(tokens, vec!["tap", "button1"]);
    }

    #[test]
    fn test_parse_command_send_keys_multiple_words() {
        let (cmd, args) = parse_command("send-keys hello world");
        assert_eq!(cmd, "send-keys");
        assert_eq!(args.positional, vec!["hello", "world"]);
    }
}
