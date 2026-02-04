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

Start an interactive TUI session:

```bash
qorvex-repl
```

The REPL provides a terminal UI with:
- Tab completion for commands, element IDs, and device UDIDs
- Output history with scrolling (arrow keys)
- Session and device status in the title bar

Controls:
- `Tab` — Trigger/navigate completion popup
- `Enter` — Execute command or accept completion
- `Esc` — Hide completion popup
- `q` — Quit (when input is empty)
- `↑/↓` — Navigate completion or scroll output

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
- `wait_for(id)` — Wait for element to appear (5s default timeout)
- `wait_for(id, ms)` — Wait for element with custom timeout
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

# Tap with wait (waits for element to appear first)
qorvex tap-element login-button --wait --timeout 10000

# Tap at coordinates
qorvex tap-location 100 200

# Send keyboard input
qorvex send-keys "hello world"

# Get screenshot (base64)
qorvex screenshot > screen.b64

# Get screen info (JSON)
qorvex screen-info | jq '.elements'

# Get element value
qorvex get-value text-field-id

# Get element value with wait
qorvex get-value text-field-id --wait --timeout 5000

# Wait for element to appear
qorvex wait-for loading-spinner --timeout 10000

# Log a comment to the session
qorvex comment "Starting login flow"

# Connect to a specific session
qorvex -s my-session tap-element button

# List all running sessions
qorvex list-sessions

# Get session status
qorvex status

# Get action log
qorvex log
```

Options:
- `-s, --session <name>` — Session to connect to (default: "default", or `$QORVEX_SESSION`)
- `-f, --format <text|json>` — Output format
- `-q, --quiet` — Suppress non-essential output

Command-specific options:
- `tap-element`, `get-value`: `-w, --wait` — Wait for element before acting; `-t, --timeout <ms>` — Wait timeout (default: 5000)
- `wait-for`: `-t, --timeout <ms>` — Wait timeout (default: 5000)

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

### Directory Structure

Qorvex stores runtime files in `~/.qorvex/`:

```
~/.qorvex/
├── qorvex_default.sock      # Unix socket for "default" session
├── qorvex_my-session.sock   # Unix socket for "my-session"
└── logs/
    ├── default.jsonl        # Action log for "default" session
    └── my-session.jsonl     # Action log for "my-session"
```

- **Sockets** (`~/.qorvex/qorvex_<session>.sock`) — IPC endpoints for REPL sessions. The CLI and Watcher connect to these to communicate with running sessions.
- **Logs** (`~/.qorvex/logs/<session>.jsonl`) — Persistent action logs in JSON Lines format. Each line is a timestamped action record, enabling replay, debugging, and audit trails.

Use `qorvex list-sessions` to discover running sessions by scanning for active socket files.

## License

MIT
