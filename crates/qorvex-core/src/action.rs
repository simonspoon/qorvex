//! Action types and logging for automation operations.
//!
//! This module defines the various actions that can be performed on an iOS
//! Simulator, along with the [`ActionLog`] type for recording executed actions.
//!
//! # Action Types
//!
//! Actions fall into several categories:
//!
//! - **UI Interaction**: [`ActionType::Tap`], [`ActionType::TapLocation`], [`ActionType::Swipe`], [`ActionType::LongPress`], [`ActionType::SendKeys`]
//! - **Information Retrieval**: [`ActionType::GetScreenshot`], [`ActionType::GetScreenInfo`], [`ActionType::GetValue`]
//! - **Scrolling**: [`ActionType::ScrollTo`]
//! - **Waiting**: [`ActionType::WaitFor`], [`ActionType::WaitStable`]
//! - **Session Management**: [`ActionType::StartSession`], [`ActionType::EndSession`], [`ActionType::Quit`]
//! - **Logging**: [`ActionType::LogComment`]
//!
//! # Example
//!
//! ```
//! use qorvex_core::action::{ActionType, ActionResult, ActionLog};
//!
//! // Create an action - tap by ID
//! let action = ActionType::Tap {
//!     selector: "login-button".to_string(),
//!     by_label: false,
//!     element_type: None,
//!     timeout_ms: None,
//!     fallback_frame: false,
//!     report: false,
//! };
//!
//! // Create a log entry
//! let log = ActionLog::new(action, ActionResult::Success, None, None, None);
//! println!("Action {} at {}", log.id, log.timestamp);
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

fn default_true() -> bool {
    true
}

/// Finger swipe direction of `scroll-to` (`up` scrolls content down).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScrollDirection {
    /// Finger moves up.
    Up,
    /// Finger moves down.
    Down,
    /// Finger moves left.
    Left,
    /// Finger moves right.
    Right,
}

impl ScrollDirection {
    /// The lowercase name `Swipe` takes.
    pub fn as_str(self) -> &'static str {
        match self {
            ScrollDirection::Up => "up",
            ScrollDirection::Down => "down",
            ScrollDirection::Left => "left",
            ScrollDirection::Right => "right",
        }
    }
}

impl std::str::FromStr for ScrollDirection {
    type Err = String;

    /// Case-insensitive: `up`, `down`, `left` or `right`.
    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "up" => Ok(ScrollDirection::Up),
            "down" => Ok(ScrollDirection::Down),
            "left" => Ok(ScrollDirection::Left),
            "right" => Ok(ScrollDirection::Right),
            _ => Err(format!(
                "invalid direction '{}': use up, down, left or right",
                s
            )),
        }
    }
}

/// How a [`LabelMatch`] tests an element's label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelMatcher {
    /// Case-sensitive substring of the label (taken literally).
    Contains(String),
    /// Unanchored Rust `regex` search on the label.
    Regex(String),
}

/// A text-match selector on the element label, optionally picking the Nth
/// match (0-based, depth-first tree order).
///
/// Used by `Exists`, `WaitFor` and `WaitForNot` in place of the selector;
/// matched client-side against a tree dump.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelMatch {
    /// The label test.
    pub matcher: LabelMatcher,
    /// 0-based index among matches; `None` means the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nth: Option<usize>,
}

impl std::fmt::Display for LabelMatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.matcher {
            LabelMatcher::Contains(t) => write!(f, "label contains \"{}\"", t)?,
            LabelMatcher::Regex(r) => write!(f, "label matches /{}/", r)?,
        }
        if let Some(n) = self.nth {
            write!(f, " [{}]", n)?;
        }
        Ok(())
    }
}

/// The result of executing an action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActionResult {
    /// The action completed successfully.
    Success,

    /// The action failed with the given error message.
    Failure(String),
}

/// Types of actions that can be performed on a simulator.
///
/// Actions are serialized as JSON with a `type` tag discriminator for
/// IPC transmission.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ActionType {
    /// Tap an element by ID or label.
    Tap {
        /// The selector value (accessibility ID or label).
        selector: String,
        /// If true, selector is an accessibility label; if false, it's an ID.
        by_label: bool,
        /// Optional element type filter (e.g., "Button", "TextField").
        element_type: Option<String>,
        /// If set, retry on transient errors (element not found / not hittable)
        /// until this many milliseconds have elapsed. If `None`, attempt once.
        #[serde(default)]
        timeout_ms: Option<u64>,
        /// If the normal tap fails but the element is in the tree with a
        /// non-empty frame, tap the frame centre once with `tap_location`.
        #[serde(default)]
        fallback_frame: bool,
        /// Dump the tree before and after (settled) the tap and report
        /// `ui_changed`. Costs up to about 1.2 s extra.
        #[serde(default)]
        report: bool,
    },

    /// Tap at specific screen coordinates.
    TapLocation {
        /// The x-coordinate in screen points.
        x: i32,
        /// The y-coordinate in screen points.
        y: i32,
    },

    /// Swipe the screen in a direction.
    Swipe {
        /// Direction to swipe: "up", "down", "left", or "right".
        direction: String,
        /// Swipe duration in milliseconds; `None` uses the default (300 ms).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u32>,
    },

    /// Swipe in a direction starting at the centre of an element.
    ///
    /// Same distance as [`ActionType::Swipe`]; the start point is the centre
    /// of the element's frame. Fails when the element is not found.
    SwipeFromElement {
        /// The selector value (accessibility ID or label).
        selector: String,
        /// If true, selector is an accessibility label; if false, it's an ID.
        by_label: bool,
        /// Optional element type filter (e.g., "Button", "TextField").
        element_type: Option<String>,
        /// Label text match used instead of `selector` (`--label-contains` /
        /// `--label-regex`); `None` keeps the selector search.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label_match: Option<Box<LabelMatch>>,
        /// Swipe direction (finger movement).
        direction: ScrollDirection,
        /// Swipe duration in milliseconds; `None` uses the default (300 ms).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u32>,
    },

    /// Swipe the screen between two explicit coordinates.
    SwipeCoords {
        /// Starting x-coordinate in screen points.
        from_x: i32,
        /// Starting y-coordinate in screen points.
        from_y: i32,
        /// Ending x-coordinate in screen points.
        to_x: i32,
        /// Ending y-coordinate in screen points.
        to_y: i32,
        /// Swipe duration in milliseconds; `None` uses the default (300 ms).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u32>,
    },

    /// Lock the device (press the lock button).
    Lock,

    /// Press a system key (back, home).
    Key {
        /// The key to press.
        name: crate::protocol::KeyName,
    },

    /// Long press at specific screen coordinates.
    LongPress {
        /// The x-coordinate in screen points.
        x: i32,
        /// The y-coordinate in screen points.
        y: i32,
        /// How long to press in seconds.
        duration: f64,
    },

    /// Log a comment (for documentation purposes).
    LogComment {
        /// The comment text to log.
        message: String,
    },

    /// Capture a screenshot of the current screen.
    ///
    /// Returns base64-encoded PNG data.
    GetScreenshot,

    /// Get accessibility information for all elements on screen.
    GetScreenInfo,

    /// Get the complete, unfiltered accessibility tree (nested), including
    /// elements with no identifier or label. Used by `screen-info --all`.
    ///
    /// A separate variant (rather than a field on `GetScreenInfo`) keeps the
    /// default `GetScreenInfo` request byte-identical on the wire, so old and
    /// new clients/servers interoperate on the default path.
    GetScreenTree,

    /// Get the current value of an element by ID or label.
    GetValue {
        /// The selector value (accessibility ID or label).
        selector: String,
        /// If true, selector is an accessibility label; if false, it's an ID.
        by_label: bool,
        /// Optional element type filter (e.g., "Button", "TextField").
        element_type: Option<String>,
        /// If set, retry on transient errors (element not found / not hittable)
        /// until this many milliseconds have elapsed. If `None`, attempt once.
        #[serde(default)]
        timeout_ms: Option<u64>,
    },

    /// Report whether an element is in the accessibility tree right now.
    ///
    /// One lookup, no waiting, no retry. Returns `{"exists":bool,"hittable":bool|null}`
    /// in the result data; an absent element is a success with `exists:false`,
    /// while a driver error is a failure. "Present" means found in the tree
    /// (hidden, offscreen and non-hittable elements count).
    Exists {
        /// The selector value (accessibility ID or label).
        selector: String,
        /// If true, selector is an accessibility label; if false, it's an ID.
        by_label: bool,
        /// Optional element type filter (e.g., "Button", "TextField").
        element_type: Option<String>,
        /// Label text match used instead of `selector` (`--label-contains` /
        /// `--label-regex`); `None` keeps the selector search.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label_match: Option<Box<LabelMatch>>,
    },

    /// Swipe until an element is in the accessibility tree, at most `max_swipes` times.
    ///
    /// Looks for the element first (as `Exists` does) and swipes only while it
    /// is absent. Returns `{"found":bool,"swipes":N}` in the result data; not
    /// finding it is a success with `found:false`, a driver error is a failure.
    /// "Found" means in the tree, not on screen (offscreen rows of virtualised
    /// lists can already count).
    ScrollTo {
        /// The selector value (accessibility ID or label).
        selector: String,
        /// If true, selector is an accessibility label; if false, it's an ID.
        by_label: bool,
        /// Optional element type filter (e.g., "Button", "TextField").
        element_type: Option<String>,
        /// Label text match used instead of `selector` (`--label-contains` /
        /// `--label-regex`); `None` keeps the selector search.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label_match: Option<Box<LabelMatch>>,
        /// Swipe direction (finger movement).
        direction: ScrollDirection,
        /// Maximum number of swipes (1-50).
        max_swipes: u32,
        /// Swipe duration in milliseconds; `None` uses the default (300 ms).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u32>,
    },

    /// Send keyboard input.
    SendKeys {
        /// The text to type.
        text: String,
        /// If true, the field holds exactly `text` afterwards (`--replace`);
        /// if false, `text` is appended (`--append`, the default).
        #[serde(default)]
        replace: bool,
    },

    /// Wait for an element to appear on screen by ID or label.
    WaitFor {
        /// The selector value (accessibility ID or label).
        selector: String,
        /// If true, selector is an accessibility label; if false, it's an ID.
        by_label: bool,
        /// Optional element type filter (e.g., "Button", "TextField").
        element_type: Option<String>,
        /// Maximum time to wait in milliseconds.
        timeout_ms: u64,
        /// If true, require 3 consecutive stable frames before returning success.
        /// If false, return as soon as the element exists and is hittable (faster,
        /// skips frame-stability tracking).
        #[serde(default = "default_true")]
        require_stable: bool,
        /// Label text match used instead of `selector` (`--label-contains` /
        /// `--label-regex`); `None` keeps the selector search.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label_match: Option<Box<LabelMatch>>,
    },

    /// Wait for an element to disappear from screen by ID or label.
    WaitForNot {
        /// The selector value (accessibility ID or label).
        selector: String,
        /// If true, selector is an accessibility label; if false, it's an ID.
        by_label: bool,
        /// Optional element type filter (e.g., "Button", "TextField").
        element_type: Option<String>,
        /// Maximum time to wait in milliseconds.
        timeout_ms: u64,
        /// Label text match used instead of `selector` (`--label-contains` /
        /// `--label-regex`); `None` keeps the selector search.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label_match: Option<Box<LabelMatch>>,
    },

    /// Wait until the whole UI tree is unchanged for `stable_ms` milliseconds.
    WaitStable {
        /// Quiet period: how long the tree must stay unchanged, in milliseconds.
        stable_ms: u64,
        /// Maximum time to wait in milliseconds.
        timeout_ms: u64,
    },

    /// Start a new automation session.
    StartSession,

    /// End the current session but keep the REPL running.
    EndSession,

    /// Set the target application bundle ID.
    SetTarget {
        /// The bundle identifier of the app to target (e.g., "com.example.MyApp").
        bundle_id: String,
    },

    /// Launch the target application.
    StartTarget {
        /// Whether the launch terminated a running copy first. Recorded so a
        /// log replayed through the converter reproduces the relaunch instead
        /// of silently attaching to whatever is already running.
        /// `#[serde(default)]` keeps logs written before this field readable.
        #[serde(default)]
        force: bool,
    },

    /// Terminate the target application.
    StopTarget,

    /// Get metadata about the currently targeted application.
    GetTargetInfo,

    /// Report the target app's memory footprint and the device's memory state.
    MemoryInfo {
        /// The report the command produced, or `None` when it failed. Carried
        /// in the action itself so a log line records the measurement as well
        /// as the fact that it was taken. Boxed to keep `ActionType` — and so
        /// every `ActionLog` and `SessionEvent` — small; it serializes the
        /// same either way.
        /// `#[serde(default)]` keeps logs written before this field readable.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        report: Option<Box<crate::memory::MemoryInfo>>,
    },

    /// Shut down the session's selected simulator.
    ShutdownDevice,

    /// Delete the session's selected simulator.
    DeleteDevice,

    /// Install an app bundle on the session's selected simulator.
    InstallApp {
        /// Absolute path to the `.app` bundle that was installed.
        path: String,
    },

    /// Uninstall an app from the session's selected simulator.
    UninstallApp {
        /// The bundle identifier of the app to uninstall.
        bundle_id: String,
    },

    /// Look up the path of one of an app's containers on the selected simulator.
    AppContainer {
        /// The bundle identifier of the installed app.
        bundle_id: String,
        /// Which container was asked for: `app`, `data` or `groups`. `None`
        /// means simctl's default (`app`).
        container: Option<String>,
    },

    /// List the apps installed on the session's selected simulator.
    ListApps,

    /// Create a simulator and make it this session's selected device.
    CreateDevice {
        /// Name given to the new simulator.
        name: String,
        /// Device type identifier or display name.
        device_type: String,
        /// Runtime identifier.
        runtime: String,
    },

    /// Wait until the session's selected simulator has finished booting.
    WaitForBoot,

    /// Read recent entries from the selected simulator's unified log.
    DeviceLog {
        /// How far back the log was read, in `log show --last` syntax.
        last: String,
        /// The NSPredicate the log was filtered with, if any.
        predicate: Option<String>,
    },

    /// Set the selected simulator's light/dark appearance.
    SetAppearance {
        /// `dark` or `light`.
        appearance: String,
    },

    /// Set the selected simulator's Dynamic Type content size.
    SetContentSize {
        /// A content size in simctl's spelling, e.g. `extra-small`.
        size: String,
    },

    /// Change an app's access to a privacy-protected service.
    GrantPermission {
        /// `grant`, `revoke` or `reset`.
        verb: String,
        /// The privacy service affected, e.g. `microphone`.
        service: String,
        /// The bundle identifier of the app whose access changed.
        bundle_id: String,
    },

    /// Add media files to the selected simulator's libraries.
    AddMedia {
        /// Absolute paths of the media files that were added.
        paths: Vec<String>,
    },

    /// Open a URL on the selected simulator.
    OpenUrl {
        /// The URL that was opened.
        url: String,
    },

    /// Quit the REPL entirely.
    Quit,
}

/// Parse an `x,y` coordinate pair (non-negative `i32`s), as used by
/// `swipe --from/--to`. Returns a user-facing error message on bad input.
pub fn parse_coordinate(s: &str) -> Result<(i32, i32), String> {
    let bad = || format!("invalid coordinate '{}': expected x,y", s);
    let (x, y) = s.split_once(',').ok_or_else(bad)?;
    let x: i32 = x.trim().parse().map_err(|_| bad())?;
    let y: i32 = y.trim().parse().map_err(|_| bad())?;
    if x < 0 || y < 0 {
        return Err(format!(
            "invalid coordinate '{}': coordinates must be non-negative",
            s
        ));
    }
    Ok((x, y))
}

/// Default `scroll-to` swipe budget.
pub const DEFAULT_MAX_SWIPES: u32 = 10;

/// Parses a `scroll-to --max-swipes` value: a whole number from 1 to 50.
pub fn parse_max_swipes(s: &str) -> Result<u32, String> {
    let n: u32 = s
        .trim()
        .parse()
        .map_err(|_| format!("invalid max swipes '{}': expected a whole number (1-50)", s))?;
    if !(1..=50).contains(&n) {
        return Err(format!(
            "invalid max swipes {}: must be between 1 and 50",
            n
        ));
    }
    Ok(n)
}

/// Parse a swipe duration in milliseconds (`swipe --duration-ms`). Accepts
/// 1..=10000; returns a user-facing error message otherwise.
pub fn parse_swipe_duration_ms(s: &str) -> Result<u32, String> {
    let ms: u32 = s.trim().parse().map_err(|_| {
        format!(
            "invalid duration '{}': expected a whole number of milliseconds (1-10000)",
            s
        )
    })?;
    if !(1..=10000).contains(&ms) {
        return Err(format!(
            "invalid duration {}: must be between 1 and 10000 milliseconds",
            ms
        ));
    }
    Ok(ms)
}

impl ActionType {
    /// Returns a short, static name for this action type suitable for use in
    /// tracing span metadata. Avoids Debug-formatting large enum payloads.
    pub fn name(&self) -> &'static str {
        match self {
            ActionType::Tap { .. } => "tap",
            ActionType::TapLocation { .. } => "tap_location",
            ActionType::Swipe { .. } => "swipe",
            ActionType::SwipeFromElement { .. } => "swipe_from_element",
            ActionType::SwipeCoords { .. } => "swipe_coords",
            ActionType::Lock => "lock",
            ActionType::Key { .. } => "key",
            ActionType::LongPress { .. } => "long_press",
            ActionType::LogComment { .. } => "log_comment",
            ActionType::GetScreenshot => "get_screenshot",
            ActionType::GetScreenInfo => "get_screen_info",
            ActionType::GetScreenTree => "get_screen_tree",
            ActionType::GetValue { .. } => "get_value",
            ActionType::Exists { .. } => "exists",
            ActionType::ScrollTo { .. } => "scroll_to",
            ActionType::SendKeys { .. } => "send_keys",
            ActionType::WaitFor { .. } => "wait_for",
            ActionType::WaitForNot { .. } => "wait_for_not",
            ActionType::WaitStable { .. } => "wait_stable",
            ActionType::SetTarget { .. } => "set_target",
            ActionType::StartTarget { .. } => "start_target",
            ActionType::StopTarget => "stop_target",
            ActionType::GetTargetInfo => "get_target_info",
            ActionType::MemoryInfo { .. } => "memory_info",
            ActionType::ShutdownDevice => "shutdown_device",
            ActionType::DeleteDevice => "delete_device",
            ActionType::InstallApp { .. } => "install_app",
            ActionType::UninstallApp { .. } => "uninstall_app",
            ActionType::AppContainer { .. } => "app_container",
            ActionType::ListApps => "list_apps",
            ActionType::CreateDevice { .. } => "create_device",
            ActionType::WaitForBoot => "wait_for_boot",
            ActionType::DeviceLog { .. } => "device_log",
            ActionType::SetAppearance { .. } => "set_appearance",
            ActionType::SetContentSize { .. } => "set_content_size",
            ActionType::GrantPermission { .. } => "grant_permission",
            ActionType::AddMedia { .. } => "add_media",
            ActionType::OpenUrl { .. } => "open_url",
            ActionType::StartSession => "start_session",
            ActionType::EndSession => "end_session",
            ActionType::Quit => "quit",
        }
    }

    /// Returns a human-friendly display name for CLI output.
    pub fn display_name(&self) -> &'static str {
        match self {
            ActionType::Tap { .. } | ActionType::TapLocation { .. } => "Tap",
            ActionType::Swipe { .. }
            | ActionType::SwipeFromElement { .. }
            | ActionType::SwipeCoords { .. } => "Swipe",
            ActionType::Lock => "Lock",
            ActionType::Key { .. } => "Key",
            ActionType::LongPress { .. } => "LongPress",
            ActionType::LogComment { .. } => "Comment",
            ActionType::GetScreenshot => "Screenshot",
            ActionType::GetScreenInfo | ActionType::GetScreenTree => "ScreenInfo",
            ActionType::GetValue { .. } => "GetValue",
            ActionType::Exists { .. } => "Exists",
            ActionType::ScrollTo { .. } => "ScrollTo",
            ActionType::SendKeys { .. } => "Type",
            ActionType::WaitFor { .. } => "Find",
            ActionType::WaitForNot { .. } => "Gone",
            ActionType::WaitStable { .. } => "Stable",
            ActionType::SetTarget { .. } => "Target",
            ActionType::StartTarget { .. } => "StartTarget",
            ActionType::StopTarget => "StopTarget",
            ActionType::GetTargetInfo => "TargetInfo",
            ActionType::MemoryInfo { .. } => "MemoryInfo",
            ActionType::ShutdownDevice => "ShutdownDevice",
            ActionType::DeleteDevice => "DeleteDevice",
            ActionType::InstallApp { .. } => "InstallApp",
            ActionType::UninstallApp { .. } => "UninstallApp",
            ActionType::AppContainer { .. } => "AppContainer",
            ActionType::ListApps => "ListApps",
            ActionType::CreateDevice { .. } => "CreateDevice",
            ActionType::WaitForBoot => "WaitForBoot",
            ActionType::DeviceLog { .. } => "DeviceLog",
            ActionType::SetAppearance { .. } => "SetAppearance",
            ActionType::SetContentSize { .. } => "SetContentSize",
            ActionType::GrantPermission { .. } => "GrantPermission",
            ActionType::AddMedia { .. } => "AddMedia",
            ActionType::OpenUrl { .. } => "OpenUrl",
            ActionType::StartSession => "Start",
            ActionType::EndSession => "End",
            ActionType::Quit => "Quit",
        }
    }

    /// Returns a formatted target string for CLI output.
    pub fn display_target(&self) -> String {
        match self {
            ActionType::WaitFor {
                label_match: Some(m),
                ..
            }
            | ActionType::WaitForNot {
                label_match: Some(m),
                ..
            }
            | ActionType::Exists {
                label_match: Some(m),
                ..
            }
            | ActionType::ScrollTo {
                label_match: Some(m),
                ..
            } => m.to_string(),
            ActionType::Tap {
                selector, by_label, ..
            }
            | ActionType::WaitFor {
                selector, by_label, ..
            }
            | ActionType::WaitForNot {
                selector, by_label, ..
            }
            | ActionType::GetValue {
                selector, by_label, ..
            }
            | ActionType::Exists {
                selector, by_label, ..
            }
            | ActionType::ScrollTo {
                selector, by_label, ..
            } => {
                if *by_label {
                    format!("label:'{}'", selector)
                } else {
                    selector.clone()
                }
            }
            ActionType::WaitStable { stable_ms, .. } => {
                format!("tree unchanged for {}ms", stable_ms)
            }
            ActionType::TapLocation { x, y } => format!("({},{})", x, y),
            ActionType::Swipe {
                direction,
                duration_ms,
            } => match duration_ms {
                Some(ms) => format!("{} ({} ms)", direction, ms),
                None => direction.clone(),
            },
            ActionType::SwipeFromElement {
                selector,
                by_label,
                label_match,
                direction,
                duration_ms,
                ..
            } => {
                let from = match label_match {
                    Some(m) => m.to_string(),
                    None if *by_label => format!("label:'{}'", selector),
                    None => selector.clone(),
                };
                match duration_ms {
                    Some(ms) => format!("{} from {} ({} ms)", direction.as_str(), from, ms),
                    None => format!("{} from {}", direction.as_str(), from),
                }
            }
            ActionType::Key { name } => name.as_str().to_string(),
            ActionType::SwipeCoords {
                from_x,
                from_y,
                to_x,
                to_y,
                duration_ms,
            } => match duration_ms {
                Some(ms) => format!("({},{})->({},{}) ({} ms)", from_x, from_y, to_x, to_y, ms),
                None => format!("({},{})->({},{})", from_x, from_y, to_x, to_y),
            },
            ActionType::LongPress { x, y, duration } => format!("({},{}) {:.1}s", x, y, duration),
            ActionType::SendKeys { text, .. } => {
                if text.len() > 20 {
                    format!("'{}..'", &text[..18])
                } else {
                    format!("'{}'", text)
                }
            }
            ActionType::LogComment { message } => message.clone(),
            ActionType::SetTarget { bundle_id } => bundle_id.clone(),
            ActionType::InstallApp { path } => path.clone(),
            ActionType::UninstallApp { bundle_id } => bundle_id.clone(),
            ActionType::AppContainer {
                bundle_id,
                container,
            } => format!("{} {}", bundle_id, container.as_deref().unwrap_or("app")),
            ActionType::CreateDevice { name, .. } => name.clone(),
            ActionType::DeviceLog { last, .. } => last.clone(),
            ActionType::SetAppearance { appearance } => appearance.clone(),
            ActionType::SetContentSize { size } => size.clone(),
            ActionType::GrantPermission {
                verb,
                service,
                bundle_id,
            } => format!("{} {} {}", verb, service, bundle_id),
            ActionType::AddMedia { paths } => paths.join(" "),
            ActionType::OpenUrl { url } => url.clone(),
            ActionType::StartTarget { .. }
            | ActionType::StopTarget
            | ActionType::Lock
            | ActionType::GetTargetInfo
            | ActionType::MemoryInfo { .. }
            | ActionType::ShutdownDevice
            | ActionType::DeleteDevice
            | ActionType::ListApps
            | ActionType::WaitForBoot => String::new(),
            _ => String::new(),
        }
    }
}

/// A logged action with metadata.
///
/// Each action executed through the REPL is logged with a unique identifier,
/// timestamp, the action details, result, and an optional screenshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionLog {
    /// Unique identifier for this log entry.
    pub id: Uuid,

    /// When the action was executed.
    pub timestamp: DateTime<Utc>,

    /// The action that was performed.
    pub action: ActionType,

    /// The result of the action.
    pub result: ActionResult,

    /// Screenshot captured after the action (base64-encoded PNG).
    ///
    /// Wrapped in `Arc` for efficient cloning when broadcasting to multiple watchers.
    pub screenshot: Option<Arc<String>>,

    /// How long the action took in milliseconds (e.g., for `WaitFor`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,

    /// Time spent waiting for the element to appear and become hittable (milliseconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait_ms: Option<u64>,

    /// Time spent executing the tap via the automation agent (milliseconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tap_ms: Option<u64>,

    /// Optional free-text tag for log filtering/analysis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

impl ActionLog {
    /// Creates a new action log entry.
    ///
    /// The entry is assigned a new UUID and timestamped with the current time.
    ///
    /// # Arguments
    ///
    /// * `action` - The action that was performed
    /// * `result` - The result of the action
    /// * `screenshot` - Optional base64-encoded PNG screenshot
    ///
    /// # Returns
    ///
    /// A new `ActionLog` instance with a unique ID and current timestamp.
    pub fn new(
        action: ActionType,
        result: ActionResult,
        screenshot: Option<Arc<String>>,
        duration_ms: Option<u64>,
        tag: Option<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            action,
            result,
            screenshot,
            duration_ms,
            wait_ms: None,
            tap_ms: None,
            tag,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_to_serde_round_trip_and_optional_fields_skipped() {
        let action = ActionType::ScrollTo {
            selector: "row-12".into(),
            by_label: false,
            element_type: None,
            label_match: None,
            direction: ScrollDirection::Up,
            max_swipes: 10,
            duration_ms: None,
        };
        let json = serde_json::to_string(&action).unwrap();
        assert!(!json.contains("label_match"), "{}", json);
        assert!(!json.contains("duration_ms"), "{}", json);
        assert_eq!(
            serde_json::to_string(&serde_json::from_str::<ActionType>(&json).unwrap()).unwrap(),
            json
        );
        // Optional fields absent on the wire still parse.
        let minimal = r#"{"type":"ScrollTo","selector":"x","by_label":true,"element_type":null,"direction":"down","max_swipes":3}"#;
        match serde_json::from_str::<ActionType>(minimal).unwrap() {
            ActionType::ScrollTo {
                label_match,
                duration_ms,
                max_swipes,
                ..
            } => {
                assert_eq!((label_match, duration_ms, max_swipes), (None, None, 3));
            }
            _ => panic!("expected ScrollTo"),
        }
        assert_eq!(action.name(), "scroll_to");
        assert_eq!(action.display_target(), "row-12");
    }

    #[test]
    fn label_match_field_is_additive_on_the_wire() {
        // Default path: no `label_match` key, and old JSON still parses.
        let action = ActionType::Exists {
            selector: "x".into(),
            by_label: false,
            element_type: None,
            label_match: None,
        };
        let json = serde_json::to_string(&action).unwrap();
        assert!(!json.contains("label_match"), "{}", json);
        let old = r#"{"type":"WaitForNot","selector":"x","by_label":true,"element_type":null,"timeout_ms":5}"#;
        match serde_json::from_str::<ActionType>(old).unwrap() {
            ActionType::WaitForNot { label_match, .. } => assert_eq!(label_match, None),
            _ => panic!("expected WaitForNot"),
        }
        // Round trip with a match.
        let m = LabelMatch {
            matcher: LabelMatcher::Regex("^a$".into()),
            nth: Some(2),
        };
        let action = ActionType::Exists {
            selector: String::new(),
            by_label: false,
            element_type: None,
            label_match: Some(Box::new(m.clone())),
        };
        let back: ActionType =
            serde_json::from_str(&serde_json::to_string(&action).unwrap()).unwrap();
        match back {
            ActionType::Exists { label_match, .. } => assert_eq!(label_match, Some(Box::new(m))),
            _ => panic!("expected Exists"),
        }
        assert_eq!(action.display_target(), "label matches /^a$/ [2]");
    }

    #[test]
    fn parse_coordinate_accepts_and_rejects() {
        assert_eq!(parse_coordinate("10,20"), Ok((10, 20)));
        assert_eq!(parse_coordinate(" 10 , 20"), Ok((10, 20)));
        assert_eq!(
            parse_coordinate("10;20").unwrap_err(),
            "invalid coordinate '10;20': expected x,y"
        );
        assert!(parse_coordinate("10,").is_err());
        assert!(parse_coordinate("a,b").is_err());
        assert!(parse_coordinate("-1,5")
            .unwrap_err()
            .contains("non-negative"));
    }

    #[test]
    fn legacy_start_target_log_entry_without_force_deserializes() {
        // Action logs written before `force` existed recorded `StartTarget` as
        // a bare tag. `qorvex convert` reads those files, so they must still
        // parse — and replay as the non-forcing form.
        let legacy = r#"{"type":"StartTarget"}"#;
        let action: ActionType = serde_json::from_str(legacy).unwrap();
        match action {
            ActionType::StartTarget { force } => assert!(!force),
            other => panic!("expected StartTarget, got {other:?}"),
        }
    }
}
