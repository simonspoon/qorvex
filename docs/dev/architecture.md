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
| `qorvex-core` | Core library -- driver abstraction, protocol, session, IPC, action types, executor |
| `qorvex-server` | Standalone automation server daemon -- manages sessions, agent lifecycle, and IPC |
| `qorvex-repl` | TUI REPL client with tab completion, connects to server via IPC; auto-launches server if needed; deferred startup keeps TUI responsive |
| `qorvex-live` | TUI client with live video feed (via qorvex-streamer) and IPC reconnection |
| `qorvex-cli` | Scriptable CLI client for automation pipelines, includes JSONL log-to-script converter |
| `qorvex-agent` | Swift XCTest agent for native iOS accessibility (not a Cargo crate) |
| `qorvex-streamer` | Swift standalone binary; captures Simulator window via ScreenCaptureKit and streams JPEG frames over a Unix socket (macOS 13+, not a Cargo crate) |
| `qorvex-testapp` | SwiftUI iOS test app (bundle ID: `com.qorvex.testapp`) with 5 tabs covering all automation actions; built with XcodeGen, not a Cargo crate |

## Data Flow

1. **Server** (`qorvex-server`) starts, binds an `IpcServer` on the session socket, manages agent lifecycle.
2. **REPL** renders its TUI immediately on launch (no blocking I/O). After the first frame is drawn, it spawns a background task (`startup()`) that launches the server if the socket is absent, connects as an IPC client, sends `StartSession`, and fetches initial completion data. A braille spinner in the input area animates while this is in progress. Subsequent commands (`execute_command`) also run non-blocking: the IPC send is dispatched to a tokio task, the spinner reappears, and results are polled via `check_command_result()` each event loop tick.
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

Async session state with broadcast channels for `SessionEvent`s. Maintains a ring buffer (1000 max entries) and a persistent JSONL log file in `~/.qorvex/logs/` (or `$QORVEX_LOG_DIR` if set). UI elements are fetched on demand via `FetchElements` IPC rather than cached in the session.

Constructors:
- `Session::new(simulator_udid, session_name)` -- logs to `~/.qorvex/logs/` (or `$QORVEX_LOG_DIR`)
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
- `ensure_running()` -- build (skipped if `.xctestrun` products already exist), spawn, wait with retries
- `ensure_agent_ready()` -- skips rebuild and respawn if agent is already reachable

## Connection Modes

| Target | Connection | Implementation |
|--------|-----------|---------------|
| Simulators | Direct TCP (localhost:8080) | `AgentDriver::direct(host, port)` |
| Physical devices (any) | Direct TCP via Bonjour mDNS (`<Name>.local`) | `AgentDriver::direct(hostname, port)` |

All physical devices — whether WiFi or USB-connected — use `AgentDriver::direct` via `<Name>.local` Bonjour mDNS. This hostname resolves for both connection types and avoids the need for pairing files (required by the native CoreDevice tunnel) that modern macOS no longer stores in accessible locations.

`ServerState.handle_use_device()` auto-selects the connection mode:
- **Any CoreDevice device** (found via `xcrun devicectl`): sets `direct_host = Some("<Name>.local")`, `use_core_device = false`
- **USB device found via usbmuxd**: also queries CoreDevice to populate `direct_host = Some("<Name>.local")`; falls back to `direct_host = None` only if CoreDevice lookup fails (in which case `AgentDriver::usb_device` is used)

`AgentLifecycleConfig.startup_timeout` is set to **120 seconds** for physical devices (vs 30s default for simulators). `xcodebuild test-without-building` installs and launches the test runner on the device, which takes 30–60s on first deploy.

The `usb_tunnel` module provides:
- `list_devices()` -- enumerate USB-connected devices via usbmuxd
- `connect(udid, port)` -- port forwarding through usbmuxd
- `list_tunneld_devices()` -- enumerate devices via pymobiledevice3 tunneld
- `connect_tunneld(tunnel_address, port)` -- TCP through a tunneld address

The `coredevice` module provides:
- `list_devices()` -- enumerate paired physical devices via `xcrun devicectl list devices`; always populates `hostname = Some("<Name>.local")` for every device

The `core_device_tunnel` module provides:
- `connect_coredevice(udid, port)` -- userspace TCP tunnel via `CoreDeviceProxy` (iOS 17+, no root required)

> **CoreDevice tunnel caveat:** This path is no longer used by `handle_use_device`. It remains in the codebase for manual use but requires a pairing file at `~/Library/Lockdown/PairRecords/{UDID}.plist` or `/var/db/lockdown/{UDID}.plist`. On modern macOS (Ventura+), these files are not written to those paths, so this path reliably fails with `PairingFileNotFound`. Use `AgentDriver::direct("<Name>.local", port)` instead.

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
| `FetchElements` | On-demand live element fetch for tab completion |
| `GetSessionInfo` / `GetCompletionData` | Info and tab completion (devices only) |

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

## REPL Non-Blocking Pattern (`qorvex-repl`)

All IPC operations in the REPL are non-blocking — the crossterm event loop never awaits them directly.

**Startup:** `App::new()` is synchronous and returns immediately. `startup()` spawns a tokio task that does the server launch, IPC connect, `StartSession`, and `GetCompletionData`. Results arrive via `startup_rx: mpsc::Receiver<StartupResult>`, polled by `check_startup_result()` each loop tick.

**Commands:** `execute_command()` is synchronous. It builds the `IpcRequest`, takes the `IpcClient` out of `self.client` via `.take()`, and spawns a tokio task. The task sends the request, then returns `(CommandResult, IpcClient)` through `cmd_result_rx`. `check_command_result()` restores the client and calls `display_response()` when the channel has data.

**Spinner:** `is_processing: bool` is set true before spawning and cleared on result. `spinner_frame()` computes the braille animation frame from `processing_start.elapsed()`. The event loop polls at 50ms while processing (vs 100ms idle) for smooth animation. New commands are blocked while `is_processing` is true.

**Batch mode** cannot use this pattern (no event loop). Use `App::new_blocking()` which calls `ensure_server_running`, connects, and runs `StartSession` synchronously before returning.

## Live TUI Image Pipeline (`qorvex-live`)

The live image pipeline runs in `spawn_decode_task` (blocking thread) and feeds into `AppEvent::ImageReady`.

**Implementation notes:**

- `Picker::font_size()` is a method — not a public field. Accessing it as `.font_size` fails to compile.
- `MAX_DECODE_WIDTH` / `MAX_DECODE_HEIGHT` in `main.rs` cap the image before it reaches `ratatui-image`. If these are too small (e.g., 600px), the image cannot render larger than that cap regardless of terminal size. Set them large enough for the largest expected terminal (1200×1800 covers typical fullscreen use).
- `Event::Resize` must be explicitly matched in the event poll loop — it is not automatically handled by ratatui or crossterm. If omitted, the layout will not reflow when the terminal is resized until the next key press.
- The left panel width is computed from `image_pixel_size` and `picker.font_size()` each frame. Formula: `inner_cols = img_w * inner_rows * cell_h / (img_h * cell_w)`. This makes the border hug the image's natural aspect ratio at the current terminal height.

## External Dependencies

- `xcrun simctl` -- Apple's simulator control CLI (comes with Xcode)
- `xcrun devicectl` -- Apple's physical device control CLI (Xcode 15+, used by `coredevice::list_devices`)
- `idevice` -- Rust crate for USB device tunneling via usbmuxd and CoreDevice proxy
- `xcodebuild` -- builds and launches the Swift agent
