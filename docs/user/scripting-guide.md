# Scripting Guide

Qorvex automation scripts are plain shell scripts that call `qorvex` CLI commands. This gives you the full power of bash (variables, loops, conditionals, functions) without learning a custom language.

## Converting Action Logs to Scripts

Record actions in the REPL, then convert the JSONL log to a shell script:

```bash
# From a log file
qorvex convert ~/.qorvex/logs/default_20250101_120000.jsonl > test-login.sh

# From the current session log (piped)
qorvex log -f json | qorvex convert > test-login.sh

# From stdin
qorvex convert < session.jsonl
```

The output is a valid bash script with `#!/usr/bin/env bash` and `set -euo pipefail`.

## Running Scripts

Scripts are standard bash — run them directly:

```bash
chmod +x test-login.sh
./test-login.sh

# Or target a specific session
QORVEX_SESSION=my-session ./test-login.sh
```

## Writing Scripts by Hand

Use `qorvex start` to launch the server and session in one step, and `qorvex stop` (via a `trap`) to clean up on exit or error:

```bash
#!/usr/bin/env bash
set -euo pipefail
export QORVEX_SESSION=my_test   # applies to both qorvex and qorvex-server

qorvex start                    # spawns server, waits for socket, starts session
trap 'qorvex stop || true' EXIT # clean up on exit or error

qorvex set-target com.example.myapp

# Login
qorvex tap username-field
qorvex send-keys 'test@example.com'
qorvex tap password-field
qorvex send-keys 'password123'
qorvex tap login-button

# Wait and verify
qorvex wait-for welcome-label -o 10000
value=$(qorvex get-value welcome-label)

if [ "$value" = "Welcome!" ]; then
    qorvex comment "Login test passed"
else
    echo "Login test failed: got $value" >&2
    exit 1
fi

# Navigate tabs
for tab in home settings profile; do
    qorvex tap "$tab"
    qorvex screenshot > "/tmp/${tab}.b64"
done
```

`qorvex start` is idempotent — if the server is already running for the session, it skips spawning and just starts the session.

## Available Commands

| Command | Description |
|---------|-------------|
| `qorvex start` | Start server + session in one step (use at top of script) |
| `qorvex stop` | Stop the server cleanly (use in `trap`) |
| `qorvex start-session` | Start session only (server must be running) |
| `qorvex start-agent [--project-dir <path>]` | Start automation agent explicitly |
| `qorvex tap <selector>` | Tap by accessibility ID |
| `qorvex tap <selector> --label` | Tap by label |
| `qorvex tap <selector> -T Button` | Tap with type filter |
| `qorvex tap-location <x> <y>` | Tap at coordinates |
| `qorvex swipe <direction>` | Swipe up/down/left/right |
| `qorvex send-keys 'text'` | Type text |
| `qorvex screenshot` | Capture screenshot (base64) |
| `qorvex screen-info` | Get UI elements |
| `qorvex get-value <selector>` | Get element value |
| `qorvex wait-for <selector> -o <ms>` | Wait for element |
| `qorvex wait-for-not <selector> -o <ms>` | Wait for element to disappear |
| `qorvex set-target <bundle_id>` | Set target app |
| `qorvex comment 'text'` | Log a comment |
| `qorvex boot-device <udid>` | Boot a simulator |
| `qorvex list-devices` | List simulator devices |
| `qorvex convert <log.jsonl>` | Convert log to script |

See [commands.md](commands.md) for full option details.

## Tips

- Use `set -euo pipefail` so the script stops on the first failed command.
- Use `export QORVEX_SESSION=<name>` at the top — both `qorvex` and `qorvex-server` respect it, so you never need `-s` on individual commands.
- Use `trap 'qorvex stop || true' EXIT` immediately after `qorvex start` so the server is always stopped, even on error. The `|| true` prevents the trap itself from masking the script's exit code when the server is already gone.
- Use `QORVEX_TIMEOUT` to set a default timeout (ms) for all wait/tap operations without passing `-o` on every command.
- Capture command output with `$(...)` — e.g., `value=$(qorvex get-value field-id)`.
- Use `qorvex -f json` for machine-readable output in pipelines.
- Status messages go to stderr in pipe-delimited format: `|timestamp|Action|target|elapsed_ms|`. Data (screenshots, element values) goes to stdout. Use `-q` to suppress status messages.

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Action failed (element not found, tap failed, etc.) |
| 2 | Connection error (no running REPL session) |
| 3 | Protocol error |
