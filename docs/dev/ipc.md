# IPC Reference

Inter-process communication in qorvex uses Unix domain sockets with a JSON-over-newlines protocol. This allows the REPL and script runner to expose session state to external clients like `qorvex-live` and `qorvex-cli`.

**Source:** `crates/qorvex-core/src/ipc.rs`

---

## Socket Path Convention

Each session creates a Unix socket at:

```
~/.qorvex/qorvex_{session_name}.sock
```

One socket per session. The server removes any existing socket file at the path before binding, and removes it again on `Drop`.

---

## Protocol

**JSON-over-newlines:** each message is a single JSON object terminated by `\n`. No framing headers, no length prefixes -- just newline-delimited JSON.

---

## `IpcRequest` Variants

```rust
#[serde(tag = "type")]
enum IpcRequest {
    Execute { action: ActionType },
    Subscribe,
    GetState,
    GetLog,
}
```

| Variant | Purpose |
|---------|---------|
| `Execute` | Send an action for the session to execute. The `action` field is a serialized `ActionType` enum value. |
| `Subscribe` | Begin receiving `Event` responses as session events occur (screenshots, actions, etc.). |
| `GetState` | Request current session state (session ID, latest screenshot). |
| `GetLog` | Request the full action log history. |

---

## `IpcResponse` Variants

```rust
#[serde(tag = "type")]
enum IpcResponse {
    ActionResult {
        success: bool,
        message: String,
        screenshot: Option<Arc<String>>,
        data: Option<String>,
    },
    State {
        session_id: String,
        screenshot: Option<Arc<String>>,
    },
    Log {
        entries: Vec<ActionLog>,
    },
    Event {
        event: SessionEvent,
    },
    Error {
        message: String,
    },
}
```

| Variant | Sent in response to | Fields |
|---------|---------------------|--------|
| `ActionResult` | `Execute` | `success`: whether the action succeeded. `message`: human-readable result. `screenshot`: base64-encoded PNG if a screenshot was captured post-action. `data`: optional payload (e.g., element value from `GetValue`). |
| `State` | `GetState` | `session_id`: current session identifier. `screenshot`: latest cached screenshot as base64 PNG. |
| `Log` | `GetLog` | `entries`: vector of `ActionLog` entries from the session ring buffer. |
| `Event` | `Subscribe` (streamed) | `event`: a `SessionEvent` pushed to all subscribers. Event types include `ActionLogged`, `ScreenshotUpdated`, `ScreenInfoUpdated`, `Started`, `Ended`. |
| `Error` | Any | `message`: error description. |

---

## Server Constructors

### `IpcServer::new(session, session_name)`

Creates an IPC server with an empty shared driver slot. The server starts without a connected driver; call `set_driver()` (or wire up the `shared_driver()` handle) before Execute requests will work.

### `shared_driver() -> Arc<Mutex<Option<Arc<dyn AutomationDriver>>>>`

Returns the shared driver slot. Callers clone this handle and populate it with a connected driver so that IPC `Execute` requests reuse the same backend connection rather than opening a new one.

### `set_driver(driver: Arc<dyn AutomationDriver>)`

Convenience async method to populate the shared driver slot.

### Lifecycle

- On startup: removes any existing socket file at the conventional path, then binds.
- On `Drop`: removes the socket file.

---

## `qorvex_dir() -> PathBuf`

Returns `~/.qorvex/`, creating the directory if it does not exist.

Panics if the home directory cannot be determined (i.e., `$HOME` is unset and platform-specific lookup fails).

This function is used throughout the codebase for socket paths, log directories, and configuration files.

---

## Architecture Note

The IPC server exists so that `qorvex-live` can monitor any running session in real-time, regardless of how that session was started.

- **`qorvex-repl`** runs its `ActionExecutor` in-process and spawns an `IpcServer`. After the agent connects, it calls `set_executor_with_driver()` which populates both the local executor and the IPC server's shared driver slot — so `qorvex-cli` Execute requests reuse the same TCP connection to the agent rather than opening a competing one.
- **`qorvex-auto`** similarly creates its `ScriptExecutor` and then populates the IPC server's shared driver slot with the connected driver.
- **`qorvex-cli`** is an IPC client. It connects to a running session's socket and sends `Execute` requests; the REPL's connected driver is used to fulfill them.
- **`qorvex-live`** is an IPC client. It connects, sends `Subscribe`, and renders incoming `Event` responses in a TUI.

```
qorvex-repl ──┐
              ├── IpcServer (Unix socket) ──> qorvex-live (Subscribe)
qorvex-auto ──┘                           ──> qorvex-cli  (Execute)
```
