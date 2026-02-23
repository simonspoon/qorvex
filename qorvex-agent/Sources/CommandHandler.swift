// CommandHandler.swift
// Dispatches decoded protocol requests to XCUIElement actions.

import XCTest

/// Disable XCUITest quiescence waiting on an app via private API.
/// Quiescence waiting adds 1-2s per interaction as XCUITest waits for
/// all animations/timers/network to settle. Bits: 0x1 = skip pre-event,
/// 0x2 = skip post-event.
private func disableQuiescenceWaiting(_ app: XCUIApplication) {
    app.setValue(3, forKey: "currentInteractionOptions")
}

final class CommandHandler {
    private var app: XCUIApplication

    init(app: XCUIApplication) {
        self.app = app
        disableQuiescenceWaiting(app)
    }

    /// Handle a decoded request and return a response.
    func handle(_ request: AgentRequest) -> AgentResponse {
        switch request {
        case .heartbeat:
            return .ok

        case .tapCoord(let x, let y):
            return handleTapCoord(x: x, y: y)

        case .tapElement(let selector, let timeoutMs):
            return handleTapElement(selector: selector, timeoutMs: timeoutMs)

        case .tapByLabel(let label, let timeoutMs):
            return handleTapByLabel(label: label, timeoutMs: timeoutMs)

        case .tapWithType(let selector, let byLabel, let elementType, let timeoutMs):
            return handleTapWithType(selector: selector, byLabel: byLabel, elementType: elementType, timeoutMs: timeoutMs)

        case .typeText(let text):
            return handleTypeText(text: text)

        case .swipe(let startX, let startY, let endX, let endY, let duration):
            return handleSwipe(
                startX: startX, startY: startY,
                endX: endX, endY: endY,
                duration: duration
            )

        case .longPress(let x, let y, let duration):
            return handleLongPress(x: x, y: y, duration: duration)

        case .getValue(let selector, let byLabel, let elementType, let timeoutMs):
            return handleGetValue(selector: selector, byLabel: byLabel, elementType: elementType, timeoutMs: timeoutMs)

        case .dumpTree:
            return handleDumpTree()

        case .screenshot:
            return handleScreenshot()

        case .setTarget(let bundleId):
            return handleSetTarget(bundleId: bundleId)

        case .findElement(let selector, let byLabel, let elementType):
            return handleFindElement(selector: selector, byLabel: byLabel, elementType: elementType)
        }
    }

    // MARK: - Tap coordinate

    private func handleTapCoord(x: Int32, y: Int32) -> AgentResponse {
        let coordinate = app.coordinate(
            withNormalizedOffset: CGVector(dx: 0, dy: 0)
        ).withOffset(CGVector(dx: Double(x), dy: Double(y)))
        var objcError: NSError?
        let caught = QVXTryCatch({
            coordinate.tap()
        }, &objcError)
        if !caught {
            let msg = objcError?.localizedDescription ?? "Unknown ObjC exception"
            return .error(message: "Tap failed: \(msg)")
        }
        return .ok
    }

    // MARK: - Tap by accessibility identifier

    private func handleTapElement(selector: String, timeoutMs: UInt64?) -> AgentResponse {
        let queryFn = { [app] () -> XCUIElement in
            app.descendants(matching: .any).matching(
                NSPredicate(format: "identifier == %@", selector)
            ).firstMatch
        }
        return performTap(queryFn: queryFn, description: "identifier '\(selector)'", timeoutMs: timeoutMs)
    }

    // MARK: - Tap by accessibility label

    private func handleTapByLabel(label: String, timeoutMs: UInt64?) -> AgentResponse {
        let queryFn = { [app] () -> XCUIElement in
            app.descendants(matching: .any).matching(
                NSPredicate(format: "label == %@", label)
            ).firstMatch
        }
        return performTap(queryFn: queryFn, description: "label '\(label)'", timeoutMs: timeoutMs)
    }

    // MARK: - Tap with element type filter

    private func handleTapWithType(selector: String, byLabel: Bool, elementType: String, timeoutMs: UInt64?) -> AgentResponse {
        guard let xcType = xcuiElementType(from: elementType) else {
            return .error(message: "Unknown element type '\(elementType)'")
        }
        let field = byLabel ? "label" : "identifier"
        let queryFn = { [app] () -> XCUIElement in
            app.descendants(matching: xcType).matching(
                NSPredicate(format: "%K == %@", field, selector)
            ).firstMatch
        }
        let lookupKind = byLabel ? "label" : "identifier"
        return performTap(queryFn: queryFn, description: "\(lookupKind) '\(selector)' and type '\(elementType)'", timeoutMs: timeoutMs)
    }

    // MARK: - Shared tap helper

    /// Polls for an element, waits for frame stability, re-queries for fresh
    /// coordinates, validates no frame drift, then taps.
    private func performTap(
        queryFn: @escaping () -> XCUIElement,
        description: String,
        timeoutMs: UInt64?
    ) -> AgentResponse {
        // Track frame position across polls to avoid tapping mid-animation.
        // When timeout_ms is set, require 2 consecutive polls with the same
        // frame before tapping (~50ms of stability for static elements,
        // correctly waits out modal animations). The pre-tap drift check
        // provides additional safety without an extra poll cycle.
        var lastFrame: CGRect?
        var stableCount: Int = 0
        let requiredStablePolls = 2

        let actionFn = { [queryFn] (element: XCUIElement) -> AgentResponse? in
            var errorMsg: String?
            var objcError: NSError?
            let caught = QVXTryCatch({
                guard element.exists else {
                    errorMsg = "Element with \(description) not found"
                    return
                }
                guard element.isHittable else {
                    errorMsg = "Element with \(description) exists but is not hittable"
                    return
                }
                // Wait for frame stability when retrying is enabled.
                if timeoutMs != nil {
                    let currentFrame = element.frame
                    if currentFrame != .zero {
                        if currentFrame == lastFrame {
                            stableCount += 1
                        } else {
                            lastFrame = currentFrame
                            stableCount = 1
                        }
                        if stableCount < requiredStablePolls {
                            errorMsg = "frame-unstable"
                            return
                        }
                    }
                }
                // Re-query to get a fresh element reference with up-to-date
                // coordinates. Without this, taps on modals/sheets can land on
                // stale positions when quiescence waiting is disabled.
                let fresh = queryFn()
                guard fresh.exists, fresh.isHittable else {
                    errorMsg = "Element with \(description) became unavailable on re-query"
                    return
                }
                // Verify fresh element's frame matches the stable frame.
                // If the element drifted between stability check and re-query,
                // the tap would land at wrong coordinates.
                if let stable = lastFrame {
                    let freshFrame = fresh.frame
                    if freshFrame != stable {
                        lastFrame = freshFrame
                        stableCount = 1
                        errorMsg = "frame-drifted"
                        return
                    }
                }
                fresh.tap()
            }, &objcError)
            if !caught {
                let msg = objcError?.localizedDescription ?? "Unknown ObjC exception"
                return .error(message: "Tap failed: \(msg)")
            }
            if let errorMsg = errorMsg {
                // Retryable error: return nil to poll again (if timeout allows)
                return nil
            }
            return .ok
        }

        if let result = pollUntilFound(timeoutMs: timeoutMs, query: queryFn, action: actionFn) {
            return result
        }
        return .error(message: "Element with \(description) not found")
    }

    // MARK: - Type text

    private func handleTypeText(text: String) -> AgentResponse {
        // Type text into the element that currently has keyboard focus.
        // On iOS, the first responder receives typeText events through the app.
        // We try to find the focused element first; if we can't, we check for
        // a visible keyboard and use the app directly. If neither is available,
        // return an error instead of crashing from an unhandled ObjC exception.
        let focusedElement = app.descendants(matching: .any).matching(
            NSPredicate(format: "hasKeyboardFocus == YES")
        ).firstMatch

        var errorMsg: String?
        var objcError: NSError?
        let caught = QVXTryCatch({
            if focusedElement.exists {
                focusedElement.typeText(text)
            } else if self.app.keyboards.firstMatch.exists {
                self.app.typeText(text)
            } else {
                errorMsg = "No keyboard visible; tap a text field first"
            }
        }, &objcError)
        if !caught {
            let msg = objcError?.localizedDescription ?? "Unknown ObjC exception"
            return .error(message: "TypeText failed: \(msg)")
        }
        if let errorMsg = errorMsg {
            return .error(message: errorMsg)
        }
        return .ok
    }

    // MARK: - Swipe

    private func handleSwipe(
        startX: Int32, startY: Int32,
        endX: Int32, endY: Int32,
        duration: Double?
    ) -> AgentResponse {
        let startCoord = app.coordinate(
            withNormalizedOffset: CGVector(dx: 0, dy: 0)
        ).withOffset(CGVector(dx: Double(startX), dy: Double(startY)))

        let endCoord = app.coordinate(
            withNormalizedOffset: CGVector(dx: 0, dy: 0)
        ).withOffset(CGVector(dx: Double(endX), dy: Double(endY)))

        let swipeDuration = duration ?? 0.3
        let dx = Double(endX - startX)
        let dy = Double(endY - startY)
        let distance = sqrt(dx * dx + dy * dy)
        let velocity = distance / swipeDuration  // points per second
        var objcError: NSError?
        let caught = QVXTryCatch({
            startCoord.press(forDuration: 0.05, thenDragTo: endCoord,
                             withVelocity: XCUIGestureVelocity(velocity),
                             thenHoldForDuration: 0)
        }, &objcError)
        if !caught {
            let msg = objcError?.localizedDescription ?? "Unknown ObjC exception"
            return .error(message: "Swipe failed: \(msg)")
        }
        return .ok
    }

    // MARK: - Long press

    private func handleLongPress(x: Int32, y: Int32, duration: Double) -> AgentResponse {
        let coordinate = app.coordinate(
            withNormalizedOffset: CGVector(dx: 0, dy: 0)
        ).withOffset(CGVector(dx: Double(x), dy: Double(y)))
        var objcError: NSError?
        let caught = QVXTryCatch({
            coordinate.press(forDuration: duration)
        }, &objcError)
        if !caught {
            let msg = objcError?.localizedDescription ?? "Unknown ObjC exception"
            return .error(message: "Long press failed: \(msg)")
        }
        return .ok
    }

    // MARK: - Get value

    private func handleGetValue(selector: String, byLabel: Bool, elementType: String?, timeoutMs: UInt64?) -> AgentResponse {
        let queryFn = { [app] () -> XCUIElement in
            if let typeName = elementType, let xcType = xcuiElementType(from: typeName) {
                let field = byLabel ? "label" : "identifier"
                return app.descendants(matching: xcType).matching(
                    NSPredicate(format: "%K == %@", field, selector)
                ).firstMatch
            } else {
                let field = byLabel ? "label" : "identifier"
                return app.descendants(matching: .any).matching(
                    NSPredicate(format: "%K == %@", field, selector)
                ).firstMatch
            }
        }

        let actionFn = { (element: XCUIElement) -> AgentResponse? in
            var result: AgentResponse?
            var notFound = false
            var objcError: NSError?
            let caught = QVXTryCatch({
                guard element.exists else {
                    notFound = true
                    return
                }
                let rawValue = element.value as? String ?? ""
                let value = rawValue.isEmpty ? element.label : rawValue
                if value.isEmpty {
                    result = .value(nil)
                } else {
                    result = .value(value)
                }
            }, &objcError)
            if !caught {
                let msg = objcError?.localizedDescription ?? "Unknown ObjC exception"
                return .error(message: "GetValue failed: \(msg)")
            }
            if notFound {
                return nil  // retryable
            }
            return result ?? .error(message: "GetValue produced no result")
        }

        if let result = pollUntilFound(timeoutMs: timeoutMs, query: queryFn, action: actionFn) {
            return result
        }
        let lookupKind = byLabel ? "label" : "identifier"
        let typeInfo = elementType.map { " and type '\($0)'" } ?? ""
        return .error(message: "Element with \(lookupKind) '\(selector)'\(typeInfo) not found")
    }

    // MARK: - Dump tree

    private func handleDumpTree() -> AgentResponse {
        var snapshot: XCUIElementSnapshot?
        var objcError: NSError?
        let caught = QVXTryCatch({
            do {
                snapshot = try self.app.snapshot()
            } catch {
                snapshot = nil
            }
        }, &objcError)

        if !caught {
            let msg = objcError?.localizedDescription ?? "Unknown ObjC exception"
            return .error(message: "Failed to capture accessibility tree: \(msg)")
        }

        guard let snapshot = snapshot else {
            return .error(message: "Failed to capture accessibility tree: snapshot returned nil")
        }

        guard let tree = serializeElement(snapshot) else {
            return .tree(json: "[]")
        }

        do {
            let jsonData = try JSONEncoder().encode(tree)
            guard let json = String(data: jsonData, encoding: .utf8) else {
                return .error(message: "Failed to encode tree as UTF-8")
            }
            // Wrap in array to match the Rust Vec<UIElement> format.
            return .tree(json: "[\(json)]")
        } catch {
            return .error(message: "JSON encoding failed: \(error)")
        }
    }

    // MARK: - Screenshot

    private func handleScreenshot() -> AgentResponse {
        var pngData: Data?
        var objcError: NSError?
        let caught = QVXTryCatch({
            let screenshot = XCUIScreen.main.screenshot()
            pngData = screenshot.pngRepresentation
        }, &objcError)

        if !caught {
            let msg = objcError?.localizedDescription ?? "Unknown ObjC exception"
            return .error(message: "Screenshot failed: \(msg)")
        }

        guard let data = pngData else {
            return .error(message: "Screenshot failed: no PNG data produced")
        }

        return .screenshot(data: data)
    }

    // MARK: - Set target app

    private func handleSetTarget(bundleId: String) -> AgentResponse {
        app = XCUIApplication(bundleIdentifier: bundleId)
        disableQuiescenceWaiting(app)
        return .ok
    }

    // MARK: - Find element

    private func handleFindElement(selector: String, byLabel: Bool, elementType: String?) -> AgentResponse {
        let element: XCUIElement

        if let typeName = elementType, let xcType = xcuiElementType(from: typeName) {
            let field = byLabel ? "label" : "identifier"
            element = app.descendants(matching: xcType).matching(
                NSPredicate(format: "%K == %@", field, selector)
            ).firstMatch
        } else {
            let field = byLabel ? "label" : "identifier"
            element = app.descendants(matching: .any).matching(
                NSPredicate(format: "%K == %@", field, selector)
            ).firstMatch
        }

        var result: AgentResponse?
        var objcError: NSError?
        let caught = QVXTryCatch({
            guard element.exists else {
                result = .element(json: "null")
                return
            }

            let isHittable = element.isHittable

            var snapshot: (any XCUIElementSnapshot)?
            do {
                snapshot = try element.snapshot()
            } catch {
                snapshot = nil
            }

            if let snap = snapshot {
                guard var serialized = self.serializeElement(snap) else {
                    result = .element(json: "null")
                    return
                }
                serialized = UIElementJSON(
                    AXUniqueId: serialized.AXUniqueId,
                    AXLabel: serialized.AXLabel,
                    AXValue: serialized.AXValue,
                    type: serialized.type,
                    frame: serialized.frame,
                    children: serialized.children,
                    role: serialized.role,
                    hittable: isHittable
                )
                do {
                    let jsonData = try JSONEncoder().encode(serialized)
                    if let json = String(data: jsonData, encoding: .utf8) {
                        result = .element(json: json)
                    } else {
                        result = .error(message: "FindElement: failed to encode as UTF-8")
                    }
                } catch {
                    result = .error(message: "FindElement: JSON encoding failed: \(error)")
                }
            } else {
                // Snapshot failed but element exists — return minimal info
                let frame = element.frame
                let frameJSON = FrameJSON(
                    x: Double(frame.origin.x),
                    y: Double(frame.origin.y),
                    width: Double(frame.size.width),
                    height: Double(frame.size.height)
                )
                let minimal = UIElementJSON(
                    AXUniqueId: nil,
                    AXLabel: nil,
                    AXValue: nil,
                    type: self.elementTypeName(element.elementType),
                    frame: frameJSON,
                    children: [],
                    role: nil,
                    hittable: isHittable
                )
                do {
                    let jsonData = try JSONEncoder().encode(minimal)
                    if let json = String(data: jsonData, encoding: .utf8) {
                        result = .element(json: json)
                    } else {
                        result = .error(message: "FindElement: failed to encode as UTF-8")
                    }
                } catch {
                    result = .error(message: "FindElement: JSON encoding failed: \(error)")
                }
            }
        }, &objcError)

        if !caught {
            let msg = objcError?.localizedDescription ?? "Unknown ObjC exception"
            return .error(message: "FindElement failed: \(msg)")
        }
        return result ?? .error(message: "FindElement produced no result")
    }

    // MARK: - Poll helper

    /// Poll until an element matching `query` satisfies `action`, with timeout.
    /// Returns nil only when the timeout is reached without success.
    private func pollUntilFound(
        timeoutMs: UInt64?,
        interval: TimeInterval = 0.050,
        query: () -> XCUIElement,
        action: (XCUIElement) -> AgentResponse?  // nil = retry, non-nil = done
    ) -> AgentResponse? {
        let timeout: TimeInterval = timeoutMs.map { Double($0) / 1000.0 } ?? 0
        let deadline = CFAbsoluteTimeGetCurrent() + timeout

        while true {
            let element = query()
            if let response = action(element) {
                return response
            }
            // No timeout or deadline passed → give up
            if timeout <= 0 || CFAbsoluteTimeGetCurrent() >= deadline {
                return nil
            }
            Thread.sleep(forTimeInterval: interval)
        }
    }

    // MARK: - Helpers

    /// Recursively serialize an XCUIElementSnapshot into our Codable tree structure.
    /// Returns nil for empty scaffolding nodes (no identity, no area, no surviving children).
    private func serializeElement(_ snapshot: any XCUIElementSnapshot) -> UIElementJSON? {
        let frame = snapshot.frame
        let frameJSON = FrameJSON(
            x: Double(frame.origin.x),
            y: Double(frame.origin.y),
            width: Double(frame.size.width),
            height: Double(frame.size.height)
        )

        let children = snapshot.children.compactMap { child -> UIElementJSON? in
            serializeElement(child)
        }

        let hasIdentity = !snapshot.identifier.isEmpty
            || !snapshot.label.isEmpty
            || (snapshot.value as? String).map { !$0.isEmpty } ?? false
        let hasArea = frame.width > 0 && frame.height > 0

        // Prune empty scaffolding nodes
        if !hasIdentity && !hasArea && children.isEmpty {
            return nil
        }

        return UIElementJSON(
            AXUniqueId: snapshot.identifier.isEmpty ? nil : snapshot.identifier,
            AXLabel: snapshot.label.isEmpty ? nil : snapshot.label,
            AXValue: (snapshot.value as? String).flatMap { $0.isEmpty ? nil : $0 },
            type: elementTypeName(snapshot.elementType),
            frame: frameJSON,
            children: children,
            role: nil,
            hittable: nil
        )
    }

    /// Map an XCUIElement.ElementType to its string name, matching the
    /// Rust-side element_type strings in qorvex-core.
    private func elementTypeName(_ type: XCUIElement.ElementType) -> String {
        switch type {
        case .any:              return "Any"
        case .other:            return "Other"
        case .application:      return "Application"
        case .group:            return "Group"
        case .window:           return "Window"
        case .sheet:            return "Sheet"
        case .drawer:           return "Drawer"
        case .alert:            return "Alert"
        case .dialog:           return "Dialog"
        case .button:           return "Button"
        case .radioButton:      return "RadioButton"
        case .radioGroup:       return "RadioGroup"
        case .checkBox:         return "CheckBox"
        case .disclosureTriangle: return "DisclosureTriangle"
        case .popUpButton:      return "PopUpButton"
        case .comboBox:         return "ComboBox"
        case .menuButton:       return "MenuButton"
        case .toolbarButton:    return "ToolbarButton"
        case .popover:          return "Popover"
        case .keyboard:         return "Keyboard"
        case .key:              return "Key"
        case .navigationBar:    return "NavigationBar"
        case .tabBar:           return "TabBar"
        case .tabGroup:         return "TabGroup"
        case .toolbar:          return "Toolbar"
        case .statusBar:        return "StatusBar"
        case .table:            return "Table"
        case .tableRow:         return "TableRow"
        case .tableColumn:      return "TableColumn"
        case .outline:          return "Outline"
        case .outlineRow:       return "OutlineRow"
        case .browser:          return "Browser"
        case .collectionView:   return "CollectionView"
        case .slider:           return "Slider"
        case .pageIndicator:    return "PageIndicator"
        case .progressIndicator: return "ProgressIndicator"
        case .activityIndicator: return "ActivityIndicator"
        case .segmentedControl: return "SegmentedControl"
        case .picker:           return "Picker"
        case .pickerWheel:      return "PickerWheel"
        case .`switch`:          return "Switch"
        case .toggle:           return "Toggle"
        case .link:             return "Link"
        case .image:            return "Image"
        case .icon:             return "Icon"
        case .searchField:      return "SearchField"
        case .scrollView:       return "ScrollView"
        case .scrollBar:        return "ScrollBar"
        case .staticText:       return "StaticText"
        case .textField:        return "TextField"
        case .secureTextField:  return "SecureTextField"
        case .datePicker:       return "DatePicker"
        case .textView:         return "TextView"
        case .menu:             return "Menu"
        case .menuItem:         return "MenuItem"
        case .menuBar:          return "MenuBar"
        case .menuBarItem:      return "MenuBarItem"
        case .map:              return "Map"
        case .webView:          return "WebView"
        case .incrementArrow:   return "IncrementArrow"
        case .decrementArrow:   return "DecrementArrow"
        case .timeline:         return "Timeline"
        case .ratingIndicator:  return "RatingIndicator"
        case .valueIndicator:   return "ValueIndicator"
        case .splitGroup:       return "SplitGroup"
        case .splitter:         return "Splitter"
        case .relevanceIndicator: return "RelevanceIndicator"
        case .colorWell:        return "ColorWell"
        case .helpTag:          return "HelpTag"
        case .matte:            return "Matte"
        case .dockItem:         return "DockItem"
        case .ruler:            return "Ruler"
        case .rulerMarker:      return "RulerMarker"
        case .grid:             return "Grid"
        case .levelIndicator:   return "LevelIndicator"
        case .cell:             return "Cell"
        case .layoutArea:       return "LayoutArea"
        case .layoutItem:       return "LayoutItem"
        case .handle:           return "Handle"
        case .stepper:          return "Stepper"
        case .tab:              return "Tab"
        case .touchBar:         return "TouchBar"
        case .statusItem:       return "StatusItem"
        @unknown default:       return "Unknown"
        }
    }
}

/// Convert a string element type name back to XCUIElement.ElementType.
/// Returns nil for unrecognized types.
func xcuiElementType(from name: String) -> XCUIElement.ElementType? {
    switch name {
    case "Any":                 return .any
    case "Other":               return .other
    case "Application":         return .application
    case "Group":               return .group
    case "Window":              return .window
    case "Sheet":               return .sheet
    case "Drawer":              return .drawer
    case "Alert":               return .alert
    case "Dialog":              return .dialog
    case "Button":              return .button
    case "RadioButton":         return .radioButton
    case "RadioGroup":          return .radioGroup
    case "CheckBox":            return .checkBox
    case "DisclosureTriangle":  return .disclosureTriangle
    case "PopUpButton":         return .popUpButton
    case "ComboBox":            return .comboBox
    case "MenuButton":          return .menuButton
    case "ToolbarButton":       return .toolbarButton
    case "Popover":             return .popover
    case "Keyboard":            return .keyboard
    case "Key":                 return .key
    case "NavigationBar":       return .navigationBar
    case "TabBar":              return .tabBar
    case "TabGroup":            return .tabGroup
    case "Toolbar":             return .toolbar
    case "StatusBar":           return .statusBar
    case "Table":               return .table
    case "TableRow":            return .tableRow
    case "TableColumn":         return .tableColumn
    case "Outline":             return .outline
    case "OutlineRow":          return .outlineRow
    case "Browser":             return .browser
    case "CollectionView":      return .collectionView
    case "Slider":              return .slider
    case "PageIndicator":       return .pageIndicator
    case "ProgressIndicator":   return .progressIndicator
    case "ActivityIndicator":   return .activityIndicator
    case "SegmentedControl":    return .segmentedControl
    case "Picker":              return .picker
    case "PickerWheel":         return .pickerWheel
    case "Switch":              return .`switch`
    case "Toggle":              return .toggle
    case "Link":                return .link
    case "Image":               return .image
    case "Icon":                return .icon
    case "SearchField":         return .searchField
    case "ScrollView":          return .scrollView
    case "ScrollBar":           return .scrollBar
    case "StaticText":          return .staticText
    case "TextField":           return .textField
    case "SecureTextField":     return .secureTextField
    case "DatePicker":          return .datePicker
    case "TextView":            return .textView
    case "Menu":                return .menu
    case "MenuItem":            return .menuItem
    case "MenuBar":             return .menuBar
    case "MenuBarItem":         return .menuBarItem
    case "Map":                 return .map
    case "WebView":             return .webView
    case "IncrementArrow":      return .incrementArrow
    case "DecrementArrow":      return .decrementArrow
    case "Timeline":            return .timeline
    case "RatingIndicator":     return .ratingIndicator
    case "ValueIndicator":      return .valueIndicator
    case "SplitGroup":          return .splitGroup
    case "Splitter":            return .splitter
    case "RelevanceIndicator":  return .relevanceIndicator
    case "ColorWell":           return .colorWell
    case "HelpTag":             return .helpTag
    case "Matte":               return .matte
    case "DockItem":            return .dockItem
    case "Ruler":               return .ruler
    case "RulerMarker":         return .rulerMarker
    case "Grid":                return .grid
    case "LevelIndicator":      return .levelIndicator
    case "Cell":                return .cell
    case "LayoutArea":          return .layoutArea
    case "LayoutItem":          return .layoutItem
    case "Handle":              return .handle
    case "Stepper":             return .stepper
    case "Tab":                 return .tab
    case "TouchBar":            return .touchBar
    case "StatusItem":          return .statusItem
    default:                    return nil
    }
}
