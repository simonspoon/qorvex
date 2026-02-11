//! Shared UI element types for accessibility-based automation.
//!
//! This module defines the core data structures representing UI elements
//! from the accessibility hierarchy. These types are used across all
//! automation backends and are independent
//! of any specific backend implementation.

use serde::{Deserialize, Serialize};

/// Represents a UI element from the accessibility hierarchy.
///
/// This struct contains accessibility information about a UI element as
/// reported by an automation backend. Elements form a tree structure
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
