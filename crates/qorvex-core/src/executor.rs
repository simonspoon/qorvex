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
//!     let executor = ActionExecutor::with_agent("localhost", 8080);
//!
//!     let result = executor.execute(ActionType::Tap {
//!         selector: "login-button".to_string(),
//!         by_label: false,
//!         element_type: None,
//!     }).await;
//!
//!     if result.success {
//!         println!("Tapped successfully!");
//!     }
//! }
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::action::ActionType;
use crate::driver::AutomationDriver;

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
/// The executor holds an [`AutomationDriver`] and provides methods
/// to execute various [`ActionType`]s. It handles all the high-level
/// action dispatch, delegating low-level operations to the driver.
pub struct ActionExecutor {
    /// The automation driver backend.
    driver: Arc<dyn AutomationDriver>,
    /// Whether to capture screenshots after actions.
    capture_screenshots: bool,
}

impl ActionExecutor {
    /// Creates a new executor with any [`AutomationDriver`] backend.
    ///
    /// # Arguments
    ///
    /// * `driver` - The automation driver to use for executing actions
    pub fn new(driver: Arc<dyn AutomationDriver>) -> Self {
        Self {
            driver,
            capture_screenshots: true,
        }
    }

    /// Convenience constructor: create an executor using the [`AgentDriver`](crate::agent_driver::AgentDriver) backend.
    ///
    /// # Arguments
    ///
    /// * `host` - The hostname or IP of the Swift agent
    /// * `port` - The TCP port the agent is listening on
    pub fn with_agent(host: impl Into<String>, port: u16) -> Self {
        Self::new(Arc::new(crate::agent_driver::AgentDriver::direct(host, port)))
    }

    /// Create an executor from a [`DriverConfig`](crate::driver::DriverConfig).
    ///
    /// # Arguments
    ///
    /// * `config` - The driver configuration specifying which backend to use
    pub fn from_config(config: crate::driver::DriverConfig) -> Self {
        match config {
            crate::driver::DriverConfig::Agent { host, port } => Self::with_agent(host, port),
            crate::driver::DriverConfig::Device { udid, device_port } => {
                Self::new(Arc::new(crate::agent_driver::AgentDriver::usb_device(udid, device_port)))
            }
        }
    }

    /// Returns a reference to the underlying driver.
    pub fn driver(&self) -> &Arc<dyn AutomationDriver> {
        &self.driver
    }

    /// Sets whether to capture screenshots after actions.
    pub fn set_capture_screenshots(&mut self, capture: bool) {
        self.capture_screenshots = capture;
    }

    /// Captures a screenshot and returns it as base64-encoded PNG.
    /// Returns `None` if screenshot capture is disabled.
    async fn capture_screenshot(&self) -> Option<String> {
        if !self.capture_screenshots {
            return None;
        }
        self.driver.screenshot().await.ok().map(|bytes| {
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
            ActionType::Tap { ref selector, by_label, ref element_type } => {
                let tap_result = match element_type {
                    Some(typ) => self.driver.tap_with_type(selector, by_label, typ).await,
                    None if by_label => self.driver.tap_by_label(selector).await,
                    None => self.driver.tap_element(selector).await,
                };

                match tap_result {
                    Ok(_) => {
                        let msg = if by_label {
                            format!("Tapped element with label '{}'", selector)
                        } else {
                            format!("Tapped element '{}'", selector)
                        };
                        let mut result = ExecutionResult::success(msg);
                        if let Some(screenshot) = self.capture_screenshot().await {
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

                match self.driver.tap_location(x, y).await {
                    Ok(_) => {
                        let mut result = ExecutionResult::success(format!("Tapped at ({}, {})", x, y));
                        if let Some(screenshot) = self.capture_screenshot().await {
                            result = result.with_screenshot(screenshot);
                        }
                        result
                    }
                    Err(e) => ExecutionResult::failure(e.to_string()),
                }
            }

            ActionType::Swipe { ref direction } => {
                // Use reasonable default coordinates for a typical iOS screen.
                // Center horizontally (195), swipe from 600→300 for "up", etc.
                let (start_x, start_y, end_x, end_y) = match direction.as_str() {
                    "up" => (195, 600, 195, 300),
                    "down" => (195, 300, 195, 600),
                    "left" => (300, 420, 90, 420),
                    "right" => (90, 420, 300, 420),
                    _ => {
                        return ExecutionResult::failure(format!(
                            "Invalid swipe direction '{}'. Use: up, down, left, right",
                            direction
                        ));
                    }
                };

                match self.driver.swipe(start_x, start_y, end_x, end_y, Some(0.3)).await {
                    Ok(_) => {
                        let mut result = ExecutionResult::success(format!("Swiped {}", direction));
                        if let Some(screenshot) = self.capture_screenshot().await {
                            result = result.with_screenshot(screenshot);
                        }
                        result
                    }
                    Err(e) => ExecutionResult::failure(e.to_string()),
                }
            }

            ActionType::LongPress { x, y, duration } => {
                match self.driver.long_press(x, y, duration).await {
                    Ok(_) => {
                        let mut result = ExecutionResult::success(format!(
                            "Long pressed at ({}, {}) for {:.1}s", x, y, duration
                        ));
                        if let Some(screenshot) = self.capture_screenshot().await {
                            result = result.with_screenshot(screenshot);
                        }
                        result
                    }
                    Err(e) => ExecutionResult::failure(e.to_string()),
                }
            }

            ActionType::SendKeys { ref text } => {
                match self.driver.type_text(text).await {
                    Ok(_) => {
                        let mut result = ExecutionResult::success(format!("Sent keys: '{}'", text));
                        if let Some(screenshot) = self.capture_screenshot().await {
                            result = result.with_screenshot(screenshot);
                        }
                        result
                    }
                    Err(e) => ExecutionResult::failure(e.to_string()),
                }
            }

            ActionType::GetScreenshot => {
                match self.driver.screenshot().await {
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
                match self.driver.list_elements().await {
                    Ok(elements) => {
                        match serde_json::to_string(&elements) {
                            Ok(json) => {
                                let mut result = ExecutionResult::success("Screen info retrieved")
                                    .with_data(json);
                                if let Some(screenshot) = self.capture_screenshot().await {
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

            ActionType::GetValue { ref selector, by_label, ref element_type } => {
                let value_result = match element_type {
                    Some(typ) => self.driver.get_value_with_type(selector, by_label, typ).await,
                    None if by_label => self.driver.get_element_value_by_label(selector).await,
                    None => self.driver.get_element_value(selector).await,
                };

                match value_result {
                    Ok(Some(value)) => {
                        let msg = if by_label {
                            format!("Got value for label '{}'", selector)
                        } else {
                            format!("Got value for '{}'", selector)
                        };
                        let mut result = ExecutionResult::success(msg).with_data(value);
                        if let Some(screenshot) = self.capture_screenshot().await {
                            result = result.with_screenshot(screenshot);
                        }
                        result
                    }
                    Ok(None) => {
                        let msg = if by_label {
                            format!("Element with label '{}' has no value", selector)
                        } else {
                            format!("Element '{}' has no value", selector)
                        };
                        let mut result = ExecutionResult::success(msg).with_data("null".to_string());
                        if let Some(screenshot) = self.capture_screenshot().await {
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

            ActionType::WaitFor { ref selector, by_label, ref element_type, timeout_ms } => {
                let start = Instant::now();
                let timeout = Duration::from_millis(timeout_ms);
                let poll_interval = Duration::from_millis(100);
                let stable_polls_required = 3;
                let mut last_frame: Option<(f64, f64, f64, f64)> = None;
                let mut stable_count: u32 = 0;

                loop {
                    if let Ok(found) = self.driver.find_element_with_type(
                        selector,
                        by_label,
                        element_type.as_deref(),
                    ).await {
                        if let Some(element) = found {
                            let current_frame = element.frame.as_ref()
                                .map(|f| (f.x, f.y, f.width, f.height));

                            // Require the frame to be stable across multiple consecutive
                            // polls to avoid tapping during iOS animations.
                            if current_frame.is_none() {
                                stable_count = stable_polls_required;
                            } else if current_frame == last_frame {
                                stable_count += 1;
                            } else {
                                stable_count = 1;
                                last_frame = current_frame;
                            }

                            if stable_count >= stable_polls_required {
                                let elapsed_ms = start.elapsed().as_millis() as u64;
                                let msg = if by_label {
                                    format!("Element with label '{}' found", selector)
                                } else {
                                    format!("Element '{}' found", selector)
                                };
                                let data = if let Some(ref frame) = element.frame {
                                    format!(
                                        r#"{{"elapsed_ms":{},"frame":{{"x":{},"y":{},"width":{},"height":{}}}}}"#,
                                        elapsed_ms, frame.x, frame.y, frame.width, frame.height
                                    )
                                } else {
                                    format!(r#"{{"elapsed_ms":{}}}"#, elapsed_ms)
                                };
                                let mut result = ExecutionResult::success(msg).with_data(data);
                                if let Some(screenshot) = self.capture_screenshot().await {
                                    result = result.with_screenshot(screenshot);
                                }
                                return result;
                            }
                        } else {
                            last_frame = None;
                            stable_count = 0;
                        }
                    }
                    if start.elapsed() >= timeout {
                        let elapsed_ms = start.elapsed().as_millis() as u64;
                        let msg = if by_label {
                            format!("Timeout after {}ms waiting for element with label '{}'", elapsed_ms, selector)
                        } else {
                            format!("Timeout after {}ms waiting for element '{}'", elapsed_ms, selector)
                        };
                        return ExecutionResult::failure(msg)
                            .with_data(format!(r#"{{"elapsed_ms":{}}}"#, elapsed_ms));
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
    fn test_executor_creation_with_agent() {
        let executor = ActionExecutor::with_agent("localhost", 9800);
        assert!(!executor.driver().is_connected());
    }

    #[test]
    fn test_executor_from_config_agent() {
        use crate::driver::DriverConfig;
        let config = DriverConfig::Agent { host: "localhost".to_string(), port: 9800 };
        let executor = ActionExecutor::from_config(config);
        assert!(!executor.driver().is_connected());
    }

    #[test]
    fn test_executor_from_config_device() {
        use crate::driver::DriverConfig;
        let config = DriverConfig::Device { udid: "ABC-123".to_string(), device_port: 8080 };
        let executor = ActionExecutor::from_config(config);
        assert!(!executor.driver().is_connected());
    }
}
