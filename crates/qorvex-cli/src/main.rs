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
//! # Get screen info (JSON)
//! qorvex screen-info | jq '.elements'
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

use clap::{Parser, Subcommand};
use qorvex_core::action::ActionType;
use qorvex_core::ipc::{qorvex_dir, IpcClient, IpcRequest, IpcResponse};
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
        #[arg(short = 'o', long, default_value = "5000")]
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

    /// Get UI hierarchy information (outputs JSON)
    ScreenInfo,

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
        #[arg(short = 'o', long, default_value = "5000")]
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
        #[arg(short = 'o', long, default_value = "5000")]
        timeout: u64,
    },

    /// Get current session state
    Status,

    /// Get action log history
    Log,

    /// List all running qorvex sessions
    ListSessions,
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
    // Handle ListSessions separately since it doesn't need IPC connection
    if let Command::ListSessions = cli.command {
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

    // Connect to the IPC server
    let mut client = IpcClient::connect(&cli.session)
        .await
        .map_err(|e| CliError::Connection(format!("Failed to connect to session '{}': {}", cli.session, e)))?;

    match cli.command {
        Command::Tap { ref selector, label, ref element_type, no_wait, timeout } => {
            if !no_wait {
                execute_action(&mut client, ActionType::WaitFor {
                    selector: selector.clone(),
                    by_label: label,
                    element_type: element_type.clone(),
                    timeout_ms: timeout,
                }, &cli).await?;
            }
            execute_action(&mut client, ActionType::Tap {
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
        Command::ScreenInfo => {
            execute_action(&mut client, ActionType::GetScreenInfo, &cli).await
        }
        Command::GetValue { ref selector, label, ref element_type, no_wait, timeout } => {
            if !no_wait {
                execute_action(&mut client, ActionType::WaitFor {
                    selector: selector.clone(),
                    by_label: label,
                    element_type: element_type.clone(),
                    timeout_ms: timeout,
                }, &cli).await?;
            }
            execute_action(&mut client, ActionType::GetValue {
                selector: selector.clone(),
                by_label: label,
                element_type: element_type.clone(),
            }, &cli).await
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
            }, &cli).await
        }
        Command::Status => {
            get_status(&mut client, &cli).await
        }
        Command::Log => {
            get_log(&mut client, &cli).await
        }
        // ListSessions is handled before IPC connection above
        Command::ListSessions => unreachable!(),
    }
}

async fn execute_action(client: &mut IpcClient, action: ActionType, cli: &Cli) -> Result<(), CliError> {
    let is_screenshot_action = matches!(action, ActionType::GetScreenshot);
    let is_data_action = matches!(action, ActionType::GetScreenInfo | ActionType::GetValue { .. });
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
