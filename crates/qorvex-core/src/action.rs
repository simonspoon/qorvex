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
//! - **Waiting**: [`ActionType::WaitFor`]
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

    /// Send keyboard input.
    SendKeys {
        /// The text to type.
        text: String,
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

impl ActionType {
    /// Returns a short, static name for this action type suitable for use in
    /// tracing span metadata. Avoids Debug-formatting large enum payloads.
    pub fn name(&self) -> &'static str {
        match self {
            ActionType::Tap { .. } => "tap",
            ActionType::TapLocation { .. } => "tap_location",
            ActionType::Swipe { .. } => "swipe",
            ActionType::LongPress { .. } => "long_press",
            ActionType::LogComment { .. } => "log_comment",
            ActionType::GetScreenshot => "get_screenshot",
            ActionType::GetScreenInfo => "get_screen_info",
            ActionType::GetValue { .. } => "get_value",
            ActionType::SendKeys { .. } => "send_keys",
            ActionType::WaitFor { .. } => "wait_for",
            ActionType::WaitForNot { .. } => "wait_for_not",
            ActionType::SetTarget { .. } => "set_target",
            ActionType::StartTarget { .. } => "start_target",
            ActionType::StopTarget => "stop_target",
            ActionType::GetTargetInfo => "get_target_info",
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
            ActionType::Swipe { .. } => "Swipe",
            ActionType::LongPress { .. } => "LongPress",
            ActionType::LogComment { .. } => "Comment",
            ActionType::GetScreenshot => "Screenshot",
            ActionType::GetScreenInfo => "ScreenInfo",
            ActionType::GetValue { .. } => "GetValue",
            ActionType::SendKeys { .. } => "Type",
            ActionType::WaitFor { .. } => "Find",
            ActionType::WaitForNot { .. } => "Gone",
            ActionType::SetTarget { .. } => "Target",
            ActionType::StartTarget { .. } => "StartTarget",
            ActionType::StopTarget => "StopTarget",
            ActionType::GetTargetInfo => "TargetInfo",
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
            } => {
                if *by_label {
                    format!("label:'{}'", selector)
                } else {
                    selector.clone()
                }
            }
            ActionType::TapLocation { x, y } => format!("({},{})", x, y),
            ActionType::Swipe { direction } => direction.clone(),
            ActionType::LongPress { x, y, duration } => format!("({},{}) {:.1}s", x, y, duration),
            ActionType::SendKeys { text } => {
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
            | ActionType::GetTargetInfo
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
