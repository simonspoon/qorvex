# Troubleshooting

## Agent Won't Start

**Symptoms:** `start-agent` hangs or fails, "Agent failed to become ready within timeout", "Agent process exited: exit code ..."

**Check:**

1. Is a simulator booted? Run `xcrun simctl list devices | grep Booted`
2. Is xcodegen installed? `which xcodegen` -- install via `brew install xcodegen`
3. Is the agent source configured? Check `~/.qorvex/config.json` for `agent_source_dir`. If not set, the server also checks for a Homebrew-installed agent at `HOMEBREW_PREFIX/share/qorvex/agent` (e.g., `/opt/homebrew/share/qorvex/agent`)
4. Is the agent port available? Default is 8080 (`lsof -i :8080`). Configurable via `"agent_port"` in `~/.qorvex/config.json`
5. Check Xcode build errors: run `make -C qorvex-agent build` manually to see full output

**"No agent source found" error:** This means neither `agent_source_dir` in config nor the Homebrew agent path exists. Install via `brew install simonspoon/tap/qorvex` or run `./install.sh` from the source directory.

**"Agent process exited" error:** The `xcodebuild test-without-building` process exited early before the agent became ready. The error message includes the last lines of stderr from xcodebuild. Common causes: missing build products (re-run `install.sh` or `make -C qorvex-agent build`), simulator not booted, or code signing errors.

**Common fixes:**

- Install via Homebrew: `brew install simonspoon/tap/qorvex`
- Re-run `./install.sh` to set up the agent source directory
- `start-agent /full/path/to/qorvex/qorvex-agent` -- provide path explicitly

**"Failed to start agent" from `start-session`:** The server now reports agent startup failures explicitly. Previously, `start-session` would silently succeed even when the agent failed to start. If you see this error, check the causes listed above.

**"Agent started but connection failed":** The agent process launched successfully but the server could not establish a TCP connection. Check that nothing else is using port 8080 and that the simulator is booted.

## Agent Won't Connect

**Symptoms:** "Not connected to automation backend", "Connection lost"

**Check:**

1. Is the agent process running? Look for `xcodebuild test-without-building` in Activity Monitor
2. Try stopping and restarting: `stop-agent` then `start-agent`
3. The agent binds to `127.0.0.1:8080` by default -- ensure nothing else is using that port. To change the port, add `"agent_port": 9090` to `~/.qorvex/config.json`

**Auto-recovery:**

For simulator sessions started via `start-session` or `start-agent` (managed agents), the server automatically recovers from connection drops. When a command fails with an I/O or connection error, the driver first tries a cheap TCP reconnect (the agent process may still be running — just slow on a large page), then falls back to a full kill-and-respawn only if the reconnect fails:

1. Open a fresh TCP connection and verify with a heartbeat
2. If the heartbeat succeeds, retry the command immediately — no process kill
3. If the reconnect fails: kill the old agent, respawn (without rebuilding), wait for it to become ready, re-send `set-target` if one was previously set (the fresh agent has no target state), then retry the command once

If you see a command succeed after a brief delay following a crash, recovery worked. If recovery itself fails, you'll see a message like `recovery: agent not ready: ...` — in that case, run `stop-agent` then `start-agent` manually.

Auto-recovery does **not** apply to `connect` (direct connect via `connect <host> <port>`) or physical device connections.

**Timeouts:**

- Connection timeout: 5 seconds
- Write timeout: 10 seconds — if the agent stops reading (e.g., blocked on a long operation), the write is aborted and the connection is dropped so the next command gets `NotConnected` immediately rather than blocking indefinitely
- Read timeout: 30 seconds (default) — if the agent doesn't respond within 30 seconds, the connection is closed to prevent response mismatches on subsequent commands. When `QORVEX_TIMEOUT` or `--timeout` is set, the read deadline is extended to `timeout + 15s` so long agent-side retries can complete (the 15s buffer accommodates XCTest queries that can stall for 5–10s during screen transitions)
- `dump_tree` / `get-screen-info` / `list-elements`: uses a 120s read deadline regardless of the default — apps with large accessibility trees can take well over 30s to snapshot
- Agent startup timeout: 30 seconds (3 retries); both timeout and early-exit failures trigger retries

If a read timeout occurs, the next command will report "Not connected". For managed agents, auto-recovery will first attempt a TCP reconnect (cheap; doesn't kill the agent), then fall back to a full respawn if the reconnect fails. For unmanaged agents, use `connect` in the REPL or restart the agent manually.

## Element Not Found

**Symptoms:** "Timeout waiting for element", tap/get-value fails

**Debug steps:**

1. `get-screen-info` -- inspect the current UI hierarchy
2. `list-elements` -- see all elements with identifiers or labels
3. Check the exact identifier/label: IDs are case-sensitive
4. Try glob matching: `tap login-*` to match partial IDs
5. Is the element in a different app? Use `set-target <bundle_id>` to switch
6. **`get-value` with a dynamic label (e.g., "Tapped: 3"):** Label matching requires an exact match — `get-value -l "Tapped:"` won't find `"Tapped: 3"`. Use `screen-info` and grep for the current text, or use the element's accessibility ID instead.
7. **Tapping a label doesn't focus the input field:** Tapping a `StaticText` element (e.g., the "Password" label above a text field) does not focus the associated field. Tap the field itself by its accessibility ID — use `screen-info` to find it (look for `TextField` or `SecureTextField` type).

## Element Not Hittable

**Symptoms:** "element exists but is not hittable" timeout from `tap` or `wait-for`

**What this means:** The element exists in the accessibility tree but iOS reports it as not tappable. Common causes:

- Element is behind another view (covered/overlapped)
- Element is off-screen (need to scroll first)
- Element is disabled
- Element is still animating into position

**Fixes:**

- `swipe up` or `swipe down` to scroll the element into view
- Wait longer: increase `--timeout` (default: 5000ms)
- `wait-for` additionally requires 3 stable frames (300ms of no movement), so animations must complete before it returns success

## Tap Times Out With Ongoing Animations

**Symptoms:** `tap` hangs for the full timeout duration (default 5s) when the app is showing a loading spinner or other continuous animation, even though the target element is visible and hittable in `get-screen-info`.

**Cause:** XCUITest's internal quiescence wait activates during the tap even when it has been disabled. Ongoing animations (e.g., `ProgressView`, repeating `.animation` modifiers) anywhere in the app cause XCUITest to block the tap until the animation settles — which never happens for indefinite spinners.

**The agent handles this automatically** using coordinate-based tapping (`XCUICoordinate.tap()`) rather than element-based tapping, which bypasses the quiescence pathway. If you are on an older agent build, rebuild with `make -C qorvex-agent build`.

## Physical Device Issues

### Device Not Found

**Symptoms:** `list-physical-devices` returns empty, `use-device` says "not found"

**Check:**
1. USB: Is the device connected? `xcrun xctrace list devices`
2. USB: Is usbmuxd running? `launchctl list | grep usbmuxd`
3. USB: Has the device been trusted? Check for the "Trust This Computer" dialog
4. WiFi: Is developer mode enabled? Settings → Privacy & Security → Developer Mode
5. WiFi: Is the device on the same network? `ping <DeviceName>.local`
6. Both: Run `qorvex list-physical-devices` — WiFi devices are discovered via CoreDevice (`xcrun devicectl`); USB devices via usbmuxd
7. **Device name shows as "Unknown":** `list-physical-devices` uses usbmuxd which may not have the human-readable name. To confirm the device name, run `xcrun devicectl list devices` instead.

### Agent Startup Timeout on Physical Device After Fresh Install

**Symptoms:** `start-agent` times out on a physical device immediately after running `install.sh` on a new machine, even though it works on the original machine.

**Cause:** Older `install.sh` versions only pre-built the agent for simulator. The lifecycle manager detected the simulator `.xctestrun`, skipped the build, then silently failed to install on the physical device. Current `install.sh` builds for both platforms — re-running it fixes this.

**Fix:** Re-run `./install.sh` from the qorvex source directory to build the physical device agent bundle.

### "Unlock X to Continue" / Agent Startup Timeout

**Symptoms:** `start-agent` fails with "Agent failed to become ready within timeout", or xcodebuild prints "Unlock Hillbilly to Continue"

**Cause:** The device is locked. xcodebuild waits indefinitely for unlock, but `start-agent` times out at 120s.

**Fix:** Unlock the device and retry `start-agent`. Keep the device unlocked during the entire session.

### LaunchServicesDataMismatch

**Symptoms:** `start-agent` fails with "LaunchServices GUID and sequence number do not match expected values"

**Cause:** iOS launch services cache is stale, often after a fresh deploy.

**Fix:** Retry `start-agent` — the second attempt almost always succeeds. If it persists, delete the test runner app from the device (via Settings → General → iPhone Storage) and retry.

### screen-info Hangs on Physical Device

**Symptoms:** `screen-info` command hangs for minutes or is killed with timeout when the home screen is visible.

**Cause:** The home screen (SpringBoard) has thousands of accessibility elements. Querying it over a network connection can take minutes.

**Fix:** Always launch your target app before calling `screen-info`:
```bash
xcrun devicectl device process launch --device <udid> com.example.myapp
qorvex screen-info   # Fast — only queries the app's element tree
```

### USB Tunnel Issues (USB-connected devices)

**Symptoms:** "USB tunnel error", device not found on USB

**Check:**

1. Is the device connected via USB? `idevice list` or `xcrun xctrace list devices`
2. Is developer mode enabled on the device?
3. Has the device been trusted? Check for the "Trust This Computer" dialog
4. Is usbmuxd running? `launchctl list | grep usbmuxd`

## Script Errors

### Parse Error (exit code 2)

```
Parse error at line 5: unexpected token
```

Check syntax: matching braces, quoted strings, correct keywords.

### Runtime Error (exit code 3)

```
Runtime error at line 10: Undefined variable: user
```

Variables must be assigned before use. Check for typos in variable names.

```
Runtime error at line 15: Circular include detected
```

Script A includes B which includes A. Remove the circular dependency.

```
Runtime error at line 8: Unknown setting: delay
```

Only `timeout` is a valid `set` directive.

### Action Failed (exit code 1)

```
Action failed at line 12: Timeout after 5000ms waiting for element 'submit-btn'
```

The element didn't appear within the timeout. Increase timeout with `set-timeout 10000` or `wait-for selector --timeout 10000`.

## IPC Connection Issues

**Symptoms:** qorvex-cli or qorvex-live can't connect

**Check:**

1. Is a REPL session running? The CLI and Live TUI connect via IPC to the REPL
2. Check socket file: `ls ~/.qorvex/qorvex_*.sock`
3. Wrong session name? Use `-s session-name` to specify, or set `$QORVEX_SESSION`
4. List active sessions: `qorvex list-sessions`

## Stale Socket Files

If a session crashes, the socket file may be left behind:

```bash
rm ~/.qorvex/qorvex_default.sock
```

The server cleans up old sockets on startup, but manual removal may be needed after a crash.

## Performance

- **Tab completion elements:** The REPL fetches live UI elements on demand when you type a command that requires an element selector (e.g., `tap `). Elements are cached per command and refetched when the command changes or after a submit.
