# qorvex

iOS Simulator automation and testing toolkit for macOS.

## Overview

Qorvex provides programmatic control over iOS Simulators through a Rust workspace containing three crates:

- **qorvex-core** — Core library with simulator control, UI automation, and IPC
- **qorvex-repl** — Interactive command-line interface for manual testing
- **qorvex-watcher** — TUI client for live screenshot and action log monitoring

## Requirements

- macOS with Xcode and iOS Simulators installed
- [axe](https://github.com/cameroncooke/axe) CLI tool for accessibility tree inspection
  - Install via: `brew install cameroncooke/axe/axe`
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
- `list_devices` — List all available simulators
- `use_device(udid)` — Select a simulator to use
- `boot_device(udid)` — Boot and select a simulator
- `start_session` — Begin a new automation session
- `end_session` — End the current session
- `get_session_info` — Get session status info
- `tap_element(id)` — Tap a UI element by accessibility ID
- `tap_location(x, y)` — Tap at screen coordinates
- `send_keys(text)` — Type text into the focused field
- `get_screenshot` — Capture current screen
- `get_screen_info` — Get UI hierarchy information
- `list_elements` — List actionable UI elements
- `get_element_value(id)` — Get element's current value
- `log_comment(text)` — Add a comment to the action log
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
