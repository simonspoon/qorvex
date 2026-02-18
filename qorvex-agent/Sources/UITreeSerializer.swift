// UITreeSerializer.swift
// Codable types for serializing the XCUIElement accessibility tree to JSON,
// matching the UIElement structure defined in qorvex-core/src/element.rs.

import Foundation

/// JSON representation of a UI element, matching the Rust `UIElement` struct.
///
/// Field names use the exact JSON keys expected by the Rust serde deserialization:
/// - `AXUniqueId` -> identifier
/// - `AXLabel` -> label
/// - `AXValue` -> value
/// - `type` -> element_type
/// - `frame` -> frame
/// - `children` -> children
/// - `role` -> role
struct UIElementJSON: Codable {
    let AXUniqueId: String?
    let AXLabel: String?
    let AXValue: String?
    let type: String?
    let frame: FrameJSON?
    let children: [UIElementJSON]
    let role: String?
    let hittable: Bool?
}

/// JSON representation of an element's frame (position and size in screen points).
struct FrameJSON: Codable {
    let x: Double
    let y: Double
    let width: Double
    let height: Double
}
