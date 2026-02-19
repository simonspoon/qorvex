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
//! };
//!
//! // Create a log entry
//! let log = ActionLog::new(action, ActionResult::Success, None, None);
//! println!("Action {} at {}", log.id, log.timestamp);
//! ```

use std::sync::Arc;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
            ActionType::StartSession => "start_session",
            ActionType::EndSession => "end_session",
            ActionType::Quit => "quit",
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
    pub fn new(action: ActionType, result: ActionResult, screenshot: Option<Arc<String>>, duration_ms: Option<u64>) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            action,
            result,
            screenshot,
            duration_ms,
            wait_ms: None,
            tap_ms: None,
        }
    }
}
