use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Result of an action execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActionResult {
    Success,
    Failure(String),
}

/// Types of actions that can be performed
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ActionType {
    /// Tap an element by its accessibility ID
    TapElement { id: String },
    /// Tap at specific x,y coordinates
    TapLocation { x: i32, y: i32 },
    /// Log a comment with optional screenshot
    LogComment { message: String },
    /// Get current screenshot (returns base64 PNG)
    GetScreenshot,
    /// Get all element information for current screen
    GetScreenInfo,
    /// Get the value of a specific element
    GetElementValue { id: String },
    /// Send keystrokes
    SendKeys { text: String },
    /// Start a new session
    StartSession,
    /// End session but keep REPL open
    EndSession,
    /// Quit the REPL entirely
    Quit,
}

/// A logged action with timestamp, result, and optional screenshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionLog {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub action: ActionType,
    pub result: ActionResult,
    /// Base64-encoded PNG screenshot taken after action
    pub screenshot: Option<String>,
}

impl ActionLog {
    pub fn new(action: ActionType, result: ActionResult, screenshot: Option<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            action,
            result,
            screenshot,
        }
    }
}
