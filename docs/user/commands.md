# Command Reference

Commands are available across three interfaces: the REPL (interactive), CLI (scriptable), and `.qvx` scripts. This reference covers all of them.

## Session Management

| Command | REPL | CLI | Script |
|---------|------|-----|--------|
| Start session | `start_session` | (auto) | `start_session` |
| End session | `end_session` | — | `end_session` |
| Session info | `get_session_info` | `qorvex status` | — |
| Get action log | — | `qorvex log` | — |
| List sessions | — | `qorvex list-sessions` | — |

## Device Management

| Command | REPL | CLI | Script |
|---------|------|-----|--------|
| List devices | `list_devices` | — | `list_devices` |
| Select device | `use_device(udid)` | — | `use_device("udid")` |
| Boot + select | `boot_device(udid)` | — | `boot_device("udid")` |

## Agent Management

| Command | REPL | CLI | Script |
|---------|------|-----|--------|
| Start agent | `start_agent` or `start_agent(path)` | — | — |
| Stop agent | `stop_agent` | — | — |
| Set target app | `set_target(bundle_id)` | — | `set_target("bundle_id")` |

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
| Script: `tap("selector")` | Tap by ID |
| Script: `tap("selector", "label")` | Tap by label |
| Script: `tap("selector", "label", "Button")` | Tap by label + type |

### Tap at Coordinates

| Syntax | Description |
|--------|-------------|
| REPL: `tap_location(x, y)` | Tap at screen coordinates |
| CLI: `qorvex tap-location <x> <y>` | Same |
| Script: `tap_location(x, y)` | Same |

### Swipe

| Syntax | Description |
|--------|-------------|
| REPL: `swipe()` or `swipe(direction)` | Swipe (default: up). Directions: up, down, left, right |
| Script: `swipe("direction")` | Same |

### Send Keys

| Syntax | Description |
|--------|-------------|
| REPL: `send_keys(text)` | Type text into focused field |
| CLI: `qorvex send-keys "text"` | Same |
| Script: `send_keys("text")` | Same |

### Wait For Element

| Syntax | Description |
|--------|-------------|
| REPL: `wait_for(selector)` | Wait for element by ID (5s default) |
| REPL: `wait_for(selector, timeout_ms)` | Custom timeout |
| REPL: `wait_for(selector, timeout_ms, label)` | Wait by label |
| REPL: `wait_for(selector, timeout_ms, label, type)` | Wait by label + type |
| CLI: `qorvex wait-for <selector> --timeout 10000` | Wait with timeout |
| CLI: `qorvex wait-for <selector> --label --timeout 10000` | Wait by label |
| Script: `wait_for("selector")` | Wait (uses `set timeout` or 5s default) |
| Script: `wait_for("selector", 10000)` | Custom timeout |

Wait behavior: polls every 100ms, requires element to be hittable, requires 3 consecutive stable frames (same position) before success.

## Screen and Elements

| Command | REPL | CLI | Script |
|---------|------|-----|--------|
| Screenshot | `get_screenshot` | `qorvex screenshot` | `get_screenshot` |
| Screen info | `get_screen_info` | `qorvex screen-info` | `get_screen_info` |
| List elements | `list_elements` | — | `list_elements` |

## Values

| Syntax | Description |
|--------|-------------|
| REPL: `get_value(selector)` | Get element value by ID |
| REPL: `get_value(selector, label)` | Get by label |
| REPL: `get_value(selector, --no-wait)` | Without waiting |
| CLI: `qorvex get-value <selector>` | By ID |
| CLI: `qorvex get-value <selector> --label` | By label |
| CLI: `qorvex get-value <selector> --no-wait` | Without waiting |
| Script: `value = get_value("selector")` | Capture into variable |

## Watcher

| Command | REPL | Script |
|---------|------|--------|
| Start watcher | `start_watcher` or `start_watcher(interval_ms)` | `start_watcher` or `start_watcher(500)` |
| Stop watcher | `stop_watcher` | `stop_watcher` |

Default interval: 500ms. Detects both accessibility tree changes and visual changes (via perceptual hashing).

## Logging

| Command | REPL | CLI | Script |
|---------|------|-----|--------|
| Add comment | `log_comment(text)` | `qorvex comment "text"` | `log_comment("text")` or `log("text")` |

## CLI-Specific Options

- `-s, --session <name>` -- Connect to named session (default: "default", or `$QORVEX_SESSION`)
- `-f, --format <text|json>` -- Output format
- `-q, --quiet` -- Suppress non-essential output
- `tap`, `get-value`: `-l, --label`, `-T, --type <type>`, `--no-wait`, `-o, --timeout <ms>`
- `wait-for`: `-l, --label`, `-T, --type <type>`, `-o, --timeout <ms>` (default: 5000)

## Element Selectors

Selectors support glob matching:

- `*` matches any number of characters
- `?` matches exactly one character

Example: `tap(login-*)` matches `login-button`, `login-field`, etc.
