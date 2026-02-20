# Command Reference

Commands are available across two interfaces: the REPL (interactive) and CLI (scriptable). This reference covers both.

## Session Management

| Command | REPL | CLI |
|---------|------|-----|
| Start session | `start_session` | (auto) |
| End session | `end_session` | — |
| Session info | `get_session_info` | `qorvex status` |
| Get action log | — | `qorvex log` |
| List sessions | — | `qorvex list-sessions` |

## Device Management

| Command | REPL | CLI |
|---------|------|-----|
| List devices | `list_devices` | `qorvex list-devices` |
| Select device | `use_device(udid)` | — |
| Boot + select | `boot_device(udid)` | `qorvex boot-device <udid>` |

## Agent Management

| Command | REPL | CLI |
|---------|------|-----|
| Start agent | `start_agent` or `start_agent(path)` | — |
| Stop agent | `stop_agent` | — |
| Set target app | `set_target(bundle_id)` | `qorvex set-target <bundle_id>` |

## UI Interaction

### Tap

| Syntax | Description |
|--------|-------------|
| REPL: `tap(selector)` | Tap by accessibility ID |
| REPL: `tap(selector, label)` | Tap by label |
| REPL: `tap(selector, label, type)` | Tap by label with type filter |
| REPL: `tap(selector, --no-wait)` | Tap without waiting for element |
| CLI: `qorvex tap <selector>` | Tap by ID |
| CLI: `qorvex tap <selector> --label` | Tap by label |
| CLI: `qorvex tap <selector> --label --type Button` | Tap by label + type |
| CLI: `qorvex tap <selector> --no-wait` | Skip auto-wait |

Tap auto-wait behavior (unless `--no-wait`): polls every 100ms until the element both exists and is hittable, then taps. Fails with timeout if the element never becomes hittable. This is faster than the explicit `wait-for` (no frame-stability check) but still guards against elements that are covered, off-screen, or mid-animation.

### Tap at Coordinates

| Syntax | Description |
|--------|-------------|
| REPL: `tap_location(x, y)` | Tap at screen coordinates |
| CLI: `qorvex tap-location <x> <y>` | Same |

### Swipe

| Syntax | Description |
|--------|-------------|
| REPL: `swipe()` or `swipe(direction)` | Swipe (default: up). Directions: up, down, left, right |
| CLI: `qorvex swipe <direction>` | Same |

### Send Keys

| Syntax | Description |
|--------|-------------|
| REPL: `send_keys(text)` | Type text into focused field |
| CLI: `qorvex send-keys "text"` | Same |

### Wait For Element

| Syntax | Description |
|--------|-------------|
| REPL: `wait_for(selector)` | Wait for element by ID (5s default) |
| REPL: `wait_for(selector, timeout_ms)` | Custom timeout |
| REPL: `wait_for(selector, timeout_ms, label)` | Wait by label |
| REPL: `wait_for(selector, timeout_ms, label, type)` | Wait by label + type |
| CLI: `qorvex wait-for <selector> --timeout 10000` | Wait with timeout |
| CLI: `qorvex wait-for <selector> --label --timeout 10000` | Wait by label |

Wait behavior: polls every 100ms, requires element to be hittable, requires 3 consecutive stable frames (same position) before success. This is the strict mode used by the explicit `wait-for` command.

### Wait For Element to Disappear

| Syntax | Description |
|--------|-------------|
| REPL: `wait_for_not(selector)` | Wait for element to disappear by ID (5s default) |
| REPL: `wait_for_not(selector, timeout_ms)` | Custom timeout |
| REPL: `wait_for_not(selector, timeout_ms, label)` | Wait by label |
| REPL: `wait_for_not(selector, timeout_ms, label, type)` | Wait by label + type |
| CLI: `qorvex wait-for-not <selector> --timeout 10000` | Wait for disappearance |
| CLI: `qorvex wait-for-not <selector> --label --timeout 10000` | Wait by label |

Returns success as soon as the element is absent or not hittable. Fails with timeout if element persists.

## Screen and Elements

| Command | REPL | CLI |
|---------|------|-----|
| Screenshot | `get_screenshot` | `qorvex screenshot` |
| Screen info | `get_screen_info` | `qorvex screen-info` |
| List elements | `list_elements` | — |

`qorvex screen-info` outputs actionable elements as concise JSON by default (no null fields, rounded frame values). Use `--full` to get the complete raw JSON, or `--pretty` for REPL-style formatted output. `qorvex get-value` prints the element value to stdout. Status messages go to stderr.

## Values

| Syntax | Description |
|--------|-------------|
| REPL: `get_value(selector)` | Get element value by ID |
| REPL: `get_value(selector, label)` | Get by label |
| REPL: `get_value(selector, --no-wait)` | Without waiting |
| CLI: `qorvex get-value <selector>` | By ID |
| CLI: `qorvex get-value <selector> --label` | By label |
| CLI: `qorvex get-value <selector> --no-wait` | Without waiting |

## Watcher

| Command | REPL |
|---------|------|
| Start watcher | `start_watcher` or `start_watcher(interval_ms)` |
| Stop watcher | `stop_watcher` |

Default interval: 500ms. Detects both accessibility tree changes and visual changes (via perceptual hashing).

## Log Conversion

| Command | Description |
|---------|-------------|
| CLI: `qorvex convert <log.jsonl>` | Convert JSONL log file to shell script |
| CLI: `qorvex convert` | Convert from stdin |

See [scripting-guide.md](scripting-guide.md) for full scripting details.

## Logging

| Command | REPL | CLI |
|---------|------|-----|
| Add comment | `log_comment(text)` | `qorvex comment "text"` |

## CLI-Specific Options

- `-s, --session <name>` -- Connect to named session (default: "default", or `$QORVEX_SESSION`)
- `-f, --format <text|json>` -- Output format
- `-q, --quiet` -- Suppress non-essential output
- `tap`, `get-value`: `-l, --label`, `-T, --type <type>`, `--no-wait`, `-o, --timeout <ms>`
- `wait-for`, `wait-for-not`: `-l, --label`, `-T, --type <type>`, `-o, --timeout <ms>` (default: 5000)

## Element Selectors

Selectors support glob matching:

- `*` matches any number of characters
- `?` matches exactly one character

Example: `tap(login-*)` matches `login-button`, `login-field`, etc.
