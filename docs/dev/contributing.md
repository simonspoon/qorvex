# Contributing Guide

This document covers common development workflows in the qorvex codebase: adding new actions, extending the protocol, testing, and recurring patterns.

**Source:** All crates.

---

## Build & Run Commands

### Building

```bash
cargo build                # all crates (debug)
cargo build --release      # all crates (release)
./install.sh               # install all Rust binaries, build agent + streamer, record agent source dir
```

### Individual Installs

```bash
cargo install --path crates/qorvex-server
cargo install --path crates/qorvex-repl
cargo install --path crates/qorvex-live
cargo install --path crates/qorvex-cli
```

### Running from Source

```bash
cargo run -p qorvex-server -- -s default                         # server
cargo run -p qorvex-repl                                          # REPL (auto-launches server)
cargo run -p qorvex-live                                          # live TUI
cargo run -p qorvex-live -- --fps 30                              # live TUI, higher frame rate
cargo run -p qorvex-live -- --no-streamer                         # live TUI, polling fallback
cargo run -p qorvex-cli -- start                                  # CLI: start server + session
cargo run -p qorvex-cli -- tap button-id                          # CLI: tap element
cargo run -p qorvex-cli -- stop                                   # CLI: stop session
cargo run -p qorvex-cli -- list-devices                           # CLI: no session needed
cargo run -p qorvex-cli -- boot-device <udid>                     # CLI: no session needed
cargo run -p qorvex-cli -- convert log.jsonl                      # CLI: no session needed
```

### Batch Modes

```bash
echo -e "help\nquit" | cargo run -p qorvex-repl -- --batch -s <session>   # REPL batch
cargo run -p qorvex-live -- --batch -s <session> --duration 5              # live TUI batch (JSONL)
```

### Swift Components

```bash
make -C qorvex-agent build       # build agent (requires Xcode)
make -C qorvex-streamer build    # build streamer (macOS 13+)
make -C qorvex-testapp build     # build test app (requires Xcode + xcodegen)
make -C qorvex-testapp install   # install test app on booted Simulator
make -C qorvex-testapp run       # install + launch test app
qorvex-streamer --udid <UDID> --fps 30 --socket-path /tmp/qvx-stream.sock   # run streamer standalone
```

---

## Adding a New Action: The Ripple

Adding a new action type touches approximately 10 files across Rust and Swift. Follow this order to ensure nothing is missed:

| Step | File | What to do |
|------|------|------------|
| 1 | `crates/qorvex-core/src/action.rs` | Add variant to `ActionType` enum |
| 2 | `crates/qorvex-core/src/protocol.rs` | Add `OpCode` variant, implement encode/decode for new `Request` variant |
| 3 | `qorvex-agent/Sources/Protocol.swift` | Add matching `OpCode` case, implement `decodeRequest` / `encodeResponse` |
| 4 | `qorvex-agent/Sources/CommandHandler.swift` | Add handler method, wire up in `handle(_ request:)` switch |
| 5 | `crates/qorvex-core/src/driver.rs` | Add method to `AutomationDriver` trait (with or without default impl) |
| 6 | `crates/qorvex-core/src/agent_driver.rs` | Implement the new method on `AgentDriver` |
| 7 | `crates/qorvex-core/src/executor.rs` | Add execution branch in `execute_inner` for the new `ActionType` |
| 8 | `crates/qorvex-repl/src/app.rs` | Add command parsing and dispatch |
| 9 | `crates/qorvex-repl/src/completion/` | Add tab completion definition for the new command |
| 10 | `crates/qorvex-cli/src/main.rs` | Add CLI subcommand and dispatch |
| 11 | `crates/qorvex-cli/src/converter.rs` | Add `action_to_command` mapping for JSONL log conversion |

Steps 1-4 define the action from protocol to agent. Steps 5-7 wire it through the Rust driver and executor. Steps 8-9 make it available in the REPL. Steps 10-11 expose it in the CLI and log converter.

---

## Adding a New OpCode

Protocol changes must be symmetric between Rust and Swift. Both sides must agree on opcode numbers, request/response shapes, and encoding order.

1. **Add numeric value** to the `OpCode` enum in both:
   - `crates/qorvex-core/src/protocol.rs` (Rust)
   - `qorvex-agent/Sources/Protocol.swift` (Swift)

2. **Add request variant** on both sides:
   - `Request` enum in `protocol.rs`
   - `AgentRequest` enum in `Protocol.swift`

3. **Implement encode/decode** on both sides:
   - `encode_request` / `decode_request` in Rust
   - `decodeRequest` / `encodeResponse` in Swift

4. **Payloads must match exactly:** same field order, same types, same encoding. The wire format is little-endian with a 4-byte length header. Mismatches will cause silent corruption or panics.

---

## Testing Guide

### Unit Tests

```bash
cargo test                              # all crates
cargo test -p qorvex-core               # core only
cargo test -p qorvex-cli                # cli only
cargo test -p qorvex-repl               # repl command parsing and arg tests
```

### Integration Tests

```bash
cargo test -p qorvex-core --test ipc_integration      # IPC server/client
cargo test -p qorvex-core --test driver_integration    # driver abstraction
cargo test -p qorvex-core --test e2e_pipeline          # full-stack pipeline
cargo test -p qorvex-core --test error_recovery        # disconnect/timeout/corruption
cargo test -p qorvex-cli  --test cli_integration       # CLI binary behavior
```

**`ipc_integration` verifies:**
- Server startup and socket creation at the expected path
- Client connection and request/response flow
- Event subscription and broadcasting to subscribers

**`driver_integration` verifies:**
- Driver connection and basic commands
- Element search and tree dump
- Error handling for missing elements and invalid commands

**`e2e_pipeline` verifies:**
- Full path from IPC client → IPC server → Session → ActionExecutor → mock TCP agent and back
- Action logging after IPC execution and session event broadcasting

**`error_recovery` verifies:**
- Agent connection drops mid-session (graceful failure, no panic)
- Agent hangs trigger the read timeout (~30 s) and surface as an error
- Garbage bytes from the agent produce a protocol error
- Normal operation after delayed agent responses
- `wait-for-not` propagates connection errors as failures (not "element gone")

**`cli_integration` verifies:**
- `qorvex --help`, `qorvex convert`, `qorvex list-devices`, and unknown subcommand exit codes and output

### Shared Test Infrastructure

`crates/qorvex-core/tests/common/mod.rs` provides helpers shared across integration test suites:

- `mock_agent(responses)` / `connected_executor(responses)` — simple mock TCP agent with canned responses
- `unique_session_name()` — UUID-based session name for test isolation
- `MockBehavior` enum + `programmable_mock_agent(behaviors)` — scriptable mock that can simulate delays, connection drops, garbage bytes, or hangs
- `TestHarness::start(responses)` — full-stack fixture: Session + ActionExecutor + mock agent + IPC server in one call

### Simulator Suite (Real Device)

`crates/qorvex-cli/tests/simulator_suite.rs` exercises the full stack against an actual iOS Simulator running `qorvex-testapp`. All 31 tests are `#[ignore]` by default.

**Prerequisites:**
1. Boot a simulator: `xcrun simctl boot <UDID>`
2. Install testapp: `make -C qorvex-testapp run`

**Run:**
```bash
cargo test -p qorvex-cli --test simulator_suite -- --ignored --test-threads=1
```

`--test-threads=1` is required — all tests share one simulator and one session via `OnceLock`.

The suite covers all five testapp tabs: Controls, Text Input, Navigation, Gestures, and Dynamic.

#### XCUITest gotchas (learned while building this suite)

**LaunchScreen is required for native resolution.** Without `INFOPLIST_KEY_UILaunchScreen_Generation: YES` in the app's build settings (or a LaunchScreen storyboard), iOS runs the app in 320×480 compatibility mode instead of native resolution. This invalidates all tap coordinates, scroll counts, and element visibility assumptions. The fix is in `qorvex-testapp/project.yml`.

**`tap` vs `screen-info` see different things.** `screen-info` uses `snapshot()` which captures the full accessibility tree including off-screen elements. `tap` (and `wait-for`) uses `.descendants().matching()` which only finds elements that are on-screen and hittable. An element visible in `screen-info` output may still fail to tap if it's outside the visible area or behind an overlay like the tab bar.

**The iOS keyboard covers the tab bar.** When a text field is focused, the keyboard slides up and overlaps the tab bar. Attempting to tap tab bar buttons by label fails because they're not hittable. Use `.scrollDismissesKeyboard(.immediately)` on the containing `ScrollView` and swipe down to dismiss the keyboard before navigating tabs. Tapping static text does not dismiss the keyboard.

**Elements near the tab bar need extra scroll.** The tab bar at ~y=791 makes elements within ~50pt above it non-hittable (they're overlapped). After scrolling to the bottom of a view, check element Y positions against the tab bar Y — if less than ~50pt of clearance, scroll one more time.

### Swift Agent Tests

```bash
make -C qorvex-agent build    # generate project via xcodegen, then xcodebuild build-for-testing
make -C qorvex-agent test     # run on booted simulator (auto-detected)
```

The Makefile auto-detects the booted simulator. Ensure at least one simulator is booted before running agent tests.

---

## Common Patterns

### Selector Triple

Many actions use the pattern `(selector: String, by_label: bool, element_type: Option<String>)`:

- `selector` -- the string to match against
- `by_label` -- when `true`, matches against the accessibility `label`; when `false`, matches against the accessibility `identifier`
- `element_type` -- optional element type filter (e.g., `"button"`, `"textField"`). When set, the driver uses `find_element_with_type` which filters by both type and selector

This triple appears in `ActionType::Tap`, `ActionType::WaitFor`, `ActionType::GetValue`, and others.

### `Arc<Session>`

Sessions are always `Arc`-wrapped and shared by reference across the executor, IPC server, and watcher. Use these methods to record actions:

- `session.log_action(action_log)` -- standard action logging
- `session.log_action_timed(action_log)` -- action logging with per-phase timing fields (`wait_ms`, `tap_ms`)

### Retry-on-Failure and `--no-wait`

`ActionType::Tap` and `ActionType::GetValue` carry a `timeout_ms: Option<u64>` field. When set, the executor sends the tap/get-value directly to the agent and retries on transient errors ("not found", "not hittable") every 100ms until the timeout elapses. When `None`, the action is attempted once with no retry.

The `--no-wait` flag (CLI/REPL) sets `timeout_ms` to `None` — single attempt, immediate failure if the element isn't present. By default `timeout_ms` is `Some(5000)`.

The retry classification lives in `is_retryable_error()` in `executor.rs`. Only `DriverError::CommandFailed` messages containing "not found" or "not hittable" are retried; all other errors (connection loss, ObjC exceptions, unknown type) fail immediately.

### WaitFor Stability

`ActionType::WaitFor` has a `require_stable: bool` field that controls wait behavior:

- **`require_stable: true`** (used by explicit `wait_for` / `qorvex wait-for`): requires the element to be hittable and requires **3 consecutive polls** (at 100ms intervals) where the frame coordinates are identical before reporting success. Prevents tapping elements still animating into position.

- **`require_stable: false`**: returns as soon as the element exists and is hittable. Used when you want to wait-without-acting with a looser stability requirement.

### Poll-Loop Error Handling

When implementing a poll loop that calls a fallible driver method (e.g., `find_element_with_type`), always handle `Err` explicitly before checking the result value. A common pitfall is using `matches!` to test the happy-path condition:

```rust
// WRONG: Err(...) also evaluates to false — treated as "element absent"
let element_present = matches!(found, Ok(Some(ref el)) if el.hittable != Some(false));
```

Use a `match` instead so errors are surfaced as failures rather than silently misinterpreted:

```rust
// CORRECT
match found {
    Err(e) => return ExecutionResult::failure(format!("{}", e)),
    Ok(opt) => { /* check opt */ }
}
```

This matters most in `wait-for-not`, where a transient I/O error makes the element "appear absent" and the loop returns premature success.

### Error Handling in the Agent

Always wrap `XCUIElement` calls in `QVXTryCatch` to prevent `NSException` crashes. Stale element references, disappeared views, and timing issues all produce `NSException` rather than Swift errors. Without the Objective-C catch wrapper, any of these will terminate the agent process.

```swift
var error: NSError?
QVXTryCatch({
    element.tap()
}, &error)
if let error = error {
    // handle gracefully instead of crashing
}
```
