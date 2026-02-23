# Command Reference

Commands are available across two interfaces: the REPL (interactive) and CLI (scriptable). This reference covers both.

## Session Management

| Command | REPL | CLI |
|---------|------|-----|
| Start server + session (one step) | — | `qorvex start` |
| Start session | `start-session` | `qorvex start-session` |
| End session | `end-session` | — |
| Stop server | — | `qorvex stop` |
| Session info | `get-session-info` | `qorvex status` |
| Get action log | — | `qorvex log` |
| List sessions | — | `qorvex list-sessions` |

## Device Management

| Command | REPL | CLI |
|---------|------|-----|
| List devices | `list-devices` | `qorvex list-devices` |
| Select device | `use-device <udid>` | — |
| Boot + select | `boot-device <udid>` | `qorvex boot-device <udid>` |

## Agent Management

| Command | REPL | CLI |
|---------|------|-----|
| Start agent | `start-agent` or `start-agent <path>` | `qorvex start-agent [--project-dir <path>]` |
| Stop agent | `stop-agent` | — |
| Set target app | `set-target <bundle_id>` | `qorvex set-target <bundle_id>` |
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

Tap retry behavior (unless `--no-wait`): polls every 50ms on the agent side. On each poll, the element must be found and hittable, and its frame must be stable across 2 consecutive polls before the tap fires. This makes tap animation-aware — tapping immediately after a modal transition works without manual sleeps. Fails with timeout if the element never becomes tappable and stable. Use explicit `wait-for` if you need to assert stability before chaining other operations.

### Tap at Coordinates

| Syntax | Description |
|--------|-------------|
| `tap-location <x> <y>` | Tap at screen coordinates (REPL and CLI) |

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

## Watcher

| Command | REPL |
|---------|------|
| Start watcher | `start-watcher` or `start-watcher <ms>` |
| Stop watcher | `stop-watcher` |

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
| Add comment | `log-comment <text>` | `qorvex comment "text"` |

## CLI-Specific Options

- `-s, --session <name>` -- Connect to named session (default: "default", or `$QORVEX_SESSION`)
- `-f, --format <text|json>` -- Output format
- `-q, --quiet` -- Suppress non-essential output
- `tap`, `get-value`: `-l, --label`, `-T, --type <type>`, `--no-wait`, `-o, --timeout <ms>`
- `wait-for`, `wait-for-not`: `-l, --label`, `-T, --type <type>`, `-o, --timeout <ms>` (default: 5000)

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `QORVEX_SESSION` | `default` | Session name — respected by both `qorvex` (CLI) and `qorvex-server`. Set once at the top of a script to avoid passing `-s` on every command. |
| `QORVEX_TIMEOUT` | `5000` | Default timeout in milliseconds for `tap`, `get-value`, `wait-for`, `wait-for-not`. Overridden by `-o` / `--timeout`. |

## Element Selectors

Selectors support glob matching:

- `*` matches any number of characters
- `?` matches exactly one character

Example: `tap login-*` matches `login-button`, `login-field`, etc.
