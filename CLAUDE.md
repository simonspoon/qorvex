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

# Install binaries locally
cargo install --path crates/qorvex-repl
cargo install --path crates/qorvex-watcher

# Run tests (when added)
cargo test
cargo test -p qorvex-core
```

## Architecture

Rust workspace with three crates for iOS Simulator automation on macOS:

```
qorvex-core    - Core library (simctl, axe, session, ipc, action)
qorvex-repl    - Interactive CLI that uses core directly
qorvex-watcher - TUI client that connects via IPC to monitor sessions
```

**External dependencies:**
- `xcrun simctl` - Apple's simulator control CLI (comes with Xcode)
- `axe` - Third-party accessibility tree tool (`brew install cameroncooke/axe/axe`)

### qorvex-core modules

- **simctl.rs** - Wrapper around `xcrun simctl` for device listing, screenshots, boot, and keyboard input
- **axe.rs** - Wrapper around `axe` CLI for UI hierarchy dumps, element finding, and tap actions
- **session.rs** - Async session state with broadcast channels for events (uses `tokio::sync`)
- **ipc.rs** - Unix socket server/client for REPL↔Watcher communication (JSON-over-newlines protocol)
- **action.rs** - Action types (`TapElement`, `SendKeys`, `GetScreenshot`, etc.) and logging

### Data flow

1. REPL receives commands via stdin, executes actions via `Simctl`/`Axe`, logs to `Session`
2. Session broadcasts `SessionEvent`s to subscribers
3. Watcher connects via `IpcClient`, subscribes to events, renders TUI with ratatui
4. Screenshots are base64-encoded PNGs passed through the event system
