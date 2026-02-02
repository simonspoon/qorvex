# qorvex

iOS Simulator automation and testing toolkit for macOS.

## Overview

Qorvex provides programmatic control over iOS Simulators through a Rust workspace containing four crates:

- **qorvex-core** — Core library with simulator control, UI automation, and IPC
- **qorvex-repl** — Interactive command-line interface for manual testing
- **qorvex-watcher** — TUI client for live screenshot and action log monitoring
- **qorvex-cli** — Scriptable CLI client for automation pipelines

## Requirements

- macOS with Xcode and iOS Simulators installed
- [axe](https://github.com/cameroncooke/axe) CLI tool for accessibility tree inspection
  - Install via: `brew install cameroncooke/axe/axe`
- Rust 1.70+

## Installation

```bash
cargo install --path crates/qorvex-repl
cargo install --path crates/qorvex-watcher
cargo install --path crates/qorvex-cli
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

### CLI

Scriptable client for automation pipelines (requires a running REPL session):

```bash
# Tap an element by accessibility ID
qorvex tap-element login-button

# Tap at coordinates
qorvex tap-location 100 200

# Send keyboard input
qorvex send-keys "hello world"

# Get screenshot (base64)
qorvex screenshot > screen.b64

# Get screen info (JSON)
qorvex screen-info | jq '.elements'

# Connect to a specific session
qorvex -s my-session tap-element button

# Get session status
qorvex status

# Get action log
qorvex log
```

Options:
- `-s, --session <name>` — Session to connect to (default: "default", or `$QORVEX_SESSION`)
- `-f, --format <text|json>` — Output format
- `-q, --quiet` — Suppress non-essential output

## Architecture

```
┌─────────────┐     IPC      ┌──────────────┐
│ qorvex-repl │◄────────────►│qorvex-watcher│
└──────┬──────┘              └──────────────┘
       │                            ▲
       │ IPC                   IPC  │
       │◄───────────────────────────┤
       │                     ┌──────┴─────┐
       │                     │ qorvex-cli │
       │                     └────────────┘
       ▼
┌─────────────┐
│ qorvex-core │
├─────────────┤
│  • simctl   │ ──► iOS Simulator
│  • axe      │ ──► Accessibility tree
│  • session  │ ──► State management
│  • ipc      │ ──► Unix sockets
│  • executor │ ──► Action execution
└─────────────┘
```

## License

MIT
