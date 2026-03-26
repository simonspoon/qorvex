# qorvex

iOS Simulator and device automation toolkit for macOS.

## Overview

Qorvex provides programmatic control over iOS Simulators and physical devices through a Rust workspace with five crates and a Swift agent:

- **qorvex-core** — Core library with driver abstraction, protocol, session, IPC, and execution engine
- **qorvex-server** — Standalone automation server daemon; manages sessions, agent lifecycle, and IPC
- **qorvex-repl** — TUI REPL client for manual testing; auto-launches the server if needed
- **qorvex-live** — TUI client with inline screenshot rendering and action log monitoring
- **qorvex-cli** — Scriptable CLI client for automation pipelines, including JSONL log-to-script conversion
- **qorvex-agent** — Swift XCTest agent for native iOS accessibility automation (not a Cargo crate)
- **qorvex-streamer** — ScreenCaptureKit-based live video streamer for Simulator windows (Swift, macOS 13+)
- **qorvex-testapp** — SwiftUI iOS test app covering all automation actions; use it to verify qorvex locally

### Automation backend

Qorvex uses a native Swift XCTest agent behind the `AutomationDriver` trait:

| Target | Connection | Setup |
|--------|------------|-------|
| Simulators | Direct TCP (localhost:8080) | Build with Xcode, install via `simctl` |
| Physical devices — WiFi | Direct TCP via mDNS (`<Name>.local`) | Same WiFi network, developer mode on |
| Physical devices — USB | Direct TCP via mDNS (`<Name>.local`) | USB cable, developer mode on |

## Requirements

- macOS with Xcode and iOS Simulators installed
- Rust 1.70+
- **For Swift agent**: [xcodegen](https://github.com/yonaskolb/XcodeGen) and Xcode (see `qorvex-agent/README.md`)
- **For qorvex-streamer**: macOS 13+ and Screen Recording permission
- **For physical devices**: iOS device with developer mode enabled; connected via USB or on the same WiFi network (iOS 17+)

## Installation

### Homebrew

```bash
brew install simonspoon/tap/qorvex
```

### From GitHub Releases

Download the latest tarball from [Releases](https://github.com/simonspoon/qorvex/releases) and extract the pre-built binaries:

```bash
tar xzf qorvex-macos-arm64.tar.gz
mv qorvex-server qorvex-repl qorvex-live qorvex qorvex-streamer ~/.cargo/bin/
```

The tarball includes the agent source in `agent/`. To build it manually:

```bash
cd agent
xcodebuild build-for-testing \
  -project QorvexAgent.xcodeproj \
  -scheme QorvexAgentUITests \
  -destination "generic/platform=iOS Simulator" \
  -derivedDataPath .build
```

### From source

```bash
# Install everything: Rust binaries, Swift streamer, Swift agent (simulator + physical)
./install.sh

# Or install Rust crates individually
cargo install --path crates/qorvex-server
cargo install --path crates/qorvex-repl
cargo install --path crates/qorvex-live
cargo install --path crates/qorvex-cli
```

## Usage

### REPL

Start an interactive TUI session:

```bash
qorvex-repl
```

For non-interactive use (CI, scripting, testing), use batch mode:

```bash
echo -e "help\nlist-devices\nquit" | qorvex-repl --batch -s default
```

Batch mode reads commands from stdin, prints plain text to stdout, and exits on EOF or `quit`.

The REPL provides a terminal UI with:
- Tab completion for commands, element IDs, and device UDIDs
- Output history with scrolling (arrow keys)
- Session and device status in the title bar
- Animated spinner in the input area while commands process (non-blocking — TUI stays responsive)

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
- `list-devices` — List all available simulators
- `list-physical-devices` — List physical iOS devices connected via USB or network
- `use-device <udid>` — Select a simulator or physical device by UDID
- `boot-device <udid>` — Boot and select a simulator
- `start-agent` — Start agent using configured source dir, or connect to external agent
- `start-agent <path>` — Build and launch Swift agent from project directory
- `stop-agent` — Stop a managed agent process
- `set-target <bundle_id>` — Set target app bundle ID
- `start-target` — Launch the target app
- `stop-target` — Terminate the target app
- `set-timeout <ms>` — Set default timeout for tap/wait operations (default: 5000ms); no arg prints current value
- `start-session` — Begin a new session (auto-starts agent if configured)
- `end-session` — End the current session
- `get-session-info` — Get session status info
- `tap <selector>` — Tap element by accessibility ID
- `tap <selector> --label` — Tap element by label
- `tap <selector> --label --type <type>` — Tap element by label with type filter
- `tap <selector> --no-wait` — Tap without waiting for element
- `tap <selector> --timeout <ms>` — Tap with custom timeout
- `tap-location <x> <y>` — Tap at screen coordinates
- `swipe` — Swipe up (default)
- `swipe <direction>` — Swipe in a direction: up, down, left, right
- `send-keys <text>` — Type text into the focused field
- `wait-for <selector>` — Wait for element by ID (5s default timeout)
- `wait-for <selector> --timeout <ms>` — Wait with custom timeout
- `wait-for <selector> --label` — Wait for element by label
- `wait-for <selector> --label --type <type>` — Wait for element by label with type filter
- `wait-for-not <selector>` — Wait for element to disappear (5s default timeout)
- `wait-for-not <selector> --timeout <ms>` — Wait for disappearance with custom timeout
- `get-screenshot` — Capture current screen
- `get-screen-info` — Get UI hierarchy information
- `list-elements` — List actionable UI elements
- `get-value <selector>` — Get element's value by ID
- `get-value <selector> --label` — Get element's value by label
- `get-value <selector> --no-wait` — Get value without waiting for element
- `log-comment <text>` — Add a comment to the action log
- `help` — Show available commands
- `quit` — Exit

### Live TUI

Monitor a session in real-time with a live video feed and action log:

```bash
qorvex-live            # live feed at 15 fps (default)
qorvex-live --fps 30   # higher frame rate
qorvex-live --no-streamer  # polling fallback (no Screen Recording permission needed)
qorvex-live --batch --duration 10  # print session events as JSONL for 10 seconds
```

`qorvex-live` automatically launches `qorvex-streamer` to capture the Simulator window via ScreenCaptureKit — zero impact on the automation session. Falls back to polling if the streamer binary is not found or Screen Recording permission is denied.

Controls:
- `q` — Quit
- `r` — Refresh screenshot (polling fallback only)
- Arrow keys — Scroll action log

### CLI

Scriptable client for automation pipelines (requires a running server session):

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

# Get screen info (concise actionable elements)
qorvex screen-info

# Get full raw JSON
qorvex screen-info --full

# Get REPL-style formatted list
qorvex screen-info --pretty

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

Environment:
- `QORVEX_SESSION` — Default session name
- `QORVEX_TIMEOUT` — Default timeout in milliseconds for `tap`, `get-value`, `wait-for`, `wait-for-not` (default: 5000); overridden by `-o`
- `QORVEX_LOG_DIR` — Override log file directory (default: `~/.qorvex/logs/`)

Command-specific options:
- `tap`, `get-value`: `-l, --label` — Match by label instead of ID; `-T, --type <type>` — Filter by element type; `--no-wait` — Skip retry, attempt once; `-o, --timeout <ms>` — Retry timeout (default: 5000); `--tag <text>` — Annotate the log entry
- `wait-for`, `wait-for-not`: `-l, --label` — Match by label instead of ID; `-T, --type <type>` — Filter by element type; `-o, --timeout <ms>` — Wait timeout (default: 5000); `--tag <text>` — Annotate the log entry
- All action commands accept `--tag <text>` — free-text annotation written to the JSONL log; preserved when converting logs to scripts with `qorvex convert`

Additional CLI commands (no running session required):

```bash
# List simulator devices
qorvex list-devices

# Boot a simulator
qorvex boot-device <udid>

# Set target app
qorvex set-target com.example.MyApp

# Swipe
qorvex swipe up

# Convert action log to shell script
qorvex convert ~/.qorvex/logs/default_20250101_120000.jsonl > replay.sh

# Convert from stdin
qorvex log -f json | qorvex convert > replay.sh

# Generate shell completions (zsh, bash, fish, elvish, powershell)
eval "$(qorvex completions zsh)"
```

### Shell Scripting

Automation scripts are plain bash that call `qorvex` CLI commands. Use `qorvex start --device <udid>` to launch the server, select a device, and start the session in one step:

```bash
#!/usr/bin/env bash
set -euo pipefail
export QORVEX_SESSION=my_test

qorvex start --device "XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX"
trap 'qorvex stop || true' EXIT
qorvex set-target com.example.myapp

qorvex tap login-button
qorvex send-keys 'user@example.com'
qorvex tap password-field
qorvex send-keys 'password123'
qorvex tap submit-button

qorvex wait-for welcome-label -o 10000
value=$(qorvex get-value welcome-label)

if [ "$value" = "Welcome!" ]; then
    qorvex comment "Login test passed"
else
    echo "Failed: got $value" >&2
    exit 1
fi

for tab in home settings profile; do
    qorvex tap "$tab"
    qorvex screenshot > "/tmp/${tab}.b64"
done
```

## Architecture

```
┌──────────────┐   IPC    ┌─────────────────────────┐
│ qorvex-repl  │─────────►│                         │
└──────────────┘          │     qorvex-server        │
┌──────────────┐   IPC    │  (manages sessions,      │
│ qorvex-live  │─────────►│   agent lifecycle, IPC)  │
│              │          │                         │
│   spawns ▼   │          └───────────┬─────────────┘
│ qorvex-      │  Unix sock             TCP 8080 │
│  streamer ───┘──────────────┐          ▼
└──────────────┘  JPEG frames │  ┌─────────────────┐
┌──────────────┐   IPC        │  │  qorvex-agent   │
│ qorvex-cli   │──────────────┘  │  (Swift/XCTest) │
└──────────────┘             └────────┬────────┘
                                       │ XCUIElement
                                    simctl / USB
                                    (usbmuxd)
                                       │
                              iOS Simulator / Device
```

`qorvex-server` runs the `IpcServer` and manages session state, agent lifecycle, and automation execution. The REPL, Live TUI, and CLI are all IPC clients. `AgentDriver` communicates with the Swift agent over a binary TCP protocol; for physical devices it connects via Bonjour mDNS (`<Name>.local`), which works for both WiFi and USB-connected devices.

### Directory Structure

Qorvex stores runtime files in `~/.qorvex/`:

```
~/.qorvex/
├── config.json                  # Persistent config (agent_source_dir, etc.)
├── qorvex_default.sock          # Unix socket for "default" session
├── qorvex_my-session.sock       # Unix socket for "my-session"
├── streamer_default.sock        # Live video socket for "default" session (qorvex-live)
└── logs/
    ├── default_20250101_120000.jsonl
    └── my-session_20250101_130000.jsonl
```

- **Config** (`~/.qorvex/config.json`) — Persistent settings. Stores `agent_source_dir` so that `start-session` and `start-agent` can auto-build the Swift agent. Written by `install.sh`. When `agent_source_dir` is not set, the server automatically checks for a Homebrew-installed agent at `HOMEBREW_PREFIX/share/qorvex/agent`.
- **Sockets** (`~/.qorvex/qorvex_<session>.sock`) — IPC endpoints for REPL sessions. The CLI and Live TUI use these to communicate.
- **Logs** (`~/.qorvex/logs/<session>_<timestamp>.jsonl`) — Persistent action logs from REPL sessions in JSON Lines format. Use `qorvex convert` to turn these into shell scripts.

Use `qorvex list-sessions` to discover running sessions by scanning for active socket files.

## License

MIT
