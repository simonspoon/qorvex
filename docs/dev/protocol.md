# Binary Protocol Reference

This document describes the binary wire protocol used for communication between the Rust crates (`qorvex-core`) and the Swift XCTest agent (`qorvex-agent`) over TCP.

## Source Files

| Side | Path |
|------|------|
| Rust | `crates/qorvex-core/src/protocol.rs` |
| Swift | `qorvex-agent/Sources/Protocol.swift` |

Both sides use a sequential reader pattern (`ProtocolCursor` in Rust, equivalent in Swift) for decoding.

## Wire Format

```
┌──────────────────────┬───────────┬─────────────────┐
│  4-byte LE u32       │  1-byte   │  payload bytes  │
│  length              │  opcode   │                 │
└──────────────────────┴───────────┴─────────────────┘
```

- `length` encodes the total byte count of `opcode + payload`. It does **not** include the 4-byte length header itself.
- All multi-byte integers are **little-endian**.

## Encoding Conventions

| Type | Wire Format |
|------|-------------|
| String | `[u32 LE byte_count][UTF-8 bytes]` |
| Optional String | `[u8 flag: 0=None, 1=Some][string if present]` |
| Optional u64 (trailing) | `[u8 flag: 0=None, 1=Some][u64 LE if present]` — only written/read when bytes remain |
| Bool | `[u8: 0=false, 1=true]` |
| Raw Bytes (screenshots) | `[u32 LE byte_count][raw bytes]` |
| i32 | 4 bytes little-endian |
| u64 | 8 bytes little-endian |
| f64 | 8 bytes little-endian (IEEE 754) |

## Request OpCodes

| Variant | Value | Payload | Description |
|---------|-------|---------|-------------|
| Heartbeat | `0x01` | (none) | Keep-alive ping |
| TapCoord | `0x02` | `i32 x`, `i32 y` | Tap at screen coordinates |
| TapElement | `0x03` | `String selector`, `Optional u64 timeout_ms` | Tap element by accessibility ID; agent retries locally when timeout_ms is set |
| TapByLabel | `0x04` | `String label`, `Optional u64 timeout_ms` | Tap element by accessibility label; agent retries locally when timeout_ms is set |
| TapWithType | `0x05` | `String selector`, `Bool by_label`, `String element_type`, `Optional u64 timeout_ms` | Tap element with type constraint; agent retries locally when timeout_ms is set |
| TypeText | `0x06` | `String text` | Type text into focused element |
| Swipe | `0x07` | `i32 start_x`, `i32 start_y`, `i32 end_x`, `i32 end_y`, `Bool has_duration`, `[f64 duration if true]` | Swipe gesture; velocity computed from distance/duration on agent side |
| GetValue | `0x08` | `String selector`, `Bool by_label`, `Optional String element_type`, `Optional u64 timeout_ms` | Get element value; agent retries locally when timeout_ms is set |
| LongPress | `0x09` | `i32 x`, `i32 y`, `f64 duration` | Long press at coordinates |
| DumpTree | `0x10` | (none) | Dump full accessibility hierarchy |
| Screenshot | `0x11` | (none) | Capture screenshot as PNG |
| SetTarget | `0x12` | `String bundle_id` | Switch target application |
| FindElement | `0x13` | `String selector`, `Bool by_label`, `Optional String element_type` | Find single element with live hittability |

### Special OpCodes (Agent-initiated)

| Variant | Value | Payload | Description |
|---------|-------|---------|-------------|
| Error | `0x99` | `String message` | Agent-initiated error, bypasses Response framing |
| Response | `0xA0` | `ResponseType byte` + type-specific payload | Standard response wrapper |

## Response Types

Response messages use opcode `0xA0` with a sub-discriminator byte immediately following the opcode:

| Type | Value | Payload | Description |
|------|-------|---------|-------------|
| Ok | `0x00` | (none) | Success, no data |
| Error | `0x01` | `String message` | Error with message |
| Tree | `0x02` | `String json` | Accessibility tree as JSON |
| Screenshot | `0x03` | `Raw Bytes data` | PNG screenshot bytes |
| Value | `0x04` | `Optional String value` | Element value (may be absent) |
| Element | `0x05` | `String json` | Single element as JSON |

### Bare Error (0x99)

The agent may also send a bare `Error` opcode (`0x99`) with a `String message`, bypassing the `Response`/`ResponseType` framing entirely. The Rust decoder handles this case and yields `Response::Error { message }`.

## Example: TapElement Encoding

To tap an element with identifier `"loginButton"` with no timeout:

```
Bytes:
  [17, 0, 0, 0]       # length = 17 (1 opcode + 4 string length + 11 string bytes + 1 timeout flag)
  [0x03]               # opcode: TapElement
  [11, 0, 0, 0]       # string byte count: 11
  "loginButton"        # UTF-8 bytes
  [0x00]               # timeout_ms flag: None
```

With a 5-second timeout (`timeout_ms = 5000`):

```
Bytes:
  [25, 0, 0, 0]       # length = 25 (1 + 4 + 11 + 1 flag + 8 u64)
  [0x03]               # opcode: TapElement
  [11, 0, 0, 0]       # string byte count: 11
  "loginButton"        # UTF-8 bytes
  [0x01]               # timeout_ms flag: Some
  [136, 19, 0, 0, 0, 0, 0, 0]  # 5000 as u64 LE
```

## Example: Response Decoding

A successful `Ok` response:

```
Bytes:
  [2, 0, 0, 0]        # length = 2 (opcode + response type)
  [0xA0]               # opcode: Response
  [0x00]               # response type: Ok
```

A `Value` response with value `"Hello"`:

```
Bytes:
  [12, 0, 0, 0]       # length = 12
  [0xA0]               # opcode: Response
  [0x04]               # response type: Value
  [0x01]               # Optional flag: Some
  [5, 0, 0, 0]        # string byte count: 5
  "Hello"              # UTF-8 bytes
```

## ProtocolError Variants (Rust)

The Rust protocol decoder returns `ProtocolError` on failure:

| Variant | Description |
|---------|-------------|
| `InvalidOpCode(u8)` | Unrecognized opcode byte |
| `InsufficientData` | Not enough bytes in buffer to complete decoding |
| `Utf8Error` | Invalid UTF-8 in a string field |
| `InvalidPayload(String)` | Payload structure doesn't match expected format for the opcode |

## Swipe Duration Encoding

The `Swipe` command uses a conditional encoding for the optional duration:

```
[i32 start_x][i32 start_y][i32 end_x][i32 end_y][u8 has_duration]
```

If `has_duration` is `1` (true), an additional `f64 duration` follows immediately. If `0` (false), no additional bytes are present.

## Agent-side Retry (timeout_ms)

Four opcodes (`0x03`, `0x04`, `0x05`, `0x08`) carry a trailing `Optional u64 timeout_ms` field. When this is `Some(N)`, the agent runs its own poll loop instead of returning an error immediately:

- The agent polls every 50ms until the element is found and hittable, or until `N` milliseconds have elapsed.
- Only "element not found" and "element not hittable" conditions are retried. ObjC exceptions (stale references, etc.) are returned immediately.
- When `timeout_ms` is `None`, the agent makes a single attempt and returns the result.

**Backwards compatibility:** The `Optional u64` uses a trailing encoding — the Rust encoder always writes the flag byte; the Swift decoder reads it only when bytes remain in the cursor. Old agents that don't read the field will silently ignore it (they stop reading after the fixed fields).

**Executor behaviour:** When `timeout_ms` is set, the Rust executor calls the `*_with_timeout` driver methods (a single TCP round-trip), rather than its own Rust-side retry loop. This eliminates per-attempt network overhead.

## FindElement vs DumpTree

`DumpTree` (`0x10`) returns the full accessibility hierarchy as a JSON tree. Element `hittable` fields in this response are **not reliable** because they come from accessibility snapshots.

`FindElement` (`0x13`) queries a single live `XCUIElement` and returns accurate `isHittable` status. This is used by `AgentDriver` overrides for `find_element`, `find_element_by_label`, and `find_element_with_type`.
