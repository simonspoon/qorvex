# Command Reference

Commands are available across two interfaces: the REPL (interactive) and CLI (scriptable). This reference covers both.

## Session Management

| Command | REPL | CLI |
|---------|------|-----|
| Start server + session (one step) | — | `qorvex start [--device <udid>]` |
| Start session | `start-session` | `qorvex start-session` |
| End session | `end-session` | — |
| Stop server | — | `qorvex stop` |
| Session info | `get-session-info` | `qorvex status` |
| Get action log | — | `qorvex log` |
| List sessions | — | `qorvex list-sessions` |

## Device Management

| Command | REPL | CLI |
|---------|------|-----|
| List simulators | `list-devices` | `qorvex list-devices` |
| List physical devices | `list-physical-devices` | `qorvex list-physical-devices` |
| Select device | `use-device <udid>` | `qorvex use-device <udid>` |
| Boot + select | `boot-device <udid>` | `qorvex boot-device <udid>` |
| Create + select | `create-device <name> <type> <runtime>` | `qorvex create-device <name> <type> <runtime>` |
| Wait for boot | `wait-for-boot` | `qorvex wait-for-boot` |
| Shut down selected device | `shutdown-device` | `qorvex shutdown-device` |
| Delete selected device | `delete-device` | `qorvex delete-device` |
| Quiet `mediaanalysisd` | `quiet-device` | `qorvex quiet-device <udid>` |
| Set appearance | `set-appearance <dark\|light>` | `qorvex set-appearance <dark\|light>` |
| Set content size | `set-content-size <size>` | `qorvex set-content-size <size>` |
| Show recent device log | `device-log [--last 5m]` | `qorvex device-log [--last 5m] [--predicate <expr>]` |

> **The device commands are session-scoped:** none of them takes a UDID. They
> act on the simulator this session selected with `use-device`, `boot-device`
> or `create-device` and on nothing else, so one session can never shut down or
> delete another session's simulator. There is no multi-device form; to act on
> a different simulator, select it first.
> ```bash
> $ qorvex use-device <udid>
> $ qorvex shutdown-device
> Shut down <udid>
> ```
> With no device selected they fail with `No device selected.`

`create-device` is the one exception: there is no device to act on until it has
made one, so it names the device — and on success it becomes this session's
selection, the mirror of `delete-device` dropping it. It prints the new UDID.

```bash
$ qorvex create-device Aperture-5e9943ac \
    com.apple.CoreSimulator.SimDeviceType.iPhone-17 \
    com.apple.CoreSimulator.SimRuntime.iOS-26-5
5E9943AC-...-...
$ qorvex boot-device 5E9943AC-...-...
$ qorvex wait-for-boot
```

`boot-device` returns as soon as `simctl boot` returns, which is when the boot
has *started*. `wait-for-boot` (`simctl bootstatus -b`) blocks until it has
finished, which is what a script should wait on before driving the UI.

`quiet-device` is the one command whose CLI and REPL forms differ: the CLI takes
a UDID because it also serves build scripts with no qorvex server running, while
the REPL form uses the session's device like everything else here. Device
selection already quiets `mediaanalysisd` once; this is for the daemon coming
back on a long-lived session.

### Appearance, Dynamic Type and logs

```bash
$ qorvex set-appearance dark
$ qorvex set-content-size accessibility-extra-extra-extra-large
$ qorvex device-log --last 2m --predicate 'subsystem == "com.example.MyApp"'
```

`set-content-size` takes the full Dynamic Type range: `extra-small`, `small`,
`medium`, `large`, `extra-large`, `extra-extra-large`,
`extra-extra-extra-large`, and the `accessibility-` variants from
`accessibility-medium` through `accessibility-extra-extra-extra-large`.

`device-log` is **one-shot**: it wraps `simctl spawn <udid> log show`, defaulting
to the last 5 minutes. The streaming `log stream` form has no command, because a
process that never returns does not fit qorvex's request/response protocol — run
it directly if you need a live tail.

## App Management

| Command | REPL | CLI |
|---------|------|-----|
| Install an app bundle | `install-app <path.app>` | `qorvex install-app <path.app>` |
| Uninstall an app | `uninstall-app <bundle_id>` | `qorvex uninstall-app <bundle_id>` |
| Get an app container path | `app-container <bundle_id> [app\|data\|groups]` | `qorvex app-container <bundle_id> [app\|data\|groups]` |
| List installed apps | `list-apps` | `qorvex list-apps` |
| Change a privacy permission | `grant-permission <verb> <service> <bundle_id>` | `qorvex grant-permission <verb> <service> <bundle_id>` |
| Add media to the libraries | `add-media <file>...` | `qorvex add-media <file>...` |
| Open a URL | `open-url <url>` | `qorvex open-url <url>` |

These wrap `simctl install`, `uninstall`, `get_app_container` and `listapps` on
the session's selected simulator — no agent needed, and no UDID to pass:

```bash
$ qorvex install-app ./build/MyApp.app       # relative paths are resolved client-side
$ qorvex app-container com.example.MyApp data
/Users/me/Library/Developer/CoreSimulator/Devices/.../data/Containers/Data/Application/...
$ qorvex list-apps
com.example.MyApp -- MyApp (User)
$ qorvex --format json list-apps
[{"bundle_id":"com.example.MyApp","display_name":"MyApp","app_type":"User"}]
```

`app-container` defaults to the `app` container (the installed `.app` bundle),
matching `simctl get_app_container`.

```bash
$ qorvex grant-permission grant microphone com.example.MyApp
$ qorvex add-media ./fixtures/one.png ./fixtures/two.mov   # several files at once
$ qorvex open-url myapp://onboarding/step-2
```

`grant-permission` takes `grant`, `revoke` or `reset`, and the simctl privacy
services: `all`, `calendar`, `contacts-limited`, `contacts`, `location`,
`location-always`, `photos-add`, `photos`, `media-library`, `microphone`,
`motion`, `reminders`, `siri`. `reset` forgets the decision, so the app prompts
again the next time it asks.

## Agent Management

| Command | REPL | CLI |
|---------|------|-----|
| Start agent | `start-agent` or `start-agent <path>` | `qorvex start-agent [--project-dir <path>]` |
| Stop agent | `stop-agent` | — |
| Set target app | `set-target <bundle_id>` | `qorvex set-target <bundle_id>` |
| Get target app info | `get-target-info` | `qorvex target-info` |
| Launch target app | `start-target [--force]` | `qorvex start-target [--force]` |
| Terminate target app | `stop-target` | `qorvex stop-target` |
| Target/device memory | `memory-info` | `qorvex memory-info` |

> **No agent needed to choose an app:** `set-target`, `start-target` and
> `stop-target` all work with only a device selected — launching and terminating
> go through `simctl`/`adb`, and `set-target` just records the id. So switching
> the app under test costs nothing:
> ```bash
> $ qorvex use-device <udid>
> $ qorvex set-target com.example.App     # no start-agent required
> $ qorvex start-target
> Launched com.example.App (pid 34955)
> ```
> With no agent reachable, `set-target` says so and still records the id:
> `Target set to com.example.App (recorded; no agent connected)`. An agent
> started later picks up the recorded target — no second `set-target`.

> **Already-running apps:** neither backend relaunches an app that is already
> up, so `start-target` reports which happened instead of silently no-opping:
> ```bash
> $ qorvex start-target
> Launched com.example.App (pid 34955)
> $ qorvex start-target
> com.example.App already running (pid 34955) — not relaunched; pass --force to restart it
> $ qorvex start-target --force
> Relaunched com.example.App (pid 35408)
> ```
> Scripts should branch on the structured form rather than the message:
> ```bash
> qorvex --format json start-target
> {"already_running":false,"bundle_id":"com.example.App","launched":true,"pid":36228}
> ```
> This matters for a suite whose fixtures reset **on launch** — without `--force`
> a second script attaches to the first one's mutated state. Either pass
> `--force`, or `stop-target` before `start-target`.

> **Physical devices:** `start-target` and `stop-target` use `xcrun simctl` and only work for simulators. To launch or terminate an app on a physical device:
> ```bash
> xcrun devicectl device process launch --device <udid> <bundle_id>
> xcrun devicectl device process terminate --device <udid> <bundle_id>
> ```
| Set default timeout | `set-timeout <ms>` | — |

## UI Interaction

### Tap

| Syntax | Description |
|--------|-------------|
| `tap <selector>` | Tap by accessibility ID |
| `tap <selector> --label` | Tap by label |
| `tap <selector> --label --type Button` | Tap by label with type filter |
| `tap <selector> --no-wait` | Tap without waiting for element |
| `tap <selector> --timeout 10000` | Tap with custom timeout |

Same syntax for both REPL and CLI (prefix CLI commands with `qorvex`).

Tap retry behavior (unless `--no-wait`): polls every 50ms on the agent side. On each poll, the element must be found and hittable, and its frame must be stable across 2 consecutive polls before the tap fires. After stability is confirmed, the element is re-queried and its frame validated against the stable position; any drift resets the check. This makes tap animation-aware — tapping immediately after a modal transition works without manual sleeps. Fails with timeout if the element never becomes tappable and stable. Use explicit `wait-for` if you need to assert stability before chaining other operations.

### Tap at Coordinates

| Syntax | Description |
|--------|-------------|
| `tap-location <x> <y>` | Tap at screen coordinates (REPL and CLI) |

### Long Press

| Syntax | Description |
|--------|-------------|
| `qorvex long-press <x> <y>` | Long press at coordinates (1.0s default) |
| `qorvex long-press <x> <y> --duration <s>` | Long press with custom duration in seconds |

### Swipe

| Syntax | Description |
|--------|-------------|
| `swipe` or `swipe <direction>` | Swipe (default: up). Directions: up, down, left, right (REPL and CLI) |

### Send Keys

| Syntax | Description |
|--------|-------------|
| `send-keys <text>` | Type text into focused field (REPL and CLI) |

### Wait For Element

| Syntax | Description |
|--------|-------------|
| `wait-for <selector>` | Wait for element by ID (uses `set-timeout` default, initially 5s) |
| `wait-for <selector> --timeout 10000` | Custom timeout |
| `wait-for <selector> --label` | Wait by label |
| `wait-for <selector> --label --type Button` | Wait by label + type |

Same syntax for both REPL and CLI (prefix CLI commands with `qorvex`).

Wait behavior: polls every 100ms, requires element to be hittable, requires 3 consecutive stable frames (same position) before success. This is the strict mode used by the explicit `wait-for` command.

### Wait For Element to Disappear

| Syntax | Description |
|--------|-------------|
| `wait-for-not <selector>` | Wait for element to disappear by ID (uses `set-timeout` default, initially 5s) |
| `wait-for-not <selector> --timeout 10000` | Custom timeout |
| `wait-for-not <selector> --label` | Wait by label |
| `wait-for-not <selector> --label --type Button` | Wait by label + type |

Same syntax for both REPL and CLI (prefix CLI commands with `qorvex`).

Returns success as soon as the element is absent or not hittable. Fails with timeout if element persists.

## Screen and Elements

| Command | REPL | CLI |
|---------|------|-----|
| Screenshot | `get-screenshot` | `qorvex screenshot` |
| Screen info | `get-screen-info` | `qorvex screen-info` |
| List elements | `list-elements` | — |

`qorvex screen-info` outputs actionable elements as concise JSON by default (no null fields, rounded frame values). Use `--full` to get the complete raw JSON, or `--pretty` for REPL-style formatted output. `qorvex get-value` prints the element value to stdout. Status messages go to stderr in pipe-delimited format: `|timestamp|Action|target|elapsed_ms|` for all actions.

## Values

| Syntax | Description |
|--------|-------------|
| `get-value <selector>` | Get element value by ID |
| `get-value <selector> --label` | Get by label |
| `get-value <selector> --no-wait` | Without waiting |

Same syntax for both REPL and CLI (prefix CLI commands with `qorvex`).

## Log Conversion

| Command | Description |
|---------|-------------|
| CLI: `qorvex convert <log.jsonl>` | Convert JSONL log file to shell script |
| CLI: `qorvex convert` | Convert from stdin |

See [scripting-guide.md](scripting-guide.md) for full scripting details.

## Shell Completions

| Command | Description |
|---------|-------------|
| `qorvex completions zsh` | Print Zsh completion script |
| `qorvex completions bash` | Print Bash completion script |
| `qorvex completions fish` | Print Fish completion script |
| `qorvex completions elvish` | Print Elvish completion script |
| `qorvex completions powershell` | Print PowerShell completion script |

No running session required. Output the script to stdout and source it in your shell profile:

```zsh
# Add to ~/.zshrc
eval "$(qorvex completions zsh)"

# Or write to a completions file (faster shell startup)
qorvex completions zsh > ~/.zfunc/_qorvex
# Ensure ~/.zfunc is in fpath: fpath=(~/.zfunc $fpath)
```

## Logging

| Command | REPL | CLI |
|---------|------|-----|
| Add comment | `log-comment <text>` | `qorvex comment "text"` |

## CLI-Specific Options

- `-s, --session <name>` -- Connect to named session (default: "default", or `$QORVEX_SESSION`)
- `-f, --format <text|json>` -- Output format
- `-q, --quiet` -- Suppress non-essential output
- `start`: `-d, --device <udid>` -- Select a device (simulator or physical) before starting the session; equivalent to sending `use-device` then `start-session` in sequence
- `tap`, `get-value`: `-l, --label`, `-T, --type <type>`, `--no-wait`, `-o, --timeout <ms>`, `--tag <text>`
- `wait-for`, `wait-for-not`: `-l, --label`, `-T, --type <type>`, `-o, --timeout <ms>` (default: 5000), `--tag <text>`
- All action commands accept `--tag <text>` — annotates the JSONL log entry; replays as `--tag` in converted scripts

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `QORVEX_SESSION` | `default` | Session name — respected by both `qorvex` (CLI) and `qorvex-server`. Set once at the top of a script to avoid passing `-s` on every command. |
| `QORVEX_TIMEOUT` | `5000` | Default timeout in milliseconds for `tap`, `get-value`, `wait-for`, `wait-for-not`. Overridden by `-o` / `--timeout`. |
| `QORVEX_LOG_DIR` | `~/.qorvex/logs/` | Override the directory where log files are written. Useful for redirecting logs to a per-run output folder in automation pipelines. |

## Element Selectors

Selectors support glob matching:

- `*` matches any number of characters
- `?` matches exactly one character

Example: `tap login-*` matches `login-button`, `login-field`, etc.

### Array-Index Syntax

When multiple elements share the same accessibility ID or label, append `[N]` (0-based) to select a specific one:

| Selector | Meaning |
|----------|---------|
| `row` | First element with ID `row` |
| `row[0]` | First element with ID `row` (explicit) |
| `row[2]` | Third element with ID `row` |
| `cell_*[1]` | Second element whose ID matches `cell_*` |

Shell quoting is required in the CLI when using brackets: `qorvex tap 'cell[2]'`. In the REPL no quoting is needed: `tap cell[2]`.

Out-of-bounds indices (e.g., `row[999]` when fewer elements exist) return an "element not found" error.
