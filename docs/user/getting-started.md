# Getting Started

## Requirements

- macOS with Xcode and iOS Simulators installed
- Rust 1.70+
- [xcodegen](https://github.com/yonaskolb/XcodeGen) (for building the Swift agent)
- For physical devices: iOS device with developer mode enabled, connected via USB or on the same WiFi network (iOS 17+), plus an Apple Development Team ID in `~/.qorvex/config.json` (see [Physical Device Signing](#physical-device-signing))

## Installation

### Homebrew (Recommended)

```bash
brew install simonspoon/tap/qorvex
```

The Swift agent is automatically built during installation (requires Xcode). The agent source is installed to `HOMEBREW_PREFIX/share/qorvex/agent` and discovered automatically at runtime -- no manual configuration needed.

### From Source

```bash
git clone <repo>
cd qorvex

# Install all binaries and configure agent source directory
./install.sh
```

`install.sh` installs all Rust binaries, records the agent project path in `~/.qorvex/config.json`, and pre-builds the Swift agent **for the simulator only**. Run it on every machine where you intend to use qorvex.

The physical-device runner is not pre-built, because it cannot be code signed correctly at install time. qorvex builds it on first use against a physical device — and rebuilds it every time — so it always carries your configured team's signature. See [Physical Device Signing](#physical-device-signing).

### Individual Crates

```bash
cargo install --path crates/qorvex-repl
cargo install --path crates/qorvex-cli
```

### Build the Swift Agent

```bash
make -C qorvex-agent build      # XCTest automation agent
```

`install.sh` builds it automatically (for the simulator; the physical-device runner is built on first use against a device).

## Physical Device Signing

Physical-device builds must be code signed; simulator builds are unsigned and need no configuration. Add your Apple Development Team ID to `~/.qorvex/config.json`:

```json
{
  "development_team": "ABCDE12345"
}
```

- **`development_team`** (required for any physical device) -- your 10-character Apple Team ID. Find it in Xcode ▸ Settings ▸ Accounts, or at developer.apple.com/account under Membership details. Without it, qorvex fails fast with a clear error instead of building a runner it cannot sign for you.
- **`agent_bundle_id`** (optional) -- e.g. `"com.example.qorvex.agent"`. Set this when the default App ID `com.qorvex.agent` is already registered to another team, which blocks automatic signing for yours. qorvex renames the agent app and its UI-test runner together (they become `<id>` and `<id>.uitests`).

The first `start-agent` against a physical device is slower than a simulator start because the runner is built and signed then -- and on every subsequent start, so the signature always matches your configured team. macOS may prompt for keychain or Apple ID access during that build, so run `qorvex start --device <udid>` in a terminal where you can answer the prompts.

## Shell Completions (Optional)

After installing, enable tab completion for the `qorvex` CLI:

```zsh
# Zsh — add to ~/.zshrc
eval "$(qorvex completions zsh)"
```

```bash
# Bash — add to ~/.bashrc
eval "$(qorvex completions bash)"
```

Also supported: `fish`, `elvish`, `powershell`.

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

## Simulator vs Physical Device

| | Simulator | Physical Device (WiFi) | Physical Device (USB) |
|---|---|---|---|
| Connection | Direct TCP on localhost:8080 | Direct TCP via mDNS (`<Name>.local`) | Direct TCP via mDNS (`<Name>.local`) |
| Setup | Boot simulator, start agent | Same WiFi network, developer mode on | USB cable, developer mode on |
| Select in REPL | `boot-device <udid>` | `use-device <udid>` | `use-device <udid>` |
| Launch app | `start-target` | `xcrun devicectl device process launch` | `xcrun devicectl device process launch` |
| Performance | Fast | ~1–2s per command | ~1–2s per command |

> **Note:** `start-target` and `stop-target` use `xcrun simctl` and only work for simulators.

## What Gets Created

After your first session, `~/.qorvex/` will contain:

```
~/.qorvex/
├── config.json                  # Agent source dir, signing team, and other settings
├── qorvex_default.sock          # IPC socket (while session is active)
└── logs/
    └── default_20250101_120000.jsonl  # Action log
```
