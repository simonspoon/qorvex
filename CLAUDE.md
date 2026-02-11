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
qorvex-live    - TUI client that connects via IPC to monitor sessions
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
- **driver.rs** - `AutomationDriver` trait (17 async + 1 sync method) and `DriverConfig` enum (`Agent`, `Device`). Includes glob matching (`*`/`?`) for element selectors and element search helpers
- **element.rs** - Shared `UIElement` and `ElementFrame` types used by all backends
- **protocol.rs** - Binary wire protocol codec (little-endian, 4-byte length header) for Rust ↔ Swift agent communication. Defines `OpCode`, `Request`, and `Response` enums

#### Backends
- **agent_client.rs** - Low-level async TCP client (`AgentClient`) for Swift agent communication with timeouts and reconnection
- **agent_driver.rs** - `AgentDriver`: implements `AutomationDriver` using Swift agent TCP connection. Supports `Direct` (simulator) and `UsbDevice` (physical) connection targets
- **agent_lifecycle.rs** - Build, install, launch, health-check, and stop the Swift agent on simulators via `xcrun simctl`
- **usb_tunnel.rs** - Physical device discovery and port forwarding via usbmuxd (`idevice` crate). Provides `list_devices()` and `connect(udid, port)`

#### Infrastructure
- **simctl.rs** - Wrapper around `xcrun simctl` for device listing, screenshots, and boot
- **session.rs** - Async session state with broadcast channels for events (uses `tokio::sync`)
- **ipc.rs** - Unix socket server/client for REPL↔Watcher/CLI communication (JSON-over-newlines protocol)
  - Socket path convention: `~/.qorvex/qorvex_{session_name}.sock`
  - Request types: `Execute`, `Subscribe`, `GetState`, `GetLog`
  - Response types: `ActionResult`, `State`, `Log`, `Event`, `Error`
- **action.rs** - Unified action types (`Tap`, `TapLocation`, `Swipe`, `LongPress`, `SendKeys`, `GetScreenshot`, `GetScreenInfo`, `GetValue`, `WaitFor`, `LogComment`, session management) with selector/by_label/element_type pattern for element lookup. `ActionLog` includes optional `duration_ms` for timed actions
- **executor.rs** - Backend-agnostic action execution engine. Takes `Arc<dyn AutomationDriver>` with convenience constructors: `with_agent(host, port)`, `from_config(config)`
- **watcher.rs** - Screen change detection via accessibility tree polling and perceptual image hashing (dHash)

### qorvex-repl modules

- **main.rs** - Entry point, event loop, and command dispatch
- **app.rs** - Application state (input, completion, output history, session references)
- **completion/** - Tab completion engine with command definitions, fuzzy matching, and context-aware suggestions
- **format.rs** - Output formatting for commands, results, and elements
- **ui/** - TUI rendering with ratatui (theme, completion popup, layout)

### qorvex-auto modules

- **main.rs** - CLI entry point with `run` and `convert` subcommands (clap)
- **ast.rs** - AST types: Script, Statement (Command, Assignment, Foreach, For, If, Set, Include), Expression, BinOp
- **parser.rs** - Two-phase parser: tokenizer + recursive descent producing AST
- **runtime.rs** - Variable environment with Value types (String, Number, List)
- **executor.rs** - Script execution engine: walks AST, dispatches commands to `ActionExecutor`, manages session/watcher lifecycle
- **converter.rs** - Converts JSONL action logs to `.qvx` scripts (record-and-replay)
- **error.rs** - Error types with distinct exit codes (parse=2, runtime=3, action=1, io=4, session=5)

### qorvex-agent (Swift)

Not a Cargo crate — a Swift XCTest UI Testing project built with `xcodebuild`.

- **AgentServer.swift** - NWListener TCP server (Network framework), accepts connections and dispatches to CommandHandler
- **Protocol.swift** - Binary protocol codec matching the Rust side (OpCode enums, request/response encoding)
- **CommandHandler.swift** - Executes XCUIElement accessibility actions (tap, swipe, type, getValue, dumpTree, screenshot)
- **UITreeSerializer.swift** - Serializes XCUIApplication accessibility hierarchy to JSON matching `UIElement`
- **QorvexAgentTests.swift** - XCTest wrapper that starts the server and blocks
- **Makefile** - Build/test commands using `xcodebuild`

### Data flow

1. REPL receives commands via stdin, executes actions via `ActionExecutor` (which delegates to `AutomationDriver`), logs to `Session`
2. Session broadcasts `SessionEvent`s to subscribers
3. Live TUI connects via `IpcClient`, subscribes to events, renders TUI with ratatui
4. CLI connects via `IpcClient`, sends `Execute` requests for scripted automation
5. Auto runner parses `.qvx` scripts, creates its own session with IPC server, and executes commands directly via `ActionExecutor`
6. Screenshots are base64-encoded PNGs passed through the event system
7. Swift agent lifecycle: install via `simctl` → launch → TCP connect → binary protocol commands → terminate
