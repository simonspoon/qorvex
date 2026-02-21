//! CLI client for iOS Simulator automation via qorvex IPC.
//!
//! This tool sends action commands to a running REPL session via Unix socket IPC.
//!
//! # Usage
//!
//! ```bash
//! # Tap an element by accessibility ID (waits for it by default)
//! qorvex tap login-button
//!
//! # Tap an element by label
//! qorvex tap "Sign In" --label
//!
//! # Tap without waiting for element
//! qorvex tap "Sign In" -l --no-wait
//!
//! # Tap a specific element type by label
//! qorvex tap "Sign In" -l -T Button
//!
//! # Tap at coordinates
//! qorvex tap-location 100 200
//!
//! # Send keyboard input
//! qorvex send-keys "hello world"
//!
//! # Get screenshot (base64)
//! qorvex screenshot > screen.b64
//!
//! # Get screen info (concise actionable elements)
//! qorvex screen-info
//!
//! # Get full raw JSON
//! qorvex screen-info --full
//!
//! # Get REPL-style formatted list
//! qorvex screen-info --pretty
//!
//! # Get element value (waits for element by default)
//! qorvex get-value username-field
//! qorvex get-value "Email" --label
//!
//! # Get value without waiting
//! qorvex get-value username-field --no-wait
//!
//! # Wait for an element
//! qorvex wait-for spinner-id
//! qorvex wait-for "Loading" -l -t 10000
//!
//! # Connect to a specific session
//! qorvex -s my-session tap button
//! ```

mod converter;

use clap::{Parser, Subcommand};
use qorvex_core::action::ActionType;
use qorvex_core::element::{UIElement, ElementFrame};
use qorvex_core::ipc::{qorvex_dir, IpcClient, IpcRequest, IpcResponse};
use qorvex_core::simctl::Simctl;
use std::path::PathBuf;
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

/// CLI client for iOS Simulator automation via qorvex IPC.
#[derive(Parser)]
#[command(name = "qorvex")]
#[command(about = "Send automation commands to a running qorvex REPL session")]
#[command(version)]
struct Cli {
    /// Session name to connect to
    #[arg(short, long, default_value = "default", env = "QORVEX_SESSION")]
    session: String,

    /// Output format: text or json
    #[arg(short, long, default_value = "text")]
    format: OutputFormat,

    /// Suppress non-essential output
    #[arg(short, long)]
    quiet: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Subcommand)]
enum Command {
    /// Tap an element by ID or label
    Tap {
        /// The selector (accessibility ID or label)
        selector: String,
        /// Match by accessibility label instead of ID
        #[arg(short, long)]
        label: bool,
        /// Filter by element type (e.g., Button, TextField)
        #[arg(short = 'T', long = "type")]
        element_type: Option<String>,
        /// Skip waiting for the element to appear before tapping
        #[arg(long)]
        no_wait: bool,
        /// Timeout in milliseconds when waiting
        #[arg(short = 'o', long, default_value = "5000", env = "QORVEX_TIMEOUT")]
        timeout: u64,
    },

    /// Tap at screen coordinates
    TapLocation {
        /// X coordinate
        x: i32,
        /// Y coordinate
        y: i32,
    },

    /// Send keyboard input
    SendKeys {
        /// Text to type
        text: String,
    },

    /// Capture a screenshot (outputs base64-encoded PNG)
    Screenshot,

    /// Get UI hierarchy information
    ScreenInfo {
        /// Output full raw JSON (original behavior)
        #[arg(long)]
        full: bool,
        /// Output REPL-style formatted list
        #[arg(long)]
        pretty: bool,
    },

    /// Get the value of an element by ID or label
    GetValue {
        /// The selector (accessibility ID or label)
        selector: String,
        /// Match by accessibility label instead of ID
        #[arg(short, long)]
        label: bool,
        /// Filter by element type (e.g., Button, TextField)
        #[arg(short = 'T', long = "type")]
        element_type: Option<String>,
        /// Skip waiting for the element to appear before getting value
        #[arg(long)]
        no_wait: bool,
        /// Timeout in milliseconds when waiting
        #[arg(short = 'o', long, default_value = "5000", env = "QORVEX_TIMEOUT")]
        timeout: u64,
    },

    /// Log a comment to the session
    Comment {
        /// The comment message
        message: String,
    },

    /// Wait for an element to appear by ID or label
    WaitFor {
        /// The selector (accessibility ID or label)
        selector: String,
        /// Match by accessibility label instead of ID
        #[arg(short, long)]
        label: bool,
        /// Filter by element type (e.g., Button, TextField)
        #[arg(short = 'T', long = "type")]
        element_type: Option<String>,
        /// Timeout in milliseconds
        #[arg(short = 'o', long, default_value = "5000", env = "QORVEX_TIMEOUT")]
        timeout: u64,
    },

    /// Wait for an element to disappear by ID or label
    WaitForNot {
        /// The selector (accessibility ID or label)
        selector: String,
        /// Match by accessibility label instead of ID
        #[arg(short, long)]
        label: bool,
        /// Filter by element type (e.g., Button, TextField)
        #[arg(short = 'T', long = "type")]
        element_type: Option<String>,
        /// Timeout in milliseconds
        #[arg(short = 'o', long, default_value = "5000", env = "QORVEX_TIMEOUT")]
        timeout: u64,
    },

    /// Swipe the screen in a direction
    Swipe {
        /// Direction: up, down, left, right
        direction: String,
    },

    /// Set the target application bundle ID
    SetTarget {
        /// Bundle identifier (e.g., com.example.MyApp)
        bundle_id: String,
    },

    /// Boot a simulator device
    BootDevice {
        /// Device UDID
        udid: String,
    },

    /// List available simulator devices
    ListDevices,

    /// Convert a JSONL action log to a shell script
    Convert {
        /// Path to the JSONL log file (reads from stdin if omitted)
        log: Option<PathBuf>,
    },

    /// Get current session state
    Status,

    /// Get action log history
    Log,

    /// List all running qorvex sessions
    ListSessions,

    /// Start server, session, and agent in one step
    Start,

    /// Start an automation session (auto-starts agent if configured)
    StartSession,

    /// Start or connect to the automation agent
    StartAgent {
        /// Path to the agent project directory
        #[arg(short, long)]
        project_dir: Option<String>,
    },

    /// Stop the server for this session
    Stop,
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {}", e);
            e.exit_code()
        }
    }
}

#[derive(Debug)]
enum CliError {
    Connection(String),
    ActionFailed(String),
    Protocol(String),
}

impl CliError {
    fn exit_code(&self) -> ExitCode {
        match self {
            CliError::Connection(_) => ExitCode::from(2),
            CliError::ActionFailed(_) => ExitCode::from(1),
            CliError::Protocol(_) => ExitCode::from(3),
        }
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::Connection(msg) => write!(f, "Connection error: {}", msg),
            CliError::ActionFailed(msg) => write!(f, "Action failed: {}", msg),
            CliError::Protocol(msg) => write!(f, "Protocol error: {}", msg),
        }
    }
}

fn discover_sessions() -> Vec<String> {
    let pattern = qorvex_dir().join("qorvex_*.sock");
    glob::glob(pattern.to_str().unwrap_or_default())
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry.ok().and_then(|path| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.strip_prefix("qorvex_"))
                    .map(String::from)
            })
        })
        .collect()
}

async fn run(cli: Cli) -> Result<(), CliError> {
    // Handle commands that don't need an IPC connection
    match cli.command {
        Command::ListSessions => {
            let sessions = discover_sessions();
            if cli.format == OutputFormat::Json {
                println!("{}", serde_json::json!({ "sessions": sessions }));
            } else {
                if sessions.is_empty() {
                    eprintln!("No running sessions found");
                } else {
                    for session in sessions {
                        println!("{}", session);
                    }
                }
            }
            return Ok(());
        }
        Command::ListDevices => {
            match Simctl::list_devices() {
                Ok(devices) => {
                    if cli.format == OutputFormat::Json {
                        println!("{}", serde_json::to_string_pretty(&devices)
                            .map_err(|e| CliError::Protocol(e.to_string()))?);
                    } else {
                        if devices.is_empty() {
                            eprintln!("No simulator devices found");
                        } else {
                            for device in &devices {
                                let state = if device.state == "Booted" { " (Booted)" } else { "" };
                                println!("{} -- {}{}", device.udid, device.name, state);
                            }
                        }
                    }
                }
                Err(e) => return Err(CliError::ActionFailed(format!("Failed to list devices: {}", e))),
            }
            return Ok(());
        }
        Command::BootDevice { ref udid } => {
            match Simctl::boot(udid) {
                Ok(()) => {
                    if cli.format == OutputFormat::Json {
                        println!("{}", serde_json::json!({ "success": true, "udid": udid }));
                    } else {
                        eprintln!("Booted device {}", udid);
                    }
                }
                Err(e) => return Err(CliError::ActionFailed(format!("Failed to boot device: {}", e))),
            }
            return Ok(());
        }
        Command::Convert { ref log } => {
            let result = match log {
                Some(path) => converter::LogConverter::convert_file(path)
                    .map_err(|e| CliError::ActionFailed(format!("Failed to convert log: {}", e))),
                None => converter::LogConverter::convert_stdin()
                    .map_err(|e| CliError::ActionFailed(format!("Failed to convert from stdin: {}", e))),
            };
            match result {
                Ok(script) => {
                    print!("{}", script);
                    return Ok(());
                }
                Err(e) => return Err(e),
            }
        }
        Command::Start => {
            return start_all(&cli).await;
        }
        _ => {} // Fall through to IPC-connected commands
    }

    // Connect to the IPC server
    let mut client = IpcClient::connect(&cli.session)
        .await
        .map_err(|e| CliError::Connection(format!("Failed to connect to session '{}': {}", cli.session, e)))?;

    match cli.command {
        Command::Tap { ref selector, label, ref element_type, no_wait, timeout } => {
            let wait = if !no_wait {
                Some(ActionType::WaitFor {
                    selector: selector.clone(),
                    by_label: label,
                    element_type: element_type.clone(),
                    timeout_ms: timeout,
                    require_stable: false,
                })
            } else {
                None
            };
            execute_with_wait(&mut client, wait, ActionType::Tap {
                selector: selector.clone(),
                by_label: label,
                element_type: element_type.clone(),
            }, &cli).await
        }
        Command::TapLocation { x, y } => {
            execute_action(&mut client, ActionType::TapLocation { x, y }, &cli).await
        }
        Command::SendKeys { ref text } => {
            execute_action(&mut client, ActionType::SendKeys { text: text.clone() }, &cli).await
        }
        Command::Screenshot => {
            execute_action(&mut client, ActionType::GetScreenshot, &cli).await
        }
        Command::ScreenInfo { full, pretty } => {
            execute_screen_info(&mut client, &cli, full, pretty).await
        }
        Command::GetValue { ref selector, label, ref element_type, no_wait, timeout } => {
            let wait = if !no_wait {
                Some(ActionType::WaitFor {
                    selector: selector.clone(),
                    by_label: label,
                    element_type: element_type.clone(),
                    timeout_ms: timeout,
                    require_stable: false,
                })
            } else {
                None
            };
            execute_with_wait(&mut client, wait, ActionType::GetValue {
                selector: selector.clone(),
                by_label: label,
                element_type: element_type.clone(),
            }, &cli).await
        }
        Command::Swipe { ref direction } => {
            execute_action(&mut client, ActionType::Swipe { direction: direction.clone() }, &cli).await
        }
        Command::SetTarget { ref bundle_id } => {
            execute_action(&mut client, ActionType::SetTarget { bundle_id: bundle_id.clone() }, &cli).await
        }
        Command::Comment { ref message } => {
            execute_action(&mut client, ActionType::LogComment { message: message.clone() }, &cli).await
        }
        Command::WaitFor { ref selector, label, ref element_type, timeout } => {
            execute_action(&mut client, ActionType::WaitFor {
                selector: selector.clone(),
                by_label: label,
                element_type: element_type.clone(),
                timeout_ms: timeout,
                require_stable: true,
            }, &cli).await
        }
        Command::WaitForNot { ref selector, label, ref element_type, timeout } => {
            execute_action(&mut client, ActionType::WaitForNot {
                selector: selector.clone(),
                by_label: label,
                element_type: element_type.clone(),
                timeout_ms: timeout,
            }, &cli).await
        }
        Command::StartSession => {
            send_command(&mut client, IpcRequest::StartSession, &cli).await
        }
        Command::StartAgent { ref project_dir } => {
            send_command(&mut client, IpcRequest::StartAgent { project_dir: project_dir.clone() }, &cli).await
        }
        Command::Stop => {
            stop_server(&mut client, &cli).await
        }
        Command::Status => {
            get_status(&mut client, &cli).await
        }
        Command::Log => {
            get_log(&mut client, &cli).await
        }
        // These commands are handled before IPC connection above
        Command::ListSessions | Command::ListDevices | Command::BootDevice { .. } | Command::Convert { .. } | Command::Start => unreachable!(),
    }
}

async fn execute_action(client: &mut IpcClient, action: ActionType, cli: &Cli) -> Result<(), CliError> {
    let is_screenshot_action = matches!(action, ActionType::GetScreenshot);
    let is_data_action = matches!(action, ActionType::GetScreenInfo | ActionType::GetValue { .. });
    let action_label = action.display_name();
    let action_target = action.display_target();
    let request = IpcRequest::Execute { action };
    let response = client
        .send(&request)
        .await
        .map_err(|e| CliError::Protocol(format!("Failed to send request: {}", e)))?;

    match response {
        IpcResponse::ActionResult { success, message, screenshot, data } => {
            if cli.format == OutputFormat::Json {
                let output = serde_json::json!({
                    "success": success,
                    "message": message,
                    "screenshot": if is_screenshot_action { screenshot.as_ref().map(|s| s.as_ref()) } else { None },
                    "data": data.as_ref().and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok()),
                });
                println!("{}", serde_json::to_string_pretty(&output).unwrap());
            } else {
                // Text format - output depends on the action
                if success {
                    // Only output screenshot for GetScreenshot command
                    if is_screenshot_action {
                        if let Some(ref ss) = screenshot {
                            println!("{}", ss);
                        }
                    }
                    // Output data payload for data-returning commands
                    if is_data_action {
                        if let Some(ref d) = data {
                            println!("{}", d);
                        }
                    }
                    if !cli.quiet {
                        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3fZ");
                        let duration_str = data.as_ref()
                            .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
                            .and_then(|parsed| parsed.get("elapsed_ms").and_then(|v| v.as_u64()))
                            .map(|ms| format!("{}ms", ms))
                            .unwrap_or_default();
                        eprintln!("|{}|{}|{}|{}|", now, action_label, action_target, duration_str);
                    }
                } else {
                    return Err(CliError::ActionFailed(message));
                }
            }
            Ok(())
        }
        IpcResponse::Error { message } => {
            Err(CliError::ActionFailed(message))
        }
        _ => {
            Err(CliError::Protocol("Unexpected response type".to_string()))
        }
    }
}

async fn execute_with_wait(
    client: &mut IpcClient,
    wait_action: Option<ActionType>,
    action: ActionType,
    cli: &Cli,
) -> Result<(), CliError> {
    // Execute wait phase and capture find duration
    let find_ms = if let Some(wait) = wait_action {
        let request = IpcRequest::Execute { action: wait };
        let response = client
            .send(&request)
            .await
            .map_err(|e| CliError::Protocol(format!("Failed to send request: {}", e)))?;
        match response {
            IpcResponse::ActionResult { success, message, data, .. } => {
                if !success {
                    return Err(CliError::ActionFailed(message));
                }
                data.as_ref()
                    .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
                    .and_then(|parsed| parsed.get("elapsed_ms").and_then(|v| v.as_u64()))
            }
            IpcResponse::Error { message } => return Err(CliError::ActionFailed(message)),
            _ => return Err(CliError::Protocol("Unexpected response type".to_string())),
        }
    } else {
        None
    };

    // Execute the actual action
    let is_screenshot_action = matches!(action, ActionType::GetScreenshot);
    let is_data_action = matches!(action, ActionType::GetScreenInfo | ActionType::GetValue { .. });
    let action_label = action.display_name();
    let action_target = action.display_target();
    let request = IpcRequest::Execute { action };
    let start = std::time::Instant::now();
    let response = client
        .send(&request)
        .await
        .map_err(|e| CliError::Protocol(format!("Failed to send request: {}", e)))?;
    let action_elapsed = start.elapsed();

    match response {
        IpcResponse::ActionResult { success, message, screenshot, data } => {
            if cli.format == OutputFormat::Json {
                let output = serde_json::json!({
                    "success": success,
                    "message": message,
                    "screenshot": if is_screenshot_action { screenshot.as_ref().map(|s| s.as_ref()) } else { None },
                    "data": data.as_ref().and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok()),
                });
                println!("{}", serde_json::to_string_pretty(&output).unwrap());
            } else if success {
                if is_screenshot_action {
                    if let Some(ref ss) = screenshot {
                        println!("{}", ss);
                    }
                }
                if is_data_action {
                    if let Some(ref d) = data {
                        println!("{}", d);
                    }
                }
                if !cli.quiet {
                    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3fZ");
                    let find_str = find_ms
                        .map(|ms| format!("{}ms", ms))
                        .unwrap_or_default();
                    let action_str = format!("{}ms", action_elapsed.as_millis());
                    eprintln!("|{}|{}|{}|{}|{}|", now, action_label, action_target, find_str, action_str);
                }
            } else {
                return Err(CliError::ActionFailed(message));
            }
            Ok(())
        }
        IpcResponse::Error { message } => Err(CliError::ActionFailed(message)),
        _ => Err(CliError::Protocol("Unexpected response type".to_string())),
    }
}

/// Check if an element is "actionable" (has an identifier or label, and is a meaningful type).
fn is_actionable(elem: &UIElement) -> bool {
    elem.identifier.is_some() || elem.label.is_some()
}

/// Filter the top-level element list to actionable elements only (no recursion into children).
fn collect_actionable(elements: &[UIElement]) -> Vec<&UIElement> {
    elements.iter().filter(|e| is_actionable(e)).collect()
}

/// Serialize a UIElement concisely: no null fields, rounded frame values.
fn element_to_concise_json(elem: &UIElement) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    if let Some(ref t) = elem.element_type {
        map.insert("type".into(), serde_json::Value::String(t.clone()));
    }
    if let Some(ref id) = elem.identifier {
        map.insert("id".into(), serde_json::Value::String(id.clone()));
    }
    if let Some(ref label) = elem.label {
        map.insert("label".into(), serde_json::Value::String(label.clone()));
    }
    if let Some(ref value) = elem.value {
        map.insert("value".into(), serde_json::Value::String(value.clone()));
    }
    if let Some(ref frame) = elem.frame {
        map.insert("frame".into(), frame_to_rounded_json(frame));
    }
    if let Some(ref role) = elem.role {
        map.insert("role".into(), serde_json::Value::String(role.clone()));
    }
    if let Some(hittable) = elem.hittable {
        map.insert("hittable".into(), serde_json::Value::Bool(hittable));
    }
    serde_json::Value::Object(map)
}

fn frame_to_rounded_json(frame: &ElementFrame) -> serde_json::Value {
    serde_json::json!({
        "x": frame.x.round() as i64,
        "y": frame.y.round() as i64,
        "width": frame.width.round() as i64,
        "height": frame.height.round() as i64,
    })
}

/// Format an element in the REPL style: `[Type] id "label" =value @(x,y)`
fn format_element_pretty(elem: &UIElement) -> String {
    let mut parts = Vec::new();
    let elem_type = elem.element_type.as_deref().unwrap_or("Unknown");
    parts.push(format!("[{}]", elem_type));
    if let Some(ref id) = elem.identifier {
        parts.push(id.clone());
    }
    if let Some(ref label) = elem.label {
        parts.push(format!("\"{}\"", label));
    }
    if let Some(ref value) = elem.value {
        parts.push(format!("={}", value));
    }
    if let Some(ref frame) = elem.frame {
        parts.push(format!("@({:.0},{:.0})", frame.x, frame.y));
    }
    parts.join(" ")
}

async fn execute_screen_info(
    client: &mut IpcClient,
    cli: &Cli,
    full: bool,
    pretty: bool,
) -> Result<(), CliError> {
    let request = IpcRequest::Execute { action: ActionType::GetScreenInfo };
    let response = client
        .send(&request)
        .await
        .map_err(|e| CliError::Protocol(format!("Failed to send request: {}", e)))?;

    match response {
        IpcResponse::ActionResult { success, message, data, .. } => {
            if !success {
                return Err(CliError::ActionFailed(message));
            }
            let data_str = data.as_deref().unwrap_or("[]");

            if full {
                // Original behavior: dump raw JSON
                println!("{}", data_str);
            } else if pretty {
                // REPL-style formatted output
                let elements: Vec<UIElement> = serde_json::from_str(data_str)
                    .map_err(|e| CliError::Protocol(format!("Failed to parse elements: {}", e)))?;
                let actionable = collect_actionable(&elements);
                for elem in &actionable {
                    println!("{}", format_element_pretty(elem));
                }
                if !cli.quiet {
                    eprintln!("{} elements", actionable.len());
                }
            } else {
                // Default: concise JSON, actionable only, no nulls, rounded frames
                let elements: Vec<UIElement> = serde_json::from_str(data_str)
                    .map_err(|e| CliError::Protocol(format!("Failed to parse elements: {}", e)))?;
                let actionable = collect_actionable(&elements);
                let concise: Vec<serde_json::Value> = actionable
                    .iter()
                    .map(|e| element_to_concise_json(e))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&concise).unwrap());
                if !cli.quiet {
                    eprintln!("{} elements", actionable.len());
                }
            }

            Ok(())
        }
        IpcResponse::Error { message } => Err(CliError::ActionFailed(message)),
        _ => Err(CliError::Protocol("Unexpected response type".to_string())),
    }
}

async fn get_status(client: &mut IpcClient, cli: &Cli) -> Result<(), CliError> {
    let response = client
        .send(&IpcRequest::GetState)
        .await
        .map_err(|e| CliError::Protocol(format!("Failed to send request: {}", e)))?;

    match response {
        IpcResponse::State { session_id, screenshot } => {
            if cli.format == OutputFormat::Json {
                let output = serde_json::json!({
                    "session_id": session_id,
                    "has_screenshot": screenshot.is_some(),
                });
                println!("{}", serde_json::to_string_pretty(&output).unwrap());
            } else {
                println!("Session ID: {}", session_id);
                println!("Has screenshot: {}", screenshot.is_some());
            }
            Ok(())
        }
        IpcResponse::Error { message } => {
            Err(CliError::ActionFailed(message))
        }
        _ => {
            Err(CliError::Protocol("Unexpected response type".to_string()))
        }
    }
}

async fn get_log(client: &mut IpcClient, cli: &Cli) -> Result<(), CliError> {
    let response = client
        .send(&IpcRequest::GetLog)
        .await
        .map_err(|e| CliError::Protocol(format!("Failed to send request: {}", e)))?;

    match response {
        IpcResponse::Log { entries } => {
            if cli.format == OutputFormat::Json {
                println!("{}", serde_json::to_string_pretty(&entries).unwrap());
            } else {
                if entries.is_empty() {
                    println!("No actions logged");
                } else {
                    for entry in entries {
                        println!("[{}] {:?} - {:?}",
                            entry.timestamp.format("%H:%M:%S"),
                            entry.action,
                            entry.result
                        );
                    }
                }
            }
            Ok(())
        }
        IpcResponse::Error { message } => {
            Err(CliError::ActionFailed(message))
        }
        _ => {
            Err(CliError::Protocol("Unexpected response type".to_string()))
        }
    }
}

async fn send_command(client: &mut IpcClient, request: IpcRequest, cli: &Cli) -> Result<(), CliError> {
    let response = client
        .send(&request)
        .await
        .map_err(|e| CliError::Protocol(format!("Failed to send request: {}", e)))?;

    match response {
        IpcResponse::CommandResult { success, message } => {
            if success {
                if !cli.quiet {
                    eprintln!("{}", message);
                }
                Ok(())
            } else {
                Err(CliError::ActionFailed(message))
            }
        }
        IpcResponse::Error { message } => Err(CliError::ActionFailed(message)),
        _ => Err(CliError::Protocol("Unexpected response".to_string())),
    }
}

async fn start_all(cli: &Cli) -> Result<(), CliError> {
    use qorvex_core::ipc::socket_path;

    let sock = socket_path(&cli.session);

    // Start server if not already running
    if !sock.exists() {
        let log_dir = qorvex_dir().join("logs");
        std::fs::create_dir_all(&log_dir).ok();
        let log_file = std::fs::File::create(log_dir.join("qorvex-server-launch.log")).ok();

        let mut cmd = std::process::Command::new("qorvex-server");
        cmd.args(["-s", &cli.session]);
        if let Some(f) = log_file {
            cmd.stdout(f.try_clone().unwrap_or_else(|_| std::fs::File::create("/dev/null").unwrap()));
            cmd.stderr(f);
        }
        cmd.spawn()
            .map_err(|e| CliError::Connection(format!("Failed to start server: {}", e)))?;

        // Wait for socket to appear (up to 5s)
        for _ in 0..50 {
            if sock.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        if !sock.exists() {
            return Err(CliError::Connection("Server did not start in time".to_string()));
        }
    }

    // Connect and start session
    let mut client = IpcClient::connect(&cli.session)
        .await
        .map_err(|e| CliError::Connection(format!("Failed to connect: {}", e)))?;

    let response = client
        .send(&IpcRequest::StartSession)
        .await
        .map_err(|e| CliError::Protocol(format!("Failed to start session: {}", e)))?;

    match response {
        IpcResponse::CommandResult { success, message } => {
            if success {
                if !cli.quiet {
                    eprintln!("{}", message);
                }
                Ok(())
            } else {
                Err(CliError::ActionFailed(message))
            }
        }
        IpcResponse::Error { message } => Err(CliError::ActionFailed(message)),
        _ => Err(CliError::Protocol("Unexpected response".to_string())),
    }
}

async fn stop_server(client: &mut IpcClient, cli: &Cli) -> Result<(), CliError> {
    let response = client
        .send(&IpcRequest::Shutdown)
        .await
        .map_err(|e| CliError::Protocol(format!("Failed to send shutdown request: {}", e)))?;

    match response {
        IpcResponse::ShutdownAck => {
            if !cli.quiet {
                eprintln!("Server stopped");
            }
            Ok(())
        }
        IpcResponse::Error { message } => Err(CliError::ActionFailed(message)),
        _ => Err(CliError::Protocol("Unexpected response to Shutdown".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};

    #[test]
    fn test_discover_sessions() {
        let qorvex_dir = qorvex_dir();

        // Ensure the qorvex directory exists
        fs::create_dir_all(&qorvex_dir).expect("Failed to create qorvex directory");

        // Create temporary socket files with unique names to avoid test collisions
        let test_sessions = ["test_session_a", "test_session_b", "test_session_c"];
        let mut created_files = Vec::new();

        for session_name in &test_sessions {
            let sock_path = qorvex_dir.join(format!("qorvex_{}.sock", session_name));
            File::create(&sock_path).expect("Failed to create test socket file");
            created_files.push(sock_path);
        }

        // Run discover_sessions and verify it finds our test sessions
        let discovered = discover_sessions();

        // Verify all test sessions are found
        for session_name in &test_sessions {
            assert!(
                discovered.contains(&session_name.to_string()),
                "discover_sessions() should find session '{}', but got: {:?}",
                session_name,
                discovered
            );
        }

        // Clean up the temporary socket files
        for path in created_files {
            let _ = fs::remove_file(path);
        }
    }
}
