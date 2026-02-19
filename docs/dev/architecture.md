# Architecture Guide

This document describes the high-level architecture of the qorvex project: a Rust workspace with four crates plus a Swift agent for iOS Simulator and physical device automation on macOS.

## Crate Dependency Graph

```
qorvex-repl  ──► qorvex-core
qorvex-live  ──► qorvex-core (via IPC)
qorvex-cli   ──► qorvex-core (via IPC)
qorvex-core  ──► qorvex-agent (TCP binary protocol)
```

| Crate | Role |
|-------|------|
| `qorvex-core` | Core library -- driver abstraction, protocol, session, IPC, action types, executor, watcher |
| `qorvex-repl` | TUI REPL with tab completion, uses core directly |
| `qorvex-live` | TUI client with screenshot rendering (ratatui-image) and IPC reconnection |
| `qorvex-cli` | Scriptable CLI client for automation pipelines, includes JSONL log-to-script converter |
| `qorvex-agent` | Swift XCTest agent for native iOS accessibility (not a Cargo crate) |

## Data Flow

1. **REPL** receives commands via stdin, executes via `ActionExecutor` (which delegates to `AutomationDriver`), logs to `Session`.
2. **Session** broadcasts `SessionEvent`s to subscribers (broadcast channel, capacity 100).
3. **Live TUI** connects via `IpcClient`, subscribes to events, renders TUI with ratatui.
4. **CLI** connects via `IpcClient`, sends `Execute` requests.
5. **Screenshots** are base64-encoded PNGs passed through the event system.
6. **Swift agent lifecycle:** build via `xcodebuild` -> install via `simctl` -> launch test -> TCP connect -> binary protocol commands -> terminate on drop.

```
┌────────────┐   stdin    ┌────────────────┐              ┌─────────┐
│  qorvex-   │──────────►│  ActionExecutor │─── TCP ────►│  Swift  │
│    repl    │           │  (qorvex-core)  │  (port 8080) │  Agent  │
└─────┬──────┘           └────────┬────────┘              └─────────┘
      │                           │
      │ log_action()              │ AutomationDriver trait
      ▼                           │
┌───────────┐                     │
│  Session   │◄────────────────────┘
│ (broadcast)│
└─────┬──────┘
      │ SessionEvent
      ▼
┌───────────────────────────────────┐
│  IPC Server (Unix socket)         │
│  qorvex_{session_name}.sock      │
└─────┬────────────┬───────────────┘
      │            │
      ▼            ▼
┌──────────┐  ┌──────────┐
│ qorvex-  │  │ qorvex-  │
│   live   │  │   cli    │
└──────────┘  └──────────┘
```

## Key Abstractions

### `AutomationDriver` trait

Backend abstraction with 18 async methods and 1 sync method. Defines the interface for all automation backends. Includes default implementations for search operations that dump the full tree and filter locally.

See [driver.md](driver.md) for the full method listing.

### `Session`

Async session state with broadcast channels for `SessionEvent`s. Maintains a ring buffer (1000 max entries), persistent JSONL log file in `~/.qorvex/logs/`, cached `current_elements`, and dual hashes (`screen_hash`, `screenshot_hash`) for change detection.

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
- `set_capture_screenshots(bool)` -- toggle post-action screenshots
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
├── qorvex_<session>.sock        # Unix socket per session
└── logs/
    └── <session>_<timestamp>.jsonl
```

- `config.json` stores `QorvexConfig` with the `agent_source_dir` field. `install.sh` records the agent project path so sessions can auto-build the agent.
- Unix socket path convention: `~/.qorvex/qorvex_{session_name}.sock`
- JSONL log files follow the naming pattern `{session_name}_{%Y%m%d_%H%M%S}.jsonl`

## IPC Protocol

The IPC layer uses Unix sockets with a JSON-over-newlines protocol.

**Request types:**

| Type | Description |
|------|-------------|
| `Execute` | Run an action command |
| `Subscribe` | Subscribe to session events |
| `GetState` | Get current session state |
| `GetLog` | Get action log history |

**Response types:**

| Type | Description |
|------|-------------|
| `ActionResult` | Result of an executed action |
| `State` | Current session state |
| `Log` | Action log entries |
| `Event` | Streamed session event |
| `Error` | Error message |

Server constructors:
- `IpcServer::new(session, name)` -- starts with empty driver slot; call `set_driver()` after the agent connects
- `shared_driver()` / `set_driver(driver)` -- wire the server to an already-connected driver so `Execute` requests reuse the existing TCP connection

## External Dependencies

- `xcrun simctl` -- Apple's simulator control CLI (comes with Xcode)
- `idevice` -- Rust crate for USB device tunneling via usbmuxd
- `xcodebuild` -- builds and launches the Swift agent
