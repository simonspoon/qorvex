//! CLI client for iOS Simulator automation via qorvex IPC.
//!
//! This tool sends action commands to a running REPL session via Unix socket IPC.
//!
//! # Usage
//!
//! ```bash
//! # Tap an element by accessibility ID
//! qorvex tap-element login-button
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
//! # Get screen info (JSON)
//! qorvex screen-info | jq '.elements'
//!
//! # Connect to a specific session
//! qorvex -s my-session tap-element button
//! ```

use clap::{Parser, Subcommand};
use qorvex_core::action::ActionType;
use qorvex_core::ipc::{IpcClient, IpcRequest, IpcResponse};
use std::process::ExitCode;

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
    /// Tap an element by accessibility ID
    TapElement {
        /// The accessibility identifier of the element to tap
        id: String,
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

    /// Get UI hierarchy information (outputs JSON)
    ScreenInfo,

    /// Get the value of an element
    GetValue {
        /// The accessibility identifier of the element
        id: String,
    },

    /// Log a comment to the session
    Comment {
        /// The comment message
        message: String,
    },

    /// Wait for an element to appear
    WaitFor {
        /// Element accessibility identifier
        id: String,
        /// Timeout in milliseconds
        #[arg(short, long, default_value = "5000")]
        timeout: u64,
    },

    /// Get current session state
    Status,

    /// Get action log history
    Log,
}

#[tokio::main]
async fn main() -> ExitCode {
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

async fn run(cli: Cli) -> Result<(), CliError> {
    // Connect to the IPC server
    let mut client = IpcClient::connect(&cli.session)
        .await
        .map_err(|e| CliError::Connection(format!("Failed to connect to session '{}': {}", cli.session, e)))?;

    match cli.command {
        Command::TapElement { ref id } => {
            execute_action(&mut client, ActionType::TapElement { id: id.clone() }, &cli).await
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
        Command::ScreenInfo => {
            execute_action(&mut client, ActionType::GetScreenInfo, &cli).await
        }
        Command::GetValue { ref id } => {
            execute_action(&mut client, ActionType::GetElementValue { id: id.clone() }, &cli).await
        }
        Command::Comment { ref message } => {
            execute_action(&mut client, ActionType::LogComment { message: message.clone() }, &cli).await
        }
        Command::WaitFor { ref id, timeout } => {
            execute_action(&mut client, ActionType::WaitFor { id: id.clone(), timeout_ms: timeout }, &cli).await
        }
        Command::Status => {
            get_status(&mut client, &cli).await
        }
        Command::Log => {
            get_log(&mut client, &cli).await
        }
    }
}

async fn execute_action(client: &mut IpcClient, action: ActionType, cli: &Cli) -> Result<(), CliError> {
    let is_screenshot_action = matches!(action, ActionType::GetScreenshot);
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
                    if !cli.quiet {
                        // Include duration from data if present
                        if let Some(ref d) = data {
                            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(d) {
                                if let Some(elapsed) = parsed.get("elapsed_ms").and_then(|v| v.as_u64()) {
                                    eprintln!("{} ({}ms)", message, elapsed);
                                } else {
                                    eprintln!("{}", message);
                                }
                            } else {
                                eprintln!("{}", message);
                            }
                        } else {
                            eprintln!("{}", message);
                        }
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
