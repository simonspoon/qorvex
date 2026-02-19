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

All methods grouped by category. The trait has 18 async methods and 1 sync method.

### Connection

| Method | Description |
|--------|-------------|
| `async fn connect(&mut self) -> Result<(), DriverError>` | Establish connection to the automation backend |
| `fn is_connected(&self) -> bool` | Check if the driver is ready (sync) |

### Tap Actions

| Method | Description |
|--------|-------------|
| `async fn tap_location(&self, x: i32, y: i32) -> Result<(), DriverError>` | Tap at screen coordinates |
| `async fn tap_element(&self, identifier: &str) -> Result<(), DriverError>` | Tap by accessibility ID |
| `async fn tap_by_label(&self, label: &str) -> Result<(), DriverError>` | Tap by accessibility label |
| `async fn tap_with_type(&self, selector: &str, by_label: bool, element_type: &str) -> Result<(), DriverError>` | Tap with element type constraint |

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
| `async fn get_element_value(&self, identifier: &str) -> Result<Option<String>, DriverError>` | Get value by accessibility ID |
| `async fn get_element_value_by_label(&self, label: &str) -> Result<Option<String>, DriverError>` | Get value by accessibility label |
| `async fn get_value_with_type(&self, selector: &str, by_label: bool, element_type: &str) -> Result<Option<String>, DriverError>` | Get value with type constraint |
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

This provides accurate **live hittability** -- the `isHittable` property is only available from live `XCUIElement` queries on the Swift side, not from accessibility tree snapshots returned by `DumpTree`.

### Connection Constructors

| Constructor | Connection Type |
|-------------|----------------|
| `AgentDriver::direct(host, port)` | Direct TCP for simulators |
| `AgentDriver::usb_device(udid, port)` | USB tunnel for physical devices |

## `flatten_elements(elements: &[UIElement]) -> Vec<UIElement>`

Public utility function that recursively collects elements from the tree where `identifier.is_some() || label.is_some()`. Uses pre-order traversal (parent before children). Children are cleared in the returned elements to produce a flat list.
