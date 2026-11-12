# Command Reference

Commands are available across two interfaces: the REPL (interactive) and CLI (scriptable). This reference covers both.

## Session Management

| Command | REPL | CLI |
|---------|------|-----|
| Start server + session (one step) | — | `qorvex start [--device <udid>] [--wait-ready]` |
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
| Lock the device (iOS only for now) | `lock` | `qorvex lock` |

### Android setup check (`doctor`)

`qorvex doctor [--platform android|ios]` is a read-only preflight for Android
(the default). It runs entirely in the CLI (no server is contacted or started)
and prints one line per check: `[ok] name: detail`, `[FAIL] name: problem. Fix: action`,
`[warn] ...` or `[skip] ...`. Checks: `adb` (on PATH), `ANDROID_HOME`
(or `ANDROID_SDK_ROOT`, config `android_sdk_root`, `local.properties` `sdk.dir`),
`JDK` (the one `start-agent` pins for Gradle, Java 17-23), `agent project`
(a directory containing `gradlew`), `devices` (`adb devices -l`; `unauthorized`
and `offline` fail) and `server` (`qorvex-server` on PATH). Checks needing adb
print `[skip]` when adb is missing. Exit code is 1 if any check failed, else 0.
`qorvex -f json doctor` prints `{"ok":bool,"checks":[{"name","status","detail","fix"}]}`.
`--platform ios` has no checks yet: it prints a notice and exits 0.

### Simulator leases and disk reclamation

These commands need no running session. A lease is a throwaway simulator cloned
from a shut-down golden device (`qorvex-golden-<model>`, created on first use and
never booted) and named `qorvex-lease-<id>`. Leases live in `~/.qorvex/leases.db`
(`$QORVEX_HOME`).

| Command | What it does |
|---------|--------------|
| `qorvex lease --model "iPhone Air" --owner <name> [--runtime <id>] [--ttl 4h]` | Clone, boot and wait; prints the UDID, then `QORVEX_DEVICE=<udid>` |
| `qorvex lease heartbeat <udid>` | Reset the lease's heartbeat |
| `qorvex lease list` | Leases with age, heartbeat age and expiry (`-f json` for JSON) |
| `qorvex release <udid>` / `--owner <name>` | Shut down and delete the clone. Idempotent; refuses names not starting `qorvex-lease-` |
| `qorvex reap [--dry-run]` | Release leases past their TTL; delete `qorvex-lease-*` devices with no lease row |
| `qorvex sims reclaim [--older-than 14d] [--yes]` | Disk reclamation for any machine. Dry run by default |

`reap` leaves an orphan alone if it is booted or was touched in the last ten
minutes. `sims reclaim` lists each candidate with its on-disk size and a total:
unavailable simulators, expired/orphan leases, and shut-down simulators idle for
`--older-than`. It never touches a booted simulator or a golden device, reports
the `CoreSimulator/Caches` size without deleting it, and points at
`xcrun simctl runtime list` for unused runtimes.

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

> **Target state:** `qorvex --format json target-info` reports `state` as one of
> `running_foreground`, `running_background`, `suspended` (iOS only),
> `not_running` or `unknown` (iOS only). Android reports `running_background`
> when the app has a live process but another app is in front.

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
| `tap <selector> --if-present` | Tap only if the element is in the tree right now; otherwise print `'<selector>' not present, skipped` to stderr and exit 0. One lookup, then a single tap attempt like `--no-wait` (same errors and exit codes). The lookup and the tap are two requests, so the element can vanish in between. Accepted together with `--no-wait` |
| `tap <selector> --timeout 10000` | Tap with custom timeout |
| `tap <selector> --fallback-frame` | If the normal tap fails (after its timeout) but the element is in the tree with a non-empty frame, tap the frame centre once (`tap-location` at x + width/2, y + height/2, rounded, clamped to >= 0). Exit 0 on success. Not found, zero-size frame, target app not in the foreground, or the fallback tap erroring: exit 1 with the original error plus a `fallback frame ...` note. The centre may land on a view covering the element; that is your choice with this flag (no occlusion check). Worst case: timeout + one target check + one lookup + one tap; `--no-wait --fallback-frame` is the fast form. Accepted with `--if-present` (probe first, then the tap with fallback) |
| `tap <selector> --report` | Also report whether the UI changed: dumps the tree before the tap and after it settles (equal on two polls 100 ms apart, capped at 1000 ms), adds up to about 1.2 s. Always on with `-f json`. Text output appends `ui_changed:<true\|false\|null>\|` to the trace line |

Same syntax for both REPL and CLI (prefix CLI commands with `qorvex`). `--fallback-frame` and `--report` are CLI only.

Tap result (`-f json` `data`): `{"elapsed_ms": N, "method": "element"|"frame_centre", "x": X, "y": Y, "ui_changed": true|false|null}`. `x`,`y` are the point tapped: for `element`, the centre of the element's frame looked up just after the tap (`null` if it has no frame or is gone, and `null` without `--report`/json since the lookup is skipped then); for `frame_centre`, the computed point. `ui_changed` is present only with `--report` or `-f json`; `null` when a tree dump failed (the tap result is not affected), `false` is information, not an error. Because `-f json` turns the report on, every `-f json tap` pays the two dumps (up to about 1.2 s); `elapsed_ms` times the tap itself, not the dumps. In text mode a frame-centre tap appends `fallback:frame_centre(x,y)|` to the trace line.

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
| `swipe` or `swipe <direction>` | Swipe (default: up in the REPL; the CLI requires a direction or `--from/--to`). Directions: up, down, left, right |
| `swipe --from <x,y> --to <x,y>` | Swipe between two coordinates (non-negative integers); cannot be combined with a direction (REPL and CLI) |

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

### Wait For the Screen to Settle

| Syntax | Description |
|--------|-------------|
| `wait-stable` | Wait until the whole UI tree is unchanged for 500 ms (timeout: `set-timeout` default / `-o`, initially 5s) |
| `wait-stable --for 300` | Custom quiet period in ms |
| `wait-stable --for 300 --timeout 2000` | Custom quiet period and timeout (CLI: `-o 2000`) |

Same syntax for both REPL and CLI (prefix CLI commands with `qorvex`). Takes no selector. Use it instead of a fixed `sleep` after a tap or navigation.

Wait behavior: dumps the full tree every 100 ms (measured from the end of the previous dump; dumps never overlap) and compares it with the previous dump: every element's id, label, value, type, role, hittable flag, frame and the tree order. Any difference restarts the quiet period, measured from the start of the dump that first saw the new tree; the first dump starts it, so `--for` equal to the timeout can succeed. Success as soon as the tree has been unchanged for `--for` ms: the CLI prints `|<time>|Stable|tree unchanged for <for>ms|<elapsed>ms|`, and `-f json` data is `{"elapsed_ms": N, "stable_ms": N, "element_count": N}` (`stable_ms` is the quiet period actually observed, `element_count` the number of elements in the final tree). `--for 0` succeeds after one successful dump. A failed dump restarts the quiet period and polling continues until the timeout.

On timeout it exits 1 with `Timeout after <ms>ms waiting for the UI to be unchanged for <for>ms (<N> tree changes seen)`, plus `; last error: ...` if a dump failed (data `{"elapsed_ms": N, "changes": N}`). A `--for` longer than the timeout is a usage error (exit 2) before anything is sent. A stable tree is only reported when the target app is up: after success the server checks the recorded target once, and a stopped or backgrounded app turns it into the usual `Target app '<bundle>' is not running; ...` failure (exit 1), because some agents still dump a static leftover tree with no app.

Comparison is exact, with no tolerance: frame values are compared as reported, so sub-pixel jitter counts as a change, and a screen with a blinking cursor, a running clock, a spinner or any animation never settles and always times out (by design). Use `wait-for` / `wait-for-not` to wait on one element on such screens.

Each dump uses the agent's normal 30 s read timeout, so a stalled agent can hold one poll past `-o`; the command then fails with that dump's error.

## Screen and Elements

| Command | REPL | CLI |
|---------|------|-----|
| Screenshot | `get-screenshot` | `qorvex screenshot` |
| Screen info | `get-screen-info` | `qorvex screen-info` |
| List elements | `list-elements` | — |

`qorvex screen-info` outputs actionable elements as concise JSON by default (no null fields, rounded frame values). Use `--full` to get the complete raw JSON, or `--pretty` for REPL-style formatted output. Default and `--full` write nothing to stderr, so `2>&1 | jq` works (use `| jq length` for the count); only `--pretty` prints `<N> elements` on stderr. With `-f json`, a failing `screen-info` prints `{"success": false, "message": "..."}` on stdout (stderr and exit code unchanged). `qorvex screen-info --all` lists every element in the agent's tree, flattened depth-first (parent before children), including elements with no id or label (they simply have no `id`/`label` keys). Each element also carries three flags: `offscreen` (its frame does not intersect the root frame, which is the first top-level element of the dump, the application/window; when the root has no frame or a zero-size one, nothing is marked offscreen), `zero_size` (width or height <= 0) and `hittable` (the agent's value, or `null` when the agent does not report it, which is every iOS element today). Elements without a frame get `offscreen: false` and `zero_size: false`. `--all --pretty` appends `[offscreen]`, `[zero-size]` and `[not-hittable]` markers and counts all elements on stderr; `--all` cannot be combined with `--full`. `--all` is subject to the agent's tree caps (iOS depth limit and 5000 elements), and needs a server from the same release: an older server closes the connection and the CLI exits 3 with a protocol error (default `screen-info` works across versions). `qorvex get-value` prints the element value to stdout. Status messages go to stderr in pipe-delimited format: `|timestamp|Action|target|elapsed_ms|` for all actions.

## Values

| Syntax | Description |
|--------|-------------|
| `get-value <selector>` | Get element value by ID |
| `get-value <selector> --label` | Get by label |
| `get-value <selector> --no-wait` | Without waiting |
| `exists <selector> [-l] [-T <type>]` | Is the element in the accessibility tree right now? One lookup, no waiting or retry |

`qorvex exists` prints `true`/`false` (with `-f json`: `{"exists":true,"hittable":<bool|null>}` or `{"exists":false}`). Exit code 0 = present, **4 = absent**; a lookup that could not be answered (target app not running, connection, protocol, driver error) keeps its usual exit code (1/2/3) and is never reported as absent. "Present" means in the agent's tree, not visible: hidden, offscreen and non-hittable elements count (`hittable` is the agent's value, `null` when the agent does not report it). Needs a server from the same release (an older server fails `exists` and `tap --if-present` only).

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
- `-o, --timeout <ms>` -- Accepted on every command, after the subcommand name. On `tap`, `get-value`, `wait-for`, `wait-for-not`, `wait-stable` it is the element timeout (default 5000, `$QORVEX_TIMEOUT`; unchanged, exit 1 on expiry). On every other command it is a hard wall-clock deadline for the whole process: default 600000 ms, `$QORVEX_COMMAND_TIMEOUT` overrides the default, the flag overrides both, `0` disables it. When it passes, the process prints `Error: Timed out after <ms>ms: <command>; the session server may still be working (check \`qorvex status\`)` and exits 124 (`start` also names the phase reached). The server is not stopped and its work is not cancelled. For these five wait commands the deadline is `max(600000 or $QORVEX_COMMAND_TIMEOUT, element timeout + 20000)`. A slow interactive signing build in `start -d <physical iPhone>` can exceed 600 s: pass `--timeout 0`.
- `start`: `-d, --device <udid>` -- Select a device (simulator or physical) before starting the session; equivalent to sending `use-device` then `start-session` in sequence. `start` recovers a dead session (removes the stale socket and spawns a new server, noting `Removed stale socket for dead session '<s>'`). It is idempotent: if the session is already ready (and `-d` is absent or names the session's device) it prints `Session already running (ready)` and exits 0 without creating a new session, so the session id and action log are kept; to clear the log, run `stop` then `start`. `--wait-ready` -- after starting, exit 1 with `session started but not ready: <reason>` unless the session is ready (single check, no polling).
- `status`: reports `session_id` and `has_screenshot`, plus readiness: text adds `Ready: yes` or `Ready: no (<reason>)`; `-f json` adds `ready`, `device`, `agent_connected`, `reason` (null when ready; otherwise `no device selected`, `no agent connected` or `agent not reachable: <error>`). Exit code is 0 whenever a session exists, ready or not. On a dead session it exits 2 with `session '<s>' is dead`.
- `list-sessions`: lists live sessions only; sockets of dead sessions (connection refused) are removed.
- `tap`, `get-value`: `-l, --label`, `-T, --type <type>`, `--no-wait`, `-o, --timeout <ms>`, `--tag <text>`
- `tap` only: `--if-present`, `--fallback-frame`, `--report`
- `wait-for`, `wait-for-not`: `-l, --label`, `-T, --type <type>`, `-o, --timeout <ms>` (default: 5000), `--tag <text>`
- `wait-stable`: `--for <ms>` (quiet period, default: 500), `-o, --timeout <ms>` (default: 5000), `--tag <text>`
- `memory-info`: `--tag <text>` — annotates the JSONL log entry; the measured report is written into the entry itself
- All action commands accept `--tag <text>` — annotates the JSONL log entry; replays as `--tag` in converted scripts

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `QORVEX_SESSION` | `default` | Session name — respected by both `qorvex` (CLI) and `qorvex-server`. Set once at the top of a script to avoid passing `-s` on every command. |
| `QORVEX_TIMEOUT` | `5000` | Default timeout in milliseconds for `tap`, `get-value`, `wait-for`, `wait-for-not`, `wait-stable`. Overridden by `-o` / `--timeout`. Does not apply to other commands. |
| `QORVEX_COMMAND_TIMEOUT` | `600000` | Hard deadline in milliseconds for the whole `qorvex` process (exit 124 when it passes). `0` disables it. Overridden by `--timeout` on every command except the five wait commands, where `--timeout` is the element timeout. |
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

### Label Text Match (`--label-contains`, `--label-regex`, `--nth`)

Only on `exists`, `wait-for` and `wait-for-not` (tap and get-value later). These flags replace the positional selector and match on the element **label** only:

| Flag | Matches when the label... |
|------|---------------------------|
| `--label-contains <text>` | contains `<text>` (case-sensitive substring; regex and glob characters are literal) |
| `--label-regex <pattern>` | matches `<pattern>` (Rust `regex` syntax, unanchored: use `^...$` for a whole-label match) |
| `--nth <N>` | with either flag above: take the Nth match, 0-based, in depth-first tree order (default: the first) |

```bash
qorvex exists --label-contains "Tap"
qorvex exists --label-regex '^Tapped: [0-9]+$' -T StaticText
qorvex wait-for --label-regex 'Tapped: 1'        # output includes the matched frame
qorvex wait-for-not --label-contains "Loading"
qorvex exists --label-contains "Row" --nth 2
```

- An element with no label or an empty label never matches. No match (or `--nth` beyond the number of matches) is "absent": `exists` exits 4, `wait-for` keeps polling until its timeout.
- `-T <type>` combines with both flags. `--label-contains` and `--label-regex` each conflict with the positional selector, with `-l` and with each other; `--nth` needs one of them (use `selector[N]` otherwise). An invalid regex is a usage error (exit 2) before anything is sent to the device.
- The match runs on a full tree dump each poll (like `screen-info`), not inside the agent, so it is somewhat slower per poll than an ID lookup. Needs a server from the same release.
