# qorvex-agent

A minimal Swift XCTest UI Testing agent that runs a TCP server inside the iOS Simulator and translates binary protocol commands into XCUIElement accessibility actions.

## Architecture

```
Rust host (qorvex-core)
    |
    | TCP connection (port 8080)
    | Binary protocol (LE integers, length-prefixed strings)
    |
    v
Swift agent (XCTest UI test)
    |
    | XCUIElement / XCUIApplication APIs
    |
    v
iOS Simulator accessibility layer
```

The agent is packaged as an XCTest UI Testing target. When `xcodebuild test` runs it, the test starts a TCP server (using Apple's Network framework `NWListener`) on port 8080 and blocks indefinitely. The Rust host connects to this server and sends binary commands; the agent executes them via XCUIElement APIs and sends responses back.

## Getting Started

### Prerequisites

- Xcode with iOS Simulator runtime
- [XcodeGen](https://github.com/yonaskolb/XcodeGen) (`brew install xcodegen`)

### Build and Run

```bash
# Install xcodegen (one-time)
make install-xcodegen

# Build the test bundle
make build

# Build and run the agent on the simulator
make run

# Single-step build + run
make test

# Clean build artifacts and generated project
make clean
```

The Makefile automatically generates the Xcode project from `project.yml` before building. No manual Xcode setup is needed.

## How It Works

### Protocol

The binary protocol is defined in `qorvex-core/src/protocol.rs` (Rust) and `Sources/Protocol.swift` (Swift). Every message is framed as:

```
[4-byte LE u32: payload length] [1-byte opcode] [payload bytes]
```

**Request opcodes** (host -> agent):

| OpCode | Name         | Payload |
|--------|-------------|---------|
| 0x01   | Heartbeat   | (none) |
| 0x02   | TapCoord    | i32 x, i32 y |
| 0x03   | TapElement  | string (accessibility ID) |
| 0x04   | TapByLabel  | string (accessibility label) |
| 0x05   | TapWithType | string selector, u8 by_label, string element_type |
| 0x06   | TypeText    | string text |
| 0x07   | Swipe       | i32 start_x, start_y, end_x, end_y, u8 has_duration, optional f64 |
| 0x08   | GetValue    | string selector, u8 by_label, optional string element_type |
| 0x10   | DumpTree    | (none) |
| 0x11   | Screenshot  | (none) |

**Response format** (agent -> host):

```
[4-byte LE length] [0xA0 opcode] [response_type byte] [payload]
```

| Type | Name       | Payload |
|------|-----------|---------|
| 0x00 | Ok        | (none) |
| 0x01 | Error     | string message |
| 0x02 | Tree      | string JSON |
| 0x03 | Screenshot| u32 data_len, bytes |
| 0x04 | Value     | u8 has_value, optional string |

### Accessibility Tree JSON

The `DumpTree` command returns a JSON tree matching the `UIElement` struct in `qorvex-core/src/element.rs`:

```json
[{
    "AXUniqueId": "login-button",
    "AXLabel": "Log In",
    "AXValue": null,
    "type": "Button",
    "frame": {"x": 100.0, "y": 400.0, "width": 190.0, "height": 44.0},
    "children": [],
    "role": null
}]
```

### Element Type Mapping

The agent maps between string type names (e.g., `"Button"`, `"TextField"`) and `XCUIElement.ElementType` enum values. The full mapping is in `CommandHandler.swift`. Common types:

- `Button`, `TextField`, `SecureTextField`, `StaticText`, `Switch`, `Toggle`
- `NavigationBar`, `TabBar`, `Table`, `Cell`, `ScrollView`
- `Image`, `Slider`, `Picker`, `Alert`, `Sheet`

## Notes

- The agent does **not** launch any app. It operates on whatever app is in the foreground. The Rust host is responsible for launching apps via `simctl`.
- Only one TCP connection is accepted at a time. A new connection replaces the old one.
- All operations are synchronous from the protocol's perspective: one request, one response, then the next request.
- The server uses port 8080 by default. The iOS Simulator shares the host's network stack, so the Rust host connects to `localhost:8080`.
- Screenshots are returned as PNG data using `XCUIScreen.main.screenshot().pngRepresentation`.
