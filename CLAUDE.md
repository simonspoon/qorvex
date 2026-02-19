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

# CLI commands that don't require a session
cargo run -p qorvex-cli -- list-devices
cargo run -p qorvex-cli -- boot-device <udid>
cargo run -p qorvex-cli -- convert log.jsonl

# Install binaries locally
cargo install --path crates/qorvex-repl
cargo install --path crates/qorvex-live
cargo install --path crates/qorvex-cli

# Run tests
cargo test
cargo test -p qorvex-core
cargo test -p qorvex-cli

# Run integration tests
cargo test -p qorvex-core --test ipc_integration
cargo test -p qorvex-core --test driver_integration

# Build Swift agent (requires Xcode)
make -C qorvex-agent build

# Install all Rust binaries
./install.sh
```

## Architecture

Rust workspace with four crates plus a Swift agent for iOS Simulator and device automation on macOS:

```
qorvex-core    - Core library (driver abstraction, protocol, session, ipc, action, executor, watcher)
qorvex-repl    - TUI REPL with tab completion, uses core directly
qorvex-live    - TUI client with screenshot rendering (ratatui-image) and IPC reconnection
qorvex-cli     - Scriptable CLI client for automation pipelines, JSONL log converter
qorvex-agent   - Swift XCTest agent for native iOS accessibility (not a Cargo crate)
```

Qorvex uses a native Swift XCTest-based agent communicating over a TCP binary protocol (port 8080). Supports simulators (direct TCP) and physical devices (via USB tunnel).

For detailed architecture, module breakdowns, protocol reference, contributor guides, and command references, read `docs/INDEX.md` and the relevant topic file before working on unfamiliar subsystems.
