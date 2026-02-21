# Contributing Guide

This document covers common development workflows in the qorvex codebase: adding new actions, extending the protocol, testing, and recurring patterns.

**Source:** All crates.

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

**`cli_integration` verifies:**
- `qorvex --help`, `qorvex convert`, `qorvex list-devices`, and unknown subcommand exit codes and output

### Shared Test Infrastructure

`crates/qorvex-core/tests/common/mod.rs` provides helpers shared across integration test suites:

- `mock_agent(responses)` / `connected_executor(responses)` — simple mock TCP agent with canned responses
- `unique_session_name()` — UUID-based session name for test isolation
- `MockBehavior` enum + `programmable_mock_agent(behaviors)` — scriptable mock that can simulate delays, connection drops, garbage bytes, or hangs
- `TestHarness::start(responses)` — full-stack fixture: Session + ActionExecutor + mock agent + IPC server in one call

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

### Auto-Wait and `--no-wait`

By default, `tap` and `get_value` wait for the target element to appear before acting. This sends `ActionType::WaitFor` with `require_stable: false` — it polls until the element exists and is hittable, then proceeds. This saves at least 200ms compared to the stable-frames path.

The `--no-wait` flag (CLI/REPL) skips the wait entirely and attempts the action immediately. Use this when you are certain the element is already present, or when testing error handling for missing elements.

### WaitFor Stability

`ActionType::WaitFor` has a `require_stable: bool` field that controls wait behavior:

- **`require_stable: true`** (used by explicit `wait_for` / `qorvex wait-for`): requires the element to be hittable and requires **3 consecutive polls** (at 100ms intervals) where the frame coordinates are identical before reporting success. Prevents tapping elements still animating into position.

- **`require_stable: false`** (used by `tap` and `get_value` auto-wait): returns as soon as the element exists and is hittable. Faster than stable mode; no frame-stability tracking.

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
