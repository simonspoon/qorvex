# Troubleshooting

## Agent Won't Start

**Symptoms:** `start-agent` hangs or fails, "Agent failed to become ready within timeout"

**Check:**

1. Is a simulator booted? Run `xcrun simctl list devices | grep Booted`
2. Is xcodegen installed? `which xcodegen` -- install via `brew install xcodegen`
3. Is the agent source configured? Check `~/.qorvex/config.json` for `agent_source_dir`
4. Is port 8080 available? `lsof -i :8080` -- kill any existing process
5. Check Xcode build errors: run `make -C qorvex-agent build` manually to see full output

**Common fixes:**

- Re-run `./install.sh` to set up the agent source directory
- `start-agent /full/path/to/qorvex/qorvex-agent` -- provide path explicitly

## Agent Won't Connect

**Symptoms:** "Not connected to automation backend", "Connection lost"

**Check:**

1. Is the agent process running? Look for `xcodebuild test-without-building` in Activity Monitor
2. Try stopping and restarting: `stop-agent` then `start-agent`
3. The agent binds to `127.0.0.1:8080` -- ensure nothing else is using that port

**Auto-recovery:**

For simulator sessions started via `start-session` or `start-agent` (managed agents), the server automatically recovers from connection drops. When a command fails with an I/O or connection error, the driver first tries a cheap TCP reconnect (the agent process may still be running — just slow on a large page), then falls back to a full kill-and-respawn only if the reconnect fails:

1. Open a fresh TCP connection and verify with a heartbeat
2. If the heartbeat succeeds, retry the command immediately — no process kill
3. If the reconnect fails: kill the old agent, respawn (without rebuilding), wait for it to become ready, re-send `set-target` if one was previously set (the fresh agent has no target state), then retry the command once

If you see a command succeed after a brief delay following a crash, recovery worked. If recovery itself fails, you'll see a message like `recovery: agent not ready: ...` — in that case, run `stop-agent` then `start-agent` manually.

Auto-recovery does **not** apply to `connect` (direct connect via `connect <host> <port>`) or physical device connections.

**Timeouts:**

- Connection timeout: 5 seconds
- Read timeout: 30 seconds (default) — if the agent doesn't respond within 30 seconds, the connection is closed to prevent response mismatches on subsequent commands. When `QORVEX_TIMEOUT` or `--timeout` is set, the read deadline is extended to `timeout + 5s` so long agent-side retries can complete
- `dump_tree` / `get-screen-info` / `list-elements`: uses a 120s read deadline regardless of the default — apps with large accessibility trees can take well over 30s to snapshot
- Agent startup timeout: 30 seconds (3 retries)

If a read timeout occurs, the next command will report "Not connected". For managed agents, auto-recovery will first attempt a TCP reconnect (cheap; doesn't kill the agent), then fall back to a full respawn if the reconnect fails. For unmanaged agents, use `connect` in the REPL or restart the agent manually.

## Element Not Found

**Symptoms:** "Timeout waiting for element", tap/get-value fails

**Debug steps:**

1. `get-screen-info` -- inspect the current UI hierarchy
2. `list-elements` -- see all elements with identifiers or labels
3. Check the exact identifier/label: IDs are case-sensitive
4. Try glob matching: `tap login-*` to match partial IDs
5. Is the element in a different app? Use `set-target <bundle_id>` to switch

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

## USB Tunnel Issues (Physical Devices)

**Symptoms:** "USB tunnel error", device not found

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

- **Watcher polling:** Default 1000ms. Lower values increase CPU usage. Set via `start-watcher <ms>`.
