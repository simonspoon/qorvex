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
mod doctor;
mod lease;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use qorvex_core::action::{
    parse_coordinate, parse_max_swipes, parse_swipe_duration_ms, ActionLog, ActionType, LabelMatch,
    LabelMatcher, ScrollDirection, DEFAULT_MAX_SWIPES,
};
use qorvex_core::adb_device::Adb;
use qorvex_core::driver::CompiledLabelMatch;
use qorvex_core::element::{ElementFrame, UIElement};
use qorvex_core::ipc::{
    agent_port_path, qorvex_dir, socket_path, IpcClient, IpcRequest, IpcResponse, Platform,
};
use qorvex_core::memory::MemoryInfo;
use qorvex_core::protocol::KeyName;
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

    /// Milliseconds. On tap, get-value, wait-for, wait-for-not and
    /// wait-stable: how long to wait for the element or a stable tree
    /// (default 5000, env QORVEX_TIMEOUT). On every other command: hard
    /// deadline for the whole process (default 600000, env
    /// QORVEX_COMMAND_TIMEOUT, 0 = no deadline); exits 124 when it passes
    #[arg(short = 'o', long, global = true)]
    timeout: Option<u64>,

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

/// System key for `qorvex key` (CLI-facing; maps to
/// [`qorvex_core::protocol::KeyName`]).
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum KeyArg {
    /// Back (Android only)
    Back,
    /// Home
    Home,
}

impl From<KeyArg> for KeyName {
    fn from(k: KeyArg) -> Self {
        match k {
            KeyArg::Back => KeyName::Back,
            KeyArg::Home => KeyName::Home,
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

#[derive(clap::Args)]
struct LeaseArgs {
    /// Device type name (e.g. "iPhone Air")
    #[arg(long)]
    model: Option<String>,
    /// Who holds the lease (used by `release --owner`)
    #[arg(long)]
    owner: Option<String>,
    /// Runtime identifier for a new golden device (default: newest iOS)
    #[arg(long)]
    runtime: Option<String>,
    /// Lease time-to-live without a heartbeat (<n>s|m|h|d)
    #[arg(long, default_value = "4h")]
    ttl: String,
}

#[derive(Subcommand)]
enum LeaseAction {
    /// Extend a lease by resetting its heartbeat
    Heartbeat {
        /// UDID of the leased simulator
        udid: String,
    },
    /// List leases with age, heartbeat and expiry
    List,
}

#[derive(Subcommand)]
enum SimsAction {
    /// Find (and with --yes delete) simulators that only waste disk
    ///
    /// Dry run by default. Candidates: unavailable simulators, expired and
    /// orphaned leases, and shut-down simulators idle for --older-than.
    /// Booted simulators and golden devices are never touched.
    Reclaim {
        /// Idle age after which a shut-down simulator is a candidate (<n>s|m|h|d, default 14d)
        #[arg(long)]
        older_than: Option<String>,
        /// Delete the candidates instead of listing them
        #[arg(long)]
        yes: bool,
    },
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
        /// Tap only if the element is in the tree right now; otherwise skip
        /// (exit 0). One lookup, then a single tap attempt like --no-wait.
        #[arg(long)]
        if_present: bool,
        /// If the tap fails but the element is in the tree with a non-empty
        /// frame, tap its frame centre once (may hit a covering view)
        #[arg(long)]
        fallback_frame: bool,
        /// Report whether the UI changed (two extra tree dumps, up to ~1.2 s);
        /// always on with -f json
        #[arg(long)]
        report: bool,
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

    /// Send keyboard input to the focused field (appends by default; empty text is a no-op)
    SendKeys {
        /// Text to type
        text: String,
        /// Type after the existing content (default)
        #[arg(long, conflicts_with = "replace")]
        append: bool,
        /// Make the field hold exactly <TEXT> afterwards (`--replace ""` clears it)
        #[arg(long)]
        replace: bool,
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
        /// List every element in the tree (also unnamed, offscreen, zero-size),
        /// each with `offscreen`, `zero_size` and `hittable` flags
        #[arg(long, conflicts_with = "full")]
        all: bool,
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
        /// Annotate the action log entry with a free-text tag
        #[arg(long)]
        tag: Option<String>,
    },

    /// Check whether an element is in the accessibility tree right now
    ///
    /// One lookup, no waiting. Exit 0 = present, 4 = absent; errors keep
    /// their usual exit codes. "Present" means in the tree, not visible.
    Exists {
        /// The selector (accessibility ID or label)
        #[arg(required_unless_present = "label_match")]
        selector: Option<String>,
        /// Match by accessibility label instead of ID
        #[arg(short, long)]
        label: bool,
        /// Filter by element type (e.g., Button, TextField)
        #[arg(short = 'T', long = "type")]
        element_type: Option<String>,
        #[command(flatten)]
        label_match: LabelMatchArgs,
        /// Annotate the action log entry with a free-text tag
        #[arg(long)]
        tag: Option<String>,
    },

    /// Swipe until an element is in the accessibility tree
    ///
    /// Checks first (like `exists`), then swipes and re-checks, up to
    /// --max-swipes times. Exit 0 = found, 4 = not found after the last
    /// swipe; errors keep their usual exit codes. "Found" means in the tree,
    /// not on screen: an offscreen row of a virtualised list can count.
    ScrollTo {
        /// The selector (accessibility ID or label)
        #[arg(required_unless_present = "label_match")]
        selector: Option<String>,
        /// Match by accessibility label instead of ID
        #[arg(short, long)]
        label: bool,
        /// Filter by element type (e.g., Button, TextField)
        #[arg(short = 'T', long = "type")]
        element_type: Option<String>,
        #[command(flatten)]
        label_match: LabelMatchArgs,
        /// Finger swipe direction (up scrolls content down)
        #[arg(long, value_parser = clap::value_parser!(ScrollDirection), default_value = "up")]
        direction: ScrollDirection,
        /// Maximum number of swipes, 1-50
        #[arg(long, value_name = "N", value_parser = parse_max_swipes, default_value_t = DEFAULT_MAX_SWIPES)]
        max_swipes: u32,
        /// Swipe duration in milliseconds, 1-10000 (default 300)
        #[arg(long = "duration-ms", value_name = "MS", value_parser = parse_swipe_duration_ms)]
        duration_ms: Option<u32>,
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
        #[arg(required_unless_present = "label_match")]
        selector: Option<String>,
        /// Match by accessibility label instead of ID
        #[arg(short, long)]
        label: bool,
        /// Filter by element type (e.g., Button, TextField)
        #[arg(short = 'T', long = "type")]
        element_type: Option<String>,
        #[command(flatten)]
        label_match: LabelMatchArgs,
        /// Annotate the action log entry with a free-text tag
        #[arg(long)]
        tag: Option<String>,
    },

    /// Wait for an element to disappear by ID or label
    WaitForNot {
        /// The selector (accessibility ID or label)
        #[arg(required_unless_present = "label_match")]
        selector: Option<String>,
        /// Match by accessibility label instead of ID
        #[arg(short, long)]
        label: bool,
        /// Filter by element type (e.g., Button, TextField)
        #[arg(short = 'T', long = "type")]
        element_type: Option<String>,
        #[command(flatten)]
        label_match: LabelMatchArgs,
        /// Annotate the action log entry with a free-text tag
        #[arg(long)]
        tag: Option<String>,
    },

    /// Wait until the whole UI tree is unchanged for a quiet period
    WaitStable {
        /// Quiet period in milliseconds: the tree must stay unchanged this long
        #[arg(long = "for", value_name = "MS", default_value_t = 500)]
        stable_ms: u64,
        /// Annotate the action log entry with a free-text tag
        #[arg(long)]
        tag: Option<String>,
    },

    /// Swipe the screen in a direction, between two coordinates, or from an element
    ///
    /// With --from-element (or --label-contains/--label-regex) the swipe
    /// starts at the centre of that element's frame and moves the same
    /// distance as a plain direction swipe. A virtualised row can sit off
    /// screen; the swipe then starts there.
    #[command(group(clap::ArgGroup::new("swipe_mode").required(true).args(["direction", "from"])))]
    #[command(group(clap::ArgGroup::new("element_mode").multiple(true).args(["selector", "label_contains", "label_regex"])))]
    Swipe {
        /// Direction: up, down, left, right
        #[arg(conflicts_with_all = ["from", "to"])]
        direction: Option<String>,
        /// Start point as x,y (requires --to)
        #[arg(long, value_name = "X,Y", value_parser = parse_coordinate, requires = "to", allow_hyphen_values = true, conflicts_with_all = ["selector", "label_contains", "label_regex"])]
        from: Option<(i32, i32)>,
        /// End point as x,y (requires --from)
        #[arg(long, value_name = "X,Y", value_parser = parse_coordinate, requires = "from", allow_hyphen_values = true)]
        to: Option<(i32, i32)>,
        /// Start the swipe at the centre of this element (accessibility ID or label)
        #[arg(long = "from-element", value_name = "SELECTOR", requires = "direction")]
        selector: Option<String>,
        /// Match the --from-element selector by accessibility label instead of ID
        #[arg(short, long, requires = "selector")]
        label: bool,
        /// Filter the --from-element match by element type (e.g., Button)
        #[arg(short = 'T', long = "type", requires = "element_mode")]
        element_type: Option<String>,
        #[command(flatten)]
        label_match: LabelMatchArgs,
        /// Swipe duration in milliseconds, 1-10000 (default 300)
        #[arg(long = "duration-ms", value_name = "MS", value_parser = parse_swipe_duration_ms)]
        duration_ms: Option<u32>,
        /// Annotate the action log entry with a free-text tag
        #[arg(long)]
        tag: Option<String>,
    },

    /// Lock the selected device (press the lock button)
    Lock {
        /// Annotate the action log entry with a free-text tag
        #[arg(long)]
        tag: Option<String>,
    },

    /// Press a system key: back (Android only) or home
    Key {
        /// Key to press
        #[arg(value_enum)]
        name: KeyArg,
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

    /// Bring the running target application to the front without relaunching
    /// it (state and pid are kept). Fails if the app is not running; use
    /// start-target for that.
    ForegroundTarget,

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

    /// Check this machine's Android setup (adb, SDK, JDK, agent project, devices)
    Doctor {
        /// Target platform (`ios` has no checks yet and exits 0)
        #[arg(long, value_enum, default_value_t = PlatformArg::Android)]
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

    /// Lease a throwaway simulator cloned from a golden device
    ///
    /// Prints the UDID, then `QORVEX_DEVICE=<udid>`. Give it back with
    /// `qorvex release`; `qorvex reap` deletes leases that stopped heartbeating.
    #[command(args_conflicts_with_subcommands = true)]
    Lease {
        #[command(flatten)]
        args: LeaseArgs,
        #[command(subcommand)]
        action: Option<LeaseAction>,
    },

    /// Delete a leased simulator and its lease (by UDID or owner)
    ///
    /// Refuses any device not named qorvex-lease-*. Idempotent.
    #[command(group(clap::ArgGroup::new("target").required(true).args(["udid", "owner"])))]
    Release {
        /// UDID of the leased simulator
        udid: Option<String>,
        /// Release every lease held by this owner
        #[arg(long)]
        owner: Option<String>,
    },

    /// Delete leases past their TTL and orphaned qorvex-lease-* simulators
    Reap {
        /// Print what would be deleted without deleting it
        #[arg(long)]
        dry_run: bool,
    },

    /// Simulator housekeeping
    Sims {
        #[command(subcommand)]
        action: SimsAction,
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

        /// Exit 1 unless the session is ready (device selected, agent answering)
        #[arg(long)]
        wait_ready: bool,
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

/// When this command started; `-f json` objects report the time since then.
static COMMAND_STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Adds `"duration_ms"` (wall-clock milliseconds since this command started)
/// as the last key of a `-f json` object. Anything else comes back unchanged.
fn with_duration_ms(json: &str) -> String {
    let ms = COMMAND_STARTED
        .get()
        .map_or(0, |started| started.elapsed().as_millis() as u64);
    insert_duration_ms(json, ms)
}

/// Appends `"duration_ms": ms` to a JSON object's text, keeping its existing
/// keys, their order and its compact/pretty layout. Non-objects (arrays, plain
/// text) and objects that already carry `duration_ms` are returned unchanged.
fn insert_duration_ms(json: &str, ms: u64) -> String {
    let trimmed = json.trim_end();
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(serde_json::Value::Object(map)) if !map.contains_key("duration_ms") => {
            let body = trimmed[..trimmed.len() - 1].trim_end();
            let sep = if map.is_empty() { "" } else { "," };
            if trimmed.contains('\n') {
                format!("{}{}\n  \"duration_ms\": {}\n}}", body, sep, ms)
            } else {
                format!("{}{}\"duration_ms\":{}}}", body, sep, ms)
            }
        }
        _ => json.to_string(),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let _ = COMMAND_STARTED.set(std::time::Instant::now());
    reset_sigpipe();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let matches = Cli::command().get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    let screen_info_json =
        matches!(cli.command, Command::ScreenInfo { .. }) && cli.format == OutputFormat::Json;

    // Validate both env vars before anything runs (usage error, exit 2).
    let is_wait = is_wait_command(&cli.command);
    let element = if is_wait {
        match element_timeout_checked(cli.timeout, std::env::var("QORVEX_TIMEOUT").ok()) {
            Ok(ms) => Some(ms),
            Err(msg) => Cli::command()
                .error(clap::error::ErrorKind::ValueValidation, msg)
                .exit(),
        }
    } else {
        None
    };
    if let (Command::WaitStable { stable_ms, .. }, Some(timeout)) = (&cli.command, element) {
        if let Err(msg) = check_wait_stable(*stable_ms, timeout) {
            Cli::command()
                .error(clap::error::ErrorKind::ArgumentConflict, msg)
                .exit();
        }
    }
    let env_deadline = match parse_env_ms("QORVEX_COMMAND_TIMEOUT") {
        Ok(v) => v,
        Err(msg) => Cli::command()
            .error(clap::error::ErrorKind::ValueValidation, msg)
            .exit(),
    };
    // `--no-wait` forwards no element timeout, so it does not stretch the deadline.
    let forwarded = match cli.command {
        Command::Tap { no_wait: true, .. }
        | Command::Tap {
            if_present: true, ..
        }
        | Command::GetValue { no_wait: true, .. } => None,
        _ => element,
    };
    if let Some(deadline) = command_deadline(is_wait, forwarded, cli.timeout, env_deadline) {
        arm_watchdog(deadline, subcommand_name(&matches), screen_info_json);
    }

    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if screen_info_json {
                println!(
                    "{}",
                    with_duration_ms(&screen_info_error_json(&e).to_string())
                );
            }
            if !matches!(e, CliError::Absent) {
                eprintln!("Error: {}", e);
            }
            e.exit_code()
        }
    }
}

/// Default hard deadline for a command, in milliseconds.
const DEFAULT_DEADLINE_MS: u64 = 600_000;
/// Margin over the element timeout so the server's own timeout message arrives first.
const ELEMENT_DEADLINE_MARGIN_MS: u64 = 20_000;

/// Phase reached by `start` (0 = none), reported when the deadline fires.
static START_PHASE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn set_start_phase(phase: u8) {
    START_PHASE.store(phase, std::sync::atomic::Ordering::SeqCst);
}

fn start_phase_name(phase: u8) -> Option<&'static str> {
    match phase {
        1 => Some("building agent"),
        2 => Some("waiting for server"),
        3 => Some("selecting device"),
        4 => Some("starting session"),
        _ => None,
    }
}

/// True for the commands where `-o/--timeout` is an element timeout.
fn is_wait_command(command: &Command) -> bool {
    matches!(
        command,
        Command::Tap { .. }
            | Command::GetValue { .. }
            | Command::WaitFor { .. }
            | Command::WaitForNot { .. }
            | Command::WaitStable { .. }
    )
}

/// Element timeout for the wait commands: flag, else `QORVEX_TIMEOUT`, else 5000.
fn element_timeout_checked(flag: Option<u64>, env: Option<String>) -> Result<u64, String> {
    if let Some(ms) = flag {
        return Ok(ms);
    }
    match env {
        Some(v) => v.trim().parse().map_err(|_| {
            format!(
                "invalid value '{}' for QORVEX_TIMEOUT: expected milliseconds",
                v
            )
        }),
        None => Ok(5000),
    }
}

/// `wait-stable` can never succeed when the quiet period exceeds the timeout.
fn check_wait_stable(stable_ms: u64, timeout_ms: u64) -> Result<(), String> {
    if stable_ms > timeout_ms {
        return Err(format!(
            "--for {}ms (quiet period, default 500) is longer than the timeout ({}ms, -o/--timeout or QORVEX_TIMEOUT); raise the timeout or lower --for",
            stable_ms, timeout_ms
        ));
    }
    Ok(())
}

/// Element timeout of a wait command (the env var was validated in `main`).
fn element_timeout(cli: &Cli) -> u64 {
    element_timeout_checked(cli.timeout, std::env::var("QORVEX_TIMEOUT").ok()).unwrap_or(5000)
}

fn parse_env_ms(name: &str) -> Result<Option<u64>, String> {
    match std::env::var(name) {
        Ok(v) => v
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| format!("invalid value '{}' for {}: expected milliseconds", v, name)),
        Err(_) => Ok(None),
    }
}

/// Hard deadline for the whole process, `None` when disabled.
///
/// `forwarded_element` is the element timeout sent to the server (wait commands
/// without `--no-wait`). On the wait commands the flag is that element timeout,
/// never the deadline.
fn command_deadline(
    is_wait: bool,
    forwarded_element: Option<u64>,
    flag: Option<u64>,
    env: Option<u64>,
) -> Option<std::time::Duration> {
    let base = if is_wait { env } else { flag.or(env) }.unwrap_or(DEFAULT_DEADLINE_MS);
    if base == 0 {
        return None;
    }
    let ms = match forwarded_element {
        Some(e) => base.max(e.saturating_add(ELEMENT_DEADLINE_MARGIN_MS)),
        None => base,
    };
    Some(std::time::Duration::from_millis(ms))
}

/// User-facing subcommand path, e.g. `lease heartbeat`.
fn subcommand_name(matches: &clap::ArgMatches) -> String {
    let mut names = Vec::new();
    let mut m = matches;
    while let Some((name, sub)) = m.subcommand() {
        names.push(name);
        m = sub;
    }
    names.join(" ")
}

/// Ends the process with exit 124 once `deadline` passes, even if the main
/// thread is blocked in synchronous work.
fn arm_watchdog(deadline: std::time::Duration, name: String, screen_info_json: bool) {
    use std::io::Write;
    std::thread::spawn(move || {
        std::thread::sleep(deadline);
        let phase = start_phase_name(START_PHASE.load(std::sync::atomic::Ordering::SeqCst))
            .map(|p| format!(" ({})", p))
            .unwrap_or_default();
        let message = format!(
            "Timed out after {}ms: {}{}; the session server may still be working (check `qorvex status`)",
            deadline.as_millis(),
            name,
            phase
        );
        // Write errors are ignored (no `println!`, which panics on them) so
        // nothing can keep the process from exiting.
        let _ = writeln!(std::io::stderr(), "Error: {}", message);
        if screen_info_json {
            let _ = writeln!(
                std::io::stdout(),
                "{}",
                with_duration_ms(
                    &serde_json::json!({ "success": false, "message": message }).to_string()
                )
            );
        }
        std::process::exit(124);
    });
}

/// Label text-match flags of `exists`, `wait-for` and `wait-for-not`; they
/// replace the positional selector.
#[derive(clap::Args, Debug, Clone, Default)]
#[command(group(clap::ArgGroup::new("label_match").args(["label_contains", "label_regex"])))]
struct LabelMatchArgs {
    /// Match elements whose label contains this text (case-sensitive, literal)
    #[arg(long, value_name = "TEXT", conflicts_with_all = ["selector", "label"])]
    label_contains: Option<String>,
    /// Match elements whose label matches this regex (Rust regex syntax, unanchored)
    #[arg(long, value_name = "PATTERN", value_parser = parse_label_regex, conflicts_with_all = ["selector", "label"])]
    label_regex: Option<String>,
    /// Pick the Nth match, 0-based, in tree order (needs --label-contains or --label-regex)
    #[arg(long, value_name = "N", requires = "label_match")]
    nth: Option<usize>,
}

impl LabelMatchArgs {
    fn to_match(&self) -> Option<Box<LabelMatch>> {
        let matcher = match (&self.label_contains, &self.label_regex) {
            (Some(t), _) => LabelMatcher::Contains(t.clone()),
            (None, Some(r)) => LabelMatcher::Regex(r.clone()),
            (None, None) => return None,
        };
        Some(Box::new(LabelMatch {
            matcher,
            nth: self.nth,
        }))
    }
}

/// Rejects an invalid `--label-regex` at parse time (usage error, exit 2).
fn parse_label_regex(s: &str) -> Result<String, String> {
    CompiledLabelMatch::new(&LabelMatch {
        matcher: LabelMatcher::Regex(s.to_string()),
        nth: None,
    })
    .map(|_| s.to_string())
}

#[derive(Debug)]
enum CliError {
    Connection(String),
    ActionFailed(String),
    Protocol(String),
    /// `exists` answered "not in the tree" (exit 4); not an error to report.
    Absent,
}

impl CliError {
    fn exit_code(&self) -> ExitCode {
        match self {
            CliError::Connection(_) => ExitCode::from(2),
            CliError::ActionFailed(_) => ExitCode::from(1),
            CliError::Protocol(_) => ExitCode::from(3),
            CliError::Absent => ExitCode::from(4),
        }
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::Connection(msg) => write!(f, "Connection error: {}", msg),
            CliError::ActionFailed(msg) => write!(f, "Action failed: {}", msg),
            CliError::Protocol(msg) => write!(f, "Protocol error: {}", msg),
            CliError::Absent => write!(f, "element not present"),
        }
    }
}

/// Result of a connect-only liveness probe of a session socket.
#[derive(Debug, PartialEq, Eq)]
enum SocketProbe {
    /// Something accepted the connection (a busy or hung server still counts).
    Live,
    /// The file exists but nothing is listening (connection refused).
    Dead,
    /// No socket file.
    Missing,
    /// Any other connect failure (e.g. permission denied); never acted on.
    Other,
}

/// Connects to `path` without sending anything, so it returns at once even
/// against a server that never answers.
///
/// A refused connect is retried once after a short pause: a server that has
/// bound its socket but not yet called `listen` also refuses, and reaping its
/// socket would orphan it.
fn probe_socket(path: &std::path::Path) -> SocketProbe {
    let connect = || std::os::unix::net::UnixStream::connect(path).map(|_| ());
    let result = match connect() {
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            std::thread::sleep(std::time::Duration::from_millis(20));
            connect()
        }
        other => other,
    };
    match result {
        Ok(()) => SocketProbe::Live,
        Err(e) => match e.kind() {
            std::io::ErrorKind::ConnectionRefused => SocketProbe::Dead,
            std::io::ErrorKind::NotFound => SocketProbe::Missing,
            _ => SocketProbe::Other,
        },
    }
}

/// Finds running sessions, pairing each with the agent port recorded in its
/// `.port` sidecar. The port is `None` for a session started before sessions
/// got their own ports, or one whose sidecar is unreadable.
fn discover_sessions() -> Vec<(String, Option<u16>)> {
    let dir = qorvex_dir();
    let pattern = dir.join("qorvex_*.sock");
    glob::glob(pattern.to_str().unwrap_or_default())
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry.ok().and_then(|path| {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.strip_prefix("qorvex_"))
                    .map(String::from)?;
                // Reap dead sessions; anything not provably dead stays listed.
                if probe_socket(&path) == SocketProbe::Dead {
                    let _ = std::fs::remove_file(&path);
                    return None;
                }
                Some(name)
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
    if probe_socket(&path) == SocketProbe::Dead {
        let _ = std::fs::remove_file(&path);
        return Err(CliError::Connection(format!(
            "session '{}' is dead (stale socket at {}, no server listening); run `qorvex start`",
            session,
            path.display()
        )));
    }
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
        Command::Lease {
            ref args,
            ref action,
        } => {
            let json = cli.format == OutputFormat::Json;
            return match action {
                None => lease::lease(
                    args.model.as_deref(),
                    args.owner.as_deref(),
                    args.runtime.as_deref(),
                    &args.ttl,
                    json,
                ),
                Some(LeaseAction::Heartbeat { udid }) => lease::heartbeat(udid),
                Some(LeaseAction::List) => lease::list(json),
            };
        }
        Command::Release {
            ref udid,
            ref owner,
        } => {
            return lease::release_cmd(udid.as_deref(), owner.as_deref());
        }
        Command::Reap { dry_run } => return lease::reap(dry_run),
        Command::Sims {
            action:
                SimsAction::Reclaim {
                    ref older_than,
                    yes,
                },
        } => {
            return lease::reclaim(older_than.as_deref(), yes, cli.format == OutputFormat::Json);
        }
        Command::ListSessions => {
            let sessions = discover_sessions();
            if cli.format == OutputFormat::Json {
                let sessions: Vec<_> = sessions
                    .iter()
                    .map(|(name, port)| serde_json::json!({ "name": name, "agent_port": port }))
                    .collect();
                println!(
                    "{}",
                    with_duration_ms(&serde_json::json!({ "sessions": sessions }).to_string())
                );
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
        Command::Doctor { platform } => {
            if platform == PlatformArg::Ios {
                println!("doctor: iOS checks are not implemented");
                return Ok(());
            }
            let checks = doctor::run();
            if cli.format == OutputFormat::Json {
                println!(
                    "{}",
                    with_duration_ms(&doctor::to_json(&checks).to_string())
                );
            } else {
                for c in &checks {
                    println!("{}", c.line());
                }
            }
            if !doctor::all_ok(&checks) {
                // Exit 1 directly: returning an error would add an
                // "Error: ..." line to stderr after the report.
                std::process::exit(1);
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
                            println!(
                                "{}",
                                with_duration_ms(
                                    &serde_json::json!({ "success": true, "udid": udid })
                                        .to_string()
                                )
                            );
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
                            with_duration_ms(
                                &serde_json::json!({
                                    "success": true,
                                    "udid": udid,
                                    "outcome": format!("{:?}", outcome),
                                })
                                .to_string()
                            )
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
        Command::Start {
            ref device,
            wait_ready,
        } => {
            return start_all(&cli, device.clone(), wait_ready).await;
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
            if_present,
            fallback_frame,
            report,
            ref tag,
        } => {
            if if_present {
                let probe = ActionType::Exists {
                    selector: selector.clone(),
                    by_label: label,
                    element_type: element_type.clone(),
                    label_match: None,
                };
                // The lookup and the tap are two requests; the element may
                // change in between (accepted race).
                if !query_exists(&mut client, probe, tag.clone()).await?.exists {
                    if !cli.quiet {
                        eprintln!("'{}' not present, skipped", selector);
                    }
                    return Ok(());
                }
            }
            let timeout_ms = if no_wait || if_present {
                None
            } else {
                Some(element_timeout(&cli))
            };
            execute_action(
                &mut client,
                ActionType::Tap {
                    selector: selector.clone(),
                    by_label: label,
                    element_type: element_type.clone(),
                    timeout_ms,
                    fallback_frame,
                    report: report || cli.format == OutputFormat::Json,
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
        Command::SendKeys {
            ref text,
            append: _,
            replace,
            ref tag,
        } => {
            execute_action(
                &mut client,
                ActionType::SendKeys {
                    text: text.clone(),
                    replace,
                },
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
            all,
            ref tag,
        } => execute_screen_info(&mut client, &cli, full, pretty, all, tag.clone()).await,
        Command::GetValue {
            ref selector,
            label,
            ref element_type,
            no_wait,
            ref tag,
        } => {
            let timeout_ms = if no_wait {
                None
            } else {
                Some(element_timeout(&cli))
            };
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
        Command::Exists {
            ref selector,
            label,
            ref element_type,
            ref label_match,
            ref tag,
        } => {
            let probe = ActionType::Exists {
                selector: selector.clone().unwrap_or_default(),
                by_label: label,
                element_type: element_type.clone(),
                label_match: label_match.to_match(),
            };
            let found = query_exists(&mut client, probe, tag.clone()).await?;
            let out = exists_output(&found, cli.format == OutputFormat::Json);
            if cli.format == OutputFormat::Json {
                println!("{}", with_duration_ms(&out));
            } else {
                println!("{}", out);
            }
            if found.exists {
                Ok(())
            } else {
                Err(CliError::Absent)
            }
        }
        Command::ScrollTo {
            ref selector,
            label,
            ref element_type,
            ref label_match,
            direction,
            max_swipes,
            duration_ms,
            ref tag,
        } => {
            let action = ActionType::ScrollTo {
                selector: selector.clone().unwrap_or_default(),
                by_label: label,
                element_type: element_type.clone(),
                label_match: label_match.to_match(),
                direction,
                max_swipes,
                duration_ms,
            };
            let (message, found) = query_scroll_to(
                &mut client,
                action,
                tag.clone(),
                cli.format == OutputFormat::Json,
            )
            .await?;
            if cli.format == OutputFormat::Json {
                println!("{}", with_duration_ms(&message));
            } else {
                println!("{}", message);
            }
            if found {
                Ok(())
            } else {
                Err(CliError::Absent)
            }
        }
        Command::Swipe {
            ref direction,
            from,
            to,
            ref selector,
            label,
            ref element_type,
            ref label_match,
            duration_ms,
            ref tag,
        } => {
            let action = match (direction, from, to) {
                (Some(direction), _, _)
                    if selector.is_some() || label_match.to_match().is_some() =>
                {
                    ActionType::SwipeFromElement {
                        selector: selector.clone().unwrap_or_default(),
                        by_label: label,
                        element_type: element_type.clone(),
                        label_match: label_match.to_match(),
                        direction: direction.parse().map_err(|_| {
                            CliError::ActionFailed(format!(
                                "Invalid swipe direction '{}'. Use: up, down, left, right",
                                direction
                            ))
                        })?,
                        duration_ms,
                    }
                }
                (Some(direction), _, _) => ActionType::Swipe {
                    direction: direction.clone(),
                    duration_ms,
                },
                (None, Some((from_x, from_y)), Some((to_x, to_y))) => ActionType::SwipeCoords {
                    from_x,
                    from_y,
                    to_x,
                    to_y,
                    duration_ms,
                },
                // Unreachable: clap requires a direction or --from/--to.
                _ => {
                    return Err(CliError::ActionFailed(
                        "swipe requires a direction or --from x,y --to x,y".to_string(),
                    ));
                }
            };
            execute_action(&mut client, action, tag.clone(), &cli).await
        }
        Command::Lock { ref tag } => {
            execute_action(&mut client, ActionType::Lock, tag.clone(), &cli).await
        }
        Command::Key { name, ref tag } => {
            let action = ActionType::Key { name: name.into() };
            execute_action(&mut client, action, tag.clone(), &cli).await
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
            ref label_match,
            ref tag,
        } => {
            execute_action(
                &mut client,
                ActionType::WaitFor {
                    selector: selector.clone().unwrap_or_default(),
                    by_label: label,
                    element_type: element_type.clone(),
                    timeout_ms: element_timeout(&cli),
                    require_stable: true,
                    label_match: label_match.to_match(),
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
            ref label_match,
            ref tag,
        } => {
            execute_action(
                &mut client,
                ActionType::WaitForNot {
                    selector: selector.clone().unwrap_or_default(),
                    by_label: label,
                    element_type: element_type.clone(),
                    timeout_ms: element_timeout(&cli),
                    label_match: label_match.to_match(),
                },
                tag.clone(),
                &cli,
            )
            .await
        }
        Command::WaitStable { stable_ms, ref tag } => {
            execute_action(
                &mut client,
                ActionType::WaitStable {
                    stable_ms,
                    timeout_ms: element_timeout(&cli),
                },
                tag.clone(),
                &cli,
            )
            .await
        }
        Command::StartTarget { force } => execute_start_target(&mut client, &cli, force).await,
        Command::StopTarget => send_command(&mut client, IpcRequest::StopTarget, &cli).await,
        Command::ForegroundTarget => {
            send_command(&mut client, IpcRequest::ForegroundTarget, &cli).await
        }
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
        | Command::Lease { .. }
        | Command::Release { .. }
        | Command::Reap { .. }
        | Command::Sims { .. }
        | Command::ListDevices { .. }
        | Command::Doctor { .. }
        | Command::BootDevice { .. }
        | Command::Convert { .. }
        | Command::QuietDevice { .. }
        | Command::Start { .. }
        | Command::Completions { .. } => unreachable!(),
    }
}

/// Answer of an `Exists` action.
#[derive(Debug, PartialEq)]
struct ExistsAnswer {
    exists: bool,
    hittable: Option<bool>,
}

/// Parses the `Exists` result data (`{"exists":bool,"hittable":bool|null}`).
fn parse_exists(data: &str) -> Result<ExistsAnswer, CliError> {
    let v: serde_json::Value = serde_json::from_str(data)
        .map_err(|e| CliError::Protocol(format!("Invalid exists response: {}", e)))?;
    let exists = v
        .get("exists")
        .and_then(|e| e.as_bool())
        .ok_or_else(|| CliError::Protocol("Invalid exists response: no `exists`".to_string()))?;
    Ok(ExistsAnswer {
        exists,
        hittable: v.get("hittable").and_then(|h| h.as_bool()),
    })
}

/// Stdout text for `exists`: `true`/`false`, or the JSON object under `-f json`.
fn exists_output(answer: &ExistsAnswer, json: bool) -> String {
    if !json {
        answer.exists.to_string()
    } else if answer.exists {
        serde_json::json!({"exists": true, "hittable": answer.hittable}).to_string()
    } else {
        serde_json::json!({"exists": false}).to_string()
    }
}

/// Sends one `Exists` action and returns its answer. A failed lookup (dead
/// target, driver error) is an error, never "absent".
async fn query_exists(
    client: &mut IpcClient,
    action: ActionType,
    tag: Option<String>,
) -> Result<ExistsAnswer, CliError> {
    let response = client
        .send(&IpcRequest::Execute { action, tag })
        .await
        .map_err(|e| CliError::Protocol(format!("Failed to send request: {}", e)))?;
    match response {
        IpcResponse::ActionResult {
            success: true,
            data,
            ..
        } => parse_exists(data.as_deref().unwrap_or("")),
        IpcResponse::ActionResult { message, .. } | IpcResponse::Error { message } => {
            Err(CliError::ActionFailed(message))
        }
        _ => Err(CliError::Protocol("Unexpected response type".to_string())),
    }
}

/// Sends one `ScrollTo` action. Returns the line to print and whether the
/// element was found; a driver error is an error, never "not found".
async fn query_scroll_to(
    client: &mut IpcClient,
    action: ActionType,
    tag: Option<String>,
    json: bool,
) -> Result<(String, bool), CliError> {
    let response = client
        .send(&IpcRequest::Execute { action, tag })
        .await
        .map_err(|e| CliError::Protocol(format!("Failed to send request: {}", e)))?;
    match response {
        IpcResponse::ActionResult {
            success: true,
            message,
            data,
            ..
        } => parse_scroll_to(&message, data.as_deref().unwrap_or(""), json),
        IpcResponse::ActionResult { message, .. } | IpcResponse::Error { message } => {
            Err(CliError::ActionFailed(message))
        }
        _ => Err(CliError::Protocol("Unexpected response type".to_string())),
    }
}

/// Parses the `ScrollTo` result data (`{"found":bool,"swipes":N}`) into the
/// stdout line (the message, or the JSON under `-f json`) and the found flag.
fn parse_scroll_to(message: &str, data: &str, json: bool) -> Result<(String, bool), CliError> {
    let v: serde_json::Value = serde_json::from_str(data)
        .map_err(|e| CliError::Protocol(format!("Invalid scroll-to response: {}", e)))?;
    let found = v
        .get("found")
        .and_then(|f| f.as_bool())
        .ok_or_else(|| CliError::Protocol("Invalid scroll-to response: no `found`".to_string()))?;
    let line = if json {
        data.to_string()
    } else {
        message.to_string()
    };
    Ok((line, found))
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
                println!(
                    "{}",
                    with_duration_ms(&serde_json::to_string_pretty(&output).unwrap())
                );
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
                            let parsed = data
                                .as_ref()
                                .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok());
                            let duration_str = parsed
                                .as_ref()
                                .and_then(|p| p.get("elapsed_ms").and_then(|v| v.as_u64()))
                                .map(|ms| format!("{}ms", ms))
                                .unwrap_or_default();
                            eprintln!(
                                "|{}|{}|{}|{}|{}",
                                now,
                                action_label,
                                action_target,
                                duration_str,
                                parsed.as_ref().map(tap_report_suffix).unwrap_or_default()
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

/// Extra fields for the text trace line of a tap: `fallback:frame_centre(x,y)|`
/// when the frame-centre fallback tapped, `ui_changed:<v>|` when reported.
/// Empty for every other action.
fn tap_report_suffix(data: &serde_json::Value) -> String {
    let mut out = String::new();
    if data["method"] == "frame_centre" {
        out.push_str(&format!(
            "fallback:frame_centre({},{})|",
            data["x"], data["y"]
        ));
    }
    if let Some(changed) = data.get("ui_changed") {
        out.push_str(&format!("ui_changed:{}|", changed));
    }
    out
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
                    Some(ref d) => println!("{}", with_duration_ms(d)),
                    None => println!(
                        "{}",
                        with_duration_ms(&serde_json::json!({ "message": message }).to_string())
                    ),
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
                    println!("{}", with_duration_ms(d));
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

/// Visibility flags for one element of `screen-info --all`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ElementFlags {
    /// The frame has no intersection with the root element's frame.
    offscreen: bool,
    /// The frame's width or height is <= 0.
    zero_size: bool,
    /// The agent's `hittable` value; `None` when the agent does not report it.
    hittable: Option<bool>,
}

/// Flatten a nested tree depth-first (parent before children), keeping every
/// element, named or not.
fn flatten_all(elements: &[UIElement]) -> Vec<&UIElement> {
    fn walk<'a>(elements: &'a [UIElement], out: &mut Vec<&'a UIElement>) {
        for e in elements {
            out.push(e);
            walk(&e.children, out);
        }
    }
    let mut out = Vec::new();
    walk(elements, &mut out);
    out
}

/// Whether the 1-D span [a0, a1) overlaps [b0, b1). A zero-length span counts
/// as overlapping when its point lies inside [b0, b1).
fn spans_overlap(a0: f64, a1: f64, b0: f64, b1: f64) -> bool {
    if a1 > a0 {
        a0 < b1 && a1 > b0
    } else {
        a0 >= b0 && a0 < b1
    }
}

/// Compute per-element flags for a depth-first flattened element list. The
/// root frame is the first element's (the first top-level element of the
/// dump: the application/window). Elements without a frame get
/// `offscreen: false` and `zero_size: false`; when the root has no frame or a
/// zero-size one, no element is judged offscreen.
fn compute_element_flags(elements: &[&UIElement]) -> Vec<ElementFlags> {
    let root = elements
        .first()
        .and_then(|e| e.frame.as_ref())
        .filter(|r| r.width > 0.0 && r.height > 0.0);
    elements
        .iter()
        .map(|e| {
            let (offscreen, zero_size) = match e.frame.as_ref() {
                Some(f) => {
                    let offscreen = root.is_some_and(|r| {
                        !(spans_overlap(f.x, f.x + f.width, r.x, r.x + r.width)
                            && spans_overlap(f.y, f.y + f.height, r.y, r.y + r.height))
                    });
                    (offscreen, f.width <= 0.0 || f.height <= 0.0)
                }
                None => (false, false),
            };
            ElementFlags {
                offscreen,
                zero_size,
                hittable: e.hittable,
            }
        })
        .collect()
}

/// Render `screen-info --all`: every element of the nested tree in `data_str`,
/// flattened depth-first, with its flags.
fn render_screen_info_all(
    data_str: &str,
    pretty: bool,
) -> Result<(String, Option<usize>), CliError> {
    let tree: Vec<UIElement> = serde_json::from_str(data_str)
        .map_err(|e| CliError::Protocol(format!("Failed to parse elements: {}", e)))?;
    let elements = flatten_all(&tree);
    let flags = compute_element_flags(&elements);
    if pretty {
        let mut out = String::new();
        for (elem, f) in elements.iter().zip(&flags) {
            out.push_str(&format_element_pretty(elem));
            if f.offscreen {
                out.push_str(" [offscreen]");
            }
            if f.zero_size {
                out.push_str(" [zero-size]");
            }
            if f.hittable == Some(false) {
                out.push_str(" [not-hittable]");
            }
            out.push('\n');
        }
        Ok((out, Some(elements.len())))
    } else {
        let concise: Vec<serde_json::Value> = elements
            .iter()
            .zip(&flags)
            .map(|(e, f)| {
                let mut v = element_to_concise_json(e);
                let map = v.as_object_mut().expect("concise element is an object");
                map.insert("offscreen".into(), serde_json::Value::Bool(f.offscreen));
                map.insert("zero_size".into(), serde_json::Value::Bool(f.zero_size));
                map.insert(
                    "hittable".into(),
                    f.hittable
                        .map(serde_json::Value::Bool)
                        .unwrap_or(serde_json::Value::Null),
                );
                v
            })
            .collect();
        Ok((
            format!("{}\n", serde_json::to_string_pretty(&concise).unwrap()),
            None,
        ))
    }
}

/// Render `screen-info` element data as stdout text, plus the element count
/// to report on stderr (`--pretty` only).
fn render_screen_info(
    data_str: &str,
    full: bool,
    pretty: bool,
) -> Result<(String, Option<usize>), CliError> {
    if full {
        // Original behavior: dump raw JSON
        return Ok((format!("{}\n", data_str), None));
    }
    let elements: Vec<UIElement> = serde_json::from_str(data_str)
        .map_err(|e| CliError::Protocol(format!("Failed to parse elements: {}", e)))?;
    let actionable = collect_actionable(&elements);
    if pretty {
        // REPL-style formatted output
        let mut out = String::new();
        for elem in &actionable {
            out.push_str(&format_element_pretty(elem));
            out.push('\n');
        }
        Ok((out, Some(actionable.len())))
    } else {
        // Default: concise JSON, actionable only, no nulls, rounded frames
        let concise: Vec<serde_json::Value> = actionable
            .iter()
            .map(|e| element_to_concise_json(e))
            .collect();
        Ok((
            format!("{}\n", serde_json::to_string_pretty(&concise).unwrap()),
            None,
        ))
    }
}

/// The `-f json` failure object printed on stdout by `screen-info`.
fn screen_info_error_json(err: &CliError) -> serde_json::Value {
    serde_json::json!({ "success": false, "message": err.to_string() })
}

async fn execute_screen_info(
    client: &mut IpcClient,
    cli: &Cli,
    full: bool,
    pretty: bool,
    all: bool,
    tag: Option<String>,
) -> Result<(), CliError> {
    // `--all` asks for the unfiltered nested tree with a separate action, so
    // the default request stays exactly what older servers expect.
    let action = if all {
        ActionType::GetScreenTree
    } else {
        ActionType::GetScreenInfo
    };
    let request = IpcRequest::Execute { action, tag };
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
            let (out, count) = if all {
                render_screen_info_all(data_str, pretty)?
            } else {
                render_screen_info(data_str, full, pretty)?
            };
            print!("{}", out);
            // Only --pretty (the human mode) reports a count; default stays
            // pure JSON even when stdout and stderr are merged.
            if let Some(n) = count {
                if !cli.quiet {
                    eprintln!("{} elements", n);
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
            ready,
            device,
            agent_connected,
            reason,
        } => {
            if cli.format == OutputFormat::Json {
                let output = serde_json::json!({
                    "session_id": session_id,
                    "has_screenshot": screenshot.is_some(),
                    "ready": ready,
                    "device": device,
                    "agent_connected": agent_connected,
                    "reason": reason,
                });
                println!(
                    "{}",
                    with_duration_ms(&serde_json::to_string_pretty(&output).unwrap())
                );
            } else {
                println!("Session ID: {}", session_id);
                println!("Has screenshot: {}", screenshot.is_some());
                if ready {
                    println!("Ready: yes");
                } else {
                    println!("Ready: no ({})", reason.as_deref().unwrap_or("unknown"));
                }
            }
            Ok(())
        }
        IpcResponse::Error { message } => Err(CliError::ActionFailed(message)),
        _ => Err(CliError::Protocol("Unexpected response type".to_string())),
    }
}

/// One `qorvex log` text line; the duration is appended when the entry has one.
fn log_entry_line(entry: &ActionLog) -> String {
    let line = format!(
        "[{}] {:?} - {:?}",
        entry.timestamp.format("%H:%M:%S"),
        entry.action,
        entry.result
    );
    match entry.duration_ms {
        Some(ms) => format!("{} - {}ms", line, ms),
        None => line,
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
                        println!("{}", log_entry_line(&entry));
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
                println!("{}", with_duration_ms(d));
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

async fn start_all(cli: &Cli, device: Option<String>, wait_ready: bool) -> Result<(), CliError> {
    use qorvex_core::config::QorvexConfig;
    use qorvex_core::ipc::socket_path;

    let sock = socket_path(&cli.session);

    // Idempotent: a session that is already ready (on the requested device, if
    // one was named) is left untouched. Checked first so a ready session never
    // pays for the signing build. Anything else falls through as before.
    if probe_socket(&sock) == SocketProbe::Live {
        set_start_phase(2);
        if let Ok(mut client) = IpcClient::connect(&cli.session).await {
            if let Ok(IpcResponse::State {
                ready: true,
                device: ref state_device,
                ..
            }) = client.send(&IpcRequest::GetState).await
            {
                if device.is_none() || device == *state_device {
                    if !cli.quiet {
                        eprintln!("Session already running (ready)");
                    }
                    return Ok(());
                }
            }
        }
    }

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
                set_start_phase(1);
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
    set_start_phase(2);
    if probe_socket(&sock) == SocketProbe::Dead {
        let _ = std::fs::remove_file(&sock);
        if !cli.quiet {
            eprintln!("Removed stale socket for dead session '{}'", cli.session);
        }
    }
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
    set_start_phase(3);
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

    set_start_phase(4);
    let response = client
        .send(&IpcRequest::StartSession)
        .await
        .map_err(|e| CliError::Protocol(format!("Failed to start session: {}", e)))?;

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

    if wait_ready {
        match client
            .send(&IpcRequest::GetState)
            .await
            .map_err(|e| CliError::Protocol(format!("Failed to read state: {}", e)))?
        {
            IpcResponse::State { ready: true, .. } => {}
            IpcResponse::State { reason, .. } => {
                return Err(CliError::ActionFailed(format!(
                    "session started but not ready: {}",
                    reason.unwrap_or_else(|| "unknown".to_string())
                )));
            }
            IpcResponse::Error { message } => return Err(CliError::ActionFailed(message)),
            _ => return Err(CliError::Protocol("Unexpected response".to_string())),
        }
    }
    Ok(())
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
                println!(
                    "{}",
                    with_duration_ms(&serde_json::json!({ "path": path }).to_string())
                );
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
                println!(
                    "{}",
                    with_duration_ms(&serde_json::json!({ "udid": udid }).to_string())
                );
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
                println!(
                    "{}",
                    with_duration_ms(&serde_json::json!({ "log": log }).to_string())
                );
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
    use std::fs;

    #[test]
    fn screen_info_default_is_array_without_count() {
        let data = r#"[{"AXUniqueId":"a","AXLabel":"A"},{"AXUniqueId":null}]"#;
        let (out, count) = render_screen_info(data, false, false).unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&out)
            .unwrap()
            .is_array());
        assert_eq!(count, None);
    }

    #[test]
    fn screen_info_empty_is_empty_array() {
        let (out, count) = render_screen_info("[]", false, false).unwrap();
        assert_eq!(out.trim(), "[]");
        assert_eq!(count, None);
    }

    #[test]
    fn screen_info_pretty_reports_count() {
        let data = r#"[{"AXUniqueId":"a","AXLabel":"A"}]"#;
        let (out, count) = render_screen_info(data, false, true).unwrap();
        assert_eq!(out.lines().count(), 1);
        assert_eq!(count, Some(1));
    }

    #[test]
    fn screen_info_full_is_raw() {
        let (out, count) = render_screen_info("[1]", true, false).unwrap();
        assert_eq!(out, "[1]\n");
        assert_eq!(count, None);
    }

    #[test]
    fn screen_info_error_object_shape() {
        let v = screen_info_error_json(&CliError::ActionFailed("x".into()));
        let o = v.as_object().unwrap();
        assert_eq!(o.len(), 2);
        assert_eq!(o["success"], false);
        assert_eq!(o["message"], "Action failed: x");
    }

    fn el(frame: Option<(f64, f64, f64, f64)>, hittable: Option<bool>) -> UIElement {
        UIElement {
            identifier: None,
            label: None,
            value: None,
            element_type: None,
            frame: frame.map(|(x, y, width, height)| ElementFrame {
                x,
                y,
                width,
                height,
            }),
            children: vec![],
            role: None,
            hittable,
        }
    }

    fn flags_of(elems: &[UIElement]) -> Vec<ElementFlags> {
        let refs: Vec<&UIElement> = elems.iter().collect();
        compute_element_flags(&refs)
    }

    #[test]
    fn element_flags_geometry() {
        let elems = [
            el(Some((0.0, 0.0, 400.0, 800.0)), None),    // root
            el(Some((10.0, 10.0, 100.0, 40.0)), None),   // inside
            el(Some((0.0, 900.0, 400.0, 40.0)), None),   // below the fold
            el(Some((0.0, 800.0, 400.0, 40.0)), None),   // starts exactly at the edge
            el(Some((-50.0, 780.0, 100.0, 40.0)), None), // partial overlap
            el(Some((10.0, 10.0, 0.0, 40.0)), None),     // zero width, inside
            el(Some((0.0, 1200.0, 0.0, 0.0)), None),     // Android clamped, outside
            el(Some((20.0, 20.0, -5.0, 10.0)), None),    // negative width, inside
            el(None, None),                              // no frame
        ];
        let f = flags_of(&elems);
        let got: Vec<(bool, bool)> = f.iter().map(|f| (f.offscreen, f.zero_size)).collect();
        assert_eq!(
            got,
            vec![
                (false, false),
                (false, false),
                (true, false),
                (true, false),
                (false, false),
                (false, true),
                (true, true),
                (false, true),
                (false, false),
            ]
        );
    }

    #[test]
    fn element_flags_hittable_passthrough() {
        let elems = [
            el(Some((0.0, 0.0, 400.0, 800.0)), Some(true)),
            el(Some((0.0, 0.0, 10.0, 10.0)), Some(false)),
            el(Some((0.0, 0.0, 10.0, 10.0)), None),
        ];
        let h: Vec<Option<bool>> = flags_of(&elems).iter().map(|f| f.hittable).collect();
        assert_eq!(h, vec![Some(true), Some(false), None]);
    }

    #[test]
    fn element_flags_no_root() {
        assert!(flags_of(&[]).is_empty());
        // Root without a frame: nothing can be judged offscreen.
        let elems = [el(None, None), el(Some((5000.0, 5000.0, 10.0, 10.0)), None)];
        assert!(flags_of(&elems).iter().all(|f| !f.offscreen));
        // Zero-size root: treated like a missing root.
        let elems = [
            el(Some((0.0, 0.0, 0.0, 0.0)), None),
            el(Some((10.0, 10.0, 10.0, 10.0)), None),
        ];
        assert!(flags_of(&elems).iter().all(|f| !f.offscreen));
    }

    const ALL_TREE: &str = r#"[{"type":"Application","frame":{"x":0,"y":0,"width":400,"height":800},
        "children":[{"type":"Other","frame":{"x":0,"y":0,"width":400,"height":800},
          "children":[{"AXUniqueId":"row1","type":"Cell","frame":{"x":0,"y":100,"width":400,"height":40},"hittable":true},
                      {"AXLabel":"Row 30","type":"Cell","frame":{"x":0,"y":1300,"width":400,"height":40},"hittable":false}]}]}]"#;

    #[test]
    fn screen_info_all_lists_every_element_with_flags() {
        let (out, count) = render_screen_info_all(ALL_TREE, false).unwrap();
        assert_eq!(count, None);
        let v: Vec<serde_json::Value> = serde_json::from_str(&out).unwrap();
        assert_eq!(v.len(), 4);
        for e in &v {
            assert!(e["offscreen"].is_boolean());
            assert!(e["zero_size"].is_boolean());
            assert!(e.get("hittable").is_some());
            assert!(e.get("children").is_none());
        }
        // Unnamed elements are present without id/label keys.
        assert!(v[1].get("id").is_none() && v[1].get("label").is_none());
        assert_eq!(v[0]["hittable"], serde_json::Value::Null);
        assert_eq!(v[2]["id"], "row1");
        assert_eq!(v[2]["offscreen"], false);
        assert_eq!(v[3]["offscreen"], true);
        assert_eq!(v[3]["hittable"], false);
    }

    #[test]
    fn screen_info_all_pretty_markers_and_count() {
        let (out, count) = render_screen_info_all(ALL_TREE, true).unwrap();
        assert_eq!(count, Some(4));
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 4);
        assert!(lines[2].ends_with("@(0,100)"));
        assert!(lines[3].ends_with("[offscreen] [not-hittable]"));
    }

    #[test]
    fn screen_info_all_empty_is_empty_array() {
        let (out, _) = render_screen_info_all("[]", false).unwrap();
        assert_eq!(out.trim(), "[]");
    }

    #[test]
    fn screen_info_all_conflicts_with_full() {
        match Cli::try_parse_from(["qorvex", "screen-info", "--all", "--full"]) {
            Err(e) => assert_eq!(e.kind(), clap::error::ErrorKind::ArgumentConflict),
            Ok(_) => panic!("--all --full must be a usage error"),
        }
        assert!(Cli::try_parse_from(["qorvex", "screen-info", "--all", "--pretty"]).is_ok());
    }

    #[test]
    fn send_keys_append_replace_flags() {
        let replace_of = |args: &[&str]| match Cli::try_parse_from(args).unwrap().command {
            Command::SendKeys { replace, .. } => replace,
            _ => panic!("expected SendKeys"),
        };
        assert!(!replace_of(&["qorvex", "send-keys", "abc"]));
        assert!(!replace_of(&["qorvex", "send-keys", "--append", "abc"]));
        assert!(replace_of(&["qorvex", "send-keys", "--replace", "abc"]));
        assert!(replace_of(&["qorvex", "send-keys", "--replace", ""]));
        match Cli::try_parse_from(["qorvex", "send-keys", "--append", "--replace", "abc"]) {
            Err(e) => assert_eq!(e.kind(), clap::error::ErrorKind::ArgumentConflict),
            Ok(_) => panic!("--append --replace must be a usage error"),
        }
    }

    #[test]
    fn exists_parses_selector_label_type_tag() {
        let cli = Cli::try_parse_from([
            "qorvex", "exists", "Tap Me", "-l", "-T", "Button", "--tag", "t",
        ])
        .unwrap();
        match cli.command {
            Command::Exists {
                selector,
                label,
                element_type,
                label_match,
                tag,
            } => {
                assert_eq!(selector.as_deref(), Some("Tap Me"));
                assert_eq!(label_match.to_match(), None);
                assert!(label);
                assert_eq!(element_type.as_deref(), Some("Button"));
                assert_eq!(tag.as_deref(), Some("t"));
            }
            _ => panic!("expected Exists"),
        }
    }

    #[test]
    fn label_match_flags_parse_on_exists_wait_for_wait_for_not() {
        for cmd in ["exists", "wait-for", "wait-for-not"] {
            let cli = Cli::try_parse_from([
                "qorvex",
                cmd,
                "--label-regex",
                "^Tapped: [0-9]+$",
                "--nth",
                "1",
                "-T",
                "StaticText",
            ])
            .unwrap();
            let (selector, m, typ) = match cli.command {
                Command::Exists {
                    selector,
                    label_match,
                    element_type,
                    ..
                }
                | Command::WaitFor {
                    selector,
                    label_match,
                    element_type,
                    ..
                }
                | Command::WaitForNot {
                    selector,
                    label_match,
                    element_type,
                    ..
                } => (selector, label_match, element_type),
                _ => panic!("expected an element command"),
            };
            assert_eq!(selector, None);
            assert_eq!(typ.as_deref(), Some("StaticText"));
            assert_eq!(
                m.to_match().map(|m| *m),
                Some(LabelMatch {
                    matcher: LabelMatcher::Regex("^Tapped: [0-9]+$".into()),
                    nth: Some(1),
                })
            );
            let cli = Cli::try_parse_from(["qorvex", cmd, "--label-contains", "(x)"]).unwrap();
            match cli.command {
                Command::Exists { label_match, .. }
                | Command::WaitFor { label_match, .. }
                | Command::WaitForNot { label_match, .. } => assert_eq!(
                    label_match.to_match().map(|m| *m),
                    Some(LabelMatch {
                        matcher: LabelMatcher::Contains("(x)".into()),
                        nth: None,
                    })
                ),
                _ => panic!("expected an element command"),
            }
        }
    }

    #[test]
    fn label_match_flags_conflicts_are_usage_errors() {
        use clap::error::ErrorKind;
        for cmd in ["exists", "wait-for", "wait-for-not"] {
            for args in [
                &["qorvex", cmd, "sel", "--label-contains", "a"][..],
                &["qorvex", cmd, "sel", "--label-regex", "a"][..],
                &["qorvex", cmd, "-l", "--label-contains", "a"][..],
                &["qorvex", cmd, "-l", "--label-regex", "a"][..],
                &["qorvex", cmd, "--label-contains", "a", "--label-regex", "b"][..],
                &["qorvex", cmd, "sel", "--nth", "1"][..],
                &["qorvex", cmd, "--label-regex", "("][..],
                &["qorvex", cmd][..],
            ] {
                let Err(err) = Cli::try_parse_from(args) else {
                    panic!("{:?} should be rejected", args)
                };
                assert_eq!(err.exit_code(), 2, "{:?}", args);
                assert_ne!(err.kind(), ErrorKind::DisplayHelp, "{:?}", args);
            }
        }
        let Err(err) = Cli::try_parse_from(["qorvex", "exists", "--label-regex", "("]) else {
            panic!("invalid regex should be rejected")
        };
        assert!(err.to_string().contains("invalid regex '('"), "{}", err);
        // Default positional form still parses, with no label match.
        let cli = Cli::try_parse_from(["qorvex", "wait-for", "x[1]", "-l"]).unwrap();
        match cli.command {
            Command::WaitFor {
                selector,
                label,
                label_match,
                ..
            } => {
                assert_eq!(selector.as_deref(), Some("x[1]"));
                assert!(label);
                assert_eq!(label_match.to_match(), None);
            }
            _ => panic!("expected WaitFor"),
        }
    }

    #[test]
    fn wait_stable_parses_and_rejects_for_over_timeout() {
        let cli = Cli::try_parse_from(["qorvex", "wait-stable"]).unwrap();
        assert!(is_wait_command(&cli.command));
        match cli.command {
            Command::WaitStable { stable_ms, tag } => {
                assert_eq!(stable_ms, 500);
                assert_eq!(tag, None);
            }
            _ => panic!("expected WaitStable"),
        }
        let cli = Cli::try_parse_from([
            "qorvex",
            "wait-stable",
            "--for",
            "300",
            "-o",
            "2000",
            "--tag",
            "t",
        ])
        .unwrap();
        assert_eq!(cli.timeout, Some(2000));
        match cli.command {
            Command::WaitStable { stable_ms, tag } => {
                assert_eq!(stable_ms, 300);
                assert_eq!(tag.as_deref(), Some("t"));
            }
            _ => panic!("expected WaitStable"),
        }
        assert!(Cli::try_parse_from(["qorvex", "wait-stable", "sel"]).is_err());
        assert!(check_wait_stable(500, 5000).is_ok());
        assert!(check_wait_stable(1000, 1000).is_ok());
        assert!(check_wait_stable(0, 0).is_ok());
        let msg = check_wait_stable(5000, 1000).unwrap_err();
        assert!(
            msg.contains("--for 5000ms") && msg.contains("1000ms"),
            "{}",
            msg
        );
    }

    #[test]
    fn foreground_target_parses_without_args() {
        let cli = Cli::try_parse_from(["qorvex", "foreground-target"]).unwrap();
        assert!(matches!(cli.command, Command::ForegroundTarget));
        assert!(Cli::try_parse_from(["qorvex", "foreground-target", "x"]).is_err());
        assert!(Cli::try_parse_from(["qorvex", "-o", "500", "foreground-target"]).is_ok());
    }

    #[test]
    fn tap_if_present_parses_alone_and_with_no_wait() {
        for args in [
            &["qorvex", "tap", "x", "--if-present"][..],
            &["qorvex", "tap", "x", "--if-present", "--no-wait"][..],
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            match cli.command {
                Command::Tap { if_present, .. } => assert!(if_present),
                _ => panic!("expected Tap"),
            }
        }
        match Cli::try_parse_from(["qorvex", "tap", "x"]).unwrap().command {
            Command::Tap { if_present, .. } => assert!(!if_present),
            _ => panic!("expected Tap"),
        }
    }

    #[test]
    fn tap_fallback_frame_and_report_parse() {
        let cli = Cli::try_parse_from([
            "qorvex",
            "tap",
            "x",
            "--fallback-frame",
            "--report",
            "--if-present",
        ])
        .unwrap();
        match cli.command {
            Command::Tap {
                fallback_frame,
                report,
                ..
            } => assert!(fallback_frame && report),
            _ => panic!("expected Tap"),
        }
        match Cli::try_parse_from(["qorvex", "tap", "x"]).unwrap().command {
            Command::Tap {
                fallback_frame,
                report,
                ..
            } => assert!(!fallback_frame && !report),
            _ => panic!("expected Tap"),
        }
    }

    #[test]
    fn tap_report_suffix_marks_fallback_and_ui_changed() {
        let plain = serde_json::json!({"elapsed_ms": 5, "method": "element", "x": null, "y": null});
        assert_eq!(tap_report_suffix(&plain), "");
        let fb = serde_json::json!({"elapsed_ms": 5, "method": "frame_centre", "x": 120, "y": 340, "ui_changed": true});
        assert_eq!(
            tap_report_suffix(&fb),
            "fallback:frame_centre(120,340)|ui_changed:true|"
        );
        assert_eq!(tap_report_suffix(&serde_json::json!({"elapsed_ms": 5})), "");
    }

    #[test]
    fn scroll_to_parses_defaults_and_all_flags() {
        let cli = Cli::try_parse_from(["qorvex", "scroll-to", "row-12"]).unwrap();
        match cli.command {
            Command::ScrollTo {
                selector,
                label,
                direction,
                max_swipes,
                duration_ms,
                ..
            } => {
                assert_eq!(selector.as_deref(), Some("row-12"));
                assert!(!label);
                assert_eq!(direction, ScrollDirection::Up);
                assert_eq!(max_swipes, 10);
                assert_eq!(duration_ms, None);
            }
            _ => panic!("expected ScrollTo"),
        }
        let cli = Cli::try_parse_from([
            "qorvex",
            "scroll-to",
            "Row",
            "-l",
            "-T",
            "Cell",
            "--direction",
            "down",
            "--max-swipes",
            "50",
            "--duration-ms",
            "800",
            "--tag",
            "t",
        ])
        .unwrap();
        match cli.command {
            Command::ScrollTo {
                label,
                element_type,
                direction,
                max_swipes,
                duration_ms,
                tag,
                ..
            } => {
                assert!(label);
                assert_eq!(element_type.as_deref(), Some("Cell"));
                assert_eq!(direction, ScrollDirection::Down);
                assert_eq!(max_swipes, 50);
                assert_eq!(duration_ms, Some(800));
                assert_eq!(tag.as_deref(), Some("t"));
            }
            _ => panic!("expected ScrollTo"),
        }
        let cli = Cli::try_parse_from(["qorvex", "scroll-to", "--label-contains", "Row"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::ScrollTo { selector: None, .. }
        ));
    }

    #[test]
    fn scroll_to_bad_values_are_usage_errors() {
        for bad in [
            vec!["scroll-to", "x", "--max-swipes", "0"],
            vec!["scroll-to", "x", "--max-swipes", "51"],
            vec!["scroll-to", "x", "--max-swipes", "abc"],
            vec!["scroll-to", "x", "--direction", "sideways"],
            vec!["scroll-to", "x", "--duration-ms", "0"],
            vec!["scroll-to"],
        ] {
            let mut argv = vec!["qorvex"];
            argv.extend(&bad);
            let Err(err) = Cli::try_parse_from(argv) else {
                panic!("{:?} should be rejected", bad);
            };
            assert_eq!(err.exit_code(), 2, "{:?}", bad);
        }
    }

    #[test]
    fn scroll_to_answer_parse() {
        let (line, found) = parse_scroll_to(
            "Found \"r\" after 3 swipes",
            r#"{"found":true,"swipes":3}"#,
            false,
        )
        .unwrap();
        assert_eq!((line.as_str(), found), ("Found \"r\" after 3 swipes", true));
        let (line, found) = parse_scroll_to(
            "Not found \"r\" after 3 swipes",
            r#"{"found":false,"swipes":3}"#,
            true,
        )
        .unwrap();
        assert_eq!(
            (line.as_str(), found),
            (r#"{"found":false,"swipes":3}"#, false)
        );
        assert!(parse_scroll_to("m", "garbage", false).is_err());
        assert!(parse_scroll_to("m", "{}", false).is_err());
    }

    #[test]
    fn duration_ms_is_appended_to_json_objects_only() {
        // Compact objects: every old key kept, in order, duration last.
        let (line, _) = parse_scroll_to("m", r#"{"found":false,"swipes":3}"#, true).unwrap();
        assert_eq!(
            insert_duration_ms(&line, 42),
            r#"{"found":false,"swipes":3,"duration_ms":42}"#
        );
        let exists = exists_output(
            &ExistsAnswer {
                exists: true,
                hittable: Some(false),
            },
            true,
        );
        assert_eq!(
            insert_duration_ms(&exists, 7),
            r#"{"exists":true,"hittable":false,"duration_ms":7}"#
        );
        assert_eq!(
            insert_duration_ms(&screen_info_error_json(&CliError::Absent).to_string(), 5),
            format!(
                r#"{{"message":"{}","success":false,"duration_ms":5}}"#,
                CliError::Absent
            )
        );
        // Pretty objects stay pretty.
        let pretty =
            serde_json::to_string_pretty(&serde_json::json!({"success": true, "data": null}))
                .unwrap();
        let out = insert_duration_ms(&pretty, 9);
        assert_eq!(
            out,
            "{\n  \"data\": null,\n  \"success\": true,\n  \"duration_ms\": 9\n}"
        );
        assert!(serde_json::from_str::<serde_json::Value>(&out).is_ok());
        // Empty object, and an object that already has the key.
        assert_eq!(insert_duration_ms("{}", 1), r#"{"duration_ms":1}"#);
        assert_eq!(
            insert_duration_ms(r#"{"duration_ms":3}"#, 1),
            r#"{"duration_ms":3}"#
        );
        // Arrays (screen-info, log, list-*) and text are left alone.
        let screen_info = render_screen_info("[]", false, false).unwrap().0;
        assert_eq!(insert_duration_ms(&screen_info, 1), screen_info);
        assert_eq!(insert_duration_ms("true", 1), "true");
        assert_eq!(insert_duration_ms("Found it", 1), "Found it");
        // Text-mode exists output is unchanged.
        assert_eq!(
            exists_output(
                &ExistsAnswer {
                    exists: true,
                    hittable: None
                },
                false
            ),
            "true"
        );
    }

    #[test]
    fn log_text_line_shows_duration_when_present() {
        let mut entry = ActionLog::new(
            ActionType::Key {
                name: "home".parse().unwrap(),
            },
            qorvex_core::action::ActionResult::Success,
            None,
            Some(123),
            None,
        );
        let line = log_entry_line(&entry);
        assert!(line.ends_with(" - Success - 123ms"), "{line}");
        entry.duration_ms = None;
        assert!(log_entry_line(&entry).ends_with(" - Success"));
    }

    #[test]
    fn exists_exit_codes_absent_is_4_errors_unchanged() {
        assert_eq!(CliError::Absent.exit_code(), ExitCode::from(4));
        assert_eq!(
            CliError::ActionFailed("x".into()).exit_code(),
            ExitCode::from(1)
        );
        assert_eq!(
            CliError::Connection("x".into()).exit_code(),
            ExitCode::from(2)
        );
        assert_eq!(
            CliError::Protocol("x".into()).exit_code(),
            ExitCode::from(3)
        );
    }

    #[test]
    fn exists_answer_parse_and_output_shapes() {
        let present = parse_exists(r#"{"exists":true,"hittable":false}"#).unwrap();
        assert_eq!(
            present,
            ExistsAnswer {
                exists: true,
                hittable: Some(false)
            }
        );
        assert_eq!(exists_output(&present, false), "true");
        assert_eq!(
            exists_output(&present, true),
            r#"{"exists":true,"hittable":false}"#
        );
        let no_hittable = parse_exists(r#"{"exists":true,"hittable":null}"#).unwrap();
        assert_eq!(
            exists_output(&no_hittable, true),
            r#"{"exists":true,"hittable":null}"#
        );
        let absent = parse_exists(r#"{"exists":false}"#).unwrap();
        assert_eq!(exists_output(&absent, false), "false");
        assert_eq!(exists_output(&absent, true), r#"{"exists":false}"#);
        assert!(parse_exists("garbage").is_err());
        assert!(parse_exists("{}").is_err());
    }

    fn swipe_parse(args: &[&str]) -> Result<Cli, clap::Error> {
        let mut argv = vec!["qorvex", "swipe"];
        argv.extend_from_slice(args);
        Cli::try_parse_from(argv)
    }

    #[test]
    fn swipe_duration_ms_parses_on_both_forms() {
        for args in [
            vec!["up", "--duration-ms", "800"],
            vec!["--from", "1,2", "--to", "3,4", "--duration-ms", "800"],
        ] {
            match swipe_parse(&args).unwrap().command {
                Command::Swipe { duration_ms, .. } => assert_eq!(duration_ms, Some(800)),
                _ => panic!("expected swipe"),
            }
        }
        match swipe_parse(&["up", "--duration-ms", "10000"])
            .unwrap()
            .command
        {
            Command::Swipe { duration_ms, .. } => assert_eq!(duration_ms, Some(10000)),
            _ => panic!("expected swipe"),
        }
        match swipe_parse(&["up"]).unwrap().command {
            Command::Swipe { duration_ms, .. } => assert_eq!(duration_ms, None),
            _ => panic!("expected swipe"),
        }
    }

    #[test]
    fn swipe_duration_ms_bad_values_are_usage_errors() {
        for bad in ["0", "-5", "abc", "10001", "1.5", ""] {
            let err = swipe_parse(&["up", "--duration-ms", bad]).err().unwrap();
            assert_eq!(err.exit_code(), 2, "value {:?}", bad);
        }
        let err = swipe_parse(&["up", "--duration-ms", "10001"])
            .err()
            .unwrap();
        assert!(err.to_string().contains("1 and 10000"));
    }

    #[test]
    fn swipe_from_element_parses_and_conflicts() {
        match swipe_parse(&["left", "--from-element", "box", "-l", "-T", "Other"])
            .unwrap()
            .command
        {
            Command::Swipe {
                selector,
                label,
                element_type,
                ..
            } => {
                assert_eq!(selector.as_deref(), Some("box"));
                assert!(label);
                assert_eq!(element_type.as_deref(), Some("Other"));
            }
            _ => panic!("expected swipe"),
        }
        assert!(swipe_parse(&["up", "--label-contains", "x", "--nth", "1"]).is_ok());
        assert!(swipe_parse(&["up", "--from-element", "b", "--duration-ms", "9"]).is_ok());
        for bad in [
            vec!["up", "--from-element", "b", "--from", "1,2", "--to", "3,4"],
            vec!["--from", "1,2", "--to", "3,4", "--label-contains", "x"],
            vec!["--from-element", "b"],
            vec!["up", "--from-element", "b", "--label-contains", "x"],
            vec!["up", "-T", "Other"],
            vec!["up", "--label"],
        ] {
            let err = swipe_parse(&bad).err().unwrap();
            assert_eq!(err.exit_code(), 2, "{:?}", bad);
        }
    }

    #[test]
    fn swipe_direction_parses() {
        let cli = swipe_parse(&["up"]).unwrap();
        match cli.command {
            Command::Swipe {
                direction,
                from,
                to,
                ..
            } => {
                assert_eq!(direction.as_deref(), Some("up"));
                assert!(from.is_none() && to.is_none());
            }
            _ => panic!("expected swipe"),
        }
    }

    #[test]
    fn swipe_coords_parses() {
        let cli = swipe_parse(&["--from", "10,20", "--to", "30,40"]).unwrap();
        match cli.command {
            Command::Swipe {
                direction,
                from,
                to,
                ..
            } => {
                assert!(direction.is_none());
                assert_eq!(from, Some((10, 20)));
                assert_eq!(to, Some((30, 40)));
            }
            _ => panic!("expected swipe"),
        }
    }

    #[test]
    fn swipe_bad_coordinate_errors_clearly() {
        let err = swipe_parse(&["--from", "10;20", "--to", "30,40"])
            .err()
            .expect("bad coordinate should fail");
        assert!(
            err.to_string()
                .contains("invalid coordinate '10;20': expected x,y"),
            "{err}"
        );
    }

    #[test]
    fn swipe_negative_coordinate_errors() {
        let err = swipe_parse(&["--from", "-1,20", "--to", "30,40"])
            .err()
            .expect("negative coordinate should fail");
        assert!(err.to_string().contains("non-negative"), "{err}");
    }

    #[test]
    fn swipe_direction_conflicts_with_coords() {
        assert!(swipe_parse(&["up", "--from", "1,2", "--to", "3,4"]).is_err());
    }

    #[test]
    fn swipe_from_requires_to() {
        assert!(swipe_parse(&["--from", "1,2"]).is_err());
        assert!(swipe_parse(&["--to", "1,2"]).is_err());
    }

    #[test]
    fn swipe_requires_direction_or_coords() {
        assert!(swipe_parse(&[]).is_err());
    }

    #[test]
    fn lock_parses() {
        let cli = Cli::try_parse_from(["qorvex", "lock"]).unwrap();
        assert!(matches!(cli.command, Command::Lock { .. }));
    }

    #[test]
    fn key_parses_each_name() {
        for (arg, want) in [("back", KeyName::Back), ("home", KeyName::Home)] {
            let cli = Cli::try_parse_from(["qorvex", "key", arg]).unwrap();
            let Command::Key { name, .. } = cli.command else {
                panic!("expected Command::Key for {arg}")
            };
            assert_eq!(KeyName::from(name), want);
        }
    }

    #[test]
    fn key_rejects_unknown_name_with_usage_error() {
        for args in [&["qorvex", "key", "bogus"][..], &["qorvex", "key"][..]] {
            let Err(err) = Cli::try_parse_from(args) else {
                panic!("{:?} should be rejected", args)
            };
            assert_eq!(err.exit_code(), 2, "{:?}", args);
        }
        let Err(err) = Cli::try_parse_from(["qorvex", "key", "bogus"]) else {
            panic!("bogus key should be rejected")
        };
        let msg = err.to_string();
        assert!(msg.contains("back") && msg.contains("home"), "{msg}");
    }

    #[test]
    fn test_discover_sessions() {
        let qorvex_dir = qorvex_dir();

        // Ensure the qorvex directory exists
        fs::create_dir_all(&qorvex_dir).expect("Failed to create qorvex directory");

        // Create temporary socket files with unique names to avoid test collisions
        let test_sessions = ["test_session_a", "test_session_b", "test_session_c"];
        let mut created_files = Vec::new();
        let mut listeners = Vec::new();

        for session_name in &test_sessions {
            let sock_path = qorvex_dir.join(format!("qorvex_{}.sock", session_name));
            let _ = fs::remove_file(&sock_path);
            listeners.push(
                std::os::unix::net::UnixListener::bind(&sock_path)
                    .expect("Failed to bind test socket"),
            );
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
        let mut listeners = Vec::new();
        let sockets: Vec<_> = [with_port, without_port]
            .iter()
            .map(|name| {
                let path = qorvex_dir.join(format!("qorvex_{}.sock", name));
                let _ = fs::remove_file(&path);
                listeners.push(
                    std::os::unix::net::UnixListener::bind(&path)
                        .expect("Failed to bind test socket"),
                );
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

    /// A bound-then-dropped socket probes as dead; a listening one as live; a
    /// missing path as missing. Only the dead file may be reaped.
    #[test]
    fn probe_socket_classifies_live_dead_missing() {
        let dir = std::path::PathBuf::from(format!("/tmp/qvx_probe_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let live = dir.join("live.sock");
        let dead = dir.join("dead.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&live).unwrap();
        drop(std::os::unix::net::UnixListener::bind(&dead).unwrap());
        assert_eq!(probe_socket(&live), SocketProbe::Live);
        assert_eq!(probe_socket(&dead), SocketProbe::Dead);
        assert_eq!(probe_socket(&dir.join("none.sock")), SocketProbe::Missing);
        assert!(live.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn start_parses_wait_ready() {
        let cli = Cli::try_parse_from(["qorvex", "start", "--wait-ready"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Start {
                wait_ready: true,
                ..
            }
        ));
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

    fn ms(n: u64) -> Option<std::time::Duration> {
        Some(std::time::Duration::from_millis(n))
    }

    #[test]
    fn deadline_defaults_to_600s() {
        assert_eq!(command_deadline(false, None, None, None), ms(600_000));
    }

    #[test]
    fn deadline_flag_beats_env() {
        assert_eq!(
            command_deadline(false, None, Some(1000), Some(5000)),
            ms(1000)
        );
        assert_eq!(command_deadline(false, None, None, Some(5000)), ms(5000));
    }

    #[test]
    fn deadline_zero_disables() {
        assert_eq!(command_deadline(false, None, Some(0), None), None);
        assert_eq!(command_deadline(false, None, None, Some(0)), None);
        assert_eq!(command_deadline(true, Some(5000), Some(100), Some(0)), None);
    }

    #[test]
    fn element_timeout_env_does_not_change_start_deadline() {
        // QORVEX_TIMEOUT never reaches the deadline function for non-wait commands.
        assert_eq!(command_deadline(false, None, None, None), ms(600_000));
    }

    #[test]
    fn wait_command_deadline_exceeds_element_timeout() {
        assert_eq!(
            command_deadline(true, Some(700_000), Some(700_000), None),
            ms(720_000)
        );
        assert_eq!(
            command_deadline(true, Some(1000), Some(1000), None),
            ms(600_000)
        );
        // --no-wait forwards nothing: plain default.
        assert_eq!(
            command_deadline(true, None, Some(700_000), None),
            ms(600_000)
        );
    }

    #[test]
    fn timeout_flag_accepted_on_every_subcommand() {
        fn check(cmd: &clap::Command, path: &mut Vec<String>) {
            for sub in cmd.get_subcommands() {
                path.push(sub.get_name().to_string());
                if sub.has_subcommands() {
                    check(sub, path);
                }
                path.pop();
            }
            if path.is_empty() {
                return;
            }
            let mut args = vec!["qorvex".to_string()];
            args.extend(path.iter().cloned());
            args.extend(["--timeout".to_string(), "5".to_string()]);
            // Required args may be missing; only a rejected --timeout would be an
            // UnknownArgument error.
            if let Err(e) = Cli::command().try_get_matches_from(&args) {
                assert_ne!(
                    e.kind(),
                    clap::error::ErrorKind::UnknownArgument,
                    "{:?}",
                    args
                );
            }
        }
        check(&Cli::command(), &mut Vec::new());
    }

    #[test]
    fn wait_for_element_timeout_parses() {
        let cli = Cli::try_parse_from(["qorvex", "wait-for", "x"]).unwrap();
        assert_eq!(cli.timeout, None);
        assert_eq!(element_timeout_checked(cli.timeout, None), Ok(5000));
        let cli = Cli::try_parse_from(["qorvex", "wait-for", "x", "-o", "100"]).unwrap();
        assert_eq!(element_timeout_checked(cli.timeout, None), Ok(100));
        assert_eq!(element_timeout_checked(None, Some("300".into())), Ok(300));
        assert!(element_timeout_checked(None, Some("abc".into())).is_err());
    }
}
