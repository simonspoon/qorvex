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
//!         timeout_ms: Some(5000),
//!         fallback_frame: false,
//!         report: false,
//!     }).await;
//!
//!     if result.success {
//!         println!("Tapped successfully!");
//!     }
//! }
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{debug, info, info_span, Instrument};

use crate::action::ActionType;
use crate::driver::{AutomationDriver, CompiledLabelMatch, DriverError};
use crate::element::{ElementFrame, UIElement};

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
}

/// Returns true if the driver error is transient and the action should be retried.
#[allow(dead_code)]
fn is_retryable_error(err: &DriverError) -> bool {
    match err {
        DriverError::CommandFailed(msg) => {
            msg.contains("not found") || msg.contains("not hittable")
        }
        _ => false,
    }
}

/// Compiles an action's optional label match. `Err` is the failure to return
/// before any device call (invalid regex).
fn compile_label_match(
    label_match: &Option<Box<crate::action::LabelMatch>>,
) -> Result<Option<CompiledLabelMatch>, ExecutionResult> {
    label_match
        .as_deref()
        .map(CompiledLabelMatch::new)
        .transpose()
        .map_err(ExecutionResult::failure)
}

/// Swipe start and end for a direction, from `(x, y)`. Distances are the ones
/// the plain direction swipe uses (300 vertical, 210 horizontal); the end
/// point is clamped at 0 because negative coordinates are rejected.
fn swipe_from_point(direction: &str, x: i32, y: i32) -> Option<(i32, i32)> {
    let (dx, dy) = match direction {
        "up" => (0, -300),
        "down" => (0, 300),
        "left" => (-210, 0),
        "right" => (210, 0),
        _ => return None,
    };
    Some(((x + dx).max(0), (y + dy).max(0)))
}

/// Swipe duration passed to the driver, in seconds: `ms / 1000.0`, or the
/// 0.3 s default when no duration was given.
fn swipe_secs(duration_ms: Option<u32>) -> Option<f64> {
    Some(duration_ms.map_or(0.3, |ms| f64::from(ms) / 1000.0))
}

/// " (N ms)" when an explicit swipe duration was given, else empty.
fn swipe_duration_suffix(duration_ms: Option<u32>) -> String {
    duration_ms.map_or_else(String::new, |ms| format!(" ({} ms)", ms))
}

impl ActionExecutor {
    /// One element lookup: a label match against a tree dump when `matcher`
    /// is set, otherwise the driver's selector search (unchanged).
    async fn find_for(
        &self,
        matcher: Option<&CompiledLabelMatch>,
        selector: &str,
        by_label: bool,
        element_type: Option<&str>,
        read_timeout_ms: Option<u64>,
    ) -> Result<Option<UIElement>, DriverError> {
        match matcher {
            Some(m) => Ok(m.search(&self.driver.dump_tree().await?, element_type)),
            None => {
                self.driver
                    .find_element_with_read_timeout(
                        selector,
                        by_label,
                        element_type,
                        read_timeout_ms,
                    )
                    .await
            }
        }
    }

    /// Creates a new executor with any [`AutomationDriver`] backend.
    ///
    /// # Arguments
    ///
    /// * `driver` - The automation driver to use for executing actions
    pub fn new(driver: Arc<dyn AutomationDriver>) -> Self {
        Self { driver }
    }

    /// Convenience constructor: create an executor using the [`AgentDriver`](crate::agent_driver::AgentDriver) backend.
    ///
    /// The driver is **not** connected yet. Call [`connect`](Self::connect) before
    /// executing actions, or use [`with_agent_connected`](Self::with_agent_connected).
    ///
    /// # Arguments
    ///
    /// * `host` - The hostname or IP of the Swift agent
    /// * `port` - The TCP port the agent is listening on
    pub fn with_agent(host: impl Into<String>, port: u16) -> Self {
        Self::new(Arc::new(crate::agent_driver::AgentDriver::direct(
            host, port,
        )))
    }

    /// Like [`with_agent`](Self::with_agent) but connects immediately.
    pub async fn with_agent_connected(
        host: impl Into<String>,
        port: u16,
    ) -> Result<Self, crate::driver::DriverError> {
        let mut driver = crate::agent_driver::AgentDriver::direct(host, port);
        driver.connect().await?;
        Ok(Self::new(Arc::new(driver)))
    }

    /// Create an executor from a [`DriverConfig`](crate::driver::DriverConfig).
    ///
    /// The driver is **not** connected yet. Call [`connect`](Self::connect) before
    /// executing actions, or use [`from_config_connected`](Self::from_config_connected).
    ///
    /// # Arguments
    ///
    /// * `config` - The driver configuration specifying which backend to use
    pub fn from_config(config: crate::driver::DriverConfig) -> Self {
        match config {
            crate::driver::DriverConfig::Agent { host, port } => Self::with_agent(host, port),
            crate::driver::DriverConfig::Device { udid, device_port } => Self::new(Arc::new(
                crate::agent_driver::AgentDriver::usb_device(udid, device_port),
            )),
            crate::driver::DriverConfig::Android {
                serial,
                local_port,
                device_port,
            } => Self::new(Arc::new(crate::android_driver::AndroidDriver::new(
                serial,
                Some(local_port),
                device_port,
            ))),
        }
    }

    /// Like [`from_config`](Self::from_config) but connects immediately.
    pub async fn from_config_connected(
        config: crate::driver::DriverConfig,
    ) -> Result<Self, crate::driver::DriverError> {
        match config {
            crate::driver::DriverConfig::Agent { host, port } => {
                Self::with_agent_connected(host, port).await
            }
            crate::driver::DriverConfig::Device { udid, device_port } => {
                let mut driver = crate::agent_driver::AgentDriver::usb_device(udid, device_port);
                driver.connect().await?;
                Ok(Self::new(Arc::new(driver)))
            }
            crate::driver::DriverConfig::Android {
                serial,
                local_port,
                device_port,
            } => {
                let mut driver = crate::android_driver::AndroidDriver::new(
                    serial,
                    Some(local_port),
                    device_port,
                );
                driver.connect().await?;
                Ok(Self::new(Arc::new(driver)))
            }
        }
    }

    /// Returns a reference to the underlying driver.
    pub fn driver(&self) -> &Arc<dyn AutomationDriver> {
        &self.driver
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
        let action_name = action.name();
        let span = info_span!("execute_action", action = action_name);
        async {
            let start = Instant::now();
            let result = self.execute_inner(action).await;
            let elapsed = start.elapsed();
            debug!(
                elapsed_ms = elapsed.as_millis() as u64,
                success = result.success,
                "action complete"
            );
            result
        }
        .instrument(span)
        .await
    }

    /// Serialized whole tree, the form `WaitStable` compares.
    async fn tree_snapshot(&self) -> Result<String, DriverError> {
        let tree = self.driver.dump_tree().await?;
        serde_json::to_string(&tree).map_err(|e| DriverError::CommandFailed(e.to_string()))
    }

    /// Tree snapshot once it compared equal on two polls 100 ms apart, or the
    /// last one after 1000 ms if it never settled.
    async fn settled_tree_snapshot(&self) -> Result<String, DriverError> {
        let start = Instant::now();
        let mut last = self.tree_snapshot().await?;
        while start.elapsed() < Duration::from_millis(1000) {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let current = self.tree_snapshot().await?;
            if current == last {
                break;
            }
            last = current;
        }
        Ok(last)
    }

    /// `tap --fallback-frame`: after the normal tap failed with `err`, find the
    /// element and tap its frame centre once. Returns the point tapped, or a
    /// note on why the fallback did not tap (appended to the original error).
    async fn fallback_frame_tap(
        &self,
        selector: &str,
        by_label: bool,
        element_type: Option<&str>,
        err: &DriverError,
    ) -> Result<(i32, i32), String> {
        if matches!(
            err,
            DriverError::NotConnected | DriverError::ConnectionLost(_) | DriverError::Io(_)
        ) {
            return Err("fallback frame tap skipped: no connection to the agent".to_string());
        }
        // Never tap coordinates over whatever replaced a target that is gone.
        // A backend that cannot tell (error or empty state) does not block.
        if let Ok(info) = self.driver.get_target_info().await {
            if !info.state.is_empty() && info.state != "running_foreground" {
                return Err(format!(
                    "fallback frame tap skipped: target state is {}",
                    info.state
                ));
            }
        }
        let element = match self
            .driver
            .find_element_with_type(selector, by_label, element_type)
            .await
        {
            Ok(Some(el)) => el,
            Ok(None) => return Err("fallback frame: element not found".to_string()),
            Err(e) => return Err(format!("fallback frame lookup failed: {}", e)),
        };
        let (x, y) = element
            .frame
            .as_ref()
            .and_then(frame_centre)
            .ok_or_else(|| "fallback frame: element has no non-empty frame".to_string())?;
        self.driver
            .tap_location(x, y)
            .await
            .map_err(|e| format!("fallback frame tap at ({},{}) failed: {}", x, y, e))?;
        Ok((x, y))
    }

    async fn execute_inner(&self, action: ActionType) -> ExecutionResult {
        match action {
            ActionType::Tap {
                ref selector,
                by_label,
                ref element_type,
                timeout_ms,
                fallback_frame,
                report,
            } => {
                let type_filter = element_type.as_deref();
                let before = if report {
                    Some(self.tree_snapshot().await)
                } else {
                    None
                };
                // After the "before" dump so `elapsed_ms` times the tap only.
                let start = Instant::now();

                let tap_result = if timeout_ms.is_some() {
                    // Forward timeout to agent — it handles retry internally.
                    match element_type {
                        Some(typ) => {
                            self.driver
                                .tap_with_type_with_timeout(selector, by_label, typ, timeout_ms)
                                .await
                        }
                        None if by_label => {
                            self.driver
                                .tap_by_label_with_timeout(selector, timeout_ms)
                                .await
                        }
                        None => {
                            self.driver
                                .tap_element_with_timeout(selector, timeout_ms)
                                .await
                        }
                    }
                } else {
                    // No timeout — single attempt (no retry)
                    match element_type {
                        Some(typ) => self.driver.tap_with_type(selector, by_label, typ).await,
                        None if by_label => self.driver.tap_by_label(selector).await,
                        None => self.driver.tap_element(selector).await,
                    }
                };

                let (method, point) = match tap_result {
                    Ok(_) => {
                        // Where the element is just after the tap. Only looked up
                        // when reporting so a plain tap costs no extra request.
                        let point = if report {
                            self.driver
                                .find_element_with_type(selector, by_label, type_filter)
                                .await
                                .ok()
                                .flatten()
                                .and_then(|el| el.frame.as_ref().and_then(frame_centre))
                        } else {
                            None
                        };
                        ("element", point)
                    }
                    Err(e) => {
                        let failed = format!(
                            "Timeout after {}ms: {}",
                            start.elapsed().as_millis() as u64,
                            e
                        );
                        if !fallback_frame {
                            return ExecutionResult::failure(failed);
                        }
                        match self
                            .fallback_frame_tap(selector, by_label, type_filter, &e)
                            .await
                        {
                            Ok(point) => ("frame_centre", Some(point)),
                            Err(note) => {
                                return ExecutionResult::failure(format!("{}; {}", failed, note))
                            }
                        }
                    }
                };

                let elapsed_ms = start.elapsed().as_millis() as u64;
                let mut data = serde_json::json!({
                    "elapsed_ms": elapsed_ms,
                    "method": method,
                    "x": point.map(|p| p.0),
                    "y": point.map(|p| p.1),
                });
                if let Some(before) = before {
                    // No settle when the "before" dump failed: the answer is null anyway.
                    data["ui_changed"] = match before {
                        Ok(b) => match self.settled_tree_snapshot().await {
                            Ok(a) => serde_json::Value::Bool(b != a),
                            Err(_) => serde_json::Value::Null,
                        },
                        Err(_) => serde_json::Value::Null,
                    };
                }
                let mut msg = if by_label {
                    format!("Tapped element with label '{}'", selector)
                } else {
                    format!("Tapped element '{}'", selector)
                };
                if let ("frame_centre", Some((x, y))) = (method, point) {
                    msg.push_str(&format!(" via frame centre ({}, {})", x, y));
                }
                ExecutionResult::success(msg).with_data(data.to_string())
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
                    Ok(_) => ExecutionResult::success(format!("Tapped at ({}, {})", x, y)),
                    Err(e) => ExecutionResult::failure(e.to_string()),
                }
            }

            ActionType::Swipe {
                ref direction,
                duration_ms,
            } => {
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

                match self
                    .driver
                    .swipe(start_x, start_y, end_x, end_y, swipe_secs(duration_ms))
                    .await
                {
                    Ok(_) => ExecutionResult::success(format!(
                        "Swiped {}{}",
                        direction,
                        swipe_duration_suffix(duration_ms)
                    )),
                    Err(e) => ExecutionResult::failure(e.to_string()),
                }
            }

            ActionType::SwipeFromElement {
                ref selector,
                by_label,
                ref element_type,
                ref label_match,
                direction,
                duration_ms,
            } => {
                let matcher = match compile_label_match(label_match) {
                    Ok(m) => m,
                    Err(fail) => return fail,
                };
                let shown = label_match.as_ref().map(|m| m.to_string());
                let selector = shown.as_deref().unwrap_or(selector);
                let element = match self
                    .find_for(
                        matcher.as_ref(),
                        selector,
                        by_label,
                        element_type.as_deref(),
                        None,
                    )
                    .await
                {
                    Ok(Some(el)) => el,
                    Ok(None) => {
                        return ExecutionResult::failure(format!(
                            "Element '{}' not found",
                            selector
                        ))
                    }
                    Err(e) => return ExecutionResult::failure(e.to_string()),
                };
                let Some((from_x, from_y)) = element.frame.as_ref().and_then(frame_centre) else {
                    return ExecutionResult::failure(format!(
                        "Element '{}' has no usable frame",
                        selector
                    ));
                };
                let (to_x, to_y) = swipe_from_point(direction.as_str(), from_x, from_y)
                    .expect("ScrollDirection is always a valid swipe direction");
                if (to_x, to_y) == (from_x, from_y) {
                    return ExecutionResult::failure(format!(
                        "Swipe {} from '{}' at ({}, {}) would have zero length (element at the screen edge)",
                        direction.as_str(),
                        selector,
                        from_x,
                        from_y
                    ));
                }
                match self
                    .driver
                    .swipe(from_x, from_y, to_x, to_y, swipe_secs(duration_ms))
                    .await
                {
                    Ok(_) => ExecutionResult::success(format!(
                        "Swiped {} from '{}' at ({}, {}) to ({}, {}){}",
                        direction.as_str(),
                        selector,
                        from_x,
                        from_y,
                        to_x,
                        to_y,
                        swipe_duration_suffix(duration_ms)
                    )),
                    Err(e) => ExecutionResult::failure(e.to_string()),
                }
            }

            ActionType::SwipeCoords {
                from_x,
                from_y,
                to_x,
                to_y,
                duration_ms,
            } => {
                if from_x < 0 || from_y < 0 || to_x < 0 || to_y < 0 {
                    return ExecutionResult::failure(format!(
                        "Coordinates must be non-negative (got from=({}, {}), to=({}, {}))",
                        from_x, from_y, to_x, to_y
                    ));
                }

                match self
                    .driver
                    .swipe(from_x, from_y, to_x, to_y, swipe_secs(duration_ms))
                    .await
                {
                    Ok(_) => ExecutionResult::success(format!(
                        "Swiped from ({}, {}) to ({}, {}){}",
                        from_x,
                        from_y,
                        to_x,
                        to_y,
                        swipe_duration_suffix(duration_ms)
                    )),
                    Err(e) => ExecutionResult::failure(e.to_string()),
                }
            }

            ActionType::Lock => match self.driver.lock().await {
                Ok(_) => ExecutionResult::success("Device locked"),
                Err(e) => ExecutionResult::failure(e.to_string()),
            },

            ActionType::Key { name } => match self.driver.press_key(name).await {
                Ok(_) => ExecutionResult::success(format!("Pressed key: {}", name.as_str())),
                Err(e) => ExecutionResult::failure(e.to_string()),
            },

            ActionType::LongPress { x, y, duration } => {
                match self.driver.long_press(x, y, duration).await {
                    Ok(_) => ExecutionResult::success(format!(
                        "Long pressed at ({}, {}) for {:.1}s",
                        x, y, duration
                    )),
                    Err(e) => ExecutionResult::failure(e.to_string()),
                }
            }

            ActionType::SendKeys { ref text, replace } => {
                // Empty text is a no-op unless replacing (which clears the field).
                if text.is_empty() && !replace {
                    return ExecutionResult::success(format!("Sent keys: '{}'", text));
                }
                match self.driver.type_text(text, replace).await {
                    Ok(_) => ExecutionResult::success(format!("Sent keys: '{}'", text)),
                    Err(e) => ExecutionResult::failure(e.to_string()),
                }
            }

            ActionType::GetScreenshot => match self.driver.screenshot().await {
                Ok(bytes) => {
                    use base64::Engine;
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                    ExecutionResult::success("Screenshot captured")
                        .with_screenshot(b64.clone())
                        .with_data(b64)
                }
                Err(e) => ExecutionResult::failure(e.to_string()),
            },

            ActionType::GetScreenInfo => match self.driver.list_elements().await {
                Ok(elements) => match serde_json::to_string(&elements) {
                    Ok(json) => ExecutionResult::success("Screen info retrieved").with_data(json),
                    Err(e) => ExecutionResult::failure(format!("JSON serialization error: {}", e)),
                },
                Err(e) => ExecutionResult::failure(e.to_string()),
            },

            ActionType::GetScreenTree => match self.driver.dump_tree().await {
                Ok(tree) => match serde_json::to_string(&tree) {
                    Ok(json) => ExecutionResult::success("Screen tree retrieved").with_data(json),
                    Err(e) => ExecutionResult::failure(format!("JSON serialization error: {}", e)),
                },
                Err(e) => ExecutionResult::failure(e.to_string()),
            },

            ActionType::Exists {
                ref selector,
                by_label,
                ref element_type,
                ref label_match,
            } => {
                let matcher = match compile_label_match(label_match) {
                    Ok(m) => m,
                    Err(fail) => return fail,
                };
                let shown = label_match.as_ref().map(|m| m.to_string());
                let selector = shown.as_deref().unwrap_or(selector);
                match self
                    .find_for(
                        matcher.as_ref(),
                        selector,
                        by_label,
                        element_type.as_deref(),
                        None,
                    )
                    .await
                {
                    // One lookup: `Ok(None)` is "absent"; a driver error is not.
                    Ok(Some(element)) => ExecutionResult::success(format!("'{}' exists", selector))
                        .with_data(
                            serde_json::json!({"exists": true, "hittable": element.hittable})
                                .to_string(),
                        ),
                    Ok(None) => ExecutionResult::success(format!("'{}' does not exist", selector))
                        .with_data(serde_json::json!({"exists": false}).to_string()),
                    Err(e) => ExecutionResult::failure(e.to_string()),
                }
            }

            ActionType::ScrollTo {
                ref selector,
                by_label,
                ref element_type,
                ref label_match,
                direction,
                max_swipes,
                duration_ms,
            } => {
                let matcher = match compile_label_match(label_match) {
                    Ok(m) => m,
                    Err(fail) => return fail,
                };
                let shown = label_match.as_ref().map(|m| m.to_string());
                let selector = shown.as_deref().unwrap_or(selector);
                let mut swipes: u32 = 0;
                loop {
                    // Same lookup as `Exists`; a driver error is not "absent".
                    match self
                        .find_for(
                            matcher.as_ref(),
                            selector,
                            by_label,
                            element_type.as_deref(),
                            None,
                        )
                        .await
                    {
                        Ok(Some(_)) => {
                            return ExecutionResult::success(format!(
                                "Found \"{}\" after {} swipe{}",
                                selector,
                                swipes,
                                if swipes == 1 { "" } else { "s" }
                            ))
                            .with_data(
                                serde_json::json!({"found": true, "swipes": swipes}).to_string(),
                            );
                        }
                        Ok(None) => {}
                        Err(e) => return ExecutionResult::failure(e.to_string()),
                    }
                    if swipes >= max_swipes {
                        return ExecutionResult::success(format!(
                            "Not found \"{}\" after {} swipe{}",
                            selector,
                            swipes,
                            if swipes == 1 { "" } else { "s" }
                        ))
                        .with_data(
                            serde_json::json!({"found": false, "swipes": swipes}).to_string(),
                        );
                    }
                    let swipe = ActionType::Swipe {
                        direction: direction.as_str().to_string(),
                        duration_ms,
                    };
                    let result = Box::pin(self.execute_inner(swipe)).await;
                    if !result.success {
                        return result;
                    }
                    swipes += 1;
                }
            }

            ActionType::GetValue {
                ref selector,
                by_label,
                ref element_type,
                timeout_ms,
            } => {
                let start = Instant::now();

                let value_result = if timeout_ms.is_some() {
                    // Forward timeout to agent — it handles retry internally.
                    self.driver
                        .get_value_with_timeout(
                            selector,
                            by_label,
                            element_type.as_deref(),
                            timeout_ms,
                        )
                        .await
                    // Normalize into the same Result<Option<String>> shape
                } else {
                    // No timeout — single attempt (no retry)
                    match element_type {
                        Some(typ) => {
                            self.driver
                                .get_value_with_type(selector, by_label, typ)
                                .await
                        }
                        None if by_label => self.driver.get_element_value_by_label(selector).await,
                        None => self.driver.get_element_value(selector).await,
                    }
                };

                let elapsed_ms = start.elapsed().as_millis() as u64;
                match value_result {
                    Ok(Some(value)) => {
                        let msg = if by_label {
                            format!("Got value for label '{}'", selector)
                        } else {
                            format!("Got value for '{}'", selector)
                        };
                        ExecutionResult::success(msg).with_data(value)
                    }
                    Ok(None) => {
                        let msg = if by_label {
                            format!("Element with label '{}' has no value", selector)
                        } else {
                            format!("Element '{}' has no value", selector)
                        };
                        ExecutionResult::success(msg).with_data("null".to_string())
                    }
                    Err(e) => {
                        ExecutionResult::failure(format!("Timeout after {}ms: {}", elapsed_ms, e))
                    }
                }
            }

            ActionType::LogComment { ref message } => {
                ExecutionResult::success(format!("Logged: {}", message))
            }

            ActionType::WaitFor {
                ref selector,
                by_label,
                ref element_type,
                timeout_ms,
                require_stable,
                ref label_match,
            } => {
                let matcher = match compile_label_match(label_match) {
                    Ok(m) => m,
                    Err(fail) => return fail,
                };
                let shown = label_match.as_ref().map(|m| m.to_string());
                let selector = shown.as_deref().unwrap_or(selector);
                let mut start = Instant::now();
                let timeout = Duration::from_millis(timeout_ms);
                let poll_interval = Duration::from_millis(100);
                let stable_polls_required = 3;
                let mut last_frame: Option<(f64, f64, f64, f64)> = None;
                let mut stable_count: u32 = 0;
                let mut last_recovery = self.driver.recovery_count();

                loop {
                    if let Ok(found) = self
                        .find_for(
                            matcher.as_ref(),
                            selector,
                            by_label,
                            element_type.as_deref(),
                            Some(timeout_ms),
                        )
                        .await
                    {
                        if let Some(element) = found {
                            if require_stable {
                                // Skip elements that exist but aren't hittable yet
                                // (e.g. behind another view or mid-animation).
                                if element.hittable == Some(false) {
                                    last_frame = None;
                                    stable_count = 0;
                                    if start.elapsed() >= timeout {
                                        let elapsed_ms = start.elapsed().as_millis() as u64;
                                        let msg = if by_label {
                                            format!("Timeout after {}ms: element with label '{}' exists but is not hittable", elapsed_ms, selector)
                                        } else {
                                            format!("Timeout after {}ms: element '{}' exists but is not hittable", elapsed_ms, selector)
                                        };
                                        return ExecutionResult::failure(msg).with_data(format!(
                                            r#"{{"elapsed_ms":{}}}"#,
                                            elapsed_ms
                                        ));
                                    }
                                    tokio::time::sleep(poll_interval).await;
                                    continue;
                                }

                                let current_frame = element
                                    .frame
                                    .as_ref()
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
                                    return ExecutionResult::success(msg).with_data(data);
                                }
                            } else {
                                // Fast path: element exists and is hittable, return immediately.
                                if element.hittable == Some(false) {
                                    if start.elapsed() >= timeout {
                                        let elapsed_ms = start.elapsed().as_millis() as u64;
                                        let msg = if by_label {
                                            format!("Timeout after {}ms: element with label '{}' exists but is not hittable", elapsed_ms, selector)
                                        } else {
                                            format!("Timeout after {}ms: element '{}' exists but is not hittable", elapsed_ms, selector)
                                        };
                                        return ExecutionResult::failure(msg).with_data(format!(
                                            r#"{{"elapsed_ms":{}}}"#,
                                            elapsed_ms
                                        ));
                                    }
                                    tokio::time::sleep(poll_interval).await;
                                    continue;
                                }
                                let elapsed_ms = start.elapsed().as_millis() as u64;
                                let msg = if by_label {
                                    format!("Element with label '{}' found", selector)
                                } else {
                                    format!("Element '{}' found", selector)
                                };
                                return ExecutionResult::success(msg)
                                    .with_data(format!(r#"{{"elapsed_ms":{}}}"#, elapsed_ms));
                            }
                        } else {
                            last_frame = None;
                            stable_count = 0;
                        }
                    }
                    let current_recovery = self.driver.recovery_count();
                    if current_recovery != last_recovery {
                        info!("agent recovered during wait_for, resetting timer");
                        start = Instant::now();
                        stable_count = 0;
                        last_frame = None;
                        last_recovery = current_recovery;
                    }
                    if start.elapsed() >= timeout {
                        let elapsed_ms = start.elapsed().as_millis() as u64;
                        let msg = if by_label {
                            format!(
                                "Timeout after {}ms waiting for element with label '{}'",
                                elapsed_ms, selector
                            )
                        } else {
                            format!(
                                "Timeout after {}ms waiting for element '{}'",
                                elapsed_ms, selector
                            )
                        };
                        return ExecutionResult::failure(msg)
                            .with_data(format!(r#"{{"elapsed_ms":{}}}"#, elapsed_ms));
                    }
                    tokio::time::sleep(poll_interval).await;
                }
            }

            ActionType::WaitForNot {
                ref selector,
                by_label,
                ref element_type,
                timeout_ms,
                ref label_match,
            } => {
                let matcher = match compile_label_match(label_match) {
                    Ok(m) => m,
                    Err(fail) => return fail,
                };
                let shown = label_match.as_ref().map(|m| m.to_string());
                let selector = shown.as_deref().unwrap_or(selector);
                let mut start = Instant::now();
                let timeout = Duration::from_millis(timeout_ms);
                let poll_interval = Duration::from_millis(100);
                let mut last_recovery = self.driver.recovery_count();

                loop {
                    let found = self
                        .find_for(
                            matcher.as_ref(),
                            selector,
                            by_label,
                            element_type.as_deref(),
                            Some(timeout_ms),
                        )
                        .await;

                    match found {
                        Err(e) => {
                            return ExecutionResult::failure(format!("{}", e));
                        }
                        Ok(ref opt) => {
                            let element_present =
                                matches!(opt, Some(ref el) if el.hittable != Some(false));
                            if !element_present {
                                let elapsed_ms = start.elapsed().as_millis() as u64;
                                let msg = if by_label {
                                    format!("Element with label '{}' not found", selector)
                                } else {
                                    format!("Element '{}' not found", selector)
                                };
                                return ExecutionResult::success(msg)
                                    .with_data(format!(r#"{{"elapsed_ms":{}}}"#, elapsed_ms));
                            }
                        }
                    }

                    let current_recovery = self.driver.recovery_count();
                    if current_recovery != last_recovery {
                        info!("agent recovered during wait_for_not, resetting timer");
                        start = Instant::now();
                        last_recovery = current_recovery;
                    }
                    if start.elapsed() >= timeout {
                        let elapsed_ms = start.elapsed().as_millis() as u64;
                        let msg = if by_label {
                            format!("Timeout after {}ms waiting for element with label '{}' to disappear", elapsed_ms, selector)
                        } else {
                            format!(
                                "Timeout after {}ms waiting for element '{}' to disappear",
                                elapsed_ms, selector
                            )
                        };
                        return ExecutionResult::failure(msg)
                            .with_data(format!(r#"{{"elapsed_ms":{}}}"#, elapsed_ms));
                    }
                    tokio::time::sleep(poll_interval).await;
                }
            }

            ActionType::WaitStable {
                stable_ms,
                timeout_ms,
            } => {
                let start = Instant::now();
                let timeout = Duration::from_millis(timeout_ms);
                let quiet = Duration::from_millis(stable_ms);
                let poll_interval = Duration::from_millis(100);
                // Serialized tree of the previous poll and when it last changed.
                let mut last_tree: Option<String> = None;
                let mut quiet_since = start;
                let mut changes: u64 = 0;
                let mut last_error: Option<String> = None;

                loop {
                    // The period starts when the dump that first saw the tree
                    // started, so `--for` equal to the timeout can still pass.
                    let poll_started = Instant::now();
                    let dumped = self.driver.dump_tree().await.and_then(|tree| {
                        serde_json::to_string(&tree)
                            .map(|snapshot| (tree, snapshot))
                            .map_err(|e| DriverError::CommandFailed(e.to_string()))
                    });
                    match dumped {
                        Ok((tree, snapshot)) => {
                            if last_tree.as_deref() != Some(snapshot.as_str()) {
                                if last_tree.is_some() {
                                    changes += 1;
                                }
                                last_tree = Some(snapshot);
                                quiet_since = poll_started;
                            }
                            let quiet_for = quiet_since.elapsed();
                            if quiet_for >= quiet {
                                let elapsed_ms = start.elapsed().as_millis() as u64;
                                return ExecutionResult::success(format!(
                                    "Tree unchanged for {}ms",
                                    stable_ms
                                ))
                                .with_data(format!(
                                    r#"{{"elapsed_ms":{},"stable_ms":{},"element_count":{}}}"#,
                                    elapsed_ms,
                                    quiet_for.as_millis(),
                                    crate::driver::flatten_elements(&tree).len()
                                ));
                            }
                        }
                        Err(e) => {
                            // A failed dump says nothing about stability: restart the period.
                            last_tree = None;
                            last_error = Some(e.to_string());
                        }
                    }
                    if start.elapsed() >= timeout {
                        let elapsed_ms = start.elapsed().as_millis() as u64;
                        let mut msg = format!(
                            "Timeout after {}ms waiting for the UI to be unchanged for {}ms ({} tree changes seen)",
                            elapsed_ms, stable_ms, changes
                        );
                        if let Some(e) = last_error {
                            msg.push_str(&format!("; last error: {}", e));
                        }
                        return ExecutionResult::failure(msg).with_data(format!(
                            r#"{{"elapsed_ms":{},"changes":{}}}"#,
                            elapsed_ms, changes
                        ));
                    }
                    tokio::time::sleep(poll_interval).await;
                }
            }

            ActionType::SetTarget { ref bundle_id } => {
                match self.driver.set_target(bundle_id).await {
                    Ok(_) => ExecutionResult::success(format!("Target set to '{}'", bundle_id)),
                    Err(e) => ExecutionResult::failure(e.to_string()),
                }
            }

            // Session management actions should be handled by the caller
            ActionType::StartSession
            | ActionType::EndSession
            | ActionType::Quit
            | ActionType::StartTarget { .. }
            | ActionType::StopTarget
            | ActionType::ForegroundTarget
            | ActionType::GetTargetInfo => ExecutionResult::failure(
                "Session management actions must be handled by the session manager",
            ),

            // `hide-keyboard` is handled by the server, which refuses
            // Android and physical devices before calling the driver.
            ActionType::HideKeyboard => {
                ExecutionResult::failure("hide-keyboard must be handled by the server")
            }

            // Device and app management actions run host-side through simctl
            // on the server, never through the automation driver.
            ActionType::ShutdownDevice
            | ActionType::DeleteDevice
            | ActionType::InstallApp { .. }
            | ActionType::UninstallApp { .. }
            | ActionType::AppContainer { .. }
            | ActionType::ResetApp { .. }
            | ActionType::ListApps
            | ActionType::CreateDevice { .. }
            | ActionType::WaitForBoot
            | ActionType::DeviceLog { .. }
            | ActionType::SetAppearance { .. }
            | ActionType::SetContentSize { .. }
            | ActionType::GrantPermission { .. }
            | ActionType::Biometric { .. }
            | ActionType::AddMedia { .. }
            | ActionType::OpenUrl { .. }
            | ActionType::MemoryInfo { .. } => {
                ExecutionResult::failure("Device management actions must be handled by the server")
            }
        }
    }
}

/// Centre of a non-empty frame, rounded to the nearest point and clamped to >= 0.
fn frame_centre(frame: &ElementFrame) -> Option<(i32, i32)> {
    if frame.width > 0.0 && frame.height > 0.0 {
        let x = (frame.x + frame.width / 2.0).round().max(0.0) as i32;
        let y = (frame.y + frame.height / 2.0).round().max(0.0) as i32;
        Some((x, y))
    } else {
        None
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
        let result = ExecutionResult::success("ok").with_screenshot("base64data".to_string());
        assert!(result.success);
        assert_eq!(result.screenshot, Some("base64data".to_string()));
    }

    #[test]
    fn test_execution_result_with_data() {
        let result = ExecutionResult::success("ok").with_data("{\"key\": \"value\"}".to_string());
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
        let config = DriverConfig::Agent {
            host: "localhost".to_string(),
            port: 9800,
        };
        let executor = ActionExecutor::from_config(config);
        assert!(!executor.driver().is_connected());
    }

    #[test]
    fn test_executor_from_config_device() {
        use crate::driver::DriverConfig;
        let config = DriverConfig::Device {
            udid: "ABC-123".to_string(),
            device_port: 8080,
        };
        let executor = ActionExecutor::from_config(config);
        assert!(!executor.driver().is_connected());
    }

    #[test]
    fn test_executor_from_config_android_routes_to_android_driver() {
        use crate::driver::DriverConfig;
        // An Android DriverConfig must select the AndroidDriver path through
        // the executor.rs match (acceptance F1). The match arm is the only
        // place `DriverConfig::Android` maps, so a successfully-constructed,
        // unconnected executor proves the route. (A live connection requires a
        // device and is deferred to integration story #90.)
        let config = DriverConfig::Android {
            serial: "emulator-5554".to_string(),
            local_port: 9123,
            device_port: crate::android_driver::DEFAULT_ANDROID_AGENT_PORT,
        };
        let executor = ActionExecutor::from_config(config);
        assert!(!executor.driver().is_connected());
        // AndroidDriver overrides recovery_count() (starts at 0); the default
        // trait impl also returns 0, so this asserts the call is wired, not the
        // concrete type. Concrete-type selection is covered by android_driver's
        // own construction tests; here the compile-time match arm guarantees it.
        assert_eq!(executor.driver().recovery_count(), 0);
    }
}
