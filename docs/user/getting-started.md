# Getting Started

## Requirements

- macOS with Xcode and iOS Simulators installed
- Rust 1.70+
- [xcodegen](https://github.com/yonaskolb/XcodeGen) (for building the Swift agent)
- For physical devices: USB-connected iOS device with developer mode enabled

## Installation

### From Source (Recommended)

```bash
git clone <repo>
cd qorvex

# Install all binaries and configure agent source directory
./install.sh
```

`install.sh` installs all Rust binaries and records the agent project path in `~/.qorvex/config.json` so sessions can auto-build the Swift agent.

### Individual Crates

```bash
cargo install --path crates/qorvex-repl
cargo install --path crates/qorvex-live
cargo install --path crates/qorvex-cli
```

### Build the Swift Agent and Streamer

```bash
make -C qorvex-agent build      # XCTest automation agent
make -C qorvex-streamer build   # Live video streamer (macOS 13+)
```

`install.sh` builds both automatically.

## First Session Walkthrough

### 1. Boot a Simulator

```bash
qorvex-repl
```

In the REPL:

```
list-devices
boot-device <udid>
```

Or boot from Terminal first: `xcrun simctl boot "iPhone 16"`

### 2. Start the Agent

```
start-agent
```

This auto-builds and launches the Swift agent if `install.sh` was run. Otherwise provide the path:

```
start-agent /path/to/qorvex/qorvex-agent
```

### 3. Start a Session

```
start-session
```

This begins logging actions and connects to the IPC server.

### 4. Interact with the UI

```
get-screen-info
tap some-button-id
send-keys "hello world"
swipe down
wait-for loading-spinner --timeout 10000
get-value status-label
get-screenshot
```

### 5. Monitor with Live TUI

In another terminal:

```bash
qorvex-live           # live video feed at 15 fps (default)
qorvex-live --fps 30  # higher frame rate
```

Shows a live video feed of the Simulator window and the action log from your REPL session. Requires Screen Recording permission (macOS will prompt on first use). Use `--no-streamer` to fall back to polling if permission is unavailable.

## Simulator vs Physical Device

| | Simulator | Physical Device |
|---|---|---|
| Connection | Direct TCP on localhost:8080 | USB tunnel via usbmuxd |
| Setup | Boot simulator, start agent | Connect via USB, enable developer mode |
| REPL command | `boot-device <udid>` | `use-device <udid>` |
| Performance | Fast | Slightly slower (USB overhead) |

## What Gets Created

After your first session, `~/.qorvex/` will contain:

```
~/.qorvex/
├── config.json                  # Agent source dir config
├── qorvex_default.sock          # IPC socket (while session is active)
└── logs/
    └── default_20250101_120000.jsonl  # Action log
```
