# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
# Build all crates
cargo build

# Build release
cargo build --release

# Run server
cargo run -p qorvex-server -- -s default

# Run REPL (auto-launches server if needed)
cargo run -p qorvex-repl

# Run live TUI
cargo run -p qorvex-live

# Run CLI (start server + session, then stop)
cargo run -p qorvex-cli -- start
cargo run -p qorvex-cli -- tap button-id
cargo run -p qorvex-cli -- stop

# CLI commands that don't require a session
cargo run -p qorvex-cli -- list-devices
cargo run -p qorvex-cli -- boot-device <udid>
cargo run -p qorvex-cli -- convert log.jsonl

# Install binaries locally
cargo install --path crates/qorvex-server
cargo install --path crates/qorvex-repl
cargo install --path crates/qorvex-live
cargo install --path crates/qorvex-cli

# Run tests
cargo test
cargo test -p qorvex-core
cargo test -p qorvex-cli
cargo test -p qorvex-repl

# Run integration tests
cargo test -p qorvex-core --test ipc_integration
cargo test -p qorvex-core --test driver_integration
cargo test -p qorvex-core --test e2e_pipeline
cargo test -p qorvex-core --test error_recovery
cargo test -p qorvex-cli  --test cli_integration

# Run simulator integration tests (requires booted simulator + installed testapp)
# Boot a simulator first: xcrun simctl boot <UDID>
# Install testapp first: make -C qorvex-testapp run
cargo test -p qorvex-cli --test simulator_suite -- --ignored --test-threads=1

# Build Swift agent (requires Xcode)
make -C qorvex-agent build

# Build test app (requires Xcode + xcodegen)
make -C qorvex-testapp build

# Install test app on booted Simulator
make -C qorvex-testapp install

# Install test app and launch
make -C qorvex-testapp run

# Build streamer (macOS only, requires macOS 13+)
make -C qorvex-streamer build

# Run streamer standalone
qorvex-streamer --udid <UDID> --fps 30 --socket-path /tmp/qvx-stream.sock

# Run live TUI with streamer (default)
cargo run -p qorvex-live -- --fps 30

# Run live TUI without streamer (polling fallback)
cargo run -p qorvex-live -- --no-streamer

# Run REPL in batch mode (non-interactive, stdin → stdout, no terminal)
echo -e "help\nquit" | cargo run -p qorvex-repl -- --batch -s <session>

# Run live TUI in batch mode (print session events as JSONL, exit after N secs)
cargo run -p qorvex-live -- --batch -s <session> --duration 5

# Install all Rust binaries
./install.sh
```

## Required Reading

Before starting any task, read `docs/INDEX.md` and the relevant topic file for the subsystem you are working on.

## Architecture

Rust workspace with five crates plus a Swift agent for iOS Simulator and device automation on macOS:

```
qorvex-core    - Core library (driver abstraction, protocol, session, ipc, action, executor, watcher)
qorvex-server  - Standalone automation server daemon, manages sessions and agent lifecycle
qorvex-repl    - TUI REPL client with tab completion, connects to server via IPC
qorvex-live    - TUI client with screenshot rendering (ratatui-image) and IPC reconnection
qorvex-cli     - Scriptable CLI client for automation pipelines, JSONL log converter
qorvex-agent   - Swift XCTest agent for native iOS accessibility (not a Cargo crate)
qorvex-streamer - ScreenCaptureKit-based live video streamer for Simulator windows (Swift, macOS only)
qorvex-testapp - SwiftUI iOS test app for verifying qorvex automation (not a Cargo crate)
```

Qorvex uses a native Swift XCTest-based agent communicating over a TCP binary protocol (port 8080). Supports simulators (direct TCP) and physical devices (via USB tunnel).

For detailed architecture, module breakdowns, protocol reference, contributor guides, and command references, read `docs/INDEX.md` and the relevant topic file before working on unfamiliar subsystems.
