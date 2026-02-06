//! Interface to the `axe` accessibility tool for UI automation.
//!
//! This module provides a Rust wrapper around the `axe` CLI tool, which enables
//! accessibility-based UI inspection and interaction with iOS Simulator apps.
//!
//! # Requirements
//!
//! The `axe` tool must be installed:
//! ```bash
//! brew install cameroncooke/axe/axe
//! ```
//!
//! # Example
//!
//! ```no_run
//! use qorvex_core::axe::Axe;
//!
//! // Check if axe is available
//! if Axe::is_installed() {
//!     let udid = "SIMULATOR-UDID";
//!
//!     // Dump the UI hierarchy
//!     let hierarchy = Axe::dump_hierarchy(udid).unwrap();
//!
//!     // Find an element by accessibility ID
//!     if let Some(button) = Axe::find_element(&hierarchy, "login-button") {
//!         println!("Found button: {:?}", button.label);
//!     }
//!
//!     // Tap an element
//!     Axe::tap_element(udid, "login-button").unwrap();
//! }
//! ```

use std::process::Command;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors that can occur when interacting with the axe tool.
#[derive(Error, Debug)]
pub enum AxeError {
    /// An axe command failed to execute successfully.
    #[error("Command execution failed: {0}")]
    CommandFailed(String),

    /// The axe tool is not installed on the system.
    #[error("axe tool not found - install with: brew install cameroncooke/axe/axe")]
    NotInstalled,

    /// Failed to parse JSON output from axe.
    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),

    /// An I/O error occurred while executing the command.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Represents a UI element from the accessibility hierarchy.
///
/// This struct contains accessibility information about a UI element as
/// reported by the `axe describe-ui` command. Elements form a tree structure
/// via the `children` field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UIElement {
    /// The unique accessibility identifier for this element (AXUniqueId).
    #[serde(rename = "AXUniqueId", default)]
    pub identifier: Option<String>,

    /// The accessibility label (AXLabel), typically the user-visible text.
    #[serde(rename = "AXLabel", default)]
    pub label: Option<String>,

    /// The current value of the element (AXValue), e.g., text field contents.
    #[serde(rename = "AXValue", default)]
    pub value: Option<String>,

    /// The type of UI element (e.g., "Button", "TextField", "View").
    #[serde(rename = "type", default)]
    pub element_type: Option<String>,

    /// The element's frame (position and size) in screen coordinates.
    #[serde(default)]
    pub frame: Option<ElementFrame>,

    /// Child elements nested within this element.
    #[serde(default)]
    pub children: Vec<UIElement>,

    /// The accessibility role of this element.
    #[serde(default)]
    pub role: Option<String>,
}

/// The frame (position and dimensions) of a UI element.
///
/// Coordinates are in screen points, with the origin at the top-left
/// corner of the screen.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementFrame {
    /// The x-coordinate of the element's top-left corner.
    pub x: f64,
    /// The y-coordinate of the element's top-left corner.
    pub y: f64,
    /// The width of the element in points.
    pub width: f64,
    /// The height of the element in points.
    pub height: f64,
}

/// Wrapper for `axe` CLI commands.
///
/// Provides static methods for interacting with the iOS Simulator UI
/// through accessibility APIs. All methods are synchronous and execute
/// shell commands.
pub struct Axe;

impl Axe {
    /// Checks if the axe tool is installed and available.
    ///
    /// Uses `which axe` to determine if the tool exists in the PATH.
    ///
    /// # Returns
    ///
    /// `true` if axe is installed and available, `false` otherwise.
    pub fn is_installed() -> bool {
        Command::new("which")
            .arg("axe")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Dumps the complete UI accessibility hierarchy.
    ///
    /// Executes `axe describe-ui` to retrieve the full accessibility tree
    /// of the current screen in the specified simulator.
    ///
    /// # Arguments
    ///
    /// * `udid` - The unique device identifier of the target simulator
    ///
    /// # Returns
    ///
    /// A `Vec<UIElement>` representing the root elements of the UI tree.
    /// Each element may contain nested children.
    ///
    /// # Errors
    ///
    /// - [`AxeError::NotInstalled`] if axe is not available
    /// - [`AxeError::Io`] if the command fails to execute
    /// - [`AxeError::CommandFailed`] if axe returns an error
    /// - [`AxeError::JsonParse`] if the output cannot be parsed
    pub fn dump_hierarchy(udid: &str) -> Result<Vec<UIElement>, AxeError> {
        if !Self::is_installed() {
            return Err(AxeError::NotInstalled);
        }

        let output = Command::new("axe")
            .args(["describe-ui", "--udid", udid])
            .output()?;

        if !output.status.success() {
            return Err(AxeError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string()
            ));
        }

        let hierarchy: Vec<UIElement> = serde_json::from_slice(&output.stdout)?;
        Ok(hierarchy)
    }

    /// Finds an element by its accessibility identifier.
    ///
    /// Performs a recursive depth-first search through the element hierarchy
    /// to find an element with a matching `AXUniqueId`.
    ///
    /// # Arguments
    ///
    /// * `elements` - The root elements to search through
    /// * `identifier` - The accessibility identifier to find
    ///
    /// # Returns
    ///
    /// `Some(UIElement)` containing a clone of the found element,
    /// or `None` if no matching element exists.
    pub fn find_element(elements: &[UIElement], identifier: &str) -> Option<UIElement> {
        for element in elements {
            if element.identifier.as_deref() == Some(identifier) {
                return Some(element.clone());
            }
            if let Some(found) = Self::find_element(&element.children, identifier) {
                return Some(found);
            }
        }
        None
    }

    /// Taps at specific screen coordinates.
    ///
    /// Simulates a tap gesture at the given x,y position on the simulator screen.
    ///
    /// # Arguments
    ///
    /// * `udid` - The unique device identifier of the target simulator
    /// * `x` - The x-coordinate in screen points
    /// * `y` - The y-coordinate in screen points
    ///
    /// # Errors
    ///
    /// - [`AxeError::NotInstalled`] if axe is not available
    /// - [`AxeError::Io`] if the command fails to execute
    /// - [`AxeError::CommandFailed`] if the tap command fails
    pub fn tap(udid: &str, x: i32, y: i32) -> Result<(), AxeError> {
        if !Self::is_installed() {
            return Err(AxeError::NotInstalled);
        }

        let output = Command::new("axe")
            .args(["tap", "-x", &x.to_string(), "-y", &y.to_string(), "--udid", udid])
            .output()?;

        if !output.status.success() {
            return Err(AxeError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string()
            ));
        }
        Ok(())
    }

    /// Performs a swipe gesture from one point to another.
    ///
    /// # Arguments
    ///
    /// * `udid` - The unique device identifier of the target simulator
    /// * `start_x` - Starting x-coordinate
    /// * `start_y` - Starting y-coordinate
    /// * `end_x` - Ending x-coordinate
    /// * `end_y` - Ending y-coordinate
    /// * `duration` - Optional swipe duration in seconds
    ///
    /// # Errors
    ///
    /// - [`AxeError::NotInstalled`] if axe is not available
    /// - [`AxeError::Io`] if the command fails to execute
    /// - [`AxeError::CommandFailed`] if the swipe command fails
    pub fn swipe(
        udid: &str,
        start_x: i32,
        start_y: i32,
        end_x: i32,
        end_y: i32,
        duration: Option<f64>,
    ) -> Result<(), AxeError> {
        if !Self::is_installed() {
            return Err(AxeError::NotInstalled);
        }

        let mut cmd = Command::new("axe");
        cmd.args([
            "swipe",
            "--start-x", &start_x.to_string(),
            "--start-y", &start_y.to_string(),
            "--end-x", &end_x.to_string(),
            "--end-y", &end_y.to_string(),
            "--udid", udid,
        ]);

        if let Some(dur) = duration {
            cmd.args(["--duration", &dur.to_string()]);
        }

        let output = cmd.output()?;

        if !output.status.success() {
            return Err(AxeError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string()
            ));
        }
        Ok(())
    }

    /// Taps an element by its accessibility identifier.
    ///
    /// Uses axe's `--id` flag to locate and tap the element with the
    /// specified accessibility identifier. This is more reliable than
    /// coordinate-based tapping as it accounts for element position changes.
    ///
    /// # Arguments
    ///
    /// * `udid` - The unique device identifier of the target simulator
    /// * `identifier` - The accessibility identifier of the element to tap
    ///
    /// # Errors
    ///
    /// - [`AxeError::NotInstalled`] if axe is not available
    /// - [`AxeError::Io`] if the command fails to execute
    /// - [`AxeError::CommandFailed`] if the element is not found or tap fails
    pub fn tap_element(udid: &str, identifier: &str) -> Result<(), AxeError> {
        if !Self::is_installed() {
            return Err(AxeError::NotInstalled);
        }

        let output = Command::new("axe")
            .args(["tap", "--id", identifier, "--udid", udid])
            .output()?;

        if !output.status.success() {
            return Err(AxeError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string()
            ));
        }
        Ok(())
    }

    /// Gets the current value of an element by its accessibility identifier.
    ///
    /// Dumps the UI hierarchy and searches for the specified element,
    /// returning its `AXValue` property. This is useful for reading the
    /// contents of text fields or other value-bearing elements.
    ///
    /// # Arguments
    ///
    /// * `udid` - The unique device identifier of the target simulator
    /// * `identifier` - The accessibility identifier of the element
    ///
    /// # Returns
    ///
    /// `Ok(Some(String))` if the element has a value, `Ok(None)` if the
    /// element exists but has no value.
    ///
    /// # Errors
    ///
    /// - [`AxeError::CommandFailed`] if the element is not found
    /// - Any errors from [`Self::dump_hierarchy`]
    pub fn get_element_value(udid: &str, identifier: &str) -> Result<Option<String>, AxeError> {
        let hierarchy = Self::dump_hierarchy(udid)?;
        let element = Self::find_element(&hierarchy, identifier)
            .ok_or_else(|| AxeError::CommandFailed(format!("Element '{}' not found", identifier)))?;
        Ok(element.value.or(element.label))
    }

    /// Taps an element by its accessibility label.
    ///
    /// Uses axe's `--label` flag to locate and tap the element with the
    /// specified accessibility label (AXLabel). This is useful when elements
    /// don't have a unique accessibility identifier but have a visible label.
    ///
    /// # Arguments
    ///
    /// * `udid` - The unique device identifier of the target simulator
    /// * `label` - The accessibility label of the element to tap
    ///
    /// # Errors
    ///
    /// - [`AxeError::NotInstalled`] if axe is not available
    /// - [`AxeError::Io`] if the command fails to execute
    /// - [`AxeError::CommandFailed`] if the element is not found or tap fails
    pub fn tap_by_label(udid: &str, label: &str) -> Result<(), AxeError> {
        if !Self::is_installed() {
            return Err(AxeError::NotInstalled);
        }

        let output = Command::new("axe")
            .args(["tap", "--label", label, "--udid", udid])
            .output()?;

        if !output.status.success() {
            return Err(AxeError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string()
            ));
        }
        Ok(())
    }

    /// Finds an element by its accessibility label.
    ///
    /// Performs a recursive depth-first search through the element hierarchy
    /// to find an element with a matching `AXLabel`.
    ///
    /// # Arguments
    ///
    /// * `element` - The root element to search from
    /// * `label` - The accessibility label to find
    ///
    /// # Returns
    ///
    /// `Some(UIElement)` containing a clone of the found element,
    /// or `None` if no matching element exists.
    pub fn find_element_by_label(element: &UIElement, label: &str) -> Option<UIElement> {
        if element.label.as_deref() == Some(label) {
            return Some(element.clone());
        }
        for child in &element.children {
            if let Some(found) = Self::find_element_by_label(child, label) {
                return Some(found);
            }
        }
        None
    }

    /// Finds an element by its accessibility label in a list of elements.
    ///
    /// Performs a recursive depth-first search through the element hierarchy
    /// to find an element with a matching `AXLabel`. This is the slice-based
    /// variant that mirrors [`Self::find_element`].
    ///
    /// # Arguments
    ///
    /// * `elements` - The root elements to search through
    /// * `label` - The accessibility label to find
    ///
    /// # Returns
    ///
    /// `Some(UIElement)` containing a clone of the found element,
    /// or `None` if no matching element exists.
    pub fn find_elements_by_label(elements: &[UIElement], label: &str) -> Option<UIElement> {
        for element in elements {
            if let Some(found) = Self::find_element_by_label(element, label) {
                return Some(found);
            }
        }
        None
    }

    /// Gets the current value of an element by its accessibility label.
    ///
    /// Dumps the UI hierarchy and searches for the specified element by label,
    /// returning its `AXValue` property. This is useful for reading the
    /// contents of elements that are identified by their visible label rather
    /// than an accessibility identifier.
    ///
    /// # Arguments
    ///
    /// * `udid` - The unique device identifier of the target simulator
    /// * `label` - The accessibility label of the element
    ///
    /// # Returns
    ///
    /// `Ok(Some(String))` if the element has a value, `Ok(None)` if the
    /// element exists but has no value.
    ///
    /// # Errors
    ///
    /// - [`AxeError::CommandFailed`] if the element is not found
    /// - Any errors from [`Self::dump_hierarchy`]
    pub fn get_element_value_by_label(udid: &str, label: &str) -> Result<Option<String>, AxeError> {
        let hierarchy = Self::dump_hierarchy(udid)?;
        let element = Self::find_elements_by_label(&hierarchy, label)
            .ok_or_else(|| AxeError::CommandFailed(format!("Element with label '{}' not found", label)))?;
        Ok(element.value.or(element.label))
    }

    /// Finds an element by selector (ID or label) with optional type filtering.
    ///
    /// This is a unified search method that can match by accessibility ID or label,
    /// and optionally filter by element type.
    ///
    /// # Arguments
    ///
    /// * `elements` - The root elements to search through
    /// * `selector` - The value to match against (ID or label)
    /// * `by_label` - If true, match against label; if false, match against ID
    /// * `element_type` - Optional type filter (e.g., "Button", "TextField")
    ///
    /// # Returns
    ///
    /// `Some(UIElement)` if a matching element is found, `None` otherwise.
    pub fn find_element_with_type(
        elements: &[UIElement],
        selector: &str,
        by_label: bool,
        element_type: Option<&str>,
    ) -> Option<UIElement> {
        fn search(
            elements: &[UIElement],
            selector: &str,
            by_label: bool,
            element_type: Option<&str>,
        ) -> Option<UIElement> {
            for element in elements {
                // Check if this element matches the selector
                let selector_matches = if by_label {
                    element.label.as_deref() == Some(selector)
                } else {
                    element.identifier.as_deref() == Some(selector)
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
                if let Some(found) = search(&element.children, selector, by_label, element_type) {
                    return Some(found);
                }
            }
            None
        }

        search(elements, selector, by_label, element_type)
    }

    /// Taps an element with optional type filtering.
    ///
    /// Finds an element by ID or label with optional type filter, extracts its
    /// center coordinates, and taps at that location.
    ///
    /// # Arguments
    ///
    /// * `udid` - The unique device identifier of the target simulator
    /// * `selector` - The selector value (accessibility ID or label)
    /// * `by_label` - If true, selector is a label; if false, it's an ID
    /// * `element_type` - Optional element type filter
    ///
    /// # Errors
    ///
    /// - [`AxeError::CommandFailed`] if the element is not found
    /// - Any errors from [`Self::dump_hierarchy`] or [`Self::tap`]
    pub fn tap_with_type(
        udid: &str,
        selector: &str,
        by_label: bool,
        element_type: &str,
    ) -> Result<(), AxeError> {
        let hierarchy = Self::dump_hierarchy(udid)?;
        let element = Self::find_element_with_type(&hierarchy, selector, by_label, Some(element_type))
            .ok_or_else(|| {
                let lookup_type = if by_label { "label" } else { "ID" };
                AxeError::CommandFailed(format!(
                    "Element with {} '{}' and type '{}' not found",
                    lookup_type, selector, element_type
                ))
            })?;

        let frame = element.frame.ok_or_else(|| {
            AxeError::CommandFailed("Element has no frame".to_string())
        })?;

        let center_x = (frame.x + frame.width / 2.0) as i32;
        let center_y = (frame.y + frame.height / 2.0) as i32;

        Self::tap(udid, center_x, center_y)
    }

    /// Gets the value of an element with optional type filtering.
    ///
    /// # Arguments
    ///
    /// * `udid` - The unique device identifier of the target simulator
    /// * `selector` - The selector value (accessibility ID or label)
    /// * `by_label` - If true, selector is a label; if false, it's an ID
    /// * `element_type` - Optional element type filter
    ///
    /// # Returns
    ///
    /// `Ok(Some(String))` if the element has a value, `Ok(None)` if it exists but has no value.
    ///
    /// # Errors
    ///
    /// - [`AxeError::CommandFailed`] if the element is not found
    /// - Any errors from [`Self::dump_hierarchy`]
    pub fn get_value_with_type(
        udid: &str,
        selector: &str,
        by_label: bool,
        element_type: &str,
    ) -> Result<Option<String>, AxeError> {
        let hierarchy = Self::dump_hierarchy(udid)?;
        let element = Self::find_element_with_type(&hierarchy, selector, by_label, Some(element_type))
            .ok_or_else(|| {
                let lookup_type = if by_label { "label" } else { "ID" };
                AxeError::CommandFailed(format!(
                    "Element with {} '{}' and type '{}' not found",
                    lookup_type, selector, element_type
                ))
            })?;

        Ok(element.value.or(element.label))
    }

    /// Flattens the element hierarchy into a list.
    ///
    /// Recursively traverses the element tree and collects all elements
    /// that have either an identifier or a label. This is useful for
    /// getting a flat list of actionable elements on the screen.
    ///
    /// # Arguments
    ///
    /// * `elements` - The root elements of the hierarchy
    ///
    /// # Returns
    ///
    /// A `Vec<UIElement>` containing all elements with identifiers or labels.
    /// Elements without both identifier and label are excluded.
    pub fn list_elements(elements: &[UIElement]) -> Vec<UIElement> {
        let mut result = Vec::new();
        Self::collect_elements(elements, &mut result);
        result
    }

    fn collect_elements(elements: &[UIElement], result: &mut Vec<UIElement>) {
        for element in elements {
            if element.identifier.is_some() || element.label.is_some() {
                result.push(element.clone());
            }
            Self::collect_elements(&element.children, result);
        }
    }

    /// Parses UI hierarchy JSON into element structures.
    ///
    /// This method is exposed primarily for testing purposes. It takes
    /// raw JSON bytes (as returned by `axe describe-ui`) and parses them
    /// into a vector of UI elements.
    ///
    /// # Arguments
    ///
    /// * `json` - Raw JSON bytes from axe output
    ///
    /// # Returns
    ///
    /// A `Vec<UIElement>` representing the parsed hierarchy.
    ///
    /// # Errors
    ///
    /// - [`AxeError::JsonParse`] if the JSON is invalid
    pub fn parse_hierarchy(json: &[u8]) -> Result<Vec<UIElement>, AxeError> {
        let hierarchy: Vec<UIElement> = serde_json::from_slice(json)?;
        Ok(hierarchy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Sample hierarchy matching axe output format
    const SAMPLE_HIERARCHY: &str = r#"[
        {
            "AXUniqueId": "main-view",
            "AXLabel": "Main View",
            "type": "View",
            "frame": {"x": 0, "y": 0, "width": 390, "height": 844},
            "children": [
                {
                    "AXUniqueId": "login-button",
                    "AXLabel": "Log In",
                    "type": "Button",
                    "frame": {"x": 100, "y": 400, "width": 190, "height": 44},
                    "children": []
                },
                {
                    "AXUniqueId": "email-field",
                    "AXLabel": "Email",
                    "AXValue": "user@example.com",
                    "type": "TextField",
                    "frame": {"x": 20, "y": 200, "width": 350, "height": 44},
                    "children": []
                }
            ]
        }
    ]"#;

    const NESTED_HIERARCHY: &str = r#"[
        {
            "AXUniqueId": "root",
            "type": "View",
            "children": [
                {
                    "AXUniqueId": "level1",
                    "type": "View",
                    "children": [
                        {
                            "AXUniqueId": "level2",
                            "type": "View",
                            "children": [
                                {
                                    "AXUniqueId": "deeply-nested-button",
                                    "AXLabel": "Deep Button",
                                    "type": "Button",
                                    "children": []
                                }
                            ]
                        }
                    ]
                }
            ]
        }
    ]"#;

    const EMPTY_HIERARCHY: &str = r#"[]"#;

    const MINIMAL_ELEMENT: &str = r#"[{"children": []}]"#;

    #[test]
    fn test_parse_hierarchy_success() {
        let elements = Axe::parse_hierarchy(SAMPLE_HIERARCHY.as_bytes())
            .expect("Should parse valid hierarchy");

        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].identifier.as_deref(), Some("main-view"));
        assert_eq!(elements[0].children.len(), 2);
    }

    #[test]
    fn test_parse_hierarchy_empty() {
        let elements = Axe::parse_hierarchy(EMPTY_HIERARCHY.as_bytes())
            .expect("Should parse empty hierarchy");

        assert!(elements.is_empty());
    }

    #[test]
    fn test_parse_hierarchy_invalid_json() {
        let result = Axe::parse_hierarchy(b"not valid json");

        assert!(result.is_err());
        match result {
            Err(AxeError::JsonParse(_)) => {} // Expected
            Err(e) => panic!("Expected JsonParse error, got: {:?}", e),
            Ok(_) => panic!("Expected error, got Ok"),
        }
    }

    #[test]
    fn test_parse_hierarchy_minimal_element() {
        let elements = Axe::parse_hierarchy(MINIMAL_ELEMENT.as_bytes())
            .expect("Should parse minimal element");

        assert_eq!(elements.len(), 1);
        assert!(elements[0].identifier.is_none());
        assert!(elements[0].label.is_none());
        assert!(elements[0].value.is_none());
        assert!(elements[0].frame.is_none());
    }

    #[test]
    fn test_ui_element_all_fields() {
        let elements = Axe::parse_hierarchy(SAMPLE_HIERARCHY.as_bytes()).unwrap();
        let email_field = &elements[0].children[1];

        assert_eq!(email_field.identifier.as_deref(), Some("email-field"));
        assert_eq!(email_field.label.as_deref(), Some("Email"));
        assert_eq!(email_field.value.as_deref(), Some("user@example.com"));
        assert_eq!(email_field.element_type.as_deref(), Some("TextField"));

        let frame = email_field.frame.as_ref().unwrap();
        assert_eq!(frame.x, 20.0);
        assert_eq!(frame.y, 200.0);
        assert_eq!(frame.width, 350.0);
        assert_eq!(frame.height, 44.0);
    }

    #[test]
    fn test_find_element_direct_match() {
        let elements = Axe::parse_hierarchy(SAMPLE_HIERARCHY.as_bytes()).unwrap();
        let found = Axe::find_element(&elements, "main-view");

        assert!(found.is_some());
        assert_eq!(found.unwrap().identifier.as_deref(), Some("main-view"));
    }

    #[test]
    fn test_find_element_nested() {
        let elements = Axe::parse_hierarchy(SAMPLE_HIERARCHY.as_bytes()).unwrap();
        let found = Axe::find_element(&elements, "login-button");

        assert!(found.is_some());
        let button = found.unwrap();
        assert_eq!(button.identifier.as_deref(), Some("login-button"));
        assert_eq!(button.label.as_deref(), Some("Log In"));
    }

    #[test]
    fn test_find_element_deeply_nested() {
        let elements = Axe::parse_hierarchy(NESTED_HIERARCHY.as_bytes()).unwrap();
        let found = Axe::find_element(&elements, "deeply-nested-button");

        assert!(found.is_some());
        let button = found.unwrap();
        assert_eq!(button.label.as_deref(), Some("Deep Button"));
    }

    #[test]
    fn test_find_element_not_found() {
        let elements = Axe::parse_hierarchy(SAMPLE_HIERARCHY.as_bytes()).unwrap();
        let found = Axe::find_element(&elements, "nonexistent-element");

        assert!(found.is_none());
    }

    #[test]
    fn test_find_element_empty_hierarchy() {
        let elements: Vec<UIElement> = vec![];
        let found = Axe::find_element(&elements, "any-id");

        assert!(found.is_none());
    }

    #[test]
    fn test_list_elements_flattens_hierarchy() {
        let elements = Axe::parse_hierarchy(SAMPLE_HIERARCHY.as_bytes()).unwrap();
        let flat = Axe::list_elements(&elements);

        // Should have: main-view, login-button, email-field (all have identifier or label)
        assert_eq!(flat.len(), 3);

        let identifiers: Vec<Option<&str>> = flat.iter()
            .map(|e| e.identifier.as_deref())
            .collect();

        assert!(identifiers.contains(&Some("main-view")));
        assert!(identifiers.contains(&Some("login-button")));
        assert!(identifiers.contains(&Some("email-field")));
    }

    #[test]
    fn test_list_elements_includes_label_only() {
        // Element with label but no identifier should still be included
        let json = r#"[{
            "AXLabel": "Label Only Element",
            "type": "StaticText",
            "children": []
        }]"#;

        let elements = Axe::parse_hierarchy(json.as_bytes()).unwrap();
        let flat = Axe::list_elements(&elements);

        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].label.as_deref(), Some("Label Only Element"));
    }

    #[test]
    fn test_list_elements_excludes_unlabeled() {
        // Elements without identifier or label should be excluded
        let json = r#"[{
            "type": "View",
            "children": [
                {
                    "AXUniqueId": "included",
                    "type": "Button",
                    "children": []
                }
            ]
        }]"#;

        let elements = Axe::parse_hierarchy(json.as_bytes()).unwrap();
        let flat = Axe::list_elements(&elements);

        // Only the child with identifier should be included
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].identifier.as_deref(), Some("included"));
    }

    #[test]
    fn test_list_elements_empty_hierarchy() {
        let elements: Vec<UIElement> = vec![];
        let flat = Axe::list_elements(&elements);

        assert!(flat.is_empty());
    }

    #[test]
    fn test_axe_error_display() {
        let cmd_err = AxeError::CommandFailed("test error".to_string());
        assert!(cmd_err.to_string().contains("test error"));

        let not_installed = AxeError::NotInstalled;
        assert!(not_installed.to_string().contains("axe tool not found"));
        assert!(not_installed.to_string().contains("brew install"));
    }

    #[test]
    fn test_element_frame_parsing() {
        let json = r#"[{
            "frame": {"x": 10.5, "y": 20.75, "width": 100.0, "height": 50.25},
            "children": []
        }]"#;

        let elements = Axe::parse_hierarchy(json.as_bytes()).unwrap();
        let frame = elements[0].frame.as_ref().unwrap();

        assert!((frame.x - 10.5).abs() < f64::EPSILON);
        assert!((frame.y - 20.75).abs() < f64::EPSILON);
        assert!((frame.width - 100.0).abs() < f64::EPSILON);
        assert!((frame.height - 50.25).abs() < f64::EPSILON);
    }

    #[test]
    fn test_tap_with_invalid_udid() {
        // Skip if axe is not installed
        if !Axe::is_installed() {
            return;
        }

        let result = Axe::tap("invalid-udid-that-does-not-exist", 100, 100);
        assert!(result.is_err());
    }

    #[test]
    fn test_tap_element_with_invalid_udid() {
        if !Axe::is_installed() {
            return;
        }

        let result = Axe::tap_element("invalid-udid-that-does-not-exist", "some-button");
        assert!(result.is_err());
    }

    #[test]
    fn test_ui_element_serialization() {
        let element = UIElement {
            identifier: Some("test-id".to_string()),
            label: Some("Test Label".to_string()),
            value: Some("Test Value".to_string()),
            element_type: Some("Button".to_string()),
            frame: Some(ElementFrame {
                x: 10.0,
                y: 20.0,
                width: 100.0,
                height: 50.0,
            }),
            children: vec![],
            role: Some("button".to_string()),
        };

        let json = serde_json::to_string(&element).unwrap();
        assert!(json.contains("test-id"));
        assert!(json.contains("Test Label"));
    }

    #[test]
    fn test_ui_element_clone() {
        let original = UIElement {
            identifier: Some("test".to_string()),
            label: Some("Label".to_string()),
            value: None,
            element_type: Some("Button".to_string()),
            frame: Some(ElementFrame {
                x: 0.0, y: 0.0, width: 100.0, height: 50.0
            }),
            children: vec![],
            role: None,
        };

        let cloned = original.clone();
        assert_eq!(original.identifier, cloned.identifier);
        assert_eq!(original.label, cloned.label);
    }

    #[test]
    fn test_find_element_by_label_direct_match() {
        let elements = Axe::parse_hierarchy(SAMPLE_HIERARCHY.as_bytes()).unwrap();
        let found = Axe::find_elements_by_label(&elements, "Main View");

        assert!(found.is_some());
        assert_eq!(found.unwrap().identifier.as_deref(), Some("main-view"));
    }

    #[test]
    fn test_find_element_by_label_nested() {
        let elements = Axe::parse_hierarchy(SAMPLE_HIERARCHY.as_bytes()).unwrap();
        let found = Axe::find_elements_by_label(&elements, "Log In");

        assert!(found.is_some());
        let button = found.unwrap();
        assert_eq!(button.identifier.as_deref(), Some("login-button"));
        assert_eq!(button.label.as_deref(), Some("Log In"));
    }

    #[test]
    fn test_find_element_by_label_deeply_nested() {
        let elements = Axe::parse_hierarchy(NESTED_HIERARCHY.as_bytes()).unwrap();
        let found = Axe::find_elements_by_label(&elements, "Deep Button");

        assert!(found.is_some());
        let button = found.unwrap();
        assert_eq!(button.identifier.as_deref(), Some("deeply-nested-button"));
    }

    #[test]
    fn test_find_element_by_label_not_found() {
        let elements = Axe::parse_hierarchy(SAMPLE_HIERARCHY.as_bytes()).unwrap();
        let found = Axe::find_elements_by_label(&elements, "Nonexistent Label");

        assert!(found.is_none());
    }

    #[test]
    fn test_find_element_by_label_empty_hierarchy() {
        let elements: Vec<UIElement> = vec![];
        let found = Axe::find_elements_by_label(&elements, "any-label");

        assert!(found.is_none());
    }

    #[test]
    fn test_find_element_by_label_single_element() {
        let element = UIElement {
            identifier: Some("test-id".to_string()),
            label: Some("Test Label".to_string()),
            value: None,
            element_type: Some("Button".to_string()),
            frame: None,
            children: vec![],
            role: None,
        };

        let found = Axe::find_element_by_label(&element, "Test Label");
        assert!(found.is_some());
        assert_eq!(found.unwrap().identifier.as_deref(), Some("test-id"));
    }

    #[test]
    fn test_tap_by_label_with_invalid_udid() {
        if !Axe::is_installed() {
            return;
        }

        let result = Axe::tap_by_label("invalid-udid-that-does-not-exist", "some-label");
        assert!(result.is_err());
    }

    #[test]
    fn test_find_element_with_type_by_id() {
        let elements = Axe::parse_hierarchy(SAMPLE_HIERARCHY.as_bytes()).unwrap();

        // Find by ID without type filter
        let found = Axe::find_element_with_type(&elements, "login-button", false, None);
        assert!(found.is_some());
        assert_eq!(found.unwrap().identifier.as_deref(), Some("login-button"));

        // Find by ID with matching type filter
        let found = Axe::find_element_with_type(&elements, "login-button", false, Some("Button"));
        assert!(found.is_some());
        assert_eq!(found.unwrap().element_type.as_deref(), Some("Button"));

        // Find by ID with non-matching type filter
        let found = Axe::find_element_with_type(&elements, "login-button", false, Some("TextField"));
        assert!(found.is_none());
    }

    #[test]
    fn test_find_element_with_type_by_label() {
        let elements = Axe::parse_hierarchy(SAMPLE_HIERARCHY.as_bytes()).unwrap();

        // Find by label without type filter
        let found = Axe::find_element_with_type(&elements, "Log In", true, None);
        assert!(found.is_some());
        assert_eq!(found.unwrap().label.as_deref(), Some("Log In"));

        // Find by label with matching type filter
        let found = Axe::find_element_with_type(&elements, "Email", true, Some("TextField"));
        assert!(found.is_some());
        assert_eq!(found.unwrap().identifier.as_deref(), Some("email-field"));

        // Find by label with non-matching type filter
        let found = Axe::find_element_with_type(&elements, "Email", true, Some("Button"));
        assert!(found.is_none());
    }

    #[test]
    fn test_find_element_with_type_not_found() {
        let elements = Axe::parse_hierarchy(SAMPLE_HIERARCHY.as_bytes()).unwrap();

        // Non-existent selector
        let found = Axe::find_element_with_type(&elements, "nonexistent", false, None);
        assert!(found.is_none());

        // Existing selector but wrong lookup type
        let found = Axe::find_element_with_type(&elements, "login-button", true, None);
        assert!(found.is_none()); // "login-button" is an ID, not a label
    }
}
