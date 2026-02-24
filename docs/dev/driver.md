# Driver Abstraction Reference

This document covers the `AutomationDriver` trait, its implementations, and the supporting types that make up the driver abstraction layer in `qorvex-core`.

## Source Files

| File | Contents |
|------|----------|
| `crates/qorvex-core/src/driver.rs` | `AutomationDriver` trait, `DriverConfig`, glob matching, `flatten_elements()` |
| `crates/qorvex-core/src/element.rs` | `UIElement`, `ElementFrame` |
| `crates/qorvex-core/src/agent_driver.rs` | `AgentDriver` implementation |
| `crates/qorvex-core/src/agent_client.rs` | Low-level TCP client (`AgentClient`) |

## `AutomationDriver` Trait

All methods grouped by category. The trait has 22 async methods and 1 sync method.

### Connection

| Method | Description |
|--------|-------------|
| `async fn connect(&mut self) -> Result<(), DriverError>` | Establish connection to the automation backend |
| `fn is_connected(&self) -> bool` | Check if the driver is ready (sync) |

### Tap Actions

| Method | Description |
|--------|-------------|
| `async fn tap_location(&self, x: i32, y: i32) -> Result<(), DriverError>` | Tap at screen coordinates |
| `async fn tap_element(&self, identifier: &str) -> Result<(), DriverError>` | Tap by accessibility ID (single attempt) |
| `async fn tap_by_label(&self, label: &str) -> Result<(), DriverError>` | Tap by accessibility label (single attempt) |
| `async fn tap_with_type(&self, selector: &str, by_label: bool, element_type: &str) -> Result<(), DriverError>` | Tap with element type constraint (single attempt) |
| `async fn tap_element_with_timeout(&self, identifier: &str, timeout_ms: Option<u64>) -> Result<(), DriverError>` | Tap by ID; forwards timeout to agent for agent-side retry |
| `async fn tap_by_label_with_timeout(&self, label: &str, timeout_ms: Option<u64>) -> Result<(), DriverError>` | Tap by label; forwards timeout to agent for agent-side retry |
| `async fn tap_with_type_with_timeout(&self, selector: &str, by_label: bool, element_type: &str, timeout_ms: Option<u64>) -> Result<(), DriverError>` | Tap with type constraint; forwards timeout to agent for agent-side retry |

### Gestures

| Method | Description |
|--------|-------------|
| `async fn swipe(&self, start_x: i32, start_y: i32, end_x: i32, end_y: i32, duration: Option<f64>) -> Result<(), DriverError>` | Swipe gesture with optional duration |
| `async fn long_press(&self, x: i32, y: i32, duration: f64) -> Result<(), DriverError>` | Long press at coordinates |

### Input

| Method | Description |
|--------|-------------|
| `async fn type_text(&self, text: &str) -> Result<(), DriverError>` | Type text into focused element |

### Queries

| Method | Description |
|--------|-------------|
| `async fn dump_tree(&self) -> Result<Vec<UIElement>, DriverError>` | Dump full accessibility hierarchy |
| `async fn get_element_value(&self, identifier: &str) -> Result<Option<String>, DriverError>` | Get value by accessibility ID (single attempt) |
| `async fn get_element_value_by_label(&self, label: &str) -> Result<Option<String>, DriverError>` | Get value by accessibility label (single attempt) |
| `async fn get_value_with_type(&self, selector: &str, by_label: bool, element_type: &str) -> Result<Option<String>, DriverError>` | Get value with type constraint (single attempt) |
| `async fn get_value_with_timeout(&self, selector: &str, by_label: bool, element_type: Option<&str>, timeout_ms: Option<u64>) -> Result<Option<String>, DriverError>` | Get value; forwards timeout to agent for agent-side retry |
| `async fn screenshot(&self) -> Result<Vec<u8>, DriverError>` | Capture screenshot as raw PNG bytes |

### Search (Default Implementations)

These methods have default implementations that dump the full tree and search locally. Backends can override them for better performance.

| Method | Description |
|--------|-------------|
| `async fn list_elements(&self) -> Result<Vec<UIElement>, DriverError>` | Flatten tree, return elements with identifier or label |
| `async fn find_element(&self, identifier: &str) -> Result<Option<UIElement>, DriverError>` | Find by accessibility ID |
| `async fn find_element_by_label(&self, label: &str) -> Result<Option<UIElement>, DriverError>` | Find by accessibility label |
| `async fn find_element_with_type(&self, selector: &str, by_label: bool, element_type: Option<&str>) -> Result<Option<UIElement>, DriverError>` | Find with optional type filter |

### App Switching (Default Returns Error)

| Method | Description |
|--------|-------------|
| `async fn set_target(&self, bundle_id: &str) -> Result<(), DriverError>` | Switch the target application bundle ID |

### Recovery Observability (Default Returns 0)

| Method | Description |
|--------|-------------|
| `fn recovery_count(&self) -> u64` | Number of successful recovery events since creation; `0` for backends without recovery tracking |

## `DriverConfig`

```rust
enum DriverConfig {
    Agent { host: String, port: u16 },
    Device { udid: String, device_port: u16 },
}
```

| Variant | Use Case |
|---------|----------|
| `Agent` | Direct TCP connection to a simulator agent |
| `Device` | USB-tunneled connection to a physical device |

## `DriverError`

| Variant | Description |
|---------|-------------|
| `CommandFailed(String)` | Command execution failed with a message |
| `NotConnected` | Driver is not connected to any backend |
| `ConnectionLost(String)` | Connection was dropped with a reason |
| `Timeout` | Operation timed out |
| `Io(std::io::Error)` | Underlying I/O error |
| `JsonParse(String)` | JSON parsing failed with details |
| `UsbTunnel(UsbTunnelError)` | USB tunnel error (physical devices) |

## `UIElement`

Represents a node in the iOS accessibility hierarchy.

```rust
pub struct UIElement {
    pub identifier: Option<String>,    // serde alias: "AXUniqueId"
    pub label: Option<String>,         // serde alias: "AXLabel"
    pub value: Option<String>,         // serde alias: "AXValue"
    pub element_type: Option<String>,  // serde alias: "type"
    pub frame: Option<ElementFrame>,
    pub children: Vec<UIElement>,
    pub role: Option<String>,
    pub hittable: Option<bool>,
}
```

### Serde Aliases

The struct uses serde aliases to handle both the native field names and the XCUIElement accessibility key names:

| Field | Serde Alias |
|-------|-------------|
| `identifier` | `AXUniqueId` |
| `label` | `AXLabel` |
| `value` | `AXValue` |
| `element_type` | `type` |

## `ElementFrame`

Screen coordinates in points (top-left origin).

```rust
pub struct ElementFrame {
    pub x: f64,      // top-left x in screen points
    pub y: f64,      // top-left y in screen points
    pub width: f64,
    pub height: f64,
}
```

## Element Selector Pattern

Three action types (`Tap`, `GetValue`, `WaitFor`) share a common selector triple:

| Field | Type | Description |
|-------|------|-------------|
| `selector` | `String` | Value to match against |
| `by_label` | `bool` | `false` = match by accessibility ID, `true` = match by accessibility label |
| `element_type` | `Option<String>` | Optional type filter (e.g., `"Button"`, `"TextField"`) |

## Glob Matching

Element selectors support wildcard patterns:

| Pattern | Meaning |
|---------|---------|
| `*` | Matches any sequence of characters (including empty) |
| `?` | Matches exactly one character |

The matching uses a dynamic-programming-based algorithm. It is applied to both `identifier` and `label` fields during element search.

Examples:
- `login*` matches `loginButton`, `loginField`, `login`
- `cell_?` matches `cell_1`, `cell_A` but not `cell_12`
- `*submit*` matches any element containing "submit"

## `AgentDriver` Overrides

`AgentDriver` is the primary implementation of `AutomationDriver`, communicating with the Swift XCTest agent over TCP.

It overrides the default search methods to use the `FindElement` protocol command (`0x13`) instead of dumping the full tree:

| Override | Behavior |
|----------|----------|
| `find_element(identifier)` | Sends `FindElement` with `by_label=false` |
| `find_element_by_label(label)` | Sends `FindElement` with `by_label=true` |
| `find_element_with_type(selector, by_label, element_type)` | Sends `FindElement` with all three fields |

It also overrides the timeout-aware tap/get-value methods to forward `timeout_ms` through the protocol:

| Override | Behavior |
|----------|----------|
| `tap_element_with_timeout(identifier, timeout_ms)` | Sends `TapElement` with `timeout_ms` field |
| `tap_by_label_with_timeout(label, timeout_ms)` | Sends `TapByLabel` with `timeout_ms` field |
| `tap_with_type_with_timeout(selector, by_label, element_type, timeout_ms)` | Sends `TapWithType` with `timeout_ms` field |
| `get_value_with_timeout(selector, by_label, element_type, timeout_ms)` | Sends `GetValue` with `timeout_ms` field |

When `timeout_ms` is forwarded, the Swift agent handles the retry loop locally (50ms poll interval), eliminating one TCP round-trip per retry attempt. The default trait implementations for these methods ignore `timeout_ms` and delegate to the single-attempt versions (for backends that don't support agent-side retry).

When `timeout_ms` is `Some(ms)`, the Rust-side TCP read deadline is set to `ms + 5000ms` so the connection is not dropped before the agent finishes retrying. When `timeout_ms` is `None`, the default 30-second read timeout applies.

This provides accurate **live hittability** -- the `isHittable` property is only available from live `XCUIElement` queries on the Swift side, not from accessibility tree snapshots returned by `DumpTree`.

### Connection Constructors

| Constructor | Connection Type |
|-------------|----------------|
| `AgentDriver::direct(host, port)` | Direct TCP for simulators |
| `AgentDriver::usb_device(udid, port)` | USB tunnel for physical devices |
| `.with_lifecycle(Arc<AgentLifecycle>)` | Builder — attaches a lifecycle manager for crash recovery |

`with_lifecycle()` is a builder that takes ownership and returns `Self`, so it chains onto a constructor:

```rust
let driver = AgentDriver::direct("127.0.0.1", 8080)
    .with_lifecycle(lifecycle.clone());
```

When a lifecycle is attached, the driver automatically recovers from connection drops (see [Crash Recovery](#crash-recovery) below).

### Connection Invalidation

`AgentClient` enforces a read timeout on every response. The default is 30 seconds; calls routed through `send_with_timeout` use a caller-supplied deadline instead (used by the `*_with_timeout` driver methods when `timeout_ms` is set). If the timeout fires (or an I/O error occurs), the TCP stream is **dropped immediately** to prevent response desynchronization.

This matters when the watcher and executor share the same driver: a slow `dump_tree` or `screenshot` that times out will close the connection for both, and the next executor command will fail with `NotConnected` rather than silently reading a stale response.

`dump_tree` uses `send_with_read_timeout` with a fixed 120s deadline (125s total with the +5s buffer) rather than the default 30s, to prevent the connection from being dropped on large accessibility trees.

### Crash Recovery

When `with_lifecycle()` is set, `send()` and `send_with_read_timeout()` catch connection errors (`NotConnected`, `ConnectionLost`, `Io`) and first attempt a cheap TCP reconnect before falling back to a full kill-and-respawn recovery cycle:

**Step 1 — Try TCP reconnect (`try_reconnect`):**
1. Call `create_client()` — open a new TCP socket and verify with heartbeat
2. If successful, replace the stored client and retry the original command once — no agent kill needed

This handles the common case where a read timeout dropped the stream but the agent process is still alive (just slow on a large page).

**Step 2 — Full recovery (only if reconnect fails):**
1. Terminate the old agent process via `AgentLifecycle::terminate_agent()`
2. Respawn via `AgentLifecycle::spawn_agent()` (skips rebuild — the XCTest bundle stays on disk)
3. Wait for the new agent to accept connections via `AgentLifecycle::wait_for_ready()`
4. Create a fresh `AgentClient`, verify with heartbeat, and replace the stored client
5. Retry the original command once

If full recovery also fails (e.g., `spawn_agent` or `wait_for_ready` errors), the error is returned and no further retry is attempted.

**Recovery counter:** every successful recovery (both TCP reconnect and full kill/respawn) increments an internal `AtomicU64` accessible via `recovery_count()`. The executor's `WaitFor` and `WaitForNot` loops poll this counter after each iteration — when it changes, the loop resets its timeout start time (`Instant::now()`) and stability counters, giving the action a fresh timeout budget post-recovery.

**What recovery does NOT cover:**
- `Timeout` errors — the agent is alive but slow; not a connection issue
- `CommandFailed` / `JsonParse` — the agent responded with an error
- USB device connections — `lifecycle` is `None` for physical devices, so recovery is skipped
- `connect()` itself — recovery only activates during command sends, not the initial connect

## `flatten_elements(elements: &[UIElement]) -> Vec<UIElement>`

Public utility function that recursively collects elements from the tree where `identifier.is_some() || label.is_some()`. Uses pre-order traversal (parent before children). Children are cleared in the returned elements to produce a flat list.
