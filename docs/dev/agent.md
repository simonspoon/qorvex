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
2. Creates `CommandHandler(app:)`, which immediately disables XCUITest quiescence waiting via KVC (`currentInteractionOptions = 3`). This eliminates the 1-2s per-interaction delay that XCUITest adds waiting for animations and network to settle.
3. Creates `AgentServer(port: 8080, handler:)`.
4. Starts the TCP listener via `NWListener` (Network framework).
5. Blocks indefinitely via `XCTestExpectation` with infinite timeout.

The agent does NOT launch any app. The Rust host manages app launching via `xcrun simctl`. The `SetTarget` protocol command switches the app context by replacing the `XCUIApplication` reference inside `CommandHandler`.

### Quiescence API Notes

`XCUIApplication.shouldWaitForQuiescence` **does not exist** in modern XCTest/XCUIAutomation (it is not in any public header). Quiescence is controlled via the private `currentInteractionOptions` property (ivar: `_currentInteractionOptions`, type `unsigned int`):

- Bit 0 (`0x1`) — skip pre-event quiescence
- Bit 1 (`0x2`) — skip post-event quiescence
- Value `3` disables both

Set via KVC: `app.setValue(3, forKey: "currentInteractionOptions")`. Do **not** use `perform(NSSelectorFromString("setCurrentInteractionOptions:"), with: value)` — this boxes the value as `NSNumber`, which is not the type the setter expects for a scalar ivar.

The property and bitmask were confirmed by disassembling `XCUIAutomation.framework` and inspecting the `shouldSkipPreEventQuiescence`/`shouldSkipPostEventQuiescence` methods on `XCUIApplicationProcess`.

**`XCUIElement.tap()` re-engages quiescence internally.** Setting `currentInteractionOptions = 3` only disables pre/post-event waits at the `XCUIApplication` level. `XCUIElement.tap()` routes through `XCUIElementProxy` which has its own quiescence path — this causes indefinite hangs when *any* animation (spinner, loader) is active in the app, even on a completely unrelated element. Use `XCUICoordinate.tap()` instead: compute the element's center from its `frame` and tap via `app.coordinate(withNormalizedOffset:).withOffset()`. This bypasses the proxy's quiescence wait entirely. The `handleTapCoord` handler already uses this pattern; `performTap` was updated to match.

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
  -destination "generic/platform=iOS Simulator" \
  -derivedDataPath .build
```

The `generic/platform=iOS Simulator` destination produces a universal simulator bundle that works with any booted simulator, without requiring a specific UDID. `install.sh` runs this build step so the bundle is ready before any session starts.

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
| Builds agent | Only if `Build/Products/*.xctestrun` absent (pre-built by `install.sh` skips build) | No -- checks TCP reachability first; delegates to `ensure_running` if unreachable |
| Use case | Fresh start or known stale agent | Idempotent startup, skip build/spawn if already running |
| Retry behavior | Up to `max_retries + 1` attempts (spawn + health check) | Attempts health check first; delegates to `ensure_running` only if unreachable |

`ensure_running` calls `is_agent_built()` to detect whether a `.xctestrun` file exists in `.build/Build/Products/`. If pre-built products are present (normal case after `install.sh`), the build step is skipped and startup reduces to spawn + health check.

## Crash Recovery (via `AgentDriver::with_lifecycle`)

`AgentLifecycle` also participates in mid-session crash recovery. When an `AgentDriver` is constructed with `.with_lifecycle(Arc<AgentLifecycle>)`, any connection error during a command first tries a cheap TCP reconnect, then falls back to a full kill-and-respawn cycle if reconnect fails:

**Reconnect attempt (agent may still be alive — just slow):**
1. Open a new TCP socket to the agent and verify with heartbeat
2. If heartbeat succeeds, replace the stored client and retry the command — no process kill needed

**Full recovery (only if reconnect fails):**
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
| `tapElement` | `handleTapElement` | NSPredicate on `identifier`; uses `pollUntilFound` when `timeoutMs` is set; waits for 2 consecutive polls with stable `element.frame` before tapping; re-queries immediately before tap; taps via `XCUICoordinate` at frame center (bypasses XCUITest quiescence) |
| `tapByLabel` | `handleTapByLabel` | NSPredicate on `label`; uses `pollUntilFound` when `timeoutMs` is set; waits for 2 consecutive polls with stable `element.frame` before tapping; re-queries immediately before tap; taps via `XCUICoordinate` at frame center (bypasses XCUITest quiescence) |
| `tapWithType` | `handleTapWithType` | Maps type string to `XCUIElement.ElementType`; uses `pollUntilFound` when `timeoutMs` is set; waits for 2 consecutive polls with stable `element.frame` before tapping; re-queries immediately before tap; taps via `XCUICoordinate` at frame center (bypasses XCUITest quiescence) |
| `typeText` | `handleTypeText` | Finds element with `hasKeyboardFocus`, falls back to `app.keyboards.firstMatch` |
| `swipe` | `handleSwipe` | Computes velocity from distance/duration (`distance / seconds`), passes to `press(forDuration:thenDragTo:withVelocity:thenHoldForDuration:)` |
| `longPress` | `handleLongPress` | `coordinate.press(forDuration:)` at specified coordinates |
| `getValue` | `handleGetValue` | Returns `element.value` as String, falls back to `element.label`; uses `pollUntilFound` when `timeoutMs` is set |
| `dumpTree` | `handleDumpTree` | `app.snapshot()` via `QVXTryCatch`, serialized to JSON with empty-node pruning |
| `screenshot` | `handleScreenshot` | `XCUIScreen.main.screenshot().pngRepresentation` -- full screen capture |
| `setTarget` | `handleSetTarget` | Replaces `self.app = XCUIApplication(bundleIdentifier:)` for app context switching; disables quiescence on the new app |
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

### Modal / Stale Coordinate Gotcha

With quiescence waiting disabled, an element inside a modal sheet or page can resolve as `exists = true` and `isHittable = true` while its presentation animation is still in progress. The first query computes the element's frame from a mid-animation accessibility snapshot; calling `.tap()` on that reference lands the tap at stale coordinates (behind the modal or off-screen). The agent returns `Response::Ok` because the tap call itself didn't throw, but nothing visibly happens.

**Fix — four-layer defense in `performTap` when `timeoutMs` is set:**

1. **Frame stability check:** Each poll captures `element.frame` and tracks it across iterations (via captured `lastFrame`/`stableCount` variables in the action closure). The tap only proceeds when the same frame has been observed on 2 consecutive 50ms polls (~50ms of stability). This adds ~50ms overhead on static elements but correctly waits out modal animations (~500-700ms). Frame stability is skipped for zero-area elements (`frame == .zero`).

2. **Re-query before tap:** After the frame is confirmed stable, the element is re-queried one final time (`queryFn()`) to get the freshest possible `XCUIElement` reference. This costs one extra accessibility query per tap.

3. **Pre-tap frame drift check:** The re-queried element's frame is compared to the stable frame. If they differ (element moved between stability check and re-query), stability tracking resets and the poll retries. This catches mid-animation elements that briefly pause at an easing curve plateau. Near-zero cost since the frame is already resolved.

4. **Coordinate-based tap:** The final tap uses `XCUICoordinate.tap()` at the center of `fresh.frame` rather than `fresh.tap()` directly. `XCUIElement.tap()` routes through `XCUIElementProxy` and internally re-engages XCUITest's quiescence wait even when `currentInteractionOptions = 3` is set — causing indefinite hangs when unrelated animations (spinners, loaders) are active elsewhere in the app. `XCUICoordinate.tap()` bypasses this path. The frame coordinates are safe to use because layers 1-3 have already confirmed stability and freshness.

If any check in layers 1-3 fails, the action closure returns `nil` and `pollUntilFound` retries on the next 50ms interval.

When `timeoutMs` is `nil` (i.e., `--no-wait`), layers 1-3 are skipped and the tap fires immediately with a single attempt (layer 4 still applies).

All three tap handlers (`handleTapElement`, `handleTapByLabel`, `handleTapWithType`) delegate to `performTap(queryFn:description:timeoutMs:)` which contains the shared defense logic.

### Tree Serialization Pruning

`serializeElement` returns `UIElementJSON?` and prunes empty scaffolding nodes bottom-up: a node is omitted if it has **no identity** (empty identifier, label, and value), **no area** (zero-size frame), **and no surviving children**. Nodes with any identity or visible area are always kept. This reduces JSON payload size without losing actionable elements.

**Depth and element-count limits** are applied as defense-in-depth against pathologically large trees:

| Constant | Value | Effect when exceeded |
|----------|-------|----------------------|
| `maxTreeDepth` | 30 | Children at depth ≥ 30 return `nil` (parent still serialized) |
| `maxTreeElements` | 5000 | Once 5000 nodes have been visited, all remaining nodes return `nil` |

The element count is incremented on entry (before pruning), so it measures work done rather than output size. Both limits are checked at the start of each `serializeElement` call; hitting either causes that subtree to be silently omitted — there is no error or signal to the Rust side that the tree was truncated. These limits affect `handleDumpTree` and `handleFindElement` (both call `serializeElement`).

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
