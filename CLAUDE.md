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

# Run watcher TUI
cargo run -p qorvex-watcher

# Run CLI (requires running REPL session)
cargo run -p qorvex-cli -- tap-element button-id

# Install binaries locally
cargo install --path crates/qorvex-repl
cargo install --path crates/qorvex-watcher
cargo install --path crates/qorvex-cli

# Run tests
cargo test
cargo test -p qorvex-core

# Run integration tests
cargo test -p qorvex-core --test ipc_integration
```

## Architecture

Rust workspace with four crates for iOS Simulator automation on macOS:

```
qorvex-core    - Core library (simctl, axe, session, ipc, action, executor)
qorvex-repl    - Interactive CLI that uses core directly
qorvex-watcher - TUI client that connects via IPC to monitor sessions
qorvex-cli     - Scriptable CLI client for automation pipelines
```

**External dependencies:**
- `xcrun simctl` - Apple's simulator control CLI (comes with Xcode)
- `axe` - Third-party accessibility tree tool (`brew install cameroncooke/axe/axe`)

### qorvex-core modules

- **simctl.rs** - Wrapper around `xcrun simctl` for device listing, screenshots, boot, and keyboard input
- **axe.rs** - Wrapper around `axe` CLI for UI hierarchy dumps, element finding, and tap actions
- **session.rs** - Async session state with broadcast channels for events (uses `tokio::sync`)
- **ipc.rs** - Unix socket server/client for REPL↔Watcher/CLI communication (JSON-over-newlines protocol)
  - Socket path convention: `/tmp/qorvex_<session_name>.sock`
  - Request types: `Execute`, `Subscribe`, `Unsubscribe`, `GetState`, `GetLog`, `Ping`
  - Response types: `ActionResult`, `Subscribed`, `Unsubscribed`, `State`, `Log`, `Pong`, `Event`, `Error`
- **action.rs** - Action types (`TapElement`, `SendKeys`, `GetScreenshot`, etc.) and logging
- **executor.rs** - Action execution engine that wraps simctl/axe operations with result handling

### Data flow

1. REPL receives commands via stdin, executes actions via `ActionExecutor`, logs to `Session`
2. Session broadcasts `SessionEvent`s to subscribers
3. Watcher connects via `IpcClient`, subscribes to events, renders TUI with ratatui
4. CLI connects via `IpcClient`, sends `Execute` requests for scripted automation
5. Screenshots are base64-encoded PNGs passed through the event system
