//! Action execution for iOS Simulator automation.
//!
//! This module provides the [`ActionExecutor`] type, which handles the actual
//! execution of automation actions against a simulator. It abstracts the execution
//! logic from the REPL and IPC server, making it reusable.
//!
//! # Example
//!
//! ```no_run
//! use qorvex_core::executor::ActionExecutor;
//! use qorvex_core::action::ActionType;
//!
//! #[tokio::main]
//! async fn main() {
//!     let executor = ActionExecutor::new("SIMULATOR-UDID".to_string());
//!
//!     let result = executor.execute(ActionType::TapElement {
//!         id: "login-button".to_string()
//!     }).await;
//!
//!     if result.success {
//!         println!("Tapped successfully!");
//!     }
//! }
//! ```

use crate::action::ActionType;
use crate::axe::Axe;
use crate::simctl::Simctl;
use std::time::{Duration, Instant};

/// Result of executing an action.
///
/// Contains success/failure status along with optional data returned
/// by the action (screenshot, element value, screen info, etc.).
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    /// Whether the action completed successfully.
    pub success: bool,
    /// Human-readable description of the result.
    pub message: String,
    /// Screenshot captured after the action (base64-encoded PNG).
    pub screenshot: Option<String>,
    /// Additional data returned by the action (JSON for screen info, element values, etc.).
    pub data: Option<String>,
}

impl ExecutionResult {
    /// Creates a successful result with a message.
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: message.into(),
            screenshot: None,
            data: None,
        }
    }

    /// Creates a failure result with an error message.
    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: message.into(),
            screenshot: None,
            data: None,
        }
    }

    /// Adds a screenshot to the result.
    pub fn with_screenshot(mut self, screenshot: String) -> Self {
        self.screenshot = Some(screenshot);
        self
    }

    /// Adds data to the result.
    pub fn with_data(mut self, data: String) -> Self {
        self.data = Some(data);
        self
    }
}

/// Executes automation actions against a simulator.
///
/// The executor holds the target simulator's UDID and provides methods
/// to execute various [`ActionType`]s. It handles all the low-level
/// interaction with `simctl` and `axe`.
pub struct ActionExecutor {
    /// The UDID of the target simulator.
    simulator_udid: String,
}

impl ActionExecutor {
    /// Creates a new executor for the specified simulator.
    ///
    /// # Arguments
    ///
    /// * `simulator_udid` - The unique device identifier of the target simulator
    pub fn new(simulator_udid: String) -> Self {
        Self { simulator_udid }
    }

    /// Returns the simulator UDID.
    pub fn simulator_udid(&self) -> &str {
        &self.simulator_udid
    }

    /// Captures a screenshot and returns it as base64-encoded PNG.
    fn capture_screenshot(&self) -> Option<String> {
        Simctl::screenshot(&self.simulator_udid).ok().map(|bytes| {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        })
    }

    /// Executes an action and returns the result.
    ///
    /// This method handles all [`ActionType`] variants except session management
    /// actions (`StartSession`, `EndSession`, `Quit`), which should be handled
    /// by the caller.
    ///
    /// # Arguments
    ///
    /// * `action` - The action to execute
    ///
    /// # Returns
    ///
    /// An [`ExecutionResult`] containing success/failure status, a message,
    /// and optionally a screenshot or additional data.
    pub async fn execute(&self, action: ActionType) -> ExecutionResult {
        match action {
            ActionType::TapElement { ref id } => {
                match Axe::tap_element(&self.simulator_udid, id) {
                    Ok(_) => {
                        let mut result = ExecutionResult::success(format!("Tapped element '{}'", id));
                        if let Some(screenshot) = self.capture_screenshot() {
                            result = result.with_screenshot(screenshot);
                        }
                        result
                    }
                    Err(e) => ExecutionResult::failure(e.to_string()),
                }
            }

            ActionType::TapLocation { x, y } => {
                // Validate coordinates
                if x < 0 || y < 0 {
                    return ExecutionResult::failure(format!(
                        "Coordinates must be non-negative (got x={}, y={})",
                        x, y
                    ));
                }

                match Axe::tap(&self.simulator_udid, x, y) {
                    Ok(_) => {
                        let mut result = ExecutionResult::success(format!("Tapped at ({}, {})", x, y));
                        if let Some(screenshot) = self.capture_screenshot() {
                            result = result.with_screenshot(screenshot);
                        }
                        result
                    }
                    Err(e) => ExecutionResult::failure(e.to_string()),
                }
            }

            ActionType::SendKeys { ref text } => {
                match Simctl::send_keys(&self.simulator_udid, text) {
                    Ok(_) => {
                        let mut result = ExecutionResult::success(format!("Sent keys: '{}'", text));
                        if let Some(screenshot) = self.capture_screenshot() {
                            result = result.with_screenshot(screenshot);
                        }
                        result
                    }
                    Err(e) => ExecutionResult::failure(e.to_string()),
                }
            }

            ActionType::GetScreenshot => {
                match Simctl::screenshot(&self.simulator_udid) {
                    Ok(bytes) => {
                        use base64::Engine;
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                        ExecutionResult::success("Screenshot captured")
                            .with_screenshot(b64.clone())
                            .with_data(b64)
                    }
                    Err(e) => ExecutionResult::failure(e.to_string()),
                }
            }

            ActionType::GetScreenInfo => {
                match Axe::dump_hierarchy(&self.simulator_udid) {
                    Ok(hierarchy) => {
                        let elements = Axe::list_elements(&hierarchy);
                        match serde_json::to_string(&elements) {
                            Ok(json) => {
                                let mut result = ExecutionResult::success("Screen info retrieved")
                                    .with_data(json);
                                if let Some(screenshot) = self.capture_screenshot() {
                                    result = result.with_screenshot(screenshot);
                                }
                                result
                            }
                            Err(e) => ExecutionResult::failure(format!("JSON serialization error: {}", e)),
                        }
                    }
                    Err(e) => ExecutionResult::failure(e.to_string()),
                }
            }

            ActionType::GetElementValue { ref id } => {
                match Axe::get_element_value(&self.simulator_udid, id) {
                    Ok(Some(value)) => {
                        let mut result = ExecutionResult::success(format!("Got value for '{}'", id))
                            .with_data(value);
                        if let Some(screenshot) = self.capture_screenshot() {
                            result = result.with_screenshot(screenshot);
                        }
                        result
                    }
                    Ok(None) => {
                        let mut result = ExecutionResult::success(format!("Element '{}' has no value", id))
                            .with_data("null".to_string());
                        if let Some(screenshot) = self.capture_screenshot() {
                            result = result.with_screenshot(screenshot);
                        }
                        result
                    }
                    Err(e) => ExecutionResult::failure(e.to_string()),
                }
            }

            ActionType::LogComment { ref message } => {
                ExecutionResult::success(format!("Logged: {}", message))
            }

            ActionType::WaitFor { ref id, timeout_ms } => {
                let start = Instant::now();
                let timeout = Duration::from_millis(timeout_ms);
                let poll_interval = Duration::from_millis(100);

                loop {
                    if let Ok(elements) = Axe::dump_hierarchy(&self.simulator_udid) {
                        if Axe::find_element(&elements, id).is_some() {
                            let elapsed_ms = start.elapsed().as_millis() as u64;
                            let mut result = ExecutionResult::success(
                                format!("Element '{}' found", id)
                            ).with_data(format!(r#"{{"elapsed_ms":{}}}"#, elapsed_ms));
                            if let Some(screenshot) = self.capture_screenshot() {
                                result = result.with_screenshot(screenshot);
                            }
                            return result;
                        }
                    }
                    if start.elapsed() >= timeout {
                        let elapsed_ms = start.elapsed().as_millis() as u64;
                        return ExecutionResult::failure(format!(
                            "Timeout after {}ms waiting for element '{}'", elapsed_ms, id
                        )).with_data(format!(r#"{{"elapsed_ms":{}}}"#, elapsed_ms));
                    }
                    tokio::time::sleep(poll_interval).await;
                }
            }

            // Session management actions should be handled by the caller
            ActionType::StartSession | ActionType::EndSession | ActionType::Quit => {
                ExecutionResult::failure("Session management actions must be handled by the session manager")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execution_result_success() {
        let result = ExecutionResult::success("test message");
        assert!(result.success);
        assert_eq!(result.message, "test message");
        assert!(result.screenshot.is_none());
        assert!(result.data.is_none());
    }

    #[test]
    fn test_execution_result_failure() {
        let result = ExecutionResult::failure("error message");
        assert!(!result.success);
        assert_eq!(result.message, "error message");
    }

    #[test]
    fn test_execution_result_with_screenshot() {
        let result = ExecutionResult::success("ok")
            .with_screenshot("base64data".to_string());
        assert!(result.success);
        assert_eq!(result.screenshot, Some("base64data".to_string()));
    }

    #[test]
    fn test_execution_result_with_data() {
        let result = ExecutionResult::success("ok")
            .with_data("{\"key\": \"value\"}".to_string());
        assert!(result.success);
        assert_eq!(result.data, Some("{\"key\": \"value\"}".to_string()));
    }

    #[test]
    fn test_executor_creation() {
        let executor = ActionExecutor::new("test-udid".to_string());
        assert_eq!(executor.simulator_udid(), "test-udid");
    }

    #[tokio::test]
    async fn test_log_comment_always_succeeds() {
        let executor = ActionExecutor::new("fake-udid".to_string());
        let result = executor.execute(ActionType::LogComment {
            message: "test comment".to_string(),
        }).await;

        assert!(result.success);
        assert!(result.message.contains("test comment"));
    }

    #[tokio::test]
    async fn test_session_actions_return_error() {
        let executor = ActionExecutor::new("fake-udid".to_string());

        let result = executor.execute(ActionType::StartSession).await;
        assert!(!result.success);
        assert!(result.message.contains("session manager"));

        let result = executor.execute(ActionType::EndSession).await;
        assert!(!result.success);

        let result = executor.execute(ActionType::Quit).await;
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_tap_location_negative_coordinates() {
        let executor = ActionExecutor::new("fake-udid".to_string());

        let result = executor.execute(ActionType::TapLocation { x: -10, y: 100 }).await;
        assert!(!result.success);
        assert!(result.message.contains("non-negative"));
    }
}
