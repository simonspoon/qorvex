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
```

## Architecture

Rust workspace with five crates for iOS Simulator automation on macOS:

```
qorvex-core    - Core library (simctl, axe, session, ipc, action, executor, watcher)
qorvex-repl    - TUI REPL with tab completion, uses core directly
qorvex-live    - TUI client that connects via IPC to monitor sessions
qorvex-cli     - Scriptable CLI client for automation pipelines
qorvex-auto    - Script runner (.qvx files) and JSONL log-to-script converter
```

**External dependencies:**
- `xcrun simctl` - Apple's simulator control CLI (comes with Xcode)
- `axe` - Third-party accessibility tree tool (`brew install cameroncooke/axe/axe`)

### qorvex-core modules

- **simctl.rs** - Wrapper around `xcrun simctl` for device listing, screenshots, boot, and keyboard input
- **axe.rs** - Wrapper around `axe` CLI for UI hierarchy dumps, element finding, and tap actions
- **session.rs** - Async session state with broadcast channels for events (uses `tokio::sync`)
- **ipc.rs** - Unix socket server/client for REPL↔Watcher/CLI communication (JSON-over-newlines protocol)
  - Socket path convention: `~/.qorvex/qorvex_{session_name}.sock`
  - Request types: `Execute`, `Subscribe`, `GetState`, `GetLog`
  - Response types: `ActionResult`, `State`, `Log`, `Event`, `Error`
- **action.rs** - Unified action types (`Tap`, `TapLocation`, `SendKeys`, `GetScreenshot`, `GetScreenInfo`, `GetValue`, `WaitFor`, `LogComment`, session management) with selector/by_label/element_type pattern for element lookup
- **executor.rs** - Action execution engine that wraps simctl/axe operations with result handling
- **watcher.rs** - Screen change detection via accessibility tree polling and perceptual image hashing (dHash)

### qorvex-repl modules

- **main.rs** - Entry point, event loop, and command dispatch
- **app.rs** - Application state (input, completion, output history, session references)
- **completion/** - Tab completion engine with command definitions and context-aware suggestions
- **format.rs** - Output formatting for commands, results, and elements
- **ui/** - TUI rendering with ratatui (theme, completion popup, layout)

### qorvex-auto modules

- **main.rs** - CLI entry point with `run` and `convert` subcommands (clap)
- **ast.rs** - AST types: Script, Statement (Command, Assignment, Foreach, For, If), Expression, BinOp
- **parser.rs** - Two-phase parser: tokenizer + recursive descent producing AST
- **runtime.rs** - Variable environment with Value types (String, Number, List)
- **executor.rs** - Script execution engine: walks AST, dispatches commands to `ActionExecutor`, manages session/watcher lifecycle
- **converter.rs** - Converts JSONL action logs to `.qvx` scripts (record-and-replay)
- **error.rs** - Error types with distinct exit codes (parse=2, runtime=3, action=1, io=4, session=5)

### Data flow

1. REPL receives commands via stdin, executes actions via `ActionExecutor`, logs to `Session`
2. Session broadcasts `SessionEvent`s to subscribers
3. Live TUI connects via `IpcClient`, subscribes to events, renders TUI with ratatui
4. CLI connects via `IpcClient`, sends `Execute` requests for scripted automation
5. Auto runner parses `.qvx` scripts, creates its own session with IPC server, and executes commands directly via `ActionExecutor`
6. Screenshots are base64-encoded PNGs passed through the event system
