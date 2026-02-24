# IPC Reference

Inter-process communication in qorvex uses Unix domain sockets with a JSON-over-newlines protocol. `qorvex-server` runs an `IpcServer` that exposes session state and automation commands to clients like `qorvex-repl`, `qorvex-live`, and `qorvex-cli`.

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
    // Core
    Execute { action: ActionType, tag: Option<String> },
    Subscribe,
    GetState,
    GetLog,

    // Session management
    StartSession,
    EndSession,

    // Device management
    ListDevices,
    UseDevice { udid: String },
    BootDevice { udid: String },

    // Agent management
    StartAgent { project_dir: Option<String> },
    StopAgent,
    Connect { host: String, port: u16 },

    // Configuration
    SetTarget { bundle_id: String },
    SetTimeout { timeout_ms: u64 },
    GetTimeout,

    // Watcher
    StartWatcher { interval_ms: Option<u64> },
    StopWatcher,

    // Info
    GetSessionInfo,
    GetCompletionData,

    // Server lifecycle
    Shutdown,
}
```

| Variant | Purpose |
|---------|---------|
| `Execute` | Send an action for the session to execute. The `action` field is a serialized `ActionType` enum value. The optional `tag` field is a free-text annotation written to `ActionLog` for log filtering. |
| `Subscribe` | Begin receiving `Event` responses as session events occur (screenshots, actions, etc.). |
| `GetState` | Request current session state (session ID, latest screenshot). |
| `GetLog` | Request the full action log history. |
| `StartSession` | Start a new automation session. |
| `EndSession` | End the current session. |
| `ListDevices` | List available simulator devices. |
| `UseDevice` | Select a simulator device by UDID. |
| `BootDevice` | Boot a simulator device by UDID. |
| `StartAgent` | Start or connect to the automation agent; `project_dir` overrides the configured source directory. |
| `StopAgent` | Stop the managed agent process. |
| `Connect` | Connect to an agent at a specific host/port. |
| `SetTarget` | Set the target app bundle ID. |
| `SetTimeout` | Set the default wait timeout in milliseconds. |
| `GetTimeout` | Get the current default wait timeout. |
| `StartWatcher` | Start the screen change watcher; `interval_ms` defaults to 1000ms. |
| `StopWatcher` | Stop the screen change watcher. |
| `GetSessionInfo` | Get current session status. |
| `GetCompletionData` | Get cached elements and devices for client-side tab completion. |
| `Shutdown` | Request the server to shut down cleanly (stop agent, remove socket, exit). Intercepted by the server's accept loop before reaching `handle_request`. |

Management requests (`StartSession` and below) are only handled when the server has a `RequestHandler` attached. The built-in fallback returns an `Error` for these variants with a message directing users to `qorvex-server`.

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
    CommandResult {
        success: bool,
        message: String,
    },
    DeviceList {
        devices: Vec<SimulatorDevice>,
    },
    SessionInfo {
        session_name: String,
        active: bool,
        device_udid: Option<String>,
        action_count: usize,
    },
    CompletionData {
        elements: Vec<UIElement>,
        devices: Vec<SimulatorDevice>,
    },
    TimeoutValue {
        timeout_ms: u64,
    },
    ShutdownAck,
}
```

| Variant | Sent in response to | Fields |
|---------|---------------------|--------|
| `ActionResult` | `Execute` | `success`: whether the action succeeded. `message`: human-readable result. `screenshot`: base64-encoded PNG, set only when the action is `GetScreenshot`. `data`: optional payload (e.g., element value from `GetValue`). |
| `State` | `GetState` | `session_id`: current session identifier. `screenshot`: latest cached screenshot as base64 PNG. |
| `Log` | `GetLog` | `entries`: vector of `ActionLog` entries from the session ring buffer. |
| `Event` | `Subscribe` (streamed) | `event`: a `SessionEvent` pushed to all subscribers. Event types include `ActionLogged`, `ScreenshotUpdated`, `ScreenInfoUpdated`, `Started`, `Ended`. |
| `Error` | Any | `message`: error description. |
| `CommandResult` | Management commands | `success`: whether the command succeeded. `message`: human-readable result. |
| `DeviceList` | `ListDevices` | `devices`: list of available `SimulatorDevice` entries. |
| `SessionInfo` | `GetSessionInfo` | `session_name`, `active`, `device_udid` (if connected), `action_count`. |
| `CompletionData` | `GetCompletionData` | `elements`: cached UI elements from last screen info. `devices`: cached simulator devices. |
| `TimeoutValue` | `GetTimeout` | `timeout_ms`: current default wait timeout. |
| `ShutdownAck` | `Shutdown` | Sent immediately before the server exits. No fields. |

---

## `RequestHandler` Trait

```rust
#[async_trait]
pub trait RequestHandler: Send + Sync + 'static {
    async fn handle(
        &self,
        request: IpcRequest,
        session: Arc<Session>,
        writer: &mut tokio::net::unix::OwnedWriteHalf,
    ) -> Result<(), IpcError>;
}
```

When attached via `IpcServer::with_handler()`, all incoming IPC requests are delegated to this handler instead of the built-in hardcoded logic. `qorvex-server` uses this to provide full management command support. For streaming requests like `Subscribe`, the handler writes multiple responses to `writer`. For single-response requests, it writes one response and returns.

---

## Server Constructors

### `IpcServer::new(session, session_name)`

Creates an IPC server with an empty shared driver slot and no request handler. The server starts without a connected driver; call `set_driver()` before `Execute` requests will work. Management requests return an error directing clients to use `qorvex-server`.

### `IpcServer::with_handler(handler: Arc<dyn RequestHandler>) -> Self`

Builder method: attaches a pluggable request handler. Returns the server instance. When a handler is set, all requests are delegated to it instead of the built-in logic. Used by `qorvex-server` to wire in its own handler.

### `shared_driver() -> Arc<Mutex<Option<Arc<dyn AutomationDriver>>>>`

Returns the shared driver slot. Callers clone this handle and populate it with a connected driver so that IPC `Execute` requests reuse the same backend connection rather than opening a new one.

### `set_driver(driver: Arc<dyn AutomationDriver>)`

Convenience async method to populate the shared driver slot.

### Lifecycle

- On startup: removes any existing socket file at the conventional path, then binds.
- On `Drop`: removes the socket file.
- On `Shutdown` IPC request or SIGINT/SIGTERM: cancels the watcher, drops `ServerState` (triggering `AgentLifecycle::Drop` which kills the agent child process), removes the socket file, then exits. `ShutdownAck` is sent to the requesting client before the shutdown sequence begins.

---

## `qorvex_dir() -> PathBuf`

Returns `~/.qorvex/`, creating the directory if it does not exist.

Panics if the home directory cannot be determined (i.e., `$HOME` is unset and platform-specific lookup fails).

This function is used throughout the codebase for socket paths, log directories, and configuration files.

---

## Architecture Note

The IPC server exists so that all clients — REPL, Live TUI, and CLI — can interact with any running session in real-time, regardless of how that session was started.

- **`qorvex-server`** runs an `IpcServer` with a `RequestHandler` attached. It owns the `ActionExecutor`, `Session`, and agent lifecycle. After the agent connects, the server populates the IPC shared driver slot so `Execute` requests reuse the existing TCP connection.
- **`qorvex-repl`** is an IPC client. It auto-launches `qorvex-server` if the session socket is absent, then connects and sends management and `Execute` requests.
- **`qorvex-cli`** is an IPC client. It connects to a running session's socket and sends `Execute` and management requests.
- **`qorvex-live`** is an IPC client. It connects, sends `Subscribe`, and renders incoming `Event` responses in a TUI.

```
qorvex-server ── IpcServer (Unix socket) ──> qorvex-repl  (Execute, management)
                                         ──> qorvex-live  (Subscribe)
                                         ──> qorvex-cli   (Execute, management)
```
