# qorvex

iOS Simulator and device automation toolkit for macOS.

## Overview

Qorvex provides programmatic control over iOS Simulators and physical devices through a Rust workspace with five crates and a Swift agent:

- **qorvex-core** — Core library with driver abstraction, protocol, session, IPC, and execution engine
- **qorvex-repl** — Interactive command-line interface for manual testing
- **qorvex-live** — TUI client with inline screenshot rendering and action log monitoring
- **qorvex-cli** — Scriptable CLI client for automation pipelines
- **qorvex-auto** — Script runner for `.qvx` automation scripts and JSONL log converter
- **qorvex-agent** — Swift XCTest agent for native iOS accessibility automation (not a Cargo crate)

### Automation backend

Qorvex uses a native Swift XCTest agent behind the `AutomationDriver` trait:

| Target | Connection | Setup |
|--------|------------|-------|
| Simulators | Direct TCP (port 8080) | Build with Xcode, install via `simctl` |
| Physical devices | USB tunnel via usbmuxd | Build with Xcode, deploy to device |

## Requirements

- macOS with Xcode and iOS Simulators installed
- Rust 1.70+
- **For Swift agent**: [xcodegen](https://github.com/yonaskolb/XcodeGen) and Xcode (see `qorvex-agent/README.md`)
- **For physical devices**: USB-connected iOS device with developer mode enabled

## Installation

```bash
# Install all Rust binaries (also records agent source dir in ~/.qorvex/config.json)
./install.sh

# Or install individually
cargo install --path crates/qorvex-repl
cargo install --path crates/qorvex-live
cargo install --path crates/qorvex-cli
cargo install --path crates/qorvex-auto
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
- Mouse drag — Select text in output area
- `Ctrl+C` — Copy selection to clipboard (or quit if no selection)
- Scroll wheel — Scroll output area

Available commands:
- `list_devices` — List all available simulators
- `use_device(udid)` — Select a simulator to use
- `boot_device(udid)` — Boot and select a simulator
- `start_agent` — Start agent using configured source dir, or connect to external agent
- `start_agent(path)` — Build and launch Swift agent from project directory
- `stop_agent` — Stop a managed agent process
- `set_target(bundle_id)` — Set target app bundle ID
- `start_session` — Begin a new session (auto-starts agent if configured)
- `end_session` — End the current session
- `get_session_info` — Get session status info
- `tap(selector)` — Tap element by accessibility ID
- `tap(selector, label)` — Tap element by label (pass "label" as 2nd arg)
- `tap(selector, label, type)` — Tap element by label with type filter
- `tap(selector, --no-wait)` — Tap without waiting for element
- `tap_location(x, y)` — Tap at screen coordinates
- `swipe()` — Swipe up (default)
- `swipe(direction)` — Swipe in a direction: up, down, left, right
- `send_keys(text)` — Type text into the focused field
- `wait_for(selector)` — Wait for element by ID (5s default timeout)
- `wait_for(selector, timeout_ms)` — Wait with custom timeout
- `wait_for(selector, timeout_ms, label)` — Wait for element by label
- `wait_for(selector, timeout_ms, label, type)` — Wait for element by label with type filter
- `get_screenshot` — Capture current screen
- `get_screen_info` — Get UI hierarchy information
- `list_elements` — List actionable UI elements
- `get_value(selector)` — Get element's value by ID
- `get_value(selector, label)` — Get element's value by label
- `get_value(selector, --no-wait)` — Get value without waiting for element
- `log_comment(text)` — Add a comment to the action log
- `start_watcher` — Start screen change detection (500ms default)
- `start_watcher(interval_ms)` — Start with custom polling interval
- `stop_watcher` — Stop screen change detection
- `help` — Show available commands
- `quit` — Exit

### Live TUI

Monitor a session in real-time with inline screenshot rendering and an action log:

```bash
qorvex-live
```

Controls:
- `q` — Quit
- `r` — Refresh screenshot
- Arrow keys — Scroll action log

### CLI

Scriptable client for automation pipelines (requires a running REPL session):

```bash
# Tap an element by accessibility ID (waits for it by default)
qorvex tap login-button

# Tap an element by accessibility label
qorvex tap "Sign In" --label

# Tap a specific element type by label
qorvex tap "Sign In" --label --type Button

# Tap without waiting for element
qorvex tap login-button --no-wait

# Tap at coordinates
qorvex tap-location 100 200

# Send keyboard input
qorvex send-keys "hello world"

# Get screenshot (base64)
qorvex screenshot > screen.b64

# Get screen info (JSON)
qorvex screen-info | jq '.elements'

# Get element value by ID (waits for element by default)
qorvex get-value username-field

# Get element value by label
qorvex get-value "Email" --label

# Get value without waiting
qorvex get-value username-field --no-wait

# Wait for element to appear by ID
qorvex wait-for spinner-id --timeout 10000

# Wait for element by label
qorvex wait-for "Loading" --label --timeout 10000

# Log a comment to the session
qorvex comment "Starting login flow"

# Connect to a specific session
qorvex -s my-session tap button

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
- `tap`, `get-value`: `-l, --label` — Match by label instead of ID; `-T, --type <type>` — Filter by element type; `--no-wait` — Skip auto-wait for element; `-o, --timeout <ms>` — Wait timeout (default: 5000)
- `wait-for`: `-l, --label` — Match by label instead of ID; `-T, --type <type>` — Filter by element type; `-o, --timeout <ms>` — Wait timeout (default: 5000)

### Auto

Run `.qvx` automation scripts or convert action logs to scripts:

```bash
# Run a script against a booted simulator
qorvex-auto run test-login.qvx

# Run against a specific session
qorvex-auto -s my-session run test-login.qvx

# Convert an action log to a replayable script
qorvex-auto convert ~/.qorvex/logs/default_20250101_120000.jsonl --stdout

# Convert and save to file
qorvex-auto convert session.jsonl --name login-flow
```

The `.qvx` script language uses REPL-compatible commands with control flow:

```
# Variables and commands
use_device("UDID-HERE")
set_target("com.example.myapp")
tap("login-button")
send_keys("user@example.com")
swipe("down")

# Capture command output into variables
value = get_value("status-label")

# Control flow
if value == "Success" {
    log_comment("Login passed")
} else {
    tap("retry-button")
}

# Loops
foreach item in ["tab1", "tab2", "tab3"] {
    tap(item)
    get_screenshot
}

for i from 1 to 5 {
    tap("next-button")
}

# Set default timeout
set timeout 10000

# Include another script
include "shared/login.qvx"
```

## Architecture

```
┌─────────────┐     IPC      ┌──────────────┐
│ qorvex-repl │◄────────────►│ qorvex-live  │
└──────┬──────┘              └──────────────┘
       │                            ▲
       │ IPC                   IPC  │
       │◄───────────────────────────┤
       │                     ┌──────┴─────┐
       │                     │ qorvex-cli │
       │                     └────────────┘
       ▼
┌──────────────┐       ┌──────────────┐
│ qorvex-core  │◄──────┤ qorvex-auto  │
├──────────────┤       ├──────────────┤
│ ActionExecutor       │  .qvx parser │
│      │               │  runtime     │
│ AutomationDriver     │  executor    │
│ ┌────┴────┐          │  converter   │
│ │AgentDrvr│          └──────────────┘
│ └────┬────┘
│      │
│   TCP 8080──► qorvex-agent (Swift)
│      │              │
│   simctl    USB   XCUIElement
│      │    tunnel  accessibility
│      │  (usbmuxd)
│      ▼      ▼
│  iOS Sim  Physical
│           Device
└──────────────┘
```

`qorvex-auto` uses core directly (not via IPC) but spawns its own IPC server so `qorvex-live` can monitor script execution.

The `AutomationDriver` trait abstracts the automation backend. `AgentDriver` communicates with the Swift agent over a binary TCP protocol. For physical devices, it tunnels through usbmuxd.

### Directory Structure

Qorvex stores runtime files in `~/.qorvex/`:

```
~/.qorvex/
├── config.json                  # Persistent config (agent_source_dir, etc.)
├── qorvex_default.sock          # Unix socket for "default" session
├── qorvex_my-session.sock       # Unix socket for "my-session"
├── logs/
│   ├── default_20250101_120000.jsonl
│   └── my-session_20250101_130000.jsonl
└── automation/
    ├── logs/                # Action logs from qorvex-auto runs
    └── scripts/             # Converted .qvx scripts
```

- **Config** (`~/.qorvex/config.json`) — Persistent settings. Currently stores `agent_source_dir` so that `start_session`, `start_agent`, and `qorvex-auto run` can auto-build the Swift agent. Written by `install.sh`.
- **Sockets** (`~/.qorvex/qorvex_<session>.sock`) — IPC endpoints for REPL and auto sessions. The CLI, Live TUI, and auto runner all use these to communicate.
- **Logs** (`~/.qorvex/logs/<session>_<timestamp>.jsonl`) — Persistent action logs from REPL sessions in JSON Lines format.
- **Automation** (`~/.qorvex/automation/`) — Separate log and script directories for `qorvex-auto`. The `convert` command saves output scripts here by default.

Use `qorvex list-sessions` to discover running sessions by scanning for active socket files.

## License

MIT
