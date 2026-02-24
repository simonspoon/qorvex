# Architecture Guide

This document describes the high-level architecture of the qorvex project: a Rust workspace with five crates, a Swift XCTest agent, and a Swift ScreenCaptureKit streamer for iOS Simulator and physical device automation on macOS.

## Crate Dependency Graph

```
qorvex-server   ──► qorvex-core
qorvex-repl     ──► qorvex-core (via IPC to qorvex-server)
qorvex-live     ──► qorvex-core (via IPC to qorvex-server)
qorvex-live     ──► qorvex-streamer (spawns, reads JPEG frames via Unix socket)
qorvex-cli      ──► qorvex-core (via IPC to qorvex-server)
qorvex-core     ──► qorvex-agent (TCP binary protocol)
```

| Crate / Component | Role |
|-------|------|
| `qorvex-core` | Core library -- driver abstraction, protocol, session, IPC, action types, executor, watcher |
| `qorvex-server` | Standalone automation server daemon -- manages sessions, agent lifecycle, and IPC |
| `qorvex-repl` | TUI REPL client with tab completion, connects to server via IPC; auto-launches server if needed |
| `qorvex-live` | TUI client with live video feed (via qorvex-streamer) and IPC reconnection |
| `qorvex-cli` | Scriptable CLI client for automation pipelines, includes JSONL log-to-script converter |
| `qorvex-agent` | Swift XCTest agent for native iOS accessibility (not a Cargo crate) |
| `qorvex-streamer` | Swift standalone binary; captures Simulator window via ScreenCaptureKit and streams JPEG frames over a Unix socket (macOS 13+, not a Cargo crate) |
| `qorvex-testapp` | SwiftUI iOS test app (bundle ID: `com.qorvex.testapp`) with 5 tabs covering all automation actions; built with XcodeGen, not a Cargo crate |

## Data Flow

1. **Server** (`qorvex-server`) starts, binds an `IpcServer` on the session socket, manages agent lifecycle.
2. **REPL** auto-launches the server if the socket is absent, then connects as an IPC client. Sends commands (e.g., `StartSession`, `Execute`) via IPC.
3. **Server** executes actions via `ActionExecutor` (which delegates to `AutomationDriver`), logs to `Session`.
4. **Session** broadcasts `SessionEvent`s to subscribers (broadcast channel, capacity 100).
5. **Live TUI** connects via `IpcClient`, sends `Subscribe`, renders incoming `Event` responses in a TUI. Separately spawns `qorvex-streamer` and reads JPEG frames from a Unix socket for the live video feed.
6. **Streamer** (`qorvex-streamer`) captures the Simulator window via ScreenCaptureKit on the macOS host, encodes frames as JPEG, and writes them length-prefixed to the Unix socket. Runs as a child process of `qorvex-live`; completely independent of the XCTest agent.
7. **CLI** connects via `IpcClient`, sends `Execute` and management requests.
8. **Screenshots** (from the agent path) are base64-encoded PNGs passed through the event system.
9. **Swift agent lifecycle:** build via `xcodebuild` -> install via `simctl` -> launch test -> TCP connect -> binary protocol commands -> terminate on drop.

```
┌────────────┐   IPC     ┌───────────────────────────────────┐
│ qorvex-    │──────────►│  qorvex-server                    │
│   repl     │           │  IpcServer (Unix socket)          │
└────────────┘           │  qorvex_{session_name}.sock       │
┌────────────┐   IPC     │                                   │    ┌─────────┐
│ qorvex-    │──────────►│  ActionExecutor ──── TCP ────────►│───►│  Swift  │
│   live     │           │  (qorvex-core)       (port 8080) │    │  Agent  │
│  spawns ▼  │           │       │                           │    └─────────┘
│ qorvex-    │◄──────────│  Session (broadcast)              │
│ streamer   │  JPEG     │  SessionEvent ──► subscribers     │
│ (SCKit)    │  frames   └───────────────────────────────────┘
└────────────┘  Unix sock
┌────────────┐   IPC
│ qorvex-    │──────────►  (same server)
│   cli      │
└────────────┘
```

## Key Abstractions

### `AutomationDriver` trait

Backend abstraction with 18 async methods and 1 sync method. Defines the interface for all automation backends. Includes default implementations for search operations that dump the full tree and filter locally.

See [driver.md](driver.md) for the full method listing.

### `Session`

Async session state with broadcast channels for `SessionEvent`s. Maintains a ring buffer (1000 max entries), persistent JSONL log file in `~/.qorvex/logs/`, cached `current_elements`, and a `screen_hash` for change detection.

Constructors:
- `Session::new(simulator_udid, session_name)` -- logs to `~/.qorvex/logs/`
- `Session::new_with_log_dir(simulator_udid, session_name, log_dir)` -- custom log directory

See [session-and-events.md](session-and-events.md) for full details.

### `ActionExecutor`

Backend-agnostic action execution engine. Receives `ActionType` values and delegates to the appropriate `AutomationDriver` methods.

Constructors:
- `ActionExecutor::new(driver)` -- from an existing driver
- `ActionExecutor::with_agent(host, port)` -- creates an `AgentDriver` internally
- `ActionExecutor::from_config(config)` -- creates a driver from `DriverConfig`

Configuration:
- `driver()` -- accessor for the underlying driver

WaitFor behavior: polls every 100ms, requires the element to be hittable, and requires 3 consecutive stable frames before reporting success.

### `AgentLifecycle`

Swift agent process lifecycle management: build (`xcodebuild build-for-testing`), spawn (`test-without-building`), terminate, health-check via TCP heartbeat, and retry logic. Auto-cleanup via `Drop`.

Configured via `AgentLifecycleConfig` (port, timeout, retries).

Two orchestration methods:
- `ensure_running()` -- always rebuilds the agent
- `ensure_agent_ready()` -- skips rebuild if agent is already reachable

## Connection Modes

| Target | Connection | Implementation |
|--------|-----------|---------------|
| Simulators | Direct TCP (port 8080) | `AgentDriver::direct(host, port)` |
| Physical devices | USB tunnel via usbmuxd | `AgentDriver::usb_device(udid, port)` using `idevice` crate |

For physical devices, the `usb_tunnel` module provides:
- `list_devices()` -- enumerate connected USB devices
- `connect(udid, port)` -- establish port forwarding via usbmuxd

## Runtime Directory Structure

```
~/.qorvex/
├── config.json                  # Persistent config (agent_source_dir)
├── qorvex_<session>.sock        # Unix socket per session (IPC)
├── streamer_<session>.sock      # Unix socket for live video frames (qorvex-live)
└── logs/
    └── <session>_<timestamp>.jsonl
```

- `config.json` stores `QorvexConfig` with the `agent_source_dir` field. `install.sh` records the agent project path so sessions can auto-build the agent.
- IPC socket path convention: `~/.qorvex/qorvex_{session_name}.sock`
- Streamer socket path convention: `~/.qorvex/streamer_{session_name}.sock` — created by `qorvex-live` on startup, deleted on quit.
- JSONL log files follow the naming pattern `{session_name}_{%Y%m%d_%H%M%S}.jsonl`

## IPC Protocol

The IPC layer uses Unix sockets with a JSON-over-newlines protocol.

**Core request types:**

| Type | Description |
|------|-------------|
| `Execute` | Run an action command |
| `Subscribe` | Subscribe to session events |
| `GetState` | Get current session state |
| `GetLog` | Get action log history |

**Management request types** (handled by `qorvex-server` via `RequestHandler`):

| Type | Description |
|------|-------------|
| `StartSession` / `EndSession` | Session lifecycle |
| `ListDevices` / `UseDevice` / `BootDevice` | Device management |
| `StartAgent` / `StopAgent` / `Connect` | Agent management |
| `SetTarget` / `SetTimeout` / `GetTimeout` | Configuration |
| `StartWatcher` / `StopWatcher` | Screen change watcher |
| `GetSessionInfo` / `GetCompletionData` | Info and tab completion |

**Response types:**

| Type | Description |
|------|-------------|
| `ActionResult` | Result of an executed action |
| `State` | Current session state |
| `Log` | Action log entries |
| `Event` | Streamed session event |
| `Error` | Error message |
| `CommandResult` | Generic success/failure for management commands |
| `DeviceList` | List of simulator devices |
| `SessionInfo` | Current session status |
| `CompletionData` | Cached elements and devices for tab completion |
| `TimeoutValue` | Current default timeout |

Server constructors:
- `IpcServer::new(session, name)` -- starts with empty driver slot; call `set_driver()` after the agent connects
- `IpcServer::with_handler(handler)` -- attach a `RequestHandler` impl (builder pattern); when set, all requests are delegated to the handler instead of built-in logic
- `shared_driver()` / `set_driver(driver)` -- wire the server to an already-connected driver so `Execute` requests reuse the existing TCP connection

## External Dependencies

- `xcrun simctl` -- Apple's simulator control CLI (comes with Xcode)
- `idevice` -- Rust crate for USB device tunneling via usbmuxd
- `xcodebuild` -- builds and launches the Swift agent
