// main.swift
// CLI entry point for qorvex-streamer.
// Captures a Simulator window via ScreenCaptureKit and streams JPEG frames
// over a Unix domain socket using length-prefixed binary framing.

import Foundation
import ScreenCaptureKit
import AppKit

// Establish connection to the window server (required for CGContext/ImageIO).
let _ = NSApplication.shared

// MARK: - Argument parsing

func printUsage() -> Never {
    fputs("""
    Usage: qorvex-streamer --socket-path <path> --udid <udid> [--fps <n>] [--quality <n>]

    Options:
      --socket-path  Path for the Unix domain socket (required)
      --udid         Simulator UDID (required)
      --fps          Frames per second (default: 15)
      --quality      JPEG quality 0-100 (default: 70)

    """, stderr)
    exit(1)
}

func parseArgs() -> (socketPath: String, udid: String, fps: Int, quality: Int) {
    let args = CommandLine.arguments
    var socketPath: String?
    var udid: String?
    var fps = 15
    var quality = 70

    var i = 1
    while i < args.count {
        switch args[i] {
        case "--socket-path":
            i += 1
            guard i < args.count else { printUsage() }
            socketPath = args[i]
        case "--udid":
            i += 1
            guard i < args.count else { printUsage() }
            udid = args[i]
        case "--fps":
            i += 1
            guard i < args.count, let v = Int(args[i]), v > 0 else { printUsage() }
            fps = v
        case "--quality":
            i += 1
            guard i < args.count, let v = Int(args[i]), v >= 0, v <= 100 else { printUsage() }
            quality = v
        default:
            fputs("[qorvex-streamer] Unknown argument: \(args[i])\n", stderr)
            printUsage()
        }
        i += 1
    }

    guard let sp = socketPath, let u = udid else {
        printUsage()
    }
    return (sp, u, fps, quality)
}

// MARK: - Device name resolution

func resolveDeviceName(udid: String) -> String? {
    let pipe = Pipe()
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/xcrun")
    process.arguments = ["simctl", "list", "devices", "-j"]
    process.standardOutput = pipe
    process.standardError = FileHandle.nullDevice

    do {
        try process.run()
    } catch {
        fputs("[qorvex-streamer] Failed to run simctl: \(error)\n", stderr)
        return nil
    }

    // Read pipe data BEFORE waitUntilExit to avoid deadlock when output exceeds pipe buffer
    let data = pipe.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()
    guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let devices = json["devices"] as? [String: [[String: Any]]] else {
        return nil
    }

    for (_, deviceList) in devices {
        for device in deviceList {
            if let deviceUdid = device["udid"] as? String,
               deviceUdid.lowercased() == udid.lowercased(),
               let name = device["name"] as? String {
                return name
            }
        }
    }
    return nil
}

// MARK: - Signal handling

/// Install SIGTERM/SIGINT handlers using GCD dispatch sources (avoids C function pointer capture limitation).
func installSignalHandlers(cleanup: @escaping () -> Void) -> [DispatchSourceSignal] {
    // Ignore default signal handling so dispatch sources receive them.
    signal(SIGTERM, SIG_IGN)
    signal(SIGINT, SIG_IGN)

    let sources = [SIGTERM, SIGINT].map { sig -> DispatchSourceSignal in
        let source = DispatchSource.makeSignalSource(signal: sig, queue: .main)
        source.setEventHandler {
            cleanup()
            exit(0)
        }
        source.resume()
        return source
    }
    return sources
}

// MARK: - Main

let config = parseArgs()

guard let deviceName = resolveDeviceName(udid: config.udid) else {
    fputs("[qorvex-streamer] Could not resolve device name for UDID: \(config.udid)\n", stderr)
    exit(1)
}

NSLog("[qorvex-streamer] Device: %@ (UDID: %@)", deviceName, config.udid)

let socketWriter = SocketWriter(socketPath: config.socketPath)

let _signalSources = installSignalHandlers {
    NSLog("[qorvex-streamer] Shutting down")
    socketWriter.close()
}
_ = _signalSources // Keep sources alive

// Bind the socket and wait for a client before starting capture.
do {
    try socketWriter.bind()
} catch {
    fputs("[qorvex-streamer] Failed to bind socket: \(error)\n", stderr)
    exit(1)
}

NSLog("[qorvex-streamer] Socket bound at %@, waiting for client", config.socketPath)
socketWriter.acceptClient()
NSLog("[qorvex-streamer] Client connected")

// Use a semaphore to bridge async -> sync in main.
let sem = DispatchSemaphore(value: 0)
var streamer: FrameStreamer?

Task {
    do {
        let (window, display) = try await WindowFinder.findSimulatorWindow(deviceName: deviceName)
        NSLog("[qorvex-streamer] Found window: %@", window.title ?? "<untitled>")

        let fs = FrameStreamer(
            window: window,
            display: display,
            fps: config.fps,
            quality: config.quality,
            socketWriter: socketWriter
        )
        streamer = fs
        try await fs.start()
        NSLog("[qorvex-streamer] Streaming at %d fps, quality %d", config.fps, config.quality)
        sem.signal()
    } catch {
        let desc = "\(error)"
        if desc.contains("permission") || desc.contains("denied") || desc.contains("TCCDeny") {
            fputs("[qorvex-streamer] Screen capture permission denied\n", stderr)
            exit(2)
        }
        fputs("[qorvex-streamer] Failed to start capture: \(error)\n", stderr)
        exit(1)
    }
}

sem.wait()

// Keep the process alive.
RunLoop.current.run()
