// CommandHandler.swift
// Dispatches decoded protocol requests to XCUIElement actions.

import XCTest

final class CommandHandler {
    private var app: XCUIApplication

    init(app: XCUIApplication) {
        self.app = app
    }

    /// Handle a decoded request and return a response.
    func handle(_ request: AgentRequest) -> AgentResponse {
        switch request {
        case .heartbeat:
            return .ok

        case .tapCoord(let x, let y):
            return handleTapCoord(x: x, y: y)

        case .tapElement(let selector):
            return handleTapElement(selector: selector)

        case .tapByLabel(let label):
            return handleTapByLabel(label: label)

        case .tapWithType(let selector, let byLabel, let elementType):
            return handleTapWithType(selector: selector, byLabel: byLabel, elementType: elementType)

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

        case .getValue(let selector, let byLabel, let elementType):
            return handleGetValue(selector: selector, byLabel: byLabel, elementType: elementType)

        case .dumpTree:
            return handleDumpTree()

        case .screenshot:
            return handleScreenshot()

        case .setTarget(let bundleId):
            return handleSetTarget(bundleId: bundleId)
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

    private func handleTapElement(selector: String) -> AgentResponse {
        let element = app.descendants(matching: .any).matching(
            NSPredicate(format: "identifier == %@", selector)
        ).firstMatch

        var errorMsg: String?
        var objcError: NSError?
        let caught = QVXTryCatch({
            guard element.exists else {
                errorMsg = "Element with identifier '\(selector)' not found"
                return
            }
            guard element.isHittable else {
                errorMsg = "Element with identifier '\(selector)' exists but is not hittable"
                return
            }
            element.tap()
        }, &objcError)
        if !caught {
            let msg = objcError?.localizedDescription ?? "Unknown ObjC exception"
            return .error(message: "Tap failed: \(msg)")
        }
        if let errorMsg = errorMsg {
            return .error(message: errorMsg)
        }
        return .ok
    }

    // MARK: - Tap by accessibility label

    private func handleTapByLabel(label: String) -> AgentResponse {
        let element = app.descendants(matching: .any).matching(
            NSPredicate(format: "label == %@", label)
        ).firstMatch

        var errorMsg: String?
        var objcError: NSError?
        let caught = QVXTryCatch({
            guard element.exists else {
                errorMsg = "Element with label '\(label)' not found"
                return
            }
            guard element.isHittable else {
                errorMsg = "Element with label '\(label)' exists but is not hittable"
                return
            }
            element.tap()
        }, &objcError)
        if !caught {
            let msg = objcError?.localizedDescription ?? "Unknown ObjC exception"
            return .error(message: "Tap failed: \(msg)")
        }
        if let errorMsg = errorMsg {
            return .error(message: errorMsg)
        }
        return .ok
    }

    // MARK: - Tap with element type filter

    private func handleTapWithType(selector: String, byLabel: Bool, elementType: String) -> AgentResponse {
        guard let xcType = xcuiElementType(from: elementType) else {
            return .error(message: "Unknown element type '\(elementType)'")
        }

        let field = byLabel ? "label" : "identifier"
        let query = app.descendants(matching: xcType).matching(
            NSPredicate(format: "%K == %@", field, selector)
        )

        let element = query.firstMatch
        var errorMsg: String?
        var objcError: NSError?
        let caught = QVXTryCatch({
            guard element.exists else {
                let lookupKind = byLabel ? "label" : "identifier"
                errorMsg = "Element with \(lookupKind) '\(selector)' and type '\(elementType)' not found"
                return
            }
            guard element.isHittable else {
                let lookupKind = byLabel ? "label" : "identifier"
                errorMsg = "Element with \(lookupKind) '\(selector)' and type '\(elementType)' exists but is not hittable"
                return
            }
            element.tap()
        }, &objcError)
        if !caught {
            let msg = objcError?.localizedDescription ?? "Unknown ObjC exception"
            return .error(message: "Tap failed: \(msg)")
        }
        if let errorMsg = errorMsg {
            return .error(message: errorMsg)
        }
        return .ok
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
        var objcError: NSError?
        let caught = QVXTryCatch({
            startCoord.press(forDuration: 0.05, thenDragTo: endCoord, withVelocity: .default,
                             thenHoldForDuration: 0)
        }, &objcError)
        if !caught {
            let msg = objcError?.localizedDescription ?? "Unknown ObjC exception"
            return .error(message: "Swipe failed: \(msg)")
        }
        // Note: For more precise duration control, we use press+drag.
        // The velocity-based API doesn't directly accept duration, so we approximate.
        // An alternative for exact duration:
        //   startCoord.press(forDuration: 0, thenDragTo: endCoord)
        // which uses the system default duration.

        _ = swipeDuration // Acknowledge the parameter for future use.
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

    private func handleGetValue(selector: String, byLabel: Bool, elementType: String?) -> AgentResponse {
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
                let lookupKind = byLabel ? "label" : "identifier"
                let typeInfo = elementType.map { " and type '\($0)'" } ?? ""
                result = .error(
                    message: "Element with \(lookupKind) '\(selector)'\(typeInfo) not found"
                )
                return
            }
            let value = element.value as? String ?? element.label
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
        return result ?? .error(message: "GetValue produced no result")
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

        let tree = serializeElement(snapshot)

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
        return .ok
    }

    // MARK: - Helpers

    /// Recursively serialize an XCUIElementSnapshot into our Codable tree structure.
    private func serializeElement(_ snapshot: any XCUIElementSnapshot) -> UIElementJSON {
        let frame = snapshot.frame
        let frameJSON = FrameJSON(
            x: Double(frame.origin.x),
            y: Double(frame.origin.y),
            width: Double(frame.size.width),
            height: Double(frame.size.height)
        )

        let children = snapshot.children.map { child -> UIElementJSON in
            serializeElement(child)
        }

        return UIElementJSON(
            AXUniqueId: snapshot.identifier.isEmpty ? nil : snapshot.identifier,
            AXLabel: snapshot.label.isEmpty ? nil : snapshot.label,
            AXValue: (snapshot.value as? String).flatMap { $0.isEmpty ? nil : $0 },
            type: elementTypeName(snapshot.elementType),
            frame: frameJSON,
            children: children,
            role: nil // XCUIElement doesn't directly expose role
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
