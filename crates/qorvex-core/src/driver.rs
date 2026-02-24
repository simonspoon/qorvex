//! Automation driver trait for backend-agnostic UI automation.
//!
//! This module defines the [`AutomationDriver`] trait, which provides a common
//! interface for different automation backends (e.g., a TCP-based Swift agent
//! on a simulator or a physical device via USB tunnel). This allows the executor
//! and other consumers to work with any backend without knowing the implementation
//! details.
//!
//! # Backend Selection
//!
//! Use [`DriverConfig`] to specify which backend to use at runtime:
//!
//! ```no_run
//! use qorvex_core::driver::DriverConfig;
//!
//! // Use a TCP-based agent (simulator)
//! let config = DriverConfig::Agent {
//!     host: "localhost".to_string(),
//!     port: 9123,
//! };
//!
//! // Use a physical device via USB tunnel
//! let config = DriverConfig::Device {
//!     udid: "00008110-001A0C123456789A".to_string(),
//!     device_port: 8080,
//! };
//! ```

use async_trait::async_trait;
use thiserror::Error;

use crate::element::UIElement;

/// Errors that can occur during automation driver operations.
///
/// This enum unifies errors from all backends behind a single type,
/// allowing consumers to handle errors uniformly regardless of the
/// underlying automation backend.
#[derive(Error, Debug)]
pub enum DriverError {
    /// A command or operation failed with the given message.
    #[error("Command failed: {0}")]
    CommandFailed(String),

    /// The backend is not available or not connected.
    #[error("Not connected to automation backend")]
    NotConnected,

    /// The TCP connection to the agent was lost.
    #[error("Connection lost: {0}")]
    ConnectionLost(String),

    /// An operation timed out.
    #[error("Operation timed out")]
    Timeout,

    /// An I/O error occurred.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Failed to parse JSON data.
    #[error("JSON parse error: {0}")]
    JsonParse(String),

    /// A USB tunnel operation failed.
    #[error("USB tunnel error: {0}")]
    UsbTunnel(#[from] crate::usb_tunnel::UsbTunnelError),
}

/// Configuration for selecting an automation backend at runtime.
#[derive(Debug, Clone)]
pub enum DriverConfig {
    /// Use a TCP-based Swift agent for automation (direct connection).
    ///
    /// Typically used for simulators, where the agent is reachable on localhost.
    Agent {
        /// The hostname or IP address of the agent.
        host: String,
        /// The TCP port the agent is listening on.
        port: u16,
    },
    /// Use a Swift agent on a physical device via USB tunnel.
    ///
    /// Connects through usbmuxd to forward traffic to the agent port on the device.
    Device {
        /// The UDID of the physical device.
        udid: String,
        /// The TCP port the agent is listening on (on the device, typically 8080).
        device_port: u16,
    },
}

/// Returns true if the pattern contains glob wildcard characters (`*` or `?`).
fn has_wildcard(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?')
}

/// Matches a string against a glob pattern with `*` (any chars) and `?` (single char).
///
/// When the pattern has no wildcards, falls back to exact equality.
fn glob_match(pattern: &str, text: &str) -> bool {
    if !has_wildcard(pattern) {
        return pattern == text;
    }

    let pat: Vec<char> = pattern.chars().collect();
    let txt: Vec<char> = text.chars().collect();
    let (plen, tlen) = (pat.len(), txt.len());

    // dp[i][j] = pattern[..i] matches text[..j]
    let mut dp = vec![vec![false; tlen + 1]; plen + 1];
    dp[0][0] = true;

    // Leading *'s can match empty text
    for i in 1..=plen {
        if pat[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
    }

    for i in 1..=plen {
        for j in 1..=tlen {
            if pat[i - 1] == '*' {
                // * matches zero chars (dp[i-1][j]) or one more char (dp[i][j-1])
                dp[i][j] = dp[i - 1][j] || dp[i][j - 1];
            } else if pat[i - 1] == '?' || pat[i - 1] == txt[j - 1] {
                dp[i][j] = dp[i - 1][j - 1];
            }
        }
    }

    dp[plen][tlen]
}

/// Recursively searches a UI element hierarchy for an element matching by identifier.
///
/// Supports glob wildcard patterns (`*` and `?`) in the identifier.
fn search_by_identifier(elements: &[UIElement], identifier: &str) -> Option<UIElement> {
    for element in elements {
        if element
            .identifier
            .as_deref()
            .map_or(false, |id| glob_match(identifier, id))
        {
            return Some(element.clone());
        }
        if let Some(found) = search_by_identifier(&element.children, identifier) {
            return Some(found);
        }
    }
    None
}

/// Recursively searches a UI element hierarchy for an element matching by label.
///
/// Supports glob wildcard patterns (`*` and `?`) in the label.
fn search_by_label(elements: &[UIElement], label: &str) -> Option<UIElement> {
    for element in elements {
        if element
            .label
            .as_deref()
            .map_or(false, |l| glob_match(label, l))
        {
            return Some(element.clone());
        }
        if let Some(found) = search_by_label(&element.children, label) {
            return Some(found);
        }
    }
    None
}

/// Recursively searches a UI element hierarchy by selector (ID or label) with optional type filter.
///
/// Supports glob wildcard patterns (`*` and `?`) in the selector.
fn search_with_type(
    elements: &[UIElement],
    selector: &str,
    by_label: bool,
    element_type: Option<&str>,
) -> Option<UIElement> {
    for element in elements {
        // Check if this element matches the selector (supports glob wildcards)
        let selector_matches = if by_label {
            element
                .label
                .as_deref()
                .map_or(false, |l| glob_match(selector, l))
        } else {
            element
                .identifier
                .as_deref()
                .map_or(false, |id| glob_match(selector, id))
        };

        // Check if type matches (if type filter is specified)
        let type_matches = match element_type {
            Some(typ) => element.element_type.as_deref() == Some(typ),
            None => true,
        };

        if selector_matches && type_matches {
            return Some(element.clone());
        }

        // Recurse into children
        if let Some(found) = search_with_type(&element.children, selector, by_label, element_type) {
            return Some(found);
        }
    }
    None
}

/// Flattens a UI element hierarchy into a list of actionable elements.
///
/// Recursively traverses the element tree and collects all elements that have
/// either an accessibility identifier or a label. Elements without both are
/// excluded, as they are typically not directly actionable.
///
/// This is the standalone equivalent of the list_elements logic, used in the
/// default [`AutomationDriver::list_elements`] implementation.
///
/// # Arguments
///
/// * `elements` - The root elements of the hierarchy to flatten
///
/// # Returns
///
/// A `Vec<UIElement>` containing all elements with identifiers or labels.
pub fn flatten_elements(elements: &[UIElement]) -> Vec<UIElement> {
    let mut result = Vec::new();
    collect_elements(elements, &mut result);
    result
}

fn collect_elements(elements: &[UIElement], result: &mut Vec<UIElement>) {
    for element in elements {
        if element.identifier.is_some() || element.label.is_some() {
            result.push(element.clone());
        }
        collect_elements(&element.children, result);
    }
}

/// Trait for backend-agnostic iOS Simulator UI automation.
///
/// Implementors provide the core automation capabilities (tapping, swiping,
/// hierarchy inspection, etc.) using their specific backend. The trait includes
/// default implementations for element search methods that work by fetching the
/// full hierarchy via [`dump_tree`](AutomationDriver::dump_tree) and searching
/// locally. Backends that support server-side search can override these for
/// better performance.
///
/// All methods that interact with the device are async to support both
/// synchronous CLI tools (wrapped in `spawn_blocking`) and async TCP agents.
///
/// # Required Methods
///
/// Implementors must provide: [`connect`](AutomationDriver::connect),
/// [`is_connected`](AutomationDriver::is_connected),
/// [`tap_location`](AutomationDriver::tap_location),
/// [`tap_element`](AutomationDriver::tap_element),
/// [`tap_by_label`](AutomationDriver::tap_by_label),
/// [`tap_with_type`](AutomationDriver::tap_with_type),
/// [`swipe`](AutomationDriver::swipe),
/// [`long_press`](AutomationDriver::long_press),
/// [`type_text`](AutomationDriver::type_text),
/// [`dump_tree`](AutomationDriver::dump_tree),
/// [`get_element_value`](AutomationDriver::get_element_value),
/// [`get_element_value_by_label`](AutomationDriver::get_element_value_by_label),
/// [`get_value_with_type`](AutomationDriver::get_value_with_type),
/// and [`screenshot`](AutomationDriver::screenshot).
#[async_trait]
pub trait AutomationDriver: Send + Sync {
    /// Establish connection to the automation backend.
    ///
    /// Verifies the backend is available.
    async fn connect(&mut self) -> Result<(), DriverError>;

    /// Check if the backend is ready to accept commands.
    fn is_connected(&self) -> bool;

    /// Tap at specific screen coordinates.
    ///
    /// # Arguments
    ///
    /// * `x` - The x-coordinate in screen points
    /// * `y` - The y-coordinate in screen points
    async fn tap_location(&self, x: i32, y: i32) -> Result<(), DriverError>;

    /// Tap an element by its accessibility identifier.
    ///
    /// # Arguments
    ///
    /// * `identifier` - The accessibility identifier (AXUniqueId) of the element
    async fn tap_element(&self, identifier: &str) -> Result<(), DriverError>;

    /// Tap an element by its accessibility label.
    ///
    /// # Arguments
    ///
    /// * `label` - The accessibility label (AXLabel) of the element
    async fn tap_by_label(&self, label: &str) -> Result<(), DriverError>;

    /// Tap an element matching a selector with an element type filter.
    ///
    /// # Arguments
    ///
    /// * `selector` - The value to match (accessibility ID or label)
    /// * `by_label` - If true, match against label; if false, match against ID
    /// * `element_type` - The element type to filter by (e.g., "Button", "TextField")
    async fn tap_with_type(
        &self,
        selector: &str,
        by_label: bool,
        element_type: &str,
    ) -> Result<(), DriverError>;

    /// Tap an element by its accessibility identifier, with agent-side retry.
    ///
    /// When `timeout_ms` is `Some`, the agent retries locally until the
    /// element is found and tapped, or the timeout is reached. This avoids
    /// per-attempt TCP round-trips compared to Rust-side polling.
    ///
    /// The default implementation ignores the timeout and delegates to
    /// [`tap_element`](Self::tap_element).
    async fn tap_element_with_timeout(
        &self,
        identifier: &str,
        timeout_ms: Option<u64>,
    ) -> Result<(), DriverError> {
        let _ = timeout_ms;
        self.tap_element(identifier).await
    }

    /// Tap an element by its accessibility label, with agent-side retry.
    ///
    /// See [`tap_element_with_timeout`](Self::tap_element_with_timeout) for details.
    async fn tap_by_label_with_timeout(
        &self,
        label: &str,
        timeout_ms: Option<u64>,
    ) -> Result<(), DriverError> {
        let _ = timeout_ms;
        self.tap_by_label(label).await
    }

    /// Tap an element with a type filter, with agent-side retry.
    ///
    /// See [`tap_element_with_timeout`](Self::tap_element_with_timeout) for details.
    async fn tap_with_type_with_timeout(
        &self,
        selector: &str,
        by_label: bool,
        element_type: &str,
        timeout_ms: Option<u64>,
    ) -> Result<(), DriverError> {
        let _ = timeout_ms;
        self.tap_with_type(selector, by_label, element_type).await
    }

    /// Perform a swipe gesture from one point to another.
    ///
    /// # Arguments
    ///
    /// * `start_x` - Starting x-coordinate
    /// * `start_y` - Starting y-coordinate
    /// * `end_x` - Ending x-coordinate
    /// * `end_y` - Ending y-coordinate
    /// * `duration` - Optional swipe duration in seconds
    async fn swipe(
        &self,
        start_x: i32,
        start_y: i32,
        end_x: i32,
        end_y: i32,
        duration: Option<f64>,
    ) -> Result<(), DriverError>;

    /// Perform a long press at specific screen coordinates.
    ///
    /// # Arguments
    ///
    /// * `x` - The x-coordinate in screen points
    /// * `y` - The y-coordinate in screen points
    /// * `duration` - How long to press in seconds
    async fn long_press(&self, x: i32, y: i32, duration: f64) -> Result<(), DriverError>;

    /// Type text into the currently focused element.
    ///
    /// # Arguments
    ///
    /// * `text` - The text to type
    async fn type_text(&self, text: &str) -> Result<(), DriverError>;

    /// Get the full UI element hierarchy.
    ///
    /// Returns the root elements of the accessibility tree for the current
    /// screen. Each element may contain nested children.
    async fn dump_tree(&self) -> Result<Vec<UIElement>, DriverError>;

    /// Get a flattened list of actionable elements.
    ///
    /// Returns all elements from the hierarchy that have either an accessibility
    /// identifier or a label. This is useful for listing what's on screen.
    ///
    /// The default implementation calls [`dump_tree`](Self::dump_tree) and
    /// flattens the result using [`flatten_elements`]. Backends may override
    /// this for better performance.
    async fn list_elements(&self) -> Result<Vec<UIElement>, DriverError> {
        let tree = self.dump_tree().await?;
        Ok(flatten_elements(&tree))
    }

    /// Find an element by its accessibility identifier.
    ///
    /// The default implementation calls [`dump_tree`](Self::dump_tree) and
    /// searches the hierarchy locally. Backends that support server-side
    /// search can override this for better performance.
    ///
    /// # Arguments
    ///
    /// * `identifier` - The accessibility identifier to find
    async fn find_element(
        &self,
        identifier: &str,
    ) -> Result<Option<UIElement>, DriverError> {
        let tree = self.dump_tree().await?;
        Ok(search_by_identifier(&tree, identifier))
    }

    /// Find an element by its accessibility label.
    ///
    /// The default implementation calls [`dump_tree`](Self::dump_tree) and
    /// searches the hierarchy locally. Backends that support server-side
    /// search can override this for better performance.
    ///
    /// # Arguments
    ///
    /// * `label` - The accessibility label to find
    async fn find_element_by_label(
        &self,
        label: &str,
    ) -> Result<Option<UIElement>, DriverError> {
        let tree = self.dump_tree().await?;
        Ok(search_by_label(&tree, label))
    }

    /// Find an element by selector with optional type filter.
    ///
    /// The default implementation calls [`dump_tree`](Self::dump_tree) and
    /// searches the hierarchy locally. Backends that support server-side
    /// search can override this for better performance.
    ///
    /// # Arguments
    ///
    /// * `selector` - The value to match (accessibility ID or label)
    /// * `by_label` - If true, match against label; if false, match against ID
    /// * `element_type` - Optional element type filter
    async fn find_element_with_type(
        &self,
        selector: &str,
        by_label: bool,
        element_type: Option<&str>,
    ) -> Result<Option<UIElement>, DriverError> {
        let tree = self.dump_tree().await?;
        Ok(search_with_type(&tree, selector, by_label, element_type))
    }

    /// Get an element's value by its accessibility identifier.
    ///
    /// # Arguments
    ///
    /// * `identifier` - The accessibility identifier of the element
    ///
    /// # Returns
    ///
    /// `Ok(Some(String))` if the element has a value, `Ok(None)` if it has no value.
    async fn get_element_value(
        &self,
        identifier: &str,
    ) -> Result<Option<String>, DriverError>;

    /// Get an element's value by its accessibility label.
    ///
    /// # Arguments
    ///
    /// * `label` - The accessibility label of the element
    ///
    /// # Returns
    ///
    /// `Ok(Some(String))` if the element has a value, `Ok(None)` if it has no value.
    async fn get_element_value_by_label(
        &self,
        label: &str,
    ) -> Result<Option<String>, DriverError>;

    /// Get an element's value with a type filter.
    ///
    /// # Arguments
    ///
    /// * `selector` - The value to match (accessibility ID or label)
    /// * `by_label` - If true, match against label; if false, match against ID
    /// * `element_type` - The element type to filter by
    ///
    /// # Returns
    ///
    /// `Ok(Some(String))` if the element has a value, `Ok(None)` if it has no value.
    async fn get_value_with_type(
        &self,
        selector: &str,
        by_label: bool,
        element_type: &str,
    ) -> Result<Option<String>, DriverError>;

    /// Get an element's value with agent-side retry.
    ///
    /// When `timeout_ms` is `Some`, the agent retries locally until the
    /// element is found, or the timeout is reached.
    ///
    /// The default implementation ignores the timeout and delegates to
    /// the appropriate get-value method.
    async fn get_value_with_timeout(
        &self,
        selector: &str,
        by_label: bool,
        element_type: Option<&str>,
        timeout_ms: Option<u64>,
    ) -> Result<Option<String>, DriverError> {
        let _ = timeout_ms;
        match element_type {
            Some(typ) => self.get_value_with_type(selector, by_label, typ).await,
            None if by_label => self.get_element_value_by_label(selector).await,
            None => self.get_element_value(selector).await,
        }
    }

    /// Returns the number of successful recovery events since creation.
    ///
    /// Backends that support automatic reconnection / respawn should override
    /// this to expose their recovery counter so the executor can reset
    /// wait timers after a recovery.  The default returns `0` (no recovery
    /// tracking).
    fn recovery_count(&self) -> u64 {
        0
    }

    /// Capture a screenshot of the current simulator screen.
    ///
    /// # Returns
    ///
    /// Raw PNG image bytes.
    async fn screenshot(&self) -> Result<Vec<u8>, DriverError>;

    /// Set the target application for accessibility queries.
    ///
    /// Not all backends support this. The default implementation returns
    /// an error.
    async fn set_target(&self, _bundle_id: &str) -> Result<(), DriverError> {
        Err(DriverError::CommandFailed("set_target not supported by this backend".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::element::ElementFrame;

    #[test]
    fn test_driver_error_display() {
        let err = DriverError::CommandFailed("tap failed".to_string());
        assert!(err.to_string().contains("tap failed"));

        let err = DriverError::NotConnected;
        assert!(err.to_string().contains("Not connected"));

        let err = DriverError::ConnectionLost("reset by peer".to_string());
        assert!(err.to_string().contains("reset by peer"));

        let err = DriverError::Timeout;
        assert!(err.to_string().contains("timed out"));

        let err = DriverError::JsonParse("unexpected token".to_string());
        assert!(err.to_string().contains("unexpected token"));
    }

    #[test]
    fn test_driver_config_variants() {
        let config = DriverConfig::Agent {
            host: "localhost".to_string(),
            port: 9123,
        };
        match config {
            DriverConfig::Agent { ref host, port } => {
                assert_eq!(host, "localhost");
                assert_eq!(port, 9123);
            }
            _ => panic!("Expected Agent variant"),
        }

        // Verify Clone works
        let cloned = config.clone();
        assert!(matches!(cloned, DriverConfig::Agent { .. }));

        let config = DriverConfig::Device {
            udid: "00008110-001A0C123456789A".to_string(),
            device_port: 8080,
        };
        match config {
            DriverConfig::Device {
                ref udid,
                device_port,
            } => {
                assert_eq!(udid, "00008110-001A0C123456789A");
                assert_eq!(device_port, 8080);
            }
            _ => panic!("Expected Device variant"),
        }
    }

    #[test]
    fn test_flatten_elements_basic() {
        let elements = vec![UIElement {
            identifier: Some("root".to_string()),
            label: Some("Root".to_string()),
            value: None,
            element_type: Some("View".to_string()),
            frame: None,
            children: vec![
                UIElement {
                    identifier: Some("child-1".to_string()),
                    label: None,
                    value: None,
                    element_type: Some("Button".to_string()),
                    frame: None,
                    children: vec![],
                    role: None,
                    hittable: None,
                },
                UIElement {
                    identifier: None,
                    label: Some("Label Only".to_string()),
                    value: None,
                    element_type: Some("StaticText".to_string()),
                    frame: None,
                    children: vec![],
                    role: None,
                    hittable: None,
                },
            ],
            role: None,
                    hittable: None,
        }];

        let flat = flatten_elements(&elements);
        assert_eq!(flat.len(), 3);
        assert_eq!(flat[0].identifier.as_deref(), Some("root"));
        assert_eq!(flat[1].identifier.as_deref(), Some("child-1"));
        assert_eq!(flat[2].label.as_deref(), Some("Label Only"));
    }

    #[test]
    fn test_flatten_elements_excludes_unlabeled() {
        let elements = vec![UIElement {
            identifier: None,
            label: None,
            value: None,
            element_type: Some("View".to_string()),
            frame: None,
            children: vec![UIElement {
                identifier: Some("included".to_string()),
                label: None,
                value: None,
                element_type: Some("Button".to_string()),
                frame: None,
                children: vec![],
                role: None,
                    hittable: None,
            }],
            role: None,
                    hittable: None,
        }];

        let flat = flatten_elements(&elements);
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].identifier.as_deref(), Some("included"));
    }

    #[test]
    fn test_flatten_elements_empty() {
        let elements: Vec<UIElement> = vec![];
        let flat = flatten_elements(&elements);
        assert!(flat.is_empty());
    }

    #[test]
    fn test_flatten_elements_deeply_nested() {
        let elements = vec![UIElement {
            identifier: Some("level0".to_string()),
            label: None,
            value: None,
            element_type: None,
            frame: None,
            children: vec![UIElement {
                identifier: None,
                label: None,
                value: None,
                element_type: None,
                frame: None,
                children: vec![UIElement {
                    identifier: Some("level2".to_string()),
                    label: None,
                    value: None,
                    element_type: None,
                    frame: None,
                    children: vec![UIElement {
                        identifier: Some("level3".to_string()),
                        label: Some("Deep".to_string()),
                        value: None,
                        element_type: Some("Button".to_string()),
                        frame: Some(ElementFrame {
                            x: 10.0,
                            y: 20.0,
                            width: 100.0,
                            height: 44.0,
                        }),
                        children: vec![],
                        role: None,
                    hittable: None,
                    }],
                    role: None,
                    hittable: None,
                }],
                role: None,
                    hittable: None,
            }],
            role: None,
                    hittable: None,
        }];

        let flat = flatten_elements(&elements);
        // level0 (has id), level1 skipped (no id or label), level2 (has id), level3 (has both)
        assert_eq!(flat.len(), 3);
        assert_eq!(flat[0].identifier.as_deref(), Some("level0"));
        assert_eq!(flat[1].identifier.as_deref(), Some("level2"));
        assert_eq!(flat[2].identifier.as_deref(), Some("level3"));
    }

    #[test]
    fn test_flatten_elements_with_mixed_hierarchy() {
        // Verify flatten_elements correctly handles a hierarchy with mixed
        // identifiable and non-identifiable elements, including nesting.
        let elements = vec![UIElement {
            identifier: Some("main-view".to_string()),
            label: Some("Main View".to_string()),
            value: None,
            element_type: Some("View".to_string()),
            frame: None,
            children: vec![
                UIElement {
                    identifier: Some("login-button".to_string()),
                    label: Some("Log In".to_string()),
                    value: None,
                    element_type: Some("Button".to_string()),
                    frame: None,
                    children: vec![],
                    role: None,
                    hittable: None,
                },
                UIElement {
                    identifier: None,
                    label: None,
                    value: None,
                    element_type: Some("View".to_string()),
                    frame: None,
                    children: vec![UIElement {
                        identifier: Some("nested".to_string()),
                        label: None,
                        value: None,
                        element_type: Some("TextField".to_string()),
                        frame: None,
                        children: vec![],
                        role: None,
                    hittable: None,
                    }],
                    role: None,
                    hittable: None,
                },
            ],
            role: None,
                    hittable: None,
        }];

        let flat = flatten_elements(&elements);

        // Should include: main-view (has id+label), login-button (has id+label),
        // nested (has id). The anonymous View container (no id, no label) is excluded.
        assert_eq!(flat.len(), 3);
        assert_eq!(flat[0].identifier.as_deref(), Some("main-view"));
        assert_eq!(flat[1].identifier.as_deref(), Some("login-button"));
        assert_eq!(flat[2].identifier.as_deref(), Some("nested"));
    }

    #[test]
    fn test_search_by_identifier_exact() {
        let elements = vec![UIElement {
            identifier: Some("root".to_string()),
            label: None,
            value: None,
            element_type: None,
            frame: None,
            children: vec![UIElement {
                identifier: Some("child-btn".to_string()),
                label: Some("Click Me".to_string()),
                value: None,
                element_type: Some("Button".to_string()),
                frame: None,
                children: vec![],
                role: None,
                    hittable: None,
            }],
            role: None,
                    hittable: None,
        }];

        let found = search_by_identifier(&elements, "child-btn");
        assert!(found.is_some());
        assert_eq!(found.unwrap().identifier.as_deref(), Some("child-btn"));

        let not_found = search_by_identifier(&elements, "nonexistent");
        assert!(not_found.is_none());
    }

    #[test]
    fn test_search_by_identifier_glob() {
        let elements = vec![UIElement {
            identifier: Some("login-button".to_string()),
            label: None,
            value: None,
            element_type: None,
            frame: None,
            children: vec![UIElement {
                identifier: Some("email-field".to_string()),
                label: None,
                value: None,
                element_type: None,
                frame: None,
                children: vec![],
                role: None,
                    hittable: None,
            }],
            role: None,
                    hittable: None,
        }];

        let found = search_by_identifier(&elements, "login-*");
        assert!(found.is_some());
        assert_eq!(found.unwrap().identifier.as_deref(), Some("login-button"));

        let found = search_by_identifier(&elements, "*-field");
        assert!(found.is_some());
        assert_eq!(found.unwrap().identifier.as_deref(), Some("email-field"));
    }

    #[test]
    fn test_search_by_label_exact() {
        let elements = vec![UIElement {
            identifier: None,
            label: Some("Submit".to_string()),
            value: None,
            element_type: None,
            frame: None,
            children: vec![],
            role: None,
                    hittable: None,
        }];

        let found = search_by_label(&elements, "Submit");
        assert!(found.is_some());

        let not_found = search_by_label(&elements, "Cancel");
        assert!(not_found.is_none());
    }

    #[test]
    fn test_search_by_label_glob() {
        let elements = vec![UIElement {
            identifier: None,
            label: Some("Log In".to_string()),
            value: None,
            element_type: None,
            frame: None,
            children: vec![],
            role: None,
                    hittable: None,
        }];

        let found = search_by_label(&elements, "Log*");
        assert!(found.is_some());
        assert_eq!(found.unwrap().label.as_deref(), Some("Log In"));
    }

    #[test]
    fn test_search_with_type_by_id_and_type() {
        let elements = vec![UIElement {
            identifier: Some("submit-btn".to_string()),
            label: Some("Submit".to_string()),
            value: None,
            element_type: Some("Button".to_string()),
            frame: None,
            children: vec![],
            role: None,
                    hittable: None,
        }];

        // Match by ID with correct type
        let found = search_with_type(&elements, "submit-btn", false, Some("Button"));
        assert!(found.is_some());

        // Match by ID with wrong type
        let found = search_with_type(&elements, "submit-btn", false, Some("TextField"));
        assert!(found.is_none());

        // Match by label with no type filter
        let found = search_with_type(&elements, "Submit", true, None);
        assert!(found.is_some());
    }

    #[test]
    fn test_glob_match_exact() {
        assert!(glob_match("hello", "hello"));
        assert!(!glob_match("hello", "world"));
    }

    #[test]
    fn test_glob_match_star() {
        assert!(glob_match("Log*", "Log In"));
        assert!(glob_match("Log*", "Login"));
        assert!(glob_match("Log*", "Log"));
        assert!(!glob_match("Log*", "Blog"));
    }

    #[test]
    fn test_glob_match_question_mark() {
        assert!(glob_match("Item ?", "Item 1"));
        assert!(glob_match("Item ?", "Item A"));
        assert!(!glob_match("Item ?", "Item 12"));
    }

    #[test]
    fn test_glob_match_combined() {
        assert!(glob_match("Tab ?*", "Tab 1 Selected"));
        assert!(glob_match("Tab ?*", "Tab 1"));
        assert!(!glob_match("Tab ?*", "Tab "));
    }
}
