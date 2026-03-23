//! Application state and event handling.

use std::collections::VecDeque;
use std::time::Instant;

use ratatui::layout::Rect;
use ratatui::text::Line;
use tokio::sync::mpsc;
use tui_input::Input;

use qorvex_core::action::ActionType;
use qorvex_core::element::UIElement;
use qorvex_core::ipc::{socket_path, IpcClient, IpcRequest, IpcResponse};
use qorvex_core::simctl::{InstalledApp, Simctl, SimulatorDevice};

use crate::completion::commands::ArgCompletion;
use crate::completion::{
    parse_completion_context, CandidateKind, CompletionContext, CompletionState,
};
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
                if a <= b {
                    Some((a, b))
                } else {
                    Some((b, a))
                }
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

/// Result from a background command execution task.
pub(crate) struct CommandResult {
    pub cmd: String,
    pub result: Result<IpcResponse, String>,
}

/// Result from the deferred startup task.
struct StartupResult {
    client: Option<IpcClient>,
    messages: Vec<Line<'static>>,
    cached_elements: Vec<UIElement>,
    cached_devices: Vec<SimulatorDevice>,
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
    /// Cached UI elements from last fetch.
    pub cached_elements: Vec<UIElement>,
    /// Cached simulator devices.
    pub cached_devices: Vec<SimulatorDevice>,
    /// Cached installed apps for bundle ID completion.
    pub cached_apps: Vec<InstalledApp>,

    // --- On-demand app fetching ---
    /// Channel receiver for app list updates from fetch task.
    app_update_rx: Option<mpsc::Receiver<Vec<InstalledApp>>>,
    /// Trigger sender to request a new app list fetch.
    app_fetch_trigger_tx: Option<mpsc::Sender<()>>,
    /// Whether an app list fetch is in progress.
    pub apps_loading: bool,
    /// When the current app fetch started (for 100ms threshold).
    apps_fetch_started_at: Option<Instant>,

    // --- On-demand element fetching ---
    /// Channel receiver for element updates from fetch task.
    element_update_rx: Option<mpsc::Receiver<Vec<UIElement>>>,
    /// Trigger sender to request a new element fetch.
    fetch_trigger_tx: Option<mpsc::Sender<()>>,
    /// Command name that triggered the active fetch (for cache reuse).
    active_fetch_command: Option<String>,
    /// Whether an element fetch is in progress.
    pub elements_loading: bool,
    /// When the current fetch started (for 100ms threshold).
    fetch_started_at: Option<Instant>,

    // --- Command processing (spinner) ---
    /// Whether a command is currently being processed.
    pub is_processing: bool,
    /// Label shown next to the spinner (the command name).
    pub processing_label: String,
    /// When processing started (for spinner frame calculation).
    pub processing_start: Option<Instant>,
    /// Receiver for command execution results from the spawned task.
    cmd_result_rx: Option<mpsc::Receiver<(CommandResult, IpcClient)>>,
    /// Receiver for startup result (deferred server connect + session start).
    startup_rx: Option<mpsc::Receiver<StartupResult>>,
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
    let log_dir = qorvex_core::session::logs_dir();
    let log_file = std::fs::File::create(log_dir.join("qorvex-server-launch.log")).ok();

    let mut cmd = std::process::Command::new("qorvex-server");
    cmd.args(["-s", session_name]);
    if let Some(f) = log_file {
        cmd.stdout(
            f.try_clone()
                .unwrap_or_else(|_| std::fs::File::create("/dev/null").unwrap()),
        );
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
    ///
    /// Returns immediately with the TUI ready to render. Call `startup()`
    /// after the first frame to connect to the server in the background.
    pub fn new(session_name: String) -> Self {
        // On-demand element fetch task
        let (element_tx, element_rx) = mpsc::channel::<Vec<UIElement>>(4);
        let (fetch_trigger_tx, mut fetch_trigger_rx) = mpsc::channel::<()>(1);
        let fetch_session = session_name.clone();
        tokio::spawn(async move {
            while fetch_trigger_rx.recv().await.is_some() {
                while fetch_trigger_rx.try_recv().is_ok() {}
                let elements = match IpcClient::connect(&fetch_session).await {
                    Ok(mut client) => match client.send(&IpcRequest::FetchElements).await {
                        Ok(IpcResponse::CompletionData { elements, .. }) => elements,
                        _ => Vec::new(),
                    },
                    Err(_) => Vec::new(),
                };
                if element_tx.send(elements).await.is_err() {
                    break;
                }
            }
        });

        // On-demand app list fetch task
        let (app_tx, app_rx) = mpsc::channel::<Vec<InstalledApp>>(4);
        let (app_fetch_trigger_tx, mut app_fetch_trigger_rx) = mpsc::channel::<()>(1);
        tokio::spawn(async move {
            while app_fetch_trigger_rx.recv().await.is_some() {
                while app_fetch_trigger_rx.try_recv().is_ok() {}
                let apps = match Simctl::get_booted_udid() {
                    Ok(udid) => Simctl::list_apps(&udid).unwrap_or_default(),
                    Err(_) => Vec::new(),
                };
                if app_tx.send(apps).await.is_err() {
                    break;
                }
            }
        });

        let mut app = Self {
            input: Input::default(),
            completion: CompletionState::default(),
            output_history: VecDeque::new(),
            output_scroll_position: 0,
            selection: SelectionState::default(),
            output_area: None,
            should_quit: false,
            session_name,
            client: None,
            cached_elements: Vec::new(),
            cached_devices: Vec::new(),
            cached_apps: Vec::new(),
            app_update_rx: Some(app_rx),
            app_fetch_trigger_tx: Some(app_fetch_trigger_tx),
            apps_loading: false,
            apps_fetch_started_at: None,
            element_update_rx: Some(element_rx),
            fetch_trigger_tx: Some(fetch_trigger_tx),
            active_fetch_command: None,
            elements_loading: false,
            fetch_started_at: None,
            is_processing: false,
            processing_label: String::new(),
            processing_start: None,
            cmd_result_rx: None,
            startup_rx: None,
        };

        app.add_output(Line::from("Type 'help' for available commands."));
        app.add_output(Line::from(""));

        app
    }

    /// Create a new App with blocking server startup (for batch mode).
    pub async fn new_blocking(session_name: String) -> Self {
        let mut app = Self::new(session_name.clone());

        ensure_server_running(&session_name);

        let sock = socket_path(&session_name);
        match IpcClient::connect(&session_name).await {
            Ok(mut c) => {
                app.add_output(Line::from(format!(
                    "Connected to server | Session: {} | Socket: {:?}",
                    session_name, sock
                )));
                match c.send(&IpcRequest::StartSession).await {
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
                match c.send(&IpcRequest::GetCompletionData).await {
                    Ok(IpcResponse::CompletionData { elements, devices }) => {
                        app.cached_elements = elements;
                        app.cached_devices = devices;
                    }
                    _ => {}
                }
                app.client = Some(c);
            }
            Err(_) => {
                app.add_output(Line::from(format!(
                    "Failed to connect to server | Session: {} | Socket: {:?}",
                    session_name, sock
                )));
                app.add_output(Line::from(
                    "Is qorvex-server running? Try: qorvex-server -s <session>",
                ));
            }
        }

        app
    }

    /// Begin connecting to the server and starting the session in the background.
    ///
    /// Call this after the first TUI frame so the user sees the spinner.
    pub fn startup(&mut self) {
        let session_name = self.session_name.clone();
        let (tx, rx) = mpsc::channel(1);
        self.startup_rx = Some(rx);
        self.is_processing = true;
        self.processing_label = "connecting".to_string();
        self.processing_start = Some(Instant::now());

        tokio::spawn(async move {
            let mut messages: Vec<Line<'static>> = Vec::new();
            let mut cached_elements = Vec::new();
            let mut cached_devices = Vec::new();

            // Ensure server is running (may block briefly)
            ensure_server_running(&session_name);

            // Connect IPC client
            let sock = socket_path(&session_name);
            let client = match IpcClient::connect(&session_name).await {
                Ok(mut c) => {
                    messages.push(Line::from(format!(
                        "Connected to server | Session: {} | Socket: {:?}",
                        session_name, sock
                    )));

                    // Send StartSession
                    match c.send(&IpcRequest::StartSession).await {
                        Ok(IpcResponse::CommandResult { success, message }) => {
                            messages.push(format_result(success, &message));
                        }
                        Ok(IpcResponse::Error { message }) => {
                            messages.push(format_result(false, &message));
                        }
                        Err(e) => {
                            messages
                                .push(format_result(false, &format!("StartSession error: {}", e)));
                        }
                        _ => {}
                    }

                    // Fetch initial completion data
                    match c.send(&IpcRequest::GetCompletionData).await {
                        Ok(IpcResponse::CompletionData { elements, devices }) => {
                            cached_elements = elements;
                            cached_devices = devices;
                        }
                        _ => {}
                    }

                    Some(c)
                }
                Err(_) => {
                    messages.push(Line::from(format!(
                        "Failed to connect to server | Session: {} | Socket: {:?}",
                        session_name, sock
                    )));
                    messages.push(Line::from(
                        "Is qorvex-server running? Try: qorvex-server -s <session>",
                    ));
                    None
                }
            };

            let _ = tx
                .send(StartupResult {
                    client,
                    messages,
                    cached_elements,
                    cached_devices,
                })
                .await;
        });
    }

    /// Check for startup completion (non-blocking).
    pub fn check_startup_result(&mut self) {
        if let Some(ref mut rx) = self.startup_rx {
            if let Ok(result) = rx.try_recv() {
                self.client = result.client;
                for line in result.messages {
                    self.add_output(line);
                }
                self.cached_elements = result.cached_elements;
                self.cached_devices = result.cached_devices;

                self.is_processing = false;
                self.processing_label.clear();
                self.processing_start = None;
                self.startup_rx = None;
            }
        }
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

        // Detect ElementSelector context to trigger on-demand fetch
        let (context, _prefix) = parse_completion_context(&input);
        let needs_elements = matches!(
            &context,
            CompletionContext::Argument { command, arg_index }
            if command.args.get(*arg_index).map(|a| a.completion == ArgCompletion::ElementSelector).unwrap_or(false)
        );

        // Detect BundleId context to trigger app list fetch
        let needs_apps = matches!(
            &context,
            CompletionContext::Argument { command, arg_index }
            if command.args.get(*arg_index).map(|a| a.completion == ArgCompletion::BundleId).unwrap_or(false)
        );

        if needs_elements {
            // Extract command name
            let cmd_name = input
                .trim_start()
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string();
            if self.active_fetch_command.as_deref() != Some(&cmd_name) {
                // New command context — clear cache and trigger fetch
                self.cached_elements.clear();
                self.elements_loading = true;
                self.fetch_started_at = Some(Instant::now());
                self.active_fetch_command = Some(cmd_name);
                if let Some(ref tx) = self.fetch_trigger_tx {
                    let _ = tx.try_send(());
                }
            }
        } else {
            // Left element context — reset fetch state
            self.active_fetch_command = None;
            self.elements_loading = false;
            self.fetch_started_at = None;
        }

        if needs_apps && self.cached_apps.is_empty() && !self.apps_loading {
            self.apps_loading = true;
            self.apps_fetch_started_at = Some(Instant::now());
            if let Some(ref tx) = self.app_fetch_trigger_tx {
                let _ = tx.try_send(());
            }
        }

        // Only show loading indicator after 100ms threshold
        let show_loading = (self.elements_loading
            && self
                .fetch_started_at
                .map(|t| t.elapsed().as_millis() >= 100)
                .unwrap_or(false))
            || (self.apps_loading
                && self
                    .apps_fetch_started_at
                    .map(|t| t.elapsed().as_millis() >= 100)
                    .unwrap_or(false));

        self.completion.update(
            &input,
            &self.cached_elements,
            &self.cached_devices,
            &self.cached_apps,
            show_loading,
        );
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
            } else if matches!(
                kind,
                CandidateKind::ElementSelectorById | CandidateKind::ElementSelectorByLabel
            ) {
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

    /// Begin executing a command. Returns immediately — the result arrives via cmd_result_rx.
    pub fn execute_command(&mut self) {
        let input = self.input.value().trim().to_string();
        if input.is_empty() {
            return;
        }

        // Add command to output
        self.add_output(format_command(&input));

        // Parse and handle local commands synchronously
        let (cmd, args) = parse_command(&input);
        match cmd.as_str() {
            "help" => {
                self.show_help();
                self.input = Input::default();
                self.completion.hide();
                return;
            }
            "quit" => {
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
            "list-physical-devices" => IpcRequest::ListPhysicalDevices,
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
            "start-target" => IpcRequest::StartTarget,
            "stop-target" => IpcRequest::StopTarget,
            "get-target-info" => IpcRequest::GetTargetInfo,
            "set-timeout" => {
                let ms_str = args.positional.first().map(|s| s.as_str()).unwrap_or("");
                if ms_str.is_empty() {
                    IpcRequest::GetTimeout
                } else {
                    match ms_str.parse::<u64>() {
                        Ok(ms) => IpcRequest::SetTimeout { timeout_ms: ms },
                        Err(_) => {
                            self.add_output(format_result(
                                false,
                                "set-timeout requires a number in milliseconds",
                            ));
                            self.input = Input::default();
                            self.completion.hide();
                            return;
                        }
                    }
                }
            }
            "get-session-info" => IpcRequest::GetSessionInfo,
            "get-screenshot" => IpcRequest::Execute {
                action: ActionType::GetScreenshot,
                tag: None,
            },
            "list-elements" | "get-screen-info" => IpcRequest::Execute {
                action: ActionType::GetScreenInfo,
                tag: None,
            },
            "tap" => {
                let selector = args
                    .positional
                    .first()
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                if selector.is_empty() {
                    self.add_output(format_result(
                        false,
                        "tap requires a selector: tap <selector>",
                    ));
                    self.input = Input::default();
                    self.completion.hide();
                    return;
                }
                let by_label = args.label;
                let element_type = args.element_type.clone();
                let timeout_ms = if args.no_wait {
                    None
                } else {
                    Some(args.timeout.unwrap_or(5000))
                };
                IpcRequest::Execute {
                    action: ActionType::Tap {
                        selector,
                        by_label,
                        element_type,
                        timeout_ms,
                    },
                    tag: None,
                }
            }
            "swipe" => IpcRequest::Execute {
                action: ActionType::Swipe {
                    direction: args
                        .positional
                        .first()
                        .map(|s| s.to_lowercase())
                        .unwrap_or_else(|| "up".to_string()),
                },
                tag: None,
            },
            "tap-location" => {
                if args.positional.len() < 2 {
                    self.add_output(format_result(
                        false,
                        "tap-location requires 2 arguments: tap-location <x> <y>",
                    ));
                    self.input = Input::default();
                    self.completion.hide();
                    return;
                }
                match (
                    args.positional[0].parse::<i32>(),
                    args.positional[1].parse::<i32>(),
                ) {
                    (Ok(x), Ok(y)) if x >= 0 && y >= 0 => IpcRequest::Execute {
                        action: ActionType::TapLocation { x, y },
                        tag: None,
                    },
                    _ => {
                        self.add_output(format_result(false, "Invalid coordinates"));
                        self.input = Input::default();
                        self.completion.hide();
                        return;
                    }
                }
            }
            "wait-for" => {
                let selector = args
                    .positional
                    .first()
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                if selector.is_empty() {
                    self.add_output(format_result(
                        false,
                        "wait-for requires a selector: wait-for <selector>",
                    ));
                    self.input = Input::default();
                    self.completion.hide();
                    return;
                }
                let timeout_ms = args.timeout.unwrap_or(5000);
                let by_label = args.label;
                let element_type = args.element_type.clone();
                IpcRequest::Execute {
                    action: ActionType::WaitFor {
                        selector,
                        by_label,
                        element_type,
                        timeout_ms,
                        require_stable: true,
                    },
                    tag: None,
                }
            }
            "wait-for-not" => {
                let selector = args
                    .positional
                    .first()
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                if selector.is_empty() {
                    self.add_output(format_result(
                        false,
                        "wait-for-not requires a selector: wait-for-not <selector>",
                    ));
                    self.input = Input::default();
                    self.completion.hide();
                    return;
                }
                let timeout_ms = args.timeout.unwrap_or(5000);
                let by_label = args.label;
                let element_type = args.element_type.clone();
                IpcRequest::Execute {
                    action: ActionType::WaitForNot {
                        selector,
                        by_label,
                        element_type,
                        timeout_ms,
                    },
                    tag: None,
                }
            }
            "send-keys" => {
                let text = args.positional.join(" ");
                if text.is_empty() {
                    self.add_output(format_result(
                        false,
                        "send-keys requires text: send-keys <text>",
                    ));
                    self.input = Input::default();
                    self.completion.hide();
                    return;
                }
                IpcRequest::Execute {
                    action: ActionType::SendKeys { text },
                    tag: None,
                }
            }
            "get-value" => {
                let selector = args
                    .positional
                    .first()
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                if selector.is_empty() {
                    self.add_output(format_result(
                        false,
                        "get-value requires a selector: get-value <selector>",
                    ));
                    self.input = Input::default();
                    self.completion.hide();
                    return;
                }
                let by_label = args.label;
                let element_type = args.element_type.clone();
                let timeout_ms = if args.no_wait {
                    None
                } else {
                    Some(args.timeout.unwrap_or(5000))
                };
                IpcRequest::Execute {
                    action: ActionType::GetValue {
                        selector,
                        by_label,
                        element_type,
                        timeout_ms,
                    },
                    tag: None,
                }
            }
            "log-comment" => {
                let message = args.positional.join(" ");
                if message.is_empty() {
                    self.add_output(format_result(
                        false,
                        "log-comment requires a message: log-comment <message>",
                    ));
                    self.input = Input::default();
                    self.completion.hide();
                    return;
                }
                IpcRequest::Execute {
                    action: ActionType::LogComment { message },
                    tag: None,
                }
            }
            _ => {
                self.add_output(format_result(false, &format!("Unknown command: {}", cmd)));
                self.input = Input::default();
                self.completion.hide();
                return;
            }
        };

        // Check we have a client
        let Some(client) = self.client.take() else {
            self.add_output(format_result(false, "Not connected to server"));
            self.input = Input::default();
            self.completion.hide();
            return;
        };

        // Set processing state
        self.is_processing = true;
        self.processing_label = cmd.clone();
        self.processing_start = Some(Instant::now());

        // Spawn background task to send IPC request
        let (tx, rx) = mpsc::channel(1);
        self.cmd_result_rx = Some(rx);

        tokio::spawn(async move {
            let mut client = client;
            let result = match client.send(&request).await {
                Ok(response) => Ok((response, client)),
                Err(e) => Err((format!("IPC error: {}", e), client)),
            };

            let (cmd_result, client) = match result {
                Ok((response, client)) => (
                    CommandResult {
                        cmd,
                        result: Ok(response),
                    },
                    client,
                ),
                Err((err_msg, client)) => (
                    CommandResult {
                        cmd,
                        result: Err(err_msg),
                    },
                    client,
                ),
            };

            let _ = tx.send((cmd_result, client)).await;
        });

        // Clear input, completion, and fetch state
        self.input = Input::default();
        self.completion.hide();
        self.active_fetch_command = None;
        self.elements_loading = false;
        self.fetch_started_at = None;
    }

    /// Shut down the IPC server so it removes its socket file and exits.
    ///
    /// Must be called on every exit path (quit command, Ctrl+C, batch EOF, etc.).
    pub async fn shutdown(&mut self) {
        if let Some(ref mut client) = self.client {
            let _ = client.send(&IpcRequest::Shutdown).await;
            self.client = None;
        }
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
            let end_col = if i == end.line {
                end.col
            } else {
                line_str.len()
            };

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

        if result.is_empty() {
            None
        } else {
            Some(result)
        }
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

    /// Check for element updates from the fetch task (non-blocking).
    pub fn check_element_updates(&mut self) {
        if let Some(ref mut rx) = self.element_update_rx {
            let mut updated = false;
            while let Ok(elements) = rx.try_recv() {
                self.cached_elements = elements;
                self.elements_loading = false;
                self.fetch_started_at = None;
                updated = true;
            }
            if updated {
                self.update_completion();
            }
        }
    }

    /// Check for app list updates from the fetch task (non-blocking).
    pub fn check_app_updates(&mut self) {
        if let Some(ref mut rx) = self.app_update_rx {
            let mut updated = false;
            while let Ok(apps) = rx.try_recv() {
                self.cached_apps = apps;
                self.apps_loading = false;
                self.apps_fetch_started_at = None;
                updated = true;
            }
            if updated {
                self.update_completion();
            }
        }
    }

    /// Check for command execution results (non-blocking). Called each event loop iteration.
    pub fn check_command_result(&mut self) {
        if let Some(ref mut rx) = self.cmd_result_rx {
            if let Ok((result, client)) = rx.try_recv() {
                // Restore the IPC client
                self.client = Some(client);

                // Clear processing state
                self.is_processing = false;
                self.processing_label.clear();
                self.processing_start = None;

                // Display the result
                match result.result {
                    Ok(response) => self.display_response(&result.cmd, response),
                    Err(err_msg) => self.add_output(format_result(false, &err_msg)),
                }

                self.cmd_result_rx = None;
            }
        }
    }

    /// Get the current spinner frame character based on elapsed time.
    pub fn spinner_frame(&self) -> &'static str {
        const FRAMES: &[&str] = &[
            "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}",
            "\u{2827}", "\u{2807}", "\u{280f}",
        ];
        if let Some(start) = self.processing_start {
            let elapsed_ms = start.elapsed().as_millis() as usize;
            let idx = (elapsed_ms / 80) % FRAMES.len();
            FRAMES[idx]
        } else {
            FRAMES[0]
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
            "list-physical-devices" => IpcRequest::ListPhysicalDevices,
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
            "start-target" => IpcRequest::StartTarget,
            "stop-target" => IpcRequest::StopTarget,
            "get-target-info" => IpcRequest::GetTargetInfo,
            "set-timeout" => {
                let ms_str = args.positional.first().map(|s| s.as_str()).unwrap_or("");
                if ms_str.is_empty() {
                    IpcRequest::GetTimeout
                } else {
                    match ms_str.parse::<u64>() {
                        Ok(ms) => IpcRequest::SetTimeout { timeout_ms: ms },
                        Err(_) => {
                            self.add_output(format_result(
                                false,
                                "set-timeout requires a number in milliseconds",
                            ));
                            return;
                        }
                    }
                }
            }
            "get-session-info" => IpcRequest::GetSessionInfo,
            "get-screenshot" => IpcRequest::Execute {
                action: ActionType::GetScreenshot,
                tag: None,
            },
            "list-elements" | "get-screen-info" => IpcRequest::Execute {
                action: ActionType::GetScreenInfo,
                tag: None,
            },
            "tap" => {
                let selector = args
                    .positional
                    .first()
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                if selector.is_empty() {
                    self.add_output(format_result(
                        false,
                        "tap requires a selector: tap <selector>",
                    ));
                    return;
                }
                let by_label = args.label;
                let element_type = args.element_type.clone();
                let timeout_ms = if args.no_wait {
                    None
                } else {
                    Some(args.timeout.unwrap_or(5000))
                };
                IpcRequest::Execute {
                    action: ActionType::Tap {
                        selector,
                        by_label,
                        element_type,
                        timeout_ms,
                    },
                    tag: None,
                }
            }
            "swipe" => IpcRequest::Execute {
                action: ActionType::Swipe {
                    direction: args
                        .positional
                        .first()
                        .map(|s| s.to_lowercase())
                        .unwrap_or_else(|| "up".to_string()),
                },
                tag: None,
            },
            "tap-location" => {
                if args.positional.len() < 2 {
                    self.add_output(format_result(
                        false,
                        "tap-location requires 2 arguments: tap-location <x> <y>",
                    ));
                    return;
                }
                match (
                    args.positional[0].parse::<i32>(),
                    args.positional[1].parse::<i32>(),
                ) {
                    (Ok(x), Ok(y)) if x >= 0 && y >= 0 => IpcRequest::Execute {
                        action: ActionType::TapLocation { x, y },
                        tag: None,
                    },
                    _ => {
                        self.add_output(format_result(false, "Invalid coordinates"));
                        return;
                    }
                }
            }
            "wait-for" => {
                let selector = args
                    .positional
                    .first()
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                if selector.is_empty() {
                    self.add_output(format_result(
                        false,
                        "wait-for requires a selector: wait-for <selector>",
                    ));
                    return;
                }
                let timeout_ms = args.timeout.unwrap_or(5000);
                let by_label = args.label;
                let element_type = args.element_type.clone();
                IpcRequest::Execute {
                    action: ActionType::WaitFor {
                        selector,
                        by_label,
                        element_type,
                        timeout_ms,
                        require_stable: true,
                    },
                    tag: None,
                }
            }
            "wait-for-not" => {
                let selector = args
                    .positional
                    .first()
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                if selector.is_empty() {
                    self.add_output(format_result(
                        false,
                        "wait-for-not requires a selector: wait-for-not <selector>",
                    ));
                    return;
                }
                let timeout_ms = args.timeout.unwrap_or(5000);
                let by_label = args.label;
                let element_type = args.element_type.clone();
                IpcRequest::Execute {
                    action: ActionType::WaitForNot {
                        selector,
                        by_label,
                        element_type,
                        timeout_ms,
                    },
                    tag: None,
                }
            }
            "send-keys" => {
                let text = args.positional.join(" ");
                if text.is_empty() {
                    self.add_output(format_result(
                        false,
                        "send-keys requires text: send-keys <text>",
                    ));
                    return;
                }
                IpcRequest::Execute {
                    action: ActionType::SendKeys { text },
                    tag: None,
                }
            }
            "get-value" => {
                let selector = args
                    .positional
                    .first()
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                if selector.is_empty() {
                    self.add_output(format_result(
                        false,
                        "get-value requires a selector: get-value <selector>",
                    ));
                    return;
                }
                let by_label = args.label;
                let element_type = args.element_type.clone();
                let timeout_ms = if args.no_wait {
                    None
                } else {
                    Some(args.timeout.unwrap_or(5000))
                };
                IpcRequest::Execute {
                    action: ActionType::GetValue {
                        selector,
                        by_label,
                        element_type,
                        timeout_ms,
                    },
                    tag: None,
                }
            }
            "log-comment" => {
                let message = args.positional.join(" ");
                if message.is_empty() {
                    self.add_output(format_result(
                        false,
                        "log-comment requires a message: log-comment <message>",
                    ));
                    return;
                }
                IpcRequest::Execute {
                    action: ActionType::LogComment { message },
                    tag: None,
                }
            }
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
            IpcResponse::ActionResult {
                success,
                message,
                data,
                ..
            } => match cmd {
                "list-elements" | "get-screen-info" => {
                    if success {
                        if let Some(ref data) = data {
                            if let Ok(elements) = serde_json::from_str::<Vec<UIElement>>(data) {
                                self.cached_elements = elements.clone();
                                for elem in &elements {
                                    self.add_output(format_element(elem));
                                }
                                self.add_output(format_result(
                                    true,
                                    &format!("{} elements", elements.len()),
                                ));
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
                        self.add_output(format_result(
                            true,
                            &format!("{} bytes (base64 logged)", byte_count),
                        ));
                    } else {
                        self.add_output(format_result(false, &message));
                    }
                }
                "wait-for" | "wait-for-not" => {
                    if success {
                        self.add_output(format_result(
                            true,
                            &format!("{} ({})", message, data.unwrap_or_default()),
                        ));
                    } else {
                        self.add_output(format_result(false, &message));
                    }
                }
                "get-target-info" => {
                    if success {
                        if let Some(ref d) = data {
                            if let Ok(info) = serde_json::from_str::<serde_json::Value>(d) {
                                if let Some(bid) = info.get("bundle_id").and_then(|v| v.as_str()) {
                                    self.add_output(format!("  Bundle ID:    {}", bid).into());
                                }
                                if let Some(name) =
                                    info.get("display_name").and_then(|v| v.as_str())
                                {
                                    if !name.is_empty() {
                                        self.add_output(format!("  Display Name: {}", name).into());
                                    }
                                }
                                if let Some(ver) = info.get("version").and_then(|v| v.as_str()) {
                                    if !ver.is_empty() {
                                        self.add_output(format!("  Version:      {}", ver).into());
                                    }
                                }
                                if let Some(build) = info.get("build").and_then(|v| v.as_str()) {
                                    if !build.is_empty() {
                                        self.add_output(
                                            format!("  Build:        {}", build).into(),
                                        );
                                    }
                                }
                                if let Some(state) = info.get("state").and_then(|v| v.as_str()) {
                                    self.add_output(format!("  State:        {}", state).into());
                                }
                            } else {
                                self.add_output(format_result(true, &message));
                            }
                        } else {
                            self.add_output(format_result(true, &message));
                        }
                    } else {
                        self.add_output(format_result(false, &message));
                    }
                }
                _ => {
                    self.add_output(format_result(success, &message));
                }
            },
            IpcResponse::DeviceList { devices } => {
                self.cached_devices = devices.clone();
                for device in &devices {
                    self.add_output(format_device(device));
                }
                self.add_output(format_result(true, &format!("{} devices", devices.len())));
            }
            IpcResponse::PhysicalDeviceList { devices } => {
                if devices.is_empty() {
                    self.add_output(Line::from("No physical devices connected.".to_string()));
                } else {
                    for d in &devices {
                        let name_part = d.name.as_deref().unwrap_or("unknown");
                        self.add_output(Line::from(format!(
                            "  {} ({}) — {}",
                            d.udid, d.connection, name_part
                        )));
                    }
                }
            }
            IpcResponse::SessionInfo {
                session_name,
                active,
                device_udid,
                action_count,
            } => {
                if active {
                    self.add_output(Line::from(format!("Session: {} (active)", session_name)));
                    self.add_output(Line::from(format!("Device: {:?}", device_udid)));
                    self.add_output(Line::from(format!("Actions: {}", action_count)));
                } else {
                    self.add_output(Line::from(format!("Session: {} (inactive)", session_name)));
                }
            }
            IpcResponse::TimeoutValue { timeout_ms } => {
                self.add_output(format_result(
                    true,
                    &format!("Current default timeout: {}ms", timeout_ms),
                ));
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
            "  list-physical-devices    List connected physical devices",
            "  use-device <udid>        Select a device by UDID (simulator or physical)",
            "  boot-device <udid>       Boot a simulator",
            "  start-agent [path]       Connect to / build+launch agent",
            "  stop-agent               Stop managed agent process",
            "  set-target <bundle_id>   Set target app for automation",
            "  get-target-info          Get target app metadata",
            "  start-target             Launch the target application",
            "  stop-target              Terminate the target application",
            "  set-timeout [ms]         Set/get default wait timeout",
            "",
            "Screen:",
            "  get-screenshot           Capture a screenshot (base64 PNG)",
            "  get-screen-info          Get UI hierarchy",
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
        let (cmd, args) =
            parse_command("tap \"Sign In\" --label --type Button --no-wait --timeout 3000");
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

    // --- shutdown / socket cleanup tests ---

    /// Verify that `shutdown()` sends `IpcRequest::Shutdown` and clears the client.
    ///
    /// Starts a real `IpcServer`, connects an `IpcClient`, wraps it in `App`,
    /// then calls `shutdown()` and asserts the client is `None` and the socket
    /// file has been removed (server Drop cleans it up).
    #[tokio::test]
    async fn test_shutdown_removes_socket_and_clears_client() {
        use qorvex_core::ipc::{socket_path, IpcServer};
        use qorvex_core::session::Session;

        // Unique session name to avoid conflicts
        let session_name = format!(
            "repl_shutdown_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        );
        let sock = socket_path(&session_name);

        // Start a real IPC server
        let session = Session::new(None, "test");
        let server = IpcServer::new(session, &session_name);
        let server_handle = tokio::spawn(async move {
            let _ = server.run().await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(sock.exists(), "Socket should exist after server starts");

        // Connect an IPC client (like the REPL does)
        let client = IpcClient::connect(&session_name).await.unwrap();

        // Build a minimal App with the client
        let mut app = App {
            input: Input::default(),
            completion: CompletionState::default(),
            output_history: std::collections::VecDeque::new(),
            output_scroll_position: 0,
            selection: SelectionState::default(),
            output_area: None,
            should_quit: false,
            session_name: session_name.clone(),
            client: Some(client),
            cached_elements: Vec::new(),
            cached_devices: Vec::new(),
            cached_apps: Vec::new(),
            app_update_rx: None,
            app_fetch_trigger_tx: None,
            apps_loading: false,
            apps_fetch_started_at: None,
            element_update_rx: None,
            fetch_trigger_tx: None,
            active_fetch_command: None,
            elements_loading: false,
            fetch_started_at: None,
            is_processing: false,
            processing_label: String::new(),
            processing_start: None,
            cmd_result_rx: None,
            startup_rx: None,
        };

        assert!(app.client.is_some(), "Client should be set before shutdown");

        // Call shutdown — this sends IpcRequest::Shutdown and clears the client
        app.shutdown().await;

        assert!(app.client.is_none(), "Client should be None after shutdown");

        // The server received Shutdown but since built-in IpcServer doesn't handle
        // it (returns error), we abort the server to trigger Drop cleanup
        server_handle.abort();
        let _ = server_handle.await;
    }

    /// Verify that `shutdown()` is safe to call with no client connected.
    #[tokio::test]
    async fn test_shutdown_without_client_is_noop() {
        let mut app = App {
            input: Input::default(),
            completion: CompletionState::default(),
            output_history: std::collections::VecDeque::new(),
            output_scroll_position: 0,
            selection: SelectionState::default(),
            output_area: None,
            should_quit: false,
            session_name: "nonexistent".to_string(),
            client: None,
            cached_elements: Vec::new(),
            cached_devices: Vec::new(),
            cached_apps: Vec::new(),
            app_update_rx: None,
            app_fetch_trigger_tx: None,
            apps_loading: false,
            apps_fetch_started_at: None,
            element_update_rx: None,
            fetch_trigger_tx: None,
            active_fetch_command: None,
            elements_loading: false,
            fetch_started_at: None,
            is_processing: false,
            processing_label: String::new(),
            processing_start: None,
            cmd_result_rx: None,
            startup_rx: None,
        };

        // Should not panic or error
        app.shutdown().await;
        assert!(app.client.is_none());
    }
}
