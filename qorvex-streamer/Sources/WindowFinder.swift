// WindowFinder.swift
// Locates the iOS Simulator window matching a given device name
// using ScreenCaptureKit's shareable content API.

import Foundation
import ScreenCaptureKit

enum WindowFinderError: Error, CustomStringConvertible {
    case noSimulatorWindow(deviceName: String)
    case noDisplayFound
    case sharingNotPermitted

    var description: String {
        switch self {
        case .noSimulatorWindow(let name):
            return "No Simulator window found for device '\(name)'. Is the Simulator running and the device booted?"
        case .noDisplayFound:
            return "No display found containing the Simulator window."
        case .sharingNotPermitted:
            return "Screen capture permission denied. Grant access in System Settings > Privacy & Security > Screen & System Audio Recording."
        }
    }
}

enum WindowFinder {

    /// Find the Simulator window whose title contains `deviceName`.
    /// Returns the matching `SCWindow` and the `SCDisplay` that contains it.
    static func findSimulatorWindow(deviceName: String) async throws -> (SCWindow, SCDisplay) {
        let content: SCShareableContent
        do {
            content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
        } catch {
            let desc = "\(error)"
            if desc.contains("permission") || desc.contains("denied") || desc.contains("TCCDeny") {
                throw WindowFinderError.sharingNotPermitted
            }
            throw error
        }

        // Find the Simulator window matching the device name.
        guard let window = content.windows.first(where: { w in
            w.owningApplication?.bundleIdentifier == "com.apple.iphonesimulator"
                && w.title?.contains(deviceName) == true
        }) else {
            throw WindowFinderError.noSimulatorWindow(deviceName: deviceName)
        }

        // Find the display that contains this window.
        guard let display = content.displays.first else {
            throw WindowFinderError.noDisplayFound
        }

        return (window, display)
    }
}
