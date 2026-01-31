# qorvex

iOS Simulator automation and testing toolkit for macOS.

## Overview

Qorvex provides programmatic control over iOS Simulators through a Rust workspace containing three crates:

- **qorvex-core** — Core library with simulator control, UI automation, and IPC
- **qorvex-repl** — Interactive command-line interface for manual testing
- **qorvex-watcher** — TUI client for live screenshot and action log monitoring

## Requirements

- macOS with Xcode and iOS Simulators installed
- [axe](https://github.com/nicklockwood/axe) CLI tool for accessibility tree inspection
- Rust 1.70+

## Installation

```bash
cargo install --path crates/qorvex-repl
cargo install --path crates/qorvex-watcher
```

## Usage

### REPL

Start an interactive session:

```bash
qorvex-repl
```

Available commands:
- `start_session` — Begin a new automation session
- `end_session` — End the current session
- `tap_element <query>` — Tap a UI element by accessibility query
- `tap_location <x> <y>` — Tap at screen coordinates
- `send_keys <text>` — Type text into the focused field
- `get_screenshot` — Capture current screen
- `get_screen_info` — Get UI hierarchy information
- `log_comment <text>` — Add a comment to the action log
- `help` — Show available commands
- `quit` — Exit

### Watcher

Monitor a session in real-time with a terminal UI:

```bash
qorvex-watcher
```

Controls:
- `q` — Quit
- `r` — Refresh screenshot
- Arrow keys — Scroll action log

## Architecture

```
┌─────────────┐     IPC      ┌─────────────┐
│ qorvex-repl │◄────────────►│qorvex-watcher│
└──────┬──────┘              └─────────────┘
       │
       ▼
┌─────────────┐
│ qorvex-core │
├─────────────┤
│  • simctl   │ ──► iOS Simulator
│  • axe      │ ──► Accessibility tree
│  • session  │ ──► State management
│  • ipc      │ ──► Unix sockets
└─────────────┘
```

## License

MIT
