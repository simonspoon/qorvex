# Swift Agent Reference

The qorvex agent is a native Swift XCTest-based process that provides iOS accessibility automation over a TCP binary protocol. It runs inside the iOS Simulator (or on a physical device) as a UI test bundle.

**Source:** `qorvex-agent/`, `crates/qorvex-core/src/agent_lifecycle.rs`

---

## Project Structure

The Swift agent is NOT a Cargo crate. It is an XCTest UI Testing project generated via [xcodegen](https://github.com/yonaskolb/XcodeGen) from `project.yml`.

```
qorvex-agent/
├── project.yml               # XcodeGen project definition (app + UI test bundle, iOS 16.0+)
├── Makefile                   # Build/test via xcodebuild with auto-detection of booted simulator
├── Sources/
│   ├── AgentServer.swift      # TCP server (Network framework, NWListener)
│   ├── Protocol.swift         # Binary protocol codec (13 request opcodes, 6 response types)
│   ├── CommandHandler.swift   # XCUIElement action dispatch
│   ├── UITreeSerializer.swift # Accessibility tree snapshot to JSON serialization
│   ├── ObjCExceptionCatcher.h # Exception bridge header
│   ├── ObjCExceptionCatcher.m # @try/@catch implementation
│   ├── BridgingHeader.h       # Swift-ObjC bridging header
│   ├── QorvexAgentTests.swift # XCTest entry point (testRunAgent)
│   └── App/
│       └── QorvexAgentApp.swift  # Minimal SwiftUI app stub (required by xcodegen)
```

---

## How the Agent Runs

The agent is an XCTest UI test: `QorvexAgentTests/testRunAgent`. On launch it:

1. Creates `XCUIApplication(bundleIdentifier: "com.apple.springboard")` -- defaults to SpringBoard.
2. Creates `CommandHandler(app:)` and `AgentServer(port: 8080, handler:)`.
3. Starts the TCP listener via `NWListener` (Network framework).
4. Blocks indefinitely via `XCTestExpectation` with infinite timeout.

The agent does NOT launch any app. The Rust host manages app launching via `xcrun simctl`. The `SetTarget` protocol command switches the app context by replacing the `XCUIApplication` reference inside `CommandHandler`.

---

## Build/Test Flow via `AgentLifecycle`

The Rust side manages the agent's full lifecycle through `AgentLifecycle` (defined in `crates/qorvex-core/src/agent_lifecycle.rs`).

### `AgentLifecycleConfig`

| Field | Type | Default |
|-------|------|---------|
| `project_dir` | `PathBuf` | (required) |
| `agent_port` | `u16` | `8080` |
| `startup_timeout` | `Duration` | 30s |
| `max_retries` | `u32` | `3` |

### Build

```bash
xcodebuild build-for-testing \
  -project QorvexAgent.xcodeproj \
  -scheme QorvexAgentUITests \
  -destination "id=<udid>" \
  -derivedDataPath .build
```

### Spawn

```bash
xcodebuild test-without-building \
  -project QorvexAgent.xcodeproj \
  -scheme QorvexAgentUITests \
  -destination "id=<udid>" \
  -derivedDataPath .build \
  -only-testing QorvexAgentUITests/QorvexAgentTests/testRunAgent
```

### Health Check

Polls every 500ms: TCP connect + heartbeat to `127.0.0.1:<port>`, repeated until success or `startup_timeout` is exceeded.

### Terminate

Kills the child process. Falls back to `xcrun simctl terminate <udid> com.qorvex.agent` if the child process is not available. Auto-cleanup via `Drop`.

---

## `ensure_running()` vs `ensure_agent_ready()`

|  | `ensure_running` | `ensure_agent_ready` |
|---|---|---|
| Always rebuilds | Yes | No -- checks TCP reachability first |
| Use case | Fresh start, known stale agent | Idempotent startup, skip rebuild if already running |
| Retry behavior | Up to `max_retries + 1` attempts (build + spawn + health check) | Attempts health check first; delegates to `ensure_running` only if unreachable |

## Crash Recovery (via `AgentDriver::with_lifecycle`)

`AgentLifecycle` also participates in mid-session crash recovery. When an `AgentDriver` is constructed with `.with_lifecycle(Arc<AgentLifecycle>)`, any connection error during a command triggers an automatic recovery cycle:

1. `terminate_agent()` — kill the old agent process
2. `spawn_agent()` — respawn without rebuilding (XCTest bundle stays on disk)
3. `wait_for_ready()` — poll until TCP accepts a heartbeat
4. Reconnect `AgentClient` and retry the command once

The server (`qorvex-server`) automatically passes the lifecycle to the driver whenever it starts a managed agent:

```rust
let lifecycle = Arc::new(AgentLifecycle::new(udid, config));
lifecycle.ensure_agent_ready().await?;
let driver = AgentDriver::direct("127.0.0.1", 8080)
    .with_lifecycle(lifecycle.clone());
```

Recovery is skipped for physical device connections (no lifecycle is attached to USB-tunnelled drivers).

---

## `CommandHandler` Dispatch

All commands are dispatched on the main thread via `AgentServer`.

| Command | Handler Method | Key Details |
|---------|---------------|-------------|
| `heartbeat` | inline | Returns `.ok` immediately |
| `tapCoord` | `handleTapCoord` | Uses `app.coordinate(withNormalizedOffset:)` with absolute offset |
| `tapElement` | `handleTapElement` | NSPredicate on `identifier`; uses `pollUntilFound` when `timeoutMs` is set |
| `tapByLabel` | `handleTapByLabel` | NSPredicate on `label`; uses `pollUntilFound` when `timeoutMs` is set |
| `tapWithType` | `handleTapWithType` | Maps type string to `XCUIElement.ElementType`; uses `pollUntilFound` when `timeoutMs` is set |
| `typeText` | `handleTypeText` | Finds element with `hasKeyboardFocus`, falls back to `app.keyboards.firstMatch` |
| `swipe` | `handleSwipe` | Computes velocity from distance/duration (`distance / seconds`), passes to `press(forDuration:thenDragTo:withVelocity:thenHoldForDuration:)` |
| `longPress` | `handleLongPress` | `coordinate.press(forDuration:)` at specified coordinates |
| `getValue` | `handleGetValue` | Returns `element.value` as String, falls back to `element.label`; uses `pollUntilFound` when `timeoutMs` is set |
| `dumpTree` | `handleDumpTree` | `app.snapshot()` via `QVXTryCatch`, serialized to JSON with empty-node pruning |
| `screenshot` | `handleScreenshot` | `XCUIScreen.main.screenshot().pngRepresentation` -- full screen capture |
| `setTarget` | `handleSetTarget` | Replaces `self.app = XCUIApplication(bundleIdentifier:)` for app context switching |
| `findElement` | `handleFindElement` | Queries live `XCUIElement` for `isHittable` (not from snapshot), overrides hittable field in response |

### `pollUntilFound` Helper

Used by `handleTapElement`, `handleTapByLabel`, `handleTapWithType`, and `handleGetValue` when `timeoutMs > 0`:

```swift
private func pollUntilFound(
    timeoutMs: UInt64?,
    interval: TimeInterval = 0.050,
    query: () -> XCUIElement,
    action: (XCUIElement) -> AgentResponse?  // nil = retry, non-nil = done
) -> AgentResponse?
```

- Polls at 50ms intervals until the action closure returns a non-nil response, or the timeout is reached.
- When `timeoutMs` is `nil` or `0`, runs the action exactly once.
- ObjC exceptions (caught by `QVXTryCatch`) are returned immediately — never retried.
- Only "not found"/"not hittable" conditions (action returns `nil`) trigger the next poll.

### Tree Serialization Pruning

`serializeElement` returns `UIElementJSON?` and prunes empty scaffolding nodes bottom-up: a node is omitted if it has **no identity** (empty identifier, label, and value), **no area** (zero-size frame), **and no surviving children**. Nodes with any identity or visible area are always kept. This reduces JSON payload size without losing actionable elements.

---

## ObjC Exception Catching

XCUIElement operations frequently throw `NSException` when element references become stale (e.g., after navigation or app state changes). These exceptions bypass Swift's error handling and would crash the agent process.

`QVXTryCatch` wraps calls in Objective-C `@try/@catch`:

```objc
// ObjCExceptionCatcher.m
void QVXTryCatch(void (^block)(void), NSError **error) {
    @try {
        block();
    } @catch (NSException *exception) {
        *error = [NSError errorWithDomain:@"com.qorvex.agent.objc-exception" ...];
    }
}
```

The bridging header (`BridgingHeader.h`) exposes this to Swift. All `CommandHandler` methods that touch `XCUIElement` properties or perform actions should use this wrapper.

---

## `UIElementJSON` Shape

```swift
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
```

This structure matches the Rust-side `UIElement` struct defined in `crates/qorvex-core/src/element.rs`.

**Important:** The `hittable` field is only populated by `findElement` (which performs a live query on the `XCUIElement`). The `dumpTree` command uses `app.snapshot()`, which does not have access to `isHittable`, so `hittable` is always `nil` in tree dumps. This distinction is why `AgentDriver` overrides the default `find_element` methods to use the `FindElement` protocol command for accurate hittability information.

---

## Binary Protocol Overview

The wire protocol uses little-endian encoding with a 4-byte length header. The full codec is defined symmetrically in:

- **Rust:** `crates/qorvex-core/src/protocol.rs`
- **Swift:** `qorvex-agent/Sources/Protocol.swift`

The protocol defines 13 request opcodes (including `SetTarget` and `FindElement`) and 6 response types. See `docs/dev/contributing.md` for guidance on adding new opcodes.
