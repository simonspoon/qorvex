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
use qorvex_core::adb_device::Adb;
use qorvex_core::element::{ElementFrame, UIElement};
use qorvex_core::ipc::{
    agent_port_path, qorvex_dir, socket_path, IpcClient, IpcRequest, IpcResponse, Platform,
};
use qorvex_core::memory::MemoryInfo;
use qorvex_core::simctl::{QuietOutcome, Simctl};
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

/// Target platform for device/agent commands (CLI-facing; maps to
/// [`qorvex_core::ipc::Platform`]).
#[derive(Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
enum PlatformArg {
    #[default]
    Ios,
    Android,
}

impl From<PlatformArg> for Platform {
    fn from(p: PlatformArg) -> Self {
        match p {
            PlatformArg::Ios => Platform::Ios,
            PlatformArg::Android => Platform::Android,
        }
    }
}

/// Which container `app-container` reports, matching the optional third
/// argument of `simctl get_app_container`.
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum ContainerArg {
    /// The installed `.app` bundle (simctl's default).
    App,
    /// The app's data container.
    Data,
    /// The app's shared app-group containers.
    Groups,
}

impl ContainerArg {
    fn as_str(self) -> &'static str {
        match self {
            ContainerArg::App => "app",
            ContainerArg::Data => "data",
            ContainerArg::Groups => "groups",
        }
    }
}

/// The appearances `simctl ui <udid> appearance` accepts.
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum AppearanceArg {
    /// Dark mode.
    Dark,
    /// Light mode.
    Light,
}

impl AppearanceArg {
    fn as_str(self) -> &'static str {
        match self {
            AppearanceArg::Dark => "dark",
            AppearanceArg::Light => "light",
        }
    }
}

/// The Dynamic Type sizes `simctl ui <udid> content_size` accepts.
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum ContentSizeArg {
    ExtraSmall,
    Small,
    Medium,
    Large,
    ExtraLarge,
    ExtraExtraLarge,
    ExtraExtraExtraLarge,
    AccessibilityMedium,
    AccessibilityLarge,
    AccessibilityExtraLarge,
    AccessibilityExtraExtraLarge,
    AccessibilityExtraExtraExtraLarge,
}

impl ContentSizeArg {
    fn as_str(self) -> &'static str {
        match self {
            ContentSizeArg::ExtraSmall => "extra-small",
            ContentSizeArg::Small => "small",
            ContentSizeArg::Medium => "medium",
            ContentSizeArg::Large => "large",
            ContentSizeArg::ExtraLarge => "extra-large",
            ContentSizeArg::ExtraExtraLarge => "extra-extra-large",
            ContentSizeArg::ExtraExtraExtraLarge => "extra-extra-extra-large",
            ContentSizeArg::AccessibilityMedium => "accessibility-medium",
            ContentSizeArg::AccessibilityLarge => "accessibility-large",
            ContentSizeArg::AccessibilityExtraLarge => "accessibility-extra-large",
            ContentSizeArg::AccessibilityExtraExtraLarge => "accessibility-extra-extra-large",
            ContentSizeArg::AccessibilityExtraExtraExtraLarge => {
                "accessibility-extra-extra-extra-large"
            }
        }
    }
}

/// What `grant-permission` does to an app's access.
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum PrivacyVerbArg {
    /// Allow access without prompting.
    Grant,
    /// Deny access without prompting.
    Revoke,
    /// Forget the decision, so the app prompts again.
    Reset,
}

impl PrivacyVerbArg {
    fn as_str(self) -> &'static str {
        match self {
            PrivacyVerbArg::Grant => "grant",
            PrivacyVerbArg::Revoke => "revoke",
            PrivacyVerbArg::Reset => "reset",
        }
    }
}

/// The privacy-protected services `simctl privacy` knows about.
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum PrivacyServiceArg {
    All,
    Calendar,
    ContactsLimited,
    Contacts,
    Location,
    LocationAlways,
    PhotosAdd,
    Photos,
    MediaLibrary,
    Microphone,
    Motion,
    Reminders,
    Siri,
}

impl PrivacyServiceArg {
    fn as_str(self) -> &'static str {
        match self {
            PrivacyServiceArg::All => "all",
            PrivacyServiceArg::Calendar => "calendar",
            PrivacyServiceArg::ContactsLimited => "contacts-limited",
            PrivacyServiceArg::Contacts => "contacts",
            PrivacyServiceArg::Location => "location",
            PrivacyServiceArg::LocationAlways => "location-always",
            PrivacyServiceArg::PhotosAdd => "photos-add",
            PrivacyServiceArg::Photos => "photos",
            PrivacyServiceArg::MediaLibrary => "media-library",
            PrivacyServiceArg::Microphone => "microphone",
            PrivacyServiceArg::Motion => "motion",
            PrivacyServiceArg::Reminders => "reminders",
            PrivacyServiceArg::Siri => "siri",
        }
    }
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
        /// Skip retry, attempt tap once without waiting
        #[arg(long)]
        no_wait: bool,
        /// Timeout in milliseconds for retrying
        #[arg(short = 'o', long, default_value = "5000", env = "QORVEX_TIMEOUT")]
        timeout: u64,
        /// Annotate the action log entry with a free-text tag
        #[arg(long)]
        tag: Option<String>,
    },

    /// Tap at screen coordinates
    TapLocation {
        /// X coordinate
        x: i32,
        /// Y coordinate
        y: i32,
        /// Annotate the action log entry with a free-text tag
        #[arg(long)]
        tag: Option<String>,
    },

    /// Long press at screen coordinates
    LongPress {
        /// X coordinate
        x: i32,
        /// Y coordinate
        y: i32,
        /// Duration in seconds (default: 1.0)
        #[arg(long, short, default_value = "1.0")]
        duration: f64,
        /// Annotate the action log entry with a free-text tag
        #[arg(long)]
        tag: Option<String>,
    },

    /// Send keyboard input
    SendKeys {
        /// Text to type
        text: String,
        /// Annotate the action log entry with a free-text tag
        #[arg(long)]
        tag: Option<String>,
    },

    /// Capture a screenshot (outputs base64-encoded PNG)
    Screenshot {
        /// Annotate the action log entry with a free-text tag
        #[arg(long)]
        tag: Option<String>,
    },

    /// Get UI hierarchy information
    ScreenInfo {
        /// Output full raw JSON (original behavior)
        #[arg(long)]
        full: bool,
        /// Output REPL-style formatted list
        #[arg(long)]
        pretty: bool,
        /// Annotate the action log entry with a free-text tag
        #[arg(long)]
        tag: Option<String>,
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
        /// Skip retry, attempt get-value once without waiting
        #[arg(long)]
        no_wait: bool,
        /// Timeout in milliseconds for retrying
        #[arg(short = 'o', long, default_value = "5000", env = "QORVEX_TIMEOUT")]
        timeout: u64,
        /// Annotate the action log entry with a free-text tag
        #[arg(long)]
        tag: Option<String>,
    },

    /// Log a comment to the session
    Comment {
        /// The comment message
        message: String,
        /// Annotate the action log entry with a free-text tag
        #[arg(long)]
        tag: Option<String>,
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
        /// Annotate the action log entry with a free-text tag
        #[arg(long)]
        tag: Option<String>,
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
        /// Annotate the action log entry with a free-text tag
        #[arg(long)]
        tag: Option<String>,
    },

    /// Swipe the screen in a direction
    Swipe {
        /// Direction: up, down, left, right
        direction: String,
        /// Annotate the action log entry with a free-text tag
        #[arg(long)]
        tag: Option<String>,
    },

    /// Set the target application bundle ID
    SetTarget {
        /// Bundle identifier (e.g., com.example.MyApp)
        bundle_id: String,
        /// Annotate the action log entry with a free-text tag
        #[arg(long)]
        tag: Option<String>,
    },

    /// Launch the target application
    StartTarget {
        /// Terminate a running copy first so the app restarts from scratch.
        /// Without it an already-running app is left as-is and reported as
        /// "already running" rather than relaunched.
        #[arg(long)]
        force: bool,
    },

    /// Terminate the target application
    StopTarget,

    /// Get metadata about the target application
    TargetInfo,

    /// Report the target app's memory footprint and the device's memory state
    ///
    /// iOS simulator and Android only; physical iOS devices are not supported.
    MemoryInfo {
        /// Annotate the action log entry with a free-text tag
        #[arg(long)]
        tag: Option<String>,
    },

    /// Boot a device (simulator UDID for iOS, AVD name / adb serial for Android)
    BootDevice {
        /// Device UDID (iOS) or AVD name / adb serial (Android)
        udid: String,
        /// Target platform
        #[arg(long, value_enum, default_value_t = PlatformArg::Ios)]
        platform: PlatformArg,
    },

    /// List available devices (simulators for iOS, adb devices for Android)
    ListDevices {
        /// Target platform
        #[arg(long, value_enum, default_value_t = PlatformArg::Ios)]
        platform: PlatformArg,
    },

    /// List connected physical iOS devices
    #[command(name = "list-physical-devices")]
    ListPhysicalDevices,

    /// Select a device (simulator or physical) by UDID
    #[command(name = "use-device")]
    UseDevice {
        /// Device UDID
        udid: String,
    },

    /// Shut down the session's selected simulator
    ///
    /// Acts only on the device this session selected with use-device or
    /// boot-device; it takes no UDID, so it can never stop another session's
    /// simulator.
    #[command(name = "shutdown-device")]
    ShutdownDevice,

    /// Delete the session's selected simulator
    ///
    /// Acts only on the device this session selected, and on exactly one
    /// device. The selection is dropped once the simulator is gone.
    #[command(name = "delete-device")]
    DeleteDevice,

    /// Install an app bundle on the session's selected simulator
    #[command(name = "install-app")]
    InstallApp {
        /// Path to the .app bundle
        path: String,
    },

    /// Uninstall an app from the session's selected simulator
    #[command(name = "uninstall-app")]
    UninstallApp {
        /// Bundle identifier (e.g., com.example.MyApp)
        bundle_id: String,
    },

    /// Print the path of an app's container on the selected simulator
    #[command(name = "app-container")]
    AppContainer {
        /// Bundle identifier (e.g., com.example.MyApp)
        bundle_id: String,
        /// Which container to report (default: app)
        #[arg(value_enum)]
        container: Option<ContainerArg>,
    },

    /// List apps installed on the session's selected simulator
    #[command(name = "list-apps")]
    ListApps,

    /// Create a simulator and select it for this session
    ///
    /// The one device command that names a device: there is nothing to select
    /// until it has run. Device type and runtime take simctl identifiers or
    /// display names.
    #[command(name = "create-device")]
    CreateDevice {
        /// Name for the new simulator
        name: String,
        /// Device type (e.g. com.apple.CoreSimulator.SimDeviceType.iPhone-17)
        device_type: String,
        /// Runtime (e.g. com.apple.CoreSimulator.SimRuntime.iOS-26-5)
        runtime: String,
    },

    /// Wait until the session's selected simulator has finished booting
    #[command(name = "wait-for-boot")]
    WaitForBoot,

    /// Read recent entries from the selected simulator's unified log
    ///
    /// One-shot only — this wraps `log show`, not `log stream`.
    #[command(name = "device-log")]
    DeviceLog {
        /// How far back to read (log show --last syntax, e.g. 5m, 1h)
        #[arg(long, default_value = "5m")]
        last: String,
        /// NSPredicate to filter with (e.g. 'subsystem == "com.example.App"')
        #[arg(long)]
        predicate: Option<String>,
    },

    /// Set the selected simulator's light/dark appearance
    #[command(name = "set-appearance")]
    SetAppearance {
        /// Appearance to switch to
        #[arg(value_enum)]
        appearance: AppearanceArg,
    },

    /// Set the selected simulator's Dynamic Type content size
    #[command(name = "set-content-size")]
    SetContentSize {
        /// Content size to switch to
        #[arg(value_enum)]
        size: ContentSizeArg,
    },

    /// Change an app's access to a privacy-protected service
    #[command(name = "grant-permission")]
    GrantPermission {
        /// Whether to grant, revoke or reset access
        #[arg(value_enum)]
        verb: PrivacyVerbArg,
        /// The service whose access changes
        #[arg(value_enum)]
        service: PrivacyServiceArg,
        /// Bundle identifier (e.g., com.example.MyApp)
        bundle_id: String,
    },

    /// Add media files to the selected simulator's libraries
    #[command(name = "add-media")]
    AddMedia {
        /// Paths to the media files (relative paths are resolved client-side)
        #[arg(required = true)]
        files: Vec<String>,
    },

    /// Open a URL on the selected simulator
    #[command(name = "open-url")]
    OpenUrl {
        /// The URL to open (deep link or https address)
        url: String,
    },

    /// Stop a simulator's runaway `mediaanalysisd` daemon
    #[command(name = "quiet-device")]
    QuietDevice {
        /// Simulator UDID
        udid: String,
    },

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
    Start {
        /// Device UDID (simulator or physical) to use for this session
        #[arg(short, long)]
        device: Option<String>,
    },

    /// Start an automation session (auto-starts agent if configured)
    StartSession,

    /// Start or connect to the automation agent
    StartAgent {
        /// Path to the agent project directory
        #[arg(short, long)]
        project_dir: Option<String>,
        /// Target platform
        #[arg(long, value_enum, default_value_t = PlatformArg::Ios)]
        platform: PlatformArg,
    },

    /// Stop the managed automation agent (leaves the server running)
    StopAgent,

    /// Stop the server for this session
    Stop,

    /// Generate shell completion scripts
    Completions {
        /// Shell to generate completions for (zsh, bash, fish, elvish, powershell)
        shell: clap_complete::Shell,
    },
}

/// Restore the default `SIGPIPE` disposition.
///
/// The Rust runtime sets `SIGPIPE` to `SIG_IGN` at startup, which turns a write
/// to a closed pipe into an `EPIPE` error that the `print!`/`println!` macros
/// then unwrap into a panic. That happens whenever output is piped to a reader
/// that exits early (e.g. `qorvex screen-info | head`). Resetting to `SIG_DFL`
/// makes us terminate quietly on the signal like a well-behaved Unix tool.
#[cfg(unix)]
fn reset_sigpipe() {
    // SAFETY: `signal` with `SIG_DFL` on `SIGPIPE` is async-signal-safe and is
    // called once before any threads write to stdout.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn reset_sigpipe() {}

#[tokio::main]
async fn main() -> ExitCode {
    reset_sigpipe();

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

/// Finds running sessions, pairing each with the agent port recorded in its
/// `.port` sidecar. The port is `None` for a session started before sessions
/// got their own ports, or one whose sidecar is unreadable.
fn discover_sessions() -> Vec<(String, Option<u16>)> {
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
        .map(|name| {
            let port = std::fs::read_to_string(agent_port_path(&name))
                .ok()
                .and_then(|s| s.trim().parse().ok());
            (name, port)
        })
        .collect()
}

/// Connects to a session's IPC server, reporting the resolved socket path when
/// there is nothing to connect to.
///
/// The socket lives under `$QORVEX_HOME` (else `~/.qorvex`), so a script that
/// exports its own `HOME` looks in an empty directory and sees only "no such
/// file". Naming the path it tried turns that into a one-line diagnosis.
async fn connect_to_session(session: &str) -> Result<IpcClient, CliError> {
    let path = socket_path(session);
    IpcClient::connect(session).await.map_err(|e| {
        if !path.exists() {
            CliError::Connection(format!(
                "no qorvex session '{}' (socket not found at {}); run `qorvex start`",
                session,
                path.display()
            ))
        } else {
            CliError::Connection(format!(
                "Failed to connect to session '{}' at {}: {}",
                session,
                path.display(),
                e
            ))
        }
    })
}

async fn run(cli: Cli) -> Result<(), CliError> {
    // Handle commands that don't need an IPC connection
    match cli.command {
        Command::ListSessions => {
            let sessions = discover_sessions();
            if cli.format == OutputFormat::Json {
                let sessions: Vec<_> = sessions
                    .iter()
                    .map(|(name, port)| serde_json::json!({ "name": name, "agent_port": port }))
                    .collect();
                println!("{}", serde_json::json!({ "sessions": sessions }));
            } else {
                if sessions.is_empty() {
                    eprintln!("No running sessions found");
                } else {
                    for (session, port) in sessions {
                        match port {
                            Some(port) => println!("{} (agent port {})", session, port),
                            None => println!("{}", session),
                        }
                    }
                }
            }
            return Ok(());
        }
        Command::ListDevices { platform } => {
            match Platform::from(platform) {
                Platform::Ios => match Simctl::list_devices() {
                    Ok(devices) => {
                        if cli.format == OutputFormat::Json {
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&devices)
                                    .map_err(|e| CliError::Protocol(e.to_string()))?
                            );
                        } else if devices.is_empty() {
                            eprintln!("No simulator devices found");
                        } else {
                            for device in &devices {
                                let state = if device.state == "Booted" {
                                    " (Booted)"
                                } else {
                                    ""
                                };
                                println!("{} -- {}{}", device.udid, device.name, state);
                            }
                        }
                    }
                    Err(e) => {
                        return Err(CliError::ActionFailed(format!(
                            "Failed to list devices: {}",
                            e
                        )))
                    }
                },
                Platform::Android => match Adb::list_devices() {
                    Ok(devices) => {
                        if cli.format == OutputFormat::Json {
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&devices)
                                    .map_err(|e| CliError::Protocol(e.to_string()))?
                            );
                        } else if devices.is_empty() {
                            eprintln!("No Android devices found");
                        } else {
                            for device in &devices {
                                let model = device.model.as_deref().unwrap_or("");
                                println!("{} -- {} [{}]", device.serial, model, device.state);
                            }
                        }
                    }
                    Err(e) => {
                        return Err(CliError::ActionFailed(format!(
                            "Failed to list Android devices: {}",
                            e
                        )))
                    }
                },
            }
            return Ok(());
        }
        Command::BootDevice { ref udid, platform } => {
            match Platform::from(platform) {
                Platform::Ios => match Simctl::boot(udid) {
                    Ok(()) => {
                        if cli.format == OutputFormat::Json {
                            println!("{}", serde_json::json!({ "success": true, "udid": udid }));
                        } else {
                            eprintln!("Booted device {}", udid);
                        }
                    }
                    Err(e) => {
                        return Err(CliError::ActionFailed(format!(
                            "Failed to boot device: {}",
                            e
                        )))
                    }
                },
                Platform::Android => {
                    // Android boot routes through the server so the selected
                    // serial / lifecycle is tracked in session state.
                    let mut client = connect_to_session(&cli.session).await?;
                    return send_command(
                        &mut client,
                        IpcRequest::BootDevice {
                            udid: udid.clone(),
                            platform: Platform::Android,
                        },
                        &cli,
                    )
                    .await;
                }
            }
            return Ok(());
        }
        Command::QuietDevice { ref udid } => {
            // Called directly rather than over IPC: the path this serves — a
            // plain `simctl boot` in a build script — has no qorvex server.
            match Simctl::quiet(udid) {
                Ok(outcome) => {
                    if cli.format == OutputFormat::Json {
                        println!(
                            "{}",
                            serde_json::json!({
                                "success": true,
                                "udid": udid,
                                "outcome": format!("{:?}", outcome),
                            })
                        );
                    } else {
                        match outcome {
                            QuietOutcome::Unloaded => {
                                eprintln!("Quieted mediaanalysisd on {}", udid)
                            }
                            QuietOutcome::AlreadyQuiet => {
                                eprintln!("mediaanalysisd already quiet on {}", udid)
                            }
                            QuietOutcome::NotBooted => {
                                eprintln!("Device {} is not booted; nothing to quiet", udid)
                            }
                        }
                    }
                }
                Err(e) => {
                    return Err(CliError::ActionFailed(format!(
                        "Failed to quiet device: {}",
                        e
                    )))
                }
            }
            return Ok(());
        }
        Command::Convert { ref log } => {
            let result = match log {
                Some(path) => converter::LogConverter::convert_file(path)
                    .map_err(|e| CliError::ActionFailed(format!("Failed to convert log: {}", e))),
                None => converter::LogConverter::convert_stdin().map_err(|e| {
                    CliError::ActionFailed(format!("Failed to convert from stdin: {}", e))
                }),
            };
            match result {
                Ok(script) => {
                    print!("{}", script);
                    return Ok(());
                }
                Err(e) => return Err(e),
            }
        }
        Command::Start { ref device } => {
            return start_all(&cli, device.clone()).await;
        }
        Command::Completions { shell } => {
            use clap::CommandFactory;
            use clap_complete::generate;
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "qorvex", &mut std::io::stdout());
            return Ok(());
        }
        _ => {} // Fall through to IPC-connected commands
    }

    // Connect to the IPC server
    let mut client = connect_to_session(&cli.session).await?;

    match cli.command {
        Command::Tap {
            ref selector,
            label,
            ref element_type,
            no_wait,
            timeout,
            ref tag,
        } => {
            let timeout_ms = if no_wait { None } else { Some(timeout) };
            execute_action(
                &mut client,
                ActionType::Tap {
                    selector: selector.clone(),
                    by_label: label,
                    element_type: element_type.clone(),
                    timeout_ms,
                },
                tag.clone(),
                &cli,
            )
            .await
        }
        Command::TapLocation { x, y, ref tag } => {
            execute_action(
                &mut client,
                ActionType::TapLocation { x, y },
                tag.clone(),
                &cli,
            )
            .await
        }
        Command::LongPress {
            x,
            y,
            duration,
            ref tag,
        } => {
            execute_action(
                &mut client,
                ActionType::LongPress { x, y, duration },
                tag.clone(),
                &cli,
            )
            .await
        }
        Command::SendKeys { ref text, ref tag } => {
            execute_action(
                &mut client,
                ActionType::SendKeys { text: text.clone() },
                tag.clone(),
                &cli,
            )
            .await
        }
        Command::Screenshot { ref tag } => {
            execute_action(&mut client, ActionType::GetScreenshot, tag.clone(), &cli).await
        }
        Command::ScreenInfo {
            full,
            pretty,
            ref tag,
        } => execute_screen_info(&mut client, &cli, full, pretty, tag.clone()).await,
        Command::GetValue {
            ref selector,
            label,
            ref element_type,
            no_wait,
            timeout,
            ref tag,
        } => {
            let timeout_ms = if no_wait { None } else { Some(timeout) };
            execute_action(
                &mut client,
                ActionType::GetValue {
                    selector: selector.clone(),
                    by_label: label,
                    element_type: element_type.clone(),
                    timeout_ms,
                },
                tag.clone(),
                &cli,
            )
            .await
        }
        Command::Swipe {
            ref direction,
            ref tag,
        } => {
            execute_action(
                &mut client,
                ActionType::Swipe {
                    direction: direction.clone(),
                },
                tag.clone(),
                &cli,
            )
            .await
        }
        Command::SetTarget {
            ref bundle_id,
            ref tag,
        } => {
            execute_action(
                &mut client,
                ActionType::SetTarget {
                    bundle_id: bundle_id.clone(),
                },
                tag.clone(),
                &cli,
            )
            .await
        }
        Command::Comment {
            ref message,
            ref tag,
        } => {
            execute_action(
                &mut client,
                ActionType::LogComment {
                    message: message.clone(),
                },
                tag.clone(),
                &cli,
            )
            .await
        }
        Command::WaitFor {
            ref selector,
            label,
            ref element_type,
            timeout,
            ref tag,
        } => {
            execute_action(
                &mut client,
                ActionType::WaitFor {
                    selector: selector.clone(),
                    by_label: label,
                    element_type: element_type.clone(),
                    timeout_ms: timeout,
                    require_stable: true,
                },
                tag.clone(),
                &cli,
            )
            .await
        }
        Command::WaitForNot {
            ref selector,
            label,
            ref element_type,
            timeout,
            ref tag,
        } => {
            execute_action(
                &mut client,
                ActionType::WaitForNot {
                    selector: selector.clone(),
                    by_label: label,
                    element_type: element_type.clone(),
                    timeout_ms: timeout,
                },
                tag.clone(),
                &cli,
            )
            .await
        }
        Command::StartTarget { force } => execute_start_target(&mut client, &cli, force).await,
        Command::StopTarget => send_command(&mut client, IpcRequest::StopTarget, &cli).await,
        Command::TargetInfo => execute_target_info(&mut client, &cli).await,
        Command::MemoryInfo { ref tag } => {
            execute_memory_info(&mut client, &cli, tag.clone()).await
        }
        Command::StartSession => send_command(&mut client, IpcRequest::StartSession, &cli).await,
        Command::StartAgent {
            ref project_dir,
            platform,
        } => {
            send_command(
                &mut client,
                IpcRequest::StartAgent {
                    project_dir: project_dir.clone(),
                    platform: Platform::from(platform),
                    java_home: qorvex_core::android_lifecycle::client_java_home_override(),
                },
                &cli,
            )
            .await
        }
        Command::StopAgent => send_command(&mut client, IpcRequest::StopAgent, &cli).await,
        Command::Stop => stop_server(&mut client, &cli).await,
        Command::Status => get_status(&mut client, &cli).await,
        Command::Log => get_log(&mut client, &cli).await,
        Command::UseDevice { ref udid } => {
            send_command(
                &mut client,
                IpcRequest::UseDevice { udid: udid.clone() },
                &cli,
            )
            .await
        }
        Command::ListPhysicalDevices => list_physical_devices(&mut client, &cli).await,
        Command::ShutdownDevice => {
            send_command(&mut client, IpcRequest::ShutdownDevice, &cli).await
        }
        Command::DeleteDevice => send_command(&mut client, IpcRequest::DeleteDevice, &cli).await,
        Command::InstallApp { ref path } => {
            // The server is a long-lived daemon with its own working
            // directory, so a relative path has to be resolved here, in the
            // shell the user typed it in.
            let resolved = std::fs::canonicalize(path)
                .map_err(|e| CliError::ActionFailed(format!("{}: {}", path, e)))?;
            send_command(
                &mut client,
                IpcRequest::InstallApp {
                    path: resolved.to_string_lossy().to_string(),
                },
                &cli,
            )
            .await
        }
        Command::UninstallApp { ref bundle_id } => {
            send_command(
                &mut client,
                IpcRequest::UninstallApp {
                    bundle_id: bundle_id.clone(),
                },
                &cli,
            )
            .await
        }
        Command::AppContainer {
            ref bundle_id,
            container,
        } => {
            app_container(
                &mut client,
                &cli,
                bundle_id.clone(),
                container.map(|c| c.as_str().to_string()),
            )
            .await
        }
        Command::ListApps => list_apps(&mut client, &cli).await,
        Command::CreateDevice {
            ref name,
            ref device_type,
            ref runtime,
        } => {
            create_device(
                &mut client,
                &cli,
                name.clone(),
                device_type.clone(),
                runtime.clone(),
            )
            .await
        }
        Command::WaitForBoot => send_command(&mut client, IpcRequest::WaitForBoot, &cli).await,
        Command::DeviceLog {
            ref last,
            ref predicate,
        } => device_log(&mut client, &cli, last.clone(), predicate.clone()).await,
        Command::SetAppearance { appearance } => {
            send_command(
                &mut client,
                IpcRequest::SetAppearance {
                    appearance: appearance.as_str().to_string(),
                },
                &cli,
            )
            .await
        }
        Command::SetContentSize { size } => {
            send_command(
                &mut client,
                IpcRequest::SetContentSize {
                    size: size.as_str().to_string(),
                },
                &cli,
            )
            .await
        }
        Command::GrantPermission {
            verb,
            service,
            ref bundle_id,
        } => {
            send_command(
                &mut client,
                IpcRequest::GrantPermission {
                    verb: verb.as_str().to_string(),
                    service: service.as_str().to_string(),
                    bundle_id: bundle_id.clone(),
                },
                &cli,
            )
            .await
        }
        Command::AddMedia { ref files } => {
            // Same reason as install-app: the server is a long-lived daemon
            // with its own working directory, so relative paths are resolved
            // in the shell the user typed them in.
            let mut paths = Vec::with_capacity(files.len());
            for file in files {
                let resolved = std::fs::canonicalize(file)
                    .map_err(|e| CliError::ActionFailed(format!("{}: {}", file, e)))?;
                paths.push(resolved.to_string_lossy().to_string());
            }
            send_command(&mut client, IpcRequest::AddMedia { paths }, &cli).await
        }
        Command::OpenUrl { ref url } => {
            send_command(&mut client, IpcRequest::OpenUrl { url: url.clone() }, &cli).await
        }
        // These commands are handled before IPC connection above
        Command::ListSessions
        | Command::ListDevices { .. }
        | Command::BootDevice { .. }
        | Command::Convert { .. }
        | Command::QuietDevice { .. }
        | Command::Start { .. }
        | Command::Completions { .. } => unreachable!(),
    }
}

async fn execute_action(
    client: &mut IpcClient,
    action: ActionType,
    tag: Option<String>,
    cli: &Cli,
) -> Result<(), CliError> {
    let is_screenshot_action = matches!(action, ActionType::GetScreenshot);
    let is_data_action = matches!(
        action,
        ActionType::GetScreenInfo | ActionType::GetValue { .. }
    );
    // Config actions change server state instead of touching the screen, and
    // the server's reply carries the only report of what happened — e.g.
    // `set-target` appends "(recorded; no agent connected)" when it stored the
    // id without an agent to push it to. The `|ts|Action|target|dur|` trace
    // line below would drop that note, so print the message instead, the way
    // `execute_start_target` does.
    let is_config_action = matches!(action, ActionType::SetTarget { .. });
    let action_label = action.display_name();
    let action_target = action.display_target();
    let request = IpcRequest::Execute { action, tag };
    let response = client
        .send(&request)
        .await
        .map_err(|e| CliError::Protocol(format!("Failed to send request: {}", e)))?;

    match response {
        IpcResponse::ActionResult {
            success,
            message,
            screenshot,
            data,
        } => {
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
                        if is_config_action {
                            eprintln!("{}", message);
                        } else {
                            let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3fZ");
                            let duration_str = data
                                .as_ref()
                                .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
                                .and_then(|parsed| {
                                    parsed.get("elapsed_ms").and_then(|v| v.as_u64())
                                })
                                .map(|ms| format!("{}ms", ms))
                                .unwrap_or_default();
                            eprintln!(
                                "|{}|{}|{}|{}|",
                                now, action_label, action_target, duration_str
                            );
                        }
                    }
                } else {
                    return Err(CliError::ActionFailed(message));
                }
            }
            Ok(())
        }
        IpcResponse::Error { message } => Err(CliError::ActionFailed(message)),
        _ => Err(CliError::Protocol("Unexpected response type".to_string())),
    }
}

/// Sends `start-target` and reports whether the app really launched.
///
/// Unlike the other lifecycle commands this does not go through
/// [`send_command`]: the server answers with an `ActionResult` carrying a
/// `{launched, already_running, pid}` payload, so a script can branch on the
/// outcome (`--format json`) instead of parsing the human message.
async fn execute_start_target(
    client: &mut IpcClient,
    cli: &Cli,
    force: bool,
) -> Result<(), CliError> {
    let response = client
        .send(&IpcRequest::StartTarget { force })
        .await
        .map_err(|e| CliError::Protocol(format!("Failed to send request: {}", e)))?;

    match response {
        IpcResponse::ActionResult {
            success,
            message,
            data,
            ..
        } => {
            if !success {
                return Err(CliError::ActionFailed(message));
            }
            if cli.format == OutputFormat::Json {
                match data {
                    Some(ref d) => println!("{}", d),
                    None => println!("{}", serde_json::json!({ "message": message })),
                }
            } else if !cli.quiet {
                eprintln!("{}", message);
            }
            Ok(())
        }
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

async fn execute_target_info(client: &mut IpcClient, cli: &Cli) -> Result<(), CliError> {
    let response = client
        .send(&IpcRequest::GetTargetInfo)
        .await
        .map_err(|e| CliError::Protocol(format!("Failed to send request: {}", e)))?;

    match response {
        IpcResponse::ActionResult {
            success,
            message,
            data,
            ..
        } => {
            if !success {
                return Err(CliError::ActionFailed(message));
            }
            if cli.format == OutputFormat::Json {
                if let Some(ref d) = data {
                    println!("{}", d);
                }
            } else if let Some(ref d) = data {
                if let Ok(info) = serde_json::from_str::<serde_json::Value>(d) {
                    if let Some(bid) = info.get("bundle_id").and_then(|v| v.as_str()) {
                        println!("Bundle ID:    {}", bid);
                    }
                    if let Some(name) = info.get("display_name").and_then(|v| v.as_str()) {
                        if !name.is_empty() {
                            println!("Display Name: {}", name);
                        }
                    }
                    if let Some(ver) = info.get("version").and_then(|v| v.as_str()) {
                        if !ver.is_empty() {
                            println!("Version:      {}", ver);
                        }
                    }
                    if let Some(build) = info.get("build").and_then(|v| v.as_str()) {
                        if !build.is_empty() {
                            println!("Build:        {}", build);
                        }
                    }
                    if let Some(state) = info.get("state").and_then(|v| v.as_str()) {
                        println!("State:        {}", state);
                    }
                } else {
                    println!("{}", d);
                }
            }
            Ok(())
        }
        IpcResponse::CommandResult { success, message } => {
            if success {
                Ok(())
            } else {
                Err(CliError::ActionFailed(message))
            }
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
    tag: Option<String>,
) -> Result<(), CliError> {
    let request = IpcRequest::Execute {
        action: ActionType::GetScreenInfo,
        tag,
    };
    let response = client
        .send(&request)
        .await
        .map_err(|e| CliError::Protocol(format!("Failed to send request: {}", e)))?;

    match response {
        IpcResponse::ActionResult {
            success,
            message,
            data,
            ..
        } => {
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
        IpcResponse::State {
            session_id,
            screenshot,
        } => {
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
        IpcResponse::Error { message } => Err(CliError::ActionFailed(message)),
        _ => Err(CliError::Protocol("Unexpected response type".to_string())),
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
                        println!(
                            "[{}] {:?} - {:?}",
                            entry.timestamp.format("%H:%M:%S"),
                            entry.action,
                            entry.result
                        );
                    }
                }
            }
            Ok(())
        }
        IpcResponse::Error { message } => Err(CliError::ActionFailed(message)),
        _ => Err(CliError::Protocol("Unexpected response type".to_string())),
    }
}

async fn execute_memory_info(
    client: &mut IpcClient,
    cli: &Cli,
    tag: Option<String>,
) -> Result<(), CliError> {
    let response = client
        .send(&IpcRequest::GetMemoryInfo { tag })
        .await
        .map_err(|e| CliError::Protocol(format!("Failed to send request: {}", e)))?;

    match response {
        IpcResponse::ActionResult {
            success,
            message,
            data,
            ..
        } => {
            if !success {
                return Err(CliError::ActionFailed(message));
            }
            let Some(ref d) = data else {
                return Ok(());
            };
            if cli.format == OutputFormat::Json {
                println!("{}", d);
            } else {
                match serde_json::from_str::<MemoryInfo>(d) {
                    Ok(info) => print_memory_info(&info),
                    Err(_) => println!("{}", d),
                }
            }
            Ok(())
        }
        IpcResponse::CommandResult { success, message } => {
            if success {
                Ok(())
            } else {
                Err(CliError::ActionFailed(message))
            }
        }
        IpcResponse::Error { message } => Err(CliError::ActionFailed(message)),
        _ => Err(CliError::Protocol("Unexpected response type".to_string())),
    }
}

/// Human-readable rendering of a [`MemoryInfo`] report. Byte counts are shown
/// in MiB as well, because a bare nine-digit figure is unreadable.
fn print_memory_info(info: &MemoryInfo) {
    let mib = |bytes: u64| format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0));
    println!("App");
    println!("  PID:        {}", info.app.pid);
    println!(
        "  Footprint:  {} ({} bytes)",
        mib(info.app.footprint_bytes),
        info.app.footprint_bytes
    );
    println!("  Source:     {}", info.app.source);
    println!("Device");
    println!(
        "  Total:      {} ({} bytes)",
        mib(info.device.total_bytes),
        info.device.total_bytes
    );
    println!(
        "  Free:       {} ({} bytes)",
        mib(info.device.free_bytes),
        info.device.free_bytes
    );
    println!("  Pressure:   {:?}", info.device.pressure);
    println!("  Source:     {}", info.device.source);
}

async fn send_command(
    client: &mut IpcClient,
    request: IpcRequest,
    cli: &Cli,
) -> Result<(), CliError> {
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

/// True when `udid` names a known simulator in `simulators` (the simctl device
/// list). `start --device` also accepts physical iOS UDIDs and Android serials;
/// neither matches a simctl device, so both correctly read as non-simulator and
/// still get the foreground physical-device signing build.
fn is_known_simulator(udid: &str, simulators: &[qorvex_core::simctl::SimulatorDevice]) -> bool {
    simulators.iter().any(|d| d.udid == udid)
}

async fn start_all(cli: &Cli, device: Option<String>) -> Result<(), CliError> {
    use qorvex_core::config::QorvexConfig;
    use qorvex_core::ipc::socket_path;

    let sock = socket_path(&cli.session);

    // For physical devices that need signing, build the agent in the foreground
    // CLI process so interactive prompts (keychain, Apple ID) work properly.
    // The server runs in the background with stdio redirected, so it can't
    // handle interactive prompts.
    //
    // `start --device <udid>` also accepts simulator UDIDs (and Android
    // serials). The build below is a *physical iOS* signing build
    // (build-for-testing for iphoneos + codesign); running it for a simulator
    // is wrong and pops Apple ID / provisioning prompts for no reason. Resolve
    // sim-vs-physical here — using the same authoritative source as the server
    // (`simctl`) — and only sign-build for a non-simulator target. A simctl
    // failure yields an empty list, so an iOS UDID we can't classify still gets
    // the signing build (preserving the physical-device path).
    let run_signing_build = match device.as_deref() {
        Some(udid) => !is_known_simulator(udid, &Simctl::list_devices().unwrap_or_default()),
        None => false,
    };
    if run_signing_build {
        let config = QorvexConfig::load();
        // Only pre-build here when a team is configured. `run_signing_build` is
        // a heuristic — it is also true for Android serials and for any UDID
        // when `simctl` fails — so a missing team must not abort the start.
        // The authoritative refusal lives in `AgentLifecycle::build_args`, which
        // the server reaches only once it knows the target really is a physical
        // iOS device.
        if let (Some(ref agent_dir), Some(ref team)) = (
            &config.effective_agent_source_dir(),
            &config.development_team,
        ) {
            let xcodeproj = agent_dir.join("QorvexAgent.xcodeproj");
            if xcodeproj.exists() {
                let derived = agent_dir.join(".build");
                let xcodeproj_str = xcodeproj.to_string_lossy().to_string();
                let derived_str = derived.to_string_lossy().to_string();
                if !cli.quiet {
                    eprintln!("Building agent for physical device...");
                }
                let team_arg = format!("DEVELOPMENT_TEAM={}", team);
                // Override the project's bundle-ID variable rather than
                // PRODUCT_BUNDLE_IDENTIFIER: a command-line build setting applies
                // to every target, which would collide the app and its UI-test
                // runner on one identifier.
                let bid_arg = config
                    .agent_bundle_id
                    .as_ref()
                    .map(|bid| format!("QORVEX_AGENT_BUNDLE_ID={}", bid));
                let mut args = vec![
                    "build-for-testing",
                    "-project",
                    &xcodeproj_str,
                    "-scheme",
                    "QorvexAgentUITests",
                    "-destination",
                    "generic/platform=iOS",
                    "-derivedDataPath",
                    &derived_str,
                    "-allowProvisioningUpdates",
                    &team_arg,
                    "CODE_SIGN_STYLE=Automatic",
                    "CODE_SIGN_IDENTITY=Apple Development",
                    "CODE_SIGNING_ALLOWED=YES",
                    "CODE_SIGNING_REQUIRED=YES",
                ];
                if let Some(ref ba) = bid_arg {
                    args.push(ba);
                }
                let status = std::process::Command::new("xcodebuild")
                    .args(&args)
                    .stdout(if cli.quiet {
                        std::process::Stdio::null()
                    } else {
                        std::process::Stdio::inherit()
                    })
                    .stderr(std::process::Stdio::inherit())
                    .stdin(std::process::Stdio::inherit())
                    .status()
                    .map_err(|e| {
                        CliError::ActionFailed(format!("Failed to run xcodebuild: {}", e))
                    })?;
                if !status.success() {
                    return Err(CliError::ActionFailed(
                        "Agent build failed (see output above)".to_string(),
                    ));
                }
            }
        }
    }

    // Start server if not already running
    if !sock.exists() {
        let log_dir = qorvex_core::session::logs_dir();
        let log_file = std::fs::File::create(log_dir.join("qorvex-server-launch.log")).ok();

        let mut cmd = std::process::Command::new("qorvex-server");
        cmd.args(["-s", &cli.session]);
        if let Some(f) = log_file {
            cmd.stdout(
                f.try_clone()
                    .unwrap_or_else(|_| std::fs::File::create("/dev/null").unwrap()),
            );
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
            return Err(CliError::Connection(
                "Server did not start in time".to_string(),
            ));
        }
    }

    // Connect and start session
    let mut client = IpcClient::connect(&cli.session)
        .await
        .map_err(|e| CliError::Connection(format!("Failed to connect: {}", e)))?;

    // Select device before starting session so agent auto-start uses the right connection mode
    if let Some(ref udid) = device {
        let response = client
            .send(&IpcRequest::UseDevice { udid: udid.clone() })
            .await
            .map_err(|e| CliError::Protocol(format!("Failed to select device: {}", e)))?;

        match response {
            IpcResponse::CommandResult { success, message } => {
                if !success {
                    return Err(CliError::ActionFailed(message));
                }
                if !cli.quiet {
                    eprintln!("{}", message);
                }
            }
            IpcResponse::Error { message } => return Err(CliError::ActionFailed(message)),
            _ => return Err(CliError::Protocol("Unexpected response".to_string())),
        }
    }

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

async fn list_physical_devices(client: &mut IpcClient, cli: &Cli) -> Result<(), CliError> {
    let response = client
        .send(&IpcRequest::ListPhysicalDevices)
        .await
        .map_err(|e| CliError::Protocol(format!("Failed to send request: {}", e)))?;

    match response {
        IpcResponse::PhysicalDeviceList { devices } => {
            if cli.format == OutputFormat::Json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&devices)
                        .map_err(|e| CliError::Protocol(e.to_string()))?
                );
            } else {
                if devices.is_empty() {
                    eprintln!("No physical devices found");
                } else {
                    for device in &devices {
                        let name = device.name.as_deref().unwrap_or("Unknown");
                        println!("{} -- {} ({})", device.udid, name, device.connection);
                    }
                }
            }
            Ok(())
        }
        IpcResponse::Error { message } => Err(CliError::ActionFailed(message)),
        _ => Err(CliError::Protocol("Unexpected response type".to_string())),
    }
}

/// Print the path of an app container on the selected simulator.
async fn app_container(
    client: &mut IpcClient,
    cli: &Cli,
    bundle_id: String,
    container: Option<String>,
) -> Result<(), CliError> {
    let response = client
        .send(&IpcRequest::AppContainer {
            bundle_id,
            container,
        })
        .await
        .map_err(|e| CliError::Protocol(format!("Failed to send request: {}", e)))?;

    match response {
        IpcResponse::AppContainer { path } => {
            if cli.format == OutputFormat::Json {
                println!("{}", serde_json::json!({ "path": path }));
            } else {
                println!("{}", path);
            }
            Ok(())
        }
        IpcResponse::CommandResult { message, .. } => Err(CliError::ActionFailed(message)),
        IpcResponse::Error { message } => Err(CliError::ActionFailed(message)),
        _ => Err(CliError::Protocol("Unexpected response type".to_string())),
    }
}

/// List the apps installed on the selected simulator.
async fn list_apps(client: &mut IpcClient, cli: &Cli) -> Result<(), CliError> {
    let response = client
        .send(&IpcRequest::ListApps)
        .await
        .map_err(|e| CliError::Protocol(format!("Failed to send request: {}", e)))?;

    match response {
        IpcResponse::AppList { apps } => {
            if cli.format == OutputFormat::Json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&apps)
                        .map_err(|e| CliError::Protocol(e.to_string()))?
                );
            } else if apps.is_empty() {
                eprintln!("No apps installed");
            } else {
                for app in &apps {
                    println!(
                        "{} -- {} ({})",
                        app.bundle_id, app.display_name, app.app_type
                    );
                }
            }
            Ok(())
        }
        IpcResponse::CommandResult { message, .. } => Err(CliError::ActionFailed(message)),
        IpcResponse::Error { message } => Err(CliError::ActionFailed(message)),
        _ => Err(CliError::Protocol("Unexpected response type".to_string())),
    }
}

/// Create a simulator and print the UDID it was given.
async fn create_device(
    client: &mut IpcClient,
    cli: &Cli,
    name: String,
    device_type: String,
    runtime: String,
) -> Result<(), CliError> {
    let response = client
        .send(&IpcRequest::CreateDevice {
            name,
            device_type,
            runtime,
        })
        .await
        .map_err(|e| CliError::Protocol(format!("Failed to send request: {}", e)))?;

    match response {
        IpcResponse::CreatedDevice { udid } => {
            if cli.format == OutputFormat::Json {
                println!("{}", serde_json::json!({ "udid": udid }));
            } else {
                println!("{}", udid);
            }
            Ok(())
        }
        IpcResponse::CommandResult { message, .. } => Err(CliError::ActionFailed(message)),
        IpcResponse::Error { message } => Err(CliError::ActionFailed(message)),
        _ => Err(CliError::Protocol("Unexpected response type".to_string())),
    }
}

/// Print recent unified-log output from the selected simulator.
async fn device_log(
    client: &mut IpcClient,
    cli: &Cli,
    last: String,
    predicate: Option<String>,
) -> Result<(), CliError> {
    let response = client
        .send(&IpcRequest::DeviceLog { last, predicate })
        .await
        .map_err(|e| CliError::Protocol(format!("Failed to send request: {}", e)))?;

    match response {
        IpcResponse::DeviceLog { log } => {
            if cli.format == OutputFormat::Json {
                println!("{}", serde_json::json!({ "log": log }));
            } else {
                print!("{}", log);
            }
            Ok(())
        }
        IpcResponse::CommandResult { message, .. } => Err(CliError::ActionFailed(message)),
        IpcResponse::Error { message } => Err(CliError::ActionFailed(message)),
        _ => Err(CliError::Protocol("Unexpected response type".to_string())),
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
        _ => Err(CliError::Protocol(
            "Unexpected response to Shutdown".to_string(),
        )),
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
                discovered.iter().any(|(name, _)| name == session_name),
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

    /// `list-sessions` reports each session's agent port, and tolerates a
    /// session whose sidecar is missing (started before per-session ports).
    #[test]
    fn test_discover_sessions_reads_agent_port() {
        let qorvex_dir = qorvex_dir();
        fs::create_dir_all(&qorvex_dir).expect("Failed to create qorvex directory");

        let with_port = "test_session_with_port";
        let without_port = "test_session_without_port";
        let sockets: Vec<_> = [with_port, without_port]
            .iter()
            .map(|name| {
                let path = qorvex_dir.join(format!("qorvex_{}.sock", name));
                File::create(&path).expect("Failed to create test socket file");
                path
            })
            .collect();
        let port_file = qorvex_dir.join(format!("qorvex_{}.port", with_port));
        fs::write(&port_file, "51234\n").expect("Failed to write test port file");

        let discovered = discover_sessions();
        assert_eq!(
            discovered
                .iter()
                .find(|(name, _)| name == with_port)
                .map(|(_, port)| *port),
            Some(Some(51234))
        );
        assert_eq!(
            discovered
                .iter()
                .find(|(name, _)| name == without_port)
                .map(|(_, port)| *port),
            Some(None),
            "a session with no sidecar is still listed"
        );

        for path in sockets {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_file(port_file);
    }

    fn sim(udid: &str) -> qorvex_core::simctl::SimulatorDevice {
        qorvex_core::simctl::SimulatorDevice {
            udid: udid.to_string(),
            name: "iPhone 15 Pro".to_string(),
            state: "Booted".to_string(),
            device_type: None,
        }
    }

    #[test]
    fn simulator_target_is_recognized() {
        let sims = vec![sim("SIM-AAAA-1111"), sim("SIM-BBBB-2222")];
        // A UDID present in the simctl list is a simulator -> no signing build.
        assert!(is_known_simulator("SIM-BBBB-2222", &sims));
    }

    #[test]
    fn physical_and_android_targets_are_not_simulators() {
        let sims = vec![sim("SIM-AAAA-1111")];
        // Physical iOS UDID (40-hex) is absent from simctl -> signing build runs.
        assert!(!is_known_simulator(
            "00008140abcdef0123456789abcdef0123456789",
            &sims
        ));
        // Android serial is likewise absent -> not a simulator.
        assert!(!is_known_simulator("emulator-5554", &sims));
        // Empty simctl list (e.g. simctl failed) -> nothing is a simulator.
        assert!(!is_known_simulator("SIM-AAAA-1111", &[]));
    }
}
