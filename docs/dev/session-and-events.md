# Session and Events Reference

This document covers the `Session` struct, `SessionEvent` system, `ActionLog` format, and change detection mechanisms in `qorvex-core`.

## Source Files

| File | Contents |
|------|----------|
| `crates/qorvex-core/src/session.rs` | `Session`, `SessionEvent` |
| `crates/qorvex-core/src/action.rs` | `ActionType`, `ActionLog`, `ActionResult` |
| `crates/qorvex-core/src/driver.rs` | `flatten_elements` (used for on-demand element fetching) |

## `Session` Struct

The `Session` is the central state object for a qorvex session. It is always wrapped in `Arc<Self>` and uses interior mutability for concurrent access.

### Fields

| Field | Type | Notes |
|-------|------|-------|
| `id` | `Uuid` | Unique session ID, auto-generated |
| `created_at` | `DateTime<Utc>` | Creation timestamp |
| `simulator_udid` | `Option<String>` | Connected simulator UDID |
| `action_log` | `RwLock<VecDeque<ActionLog>>` | Ring buffer (max 1000 entries) |
| `current_screenshot` | `RwLock<Option<Arc<String>>>` | Base64-encoded PNG |
| `event_tx` | `broadcast::Sender<SessionEvent>` | Broadcast sender (capacity 100) |
| `log_writer` | `Mutex<Option<BufWriter<File>>>` | JSONL file writer |

## Constructors

| Constructor | Log Directory |
|-------------|---------------|
| `Session::new(simulator_udid, session_name) -> Arc<Self>` | `~/.qorvex/logs/` |
| `Session::new_with_log_dir(simulator_udid, session_name, log_dir) -> Arc<Self>` | Custom path |

Log file naming: `{session_name}_{%Y%m%d_%H%M%S}.jsonl`

Example: `my_session_20260218_143022.jsonl`

## `SessionEvent` Variants

```rust
enum SessionEvent {
    ActionLogged(ActionLog),
    ScreenshotUpdated(Arc<String>),       // base64-encoded PNG
    Started { session_id: Uuid },
    Ended,
}
```

| Variant | Emitted When |
|---------|-------------|
| `ActionLogged` | An action is logged via `log_action` or `log_action_timed` |
| `ScreenshotUpdated` | A new screenshot is captured and stored |
| `Started` | A session begins |
| `Ended` | A session ends |

Events are delivered via a `tokio::sync::broadcast` channel with capacity 100. Subscribers receive events by calling `event_tx.subscribe()` to obtain a `broadcast::Receiver<SessionEvent>`.

## `ActionLog` Fields

| Field | Type | Notes |
|-------|------|-------|
| `id` | `Uuid` | Auto-generated unique ID |
| `timestamp` | `DateTime<Utc>` | Auto-generated at log time |
| `action` | `ActionType` | The action that was executed |
| `result` | `ActionResult` | Success or failure outcome |
| `screenshot` | `Option<Arc<String>>` | Post-action screenshot (base64 PNG) |
| `duration_ms` | `Option<u64>` | Total action duration in milliseconds |
| `wait_ms` | `Option<u64>` | Element lookup/wait phase duration |
| `tap_ms` | `Option<u64>` | Agent execution phase duration |
| `tag` | `Option<String>` | Free-text annotation for log filtering; omitted from JSON if `None` |

### JSONL Serialization

Screenshots are **stripped** before JSONL serialization to keep log file size manageable. The `screenshot` field is set to `None` in the serialized output.

The `wait_ms` and `tap_ms` fields provide per-phase timing breakdowns for tap actions, separating the time spent finding the element from the time spent executing the tap on the agent.

## `ActionType` Enum

```rust
enum ActionType {
    Tap { selector: String, by_label: bool, element_type: Option<String> },
    TapLocation { x: i32, y: i32 },
    Swipe { start_x: i32, start_y: i32, end_x: i32, end_y: i32, duration: Option<f64> },
    LongPress { x: i32, y: i32, duration: f64 },
    SendKeys { text: String },
    GetScreenshot,
    GetScreenInfo,
    GetValue { selector: String, by_label: bool, element_type: Option<String> },
    WaitFor { selector: String, by_label: bool, element_type: Option<String> },
    LogComment { message: String },
    StartSession,
    EndSession,
    Quit,
}
```

The `Tap`, `GetValue`, and `WaitFor` variants share a common selector triple (`selector`, `by_label`, `element_type`). See [driver.md](driver.md) for details on the selector pattern.

## Logging Methods

### `log_action`

```rust
pub async fn log_action(
    &self,
    action: ActionType,
    result: ActionResult,
    screenshot: Option<Arc<String>>,
    duration_ms: Option<u64>,
    tag: Option<String>,
)
```

Standard logging method. Appends to the ring buffer, writes to the JSONL file, and broadcasts `SessionEvent::ActionLogged`.

### `log_action_timed`

```rust
pub async fn log_action_timed(
    &self,
    action: ActionType,
    result: ActionResult,
    screenshot: Option<Arc<String>>,
    duration_ms: Option<u64>,
    wait_ms: Option<u64>,
    tap_ms: Option<u64>,
    tag: Option<String>,
)
```

Extended logging with per-phase timing. Used for tap actions where the total duration is broken into element lookup time (`wait_ms`) and agent execution time (`tap_ms`).

## Ring Buffer

The action log uses a `VecDeque<ActionLog>` with a maximum size of 1000 entries, defined by the `MAX_ACTION_LOG_SIZE` constant.

When the buffer is full, the oldest entries are dropped from the front of the deque before new entries are appended.

```
[oldest] ◄── front                        back ──► [newest]
         ◄── dropped when full     new entries ──►
```

## On-Demand Element Fetching

UI elements are no longer polled continuously. Instead, `qorvex-repl` fetches elements on demand via the `FetchElements` IPC request when the user enters an element selector context (e.g., typing `tap `). The server calls `driver.dump_tree()` and `flatten_elements()` directly.

- Elements are cached per command name in the REPL for the duration of that completion session.
- Cache is invalidated when the command changes, input is submitted, or input is cleared.
- A loading indicator appears in the completion popup after 100ms if the fetch is still in progress.
