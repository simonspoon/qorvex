# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
# Build all crates
cargo build

# Build release
cargo build --release

# Run REPL
cargo run -p qorvex-repl

# Run live TUI
cargo run -p qorvex-live

# Run CLI (requires running REPL session)
cargo run -p qorvex-cli -- tap button-id

# Run automation script
cargo run -p qorvex-auto -- run script.qvx

# Convert action log to script
cargo run -p qorvex-auto -- convert log.jsonl --stdout

# Install binaries locally
cargo install --path crates/qorvex-repl
cargo install --path crates/qorvex-live
cargo install --path crates/qorvex-cli
cargo install --path crates/qorvex-auto

# Run tests
cargo test
cargo test -p qorvex-core
cargo test -p qorvex-auto

# Run integration tests
cargo test -p qorvex-core --test ipc_integration
cargo test -p qorvex-core --test driver_integration

# Build Swift agent (requires Xcode)
make -C qorvex-agent build

# Install all Rust binaries
./install.sh
```

## Architecture

Rust workspace with five crates plus a Swift agent for iOS Simulator and device automation on macOS:

```
qorvex-core    - Core library (driver abstraction, protocol, session, ipc, action, executor, watcher)
qorvex-repl    - TUI REPL with tab completion, uses core directly
qorvex-live    - TUI client with screenshot rendering (ratatui-image) and IPC reconnection
qorvex-cli     - Scriptable CLI client for automation pipelines
qorvex-auto    - Script runner (.qvx files) and JSONL log-to-script converter
qorvex-agent   - Swift XCTest agent for native iOS accessibility (not a Cargo crate)
```

**Automation backend:**

Qorvex uses a native Swift XCTest-based agent communicating over a TCP binary protocol (port 8080). Supports simulators (direct TCP) and physical devices (via USB tunnel).

**External dependencies:**
- `xcrun simctl` - Apple's simulator control CLI (comes with Xcode)
- `idevice` - Rust crate for USB device tunneling via usbmuxd

### qorvex-core modules

#### Driver abstraction layer
- **driver.rs** - `AutomationDriver` trait (18 async + 1 sync method) and `DriverConfig` enum (`Agent`, `Device`). Includes glob matching (`*`/`?`) for element selectors, default-impl search helpers (`find_element`, `find_element_by_label`, `find_element_with_type`), `set_target()` for switching target app, and pub `flatten_elements()` utility
- **element.rs** - Shared `UIElement` (with `identifier`, `label`, `value`, `element_type`, `role`, `frame`, `children`, `hittable`) and `ElementFrame` types used by all backends
- **protocol.rs** - Binary wire protocol codec (little-endian, 4-byte length header) for Rust ↔ Swift agent communication. Defines `OpCode` (including `SetTarget` for app switching and `FindElement` for single-element lookup with live hittability), `Request`, and `Response` enums

#### Backends
- **agent_client.rs** - Low-level async TCP client (`AgentClient`) for Swift agent communication with timeouts and reconnection
- **agent_driver.rs** - `AgentDriver`: implements `AutomationDriver` using Swift agent TCP connection. Supports `Direct` (simulator) and `UsbDevice` (physical) connection targets. Includes `set_target()` to switch target app bundle ID. Overrides `find_element`, `find_element_by_label`, and `find_element_with_type` to use the `FindElement` protocol command for accurate live hittability instead of the default dump-tree-and-search
- **agent_lifecycle.rs** - `AgentLifecycle` struct managing full agent process lifecycle: build (`xcodebuild build-for-testing`), spawn (`test-without-building`), terminate, health-check via TCP heartbeat, and retry logic. Auto-cleanup via `Drop`. Configured via `AgentLifecycleConfig` (port, timeout, retries). Two orchestration methods: `ensure_running()` (always rebuilds) and `ensure_agent_ready()` (skips rebuild if agent is already reachable)
- **usb_tunnel.rs** - Physical device discovery and port forwarding via usbmuxd (`idevice` crate). Provides `list_devices()` and `connect(udid, port)`

#### Infrastructure
- **config.rs** - Persistent configuration stored in `~/.qorvex/config.json`. `QorvexConfig` with `agent_source_dir` field. `install.sh` records the agent project path so sessions can auto-build the agent
- **simctl.rs** - Wrapper around `xcrun simctl` for device listing, screenshots, and boot
- **session.rs** - Async session state with broadcast channels for `SessionEvent`s (`ActionLogged`, `ScreenshotUpdated`, `ScreenInfoUpdated`, `Started`, `Ended`). Ring buffer (1000 max entries), persistent JSONL log file in `~/.qorvex/logs/`, cached `current_elements` and dual hashes (`screen_hash`, `screenshot_hash`) for change detection. Constructors: `new(name)` and `new_with_log_dir(name, dir)`. `log_action()` for standard logging, `log_action_timed()` for actions with per-phase timing (`wait_ms`, `tap_ms`)
- **ipc.rs** - Unix socket server/client for REPL↔Watcher/CLI communication (JSON-over-newlines protocol). Server constructors: `IpcServer::new(session, name)` (defaults to localhost:8080) and `IpcServer::with_config(session, name, config)`. Also exports `qorvex_dir()` for `~/.qorvex/` path resolution
  - Socket path convention: `~/.qorvex/qorvex_{session_name}.sock`
  - Request types: `Execute`, `Subscribe`, `GetState`, `GetLog`
  - Response types: `ActionResult`, `State`, `Log`, `Event`, `Error`
- **action.rs** - `ActionType` enum (`Tap`, `TapLocation`, `Swipe`, `LongPress`, `SendKeys`, `GetScreenshot`, `GetScreenInfo`, `GetValue`, `WaitFor`, `LogComment`, `StartSession`, `EndSession`, `Quit`) with selector/by_label/element_type pattern for element lookup. `ActionLog` includes optional `duration_ms` for timed actions, plus optional `wait_ms` and `tap_ms` for per-phase tap timing (element lookup vs agent execution)
- **executor.rs** - Backend-agnostic action execution engine. Constructors: `new(driver)`, `with_agent(host, port)`, `from_config(config)`. `set_capture_screenshots(bool)` to toggle post-action screenshots. `driver()` accessor. WaitFor polls every 100ms, requires element to be hittable, and requires 3 consecutive stable frames before success
- **watcher.rs** - Screen change detection via accessibility tree polling and perceptual image hashing (dHash) with configurable `visual_change_threshold` (hamming distance 0-64). Returns `WatcherHandle` with `stop()`, `cancel()`, `is_running()` methods. Includes exponential backoff on errors

### qorvex-repl modules

- **main.rs** - Entry point, event loop, command dispatch, and mouse event handling (drag-to-select, scroll, Ctrl+C copy)
- **app.rs** - Application state (input, completion, output history, session references, text selection/clipboard via `SelectionState`/`TextPosition`, `AgentLifecycle` management). Full command set: `list_devices`, `use_device`, `boot_device`, `start_agent`, `stop_agent`, `set_target`, `start_session`, `end_session`, `start_watcher`, `stop_watcher`, `get_session_info`, `help`, `quit`, plus all action commands. `start_session` and `start_agent` auto-start the agent via `QorvexConfig` when `agent_source_dir` is set
- **completion/** - Tab completion engine with command definitions, fuzzy matching, and context-aware suggestions
- **format.rs** - Output formatting for commands, results, and elements
- **ui/** - TUI rendering with ratatui (theme, completion popup, layout, selection overlay highlighting, scrollbar)

### qorvex-auto modules

- **main.rs** - CLI entry point with `run` and `convert` subcommands (clap). `run` auto-starts the agent via `QorvexConfig` if configured
- **ast.rs** - AST types: Script, Statement (Command, Assignment, Foreach, For, If, Set, Include), Expression (including `CommandCapture` for `var = get_value(...)` syntax), BinOp (Add, Eq, NotEq)
- **parser.rs** - Two-phase parser: tokenizer + recursive descent producing AST
- **runtime.rs** - Variable environment with Value types (String, Number, List)
- **executor.rs** - `ScriptExecutor` walks AST, dispatches commands to `ActionExecutor`, manages session/watcher lifecycle. Tracks `base_dir` for include resolution, `include_stack` to prevent circular includes, `default_timeout_ms` (via `set` command). Handles device commands (`list_devices`, `boot_device`, `set_target`) and watcher commands in addition to all action types
- **converter.rs** - Converts JSONL action logs to `.qvx` scripts (record-and-replay)
- **error.rs** - `AutoError` enum with distinct exit codes: `ActionFailed`=1, `Parse`=2, `Runtime`=3, `Io`=4

### qorvex-agent (Swift)

Not a Cargo crate — a Swift XCTest UI Testing project generated via xcodegen from `project.yml`.

- **AgentServer.swift** - NWListener TCP server (Network framework), accepts connections and dispatches to CommandHandler on main thread
- **Protocol.swift** - Binary protocol codec matching the Rust side (13 request opcodes including SetTarget and FindElement, 6 response types)
- **CommandHandler.swift** - Executes XCUIElement accessibility actions (tap, tapByLabel, tapWithType, tapCoord, swipe, longPress, type, getValue, findElement, dumpTree, screenshot, setTarget) with ObjC exception catching via `QVXTryCatch()`. `findElement` queries the live `XCUIElement` for `isHittable` (unavailable from snapshots) and returns a single-element JSON response
- **UITreeSerializer.swift** - Serializes XCUIApplication accessibility hierarchy to JSON matching `UIElement`
- **ObjCExceptionCatcher.{h,m}** - Objective-C `@try/@catch` bridge preventing XCUIElement NSExceptions from crashing the agent
- **BridgingHeader.h** - Swift-ObjC bridging header for exception catcher
- **App/QorvexAgentApp.swift** - Minimal SwiftUI app stub required by xcodegen project structure
- **QorvexAgentTests.swift** - XCTest entry point that starts the server and blocks indefinitely
- **project.yml** - XcodeGen project definition (app target + UI test bundle, iOS 16.0+)
- **Makefile** - Build/test commands using `xcodebuild` with xcodegen project generation and auto-detection of booted simulator

### Data flow

1. REPL receives commands via stdin, executes actions via `ActionExecutor` (which delegates to `AutomationDriver`), logs to `Session`
2. Session broadcasts `SessionEvent`s to subscribers
3. Live TUI connects via `IpcClient`, subscribes to events, renders TUI with ratatui
4. CLI connects via `IpcClient`, sends `Execute` requests for scripted automation
5. Auto runner parses `.qvx` scripts, creates its own session with IPC server, and executes commands directly via `ActionExecutor`
6. Screenshots are base64-encoded PNGs passed through the event system
7. Swift agent lifecycle: install via `simctl` → launch → TCP connect → binary protocol commands → terminate
