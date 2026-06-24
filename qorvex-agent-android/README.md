# qorvex-agent-android

A native Kotlin UiAutomator instrumentation agent that runs a TCP server on an
Android device/emulator and translates Qorvex binary-protocol commands into
UiAutomator / `AccessibilityNodeInfo` actions. It is the Android counterpart of
`qorvex-agent/` (the Swift XCTest agent) and speaks the **exact same** wire
protocol as `qorvex-core/src/protocol.rs`, so the existing Rust `AgentClient`
connects to it unchanged.

## Architecture

```
Rust host (qorvex-core / AgentClient)
    |
    | TCP 127.0.0.1:<local_port>  (via `adb forward`)
    | Binary protocol (4-byte LE len + opcode + LE payload, length-prefixed UTF-8)
    |
    v
Kotlin agent (UiAutomator instrumentation test, blocking ServerSocket)
    |
    | UiDevice / UiAutomation / AccessibilityNodeInfo
    |
    v
Android accessibility layer
```

The agent is packaged as an instrumentation test (`AndroidJUnitRunner`) so it has
UiAutomator access to the whole device UI (ADR-2). It is launched long-lived via:

```
adb shell am instrument -w \
  -e qorvex_port <device_port> \
  -e class com.qorvex.agent.QorvexAgentTest#runAgent \
  com.qorvex.agent.test/androidx.test.runner.AndroidJUnitRunner
```

`runAgent` opens a `ServerSocket` on `qorvex_port` (default 8080) bound to
localhost and blocks in a serve loop until the process is killed. `-w` keeps the
host `adb` call attached so the lifecycle (story #88) owns the process handle.

## Source layout

| File | Role | Source set |
|---|---|---|
| `Protocol.kt` | Wire codec — `OpCode`, `AgentRequest`/`AgentResponse`, `decodeRequest`/`encodeResponse`, framing | `main` |
| `UITreeSerializer.kt` | `UIElementJSON`/`FrameJSON` → frozen serde JSON keys | `main` |
| `NodeMapper.kt` | `AccessibilityNodeInfo` → `UIElementJSON` (ADR-1) + selector resolution | `androidTest` |
| `CommandHandler.kt` | Dispatches every `ActionType` to UiAutomator | `androidTest` |
| `AgentServer.kt` | Blocking TCP server + frame loop | `androidTest` |
| `QorvexAgentTest.kt` | Instrumentation entry point (`runAgent`) | `androidTest` |

Protocol + serializer live in `main` so the pure-JVM unit tests
(`src/test/.../ProtocolWireTest.kt`, `SerializerTest.kt`) can verify wire-format
parity and the frozen JSON keys without an emulator.

## Element mapping (ADR-1, frozen)

| UIElement field (JSON key) | Android source |
|---|---|
| `identifier` (`AXUniqueId`) | `viewIdResourceName` bare entry name |
| `label` (`AXLabel`) | `text` if non-empty, else `contentDescription` |
| `value` (`AXValue`) | `text` for editable nodes, else null |
| `element_type` (`type`) | short `className` (last `.`-segment) |
| `frame` | `getBoundsInScreen` → `{x=left, y=top, width, height}` |
| `role` | full `className` (FQCN, advisory) |
| `hittable` | `isClickable && isEnabled && isVisibleToUser` |
| `children` | recursive `getChild(i)` |

## Build & test

```bash
# JVM unit tests: wire-format parity + frozen JSON keys (no emulator needed)
./gradlew testDebugUnitTest

# Compile the instrumentation agent against the Android SDK + UiAutomator
./gradlew compileDebugAndroidTestKotlin

# Build the host + instrumentation APKs
./gradlew assembleDebug assembleDebugAndroidTest
```

Requires `ANDROID_HOME` / `local.properties` pointing at an Android SDK with
platform 34/35 and build-tools. CI wiring is story #91.

## Protocol commands

Every request opcode in `protocol.rs` is handled (Heartbeat, TapCoord,
TapElement, TapByLabel, TapWithType, TypeText, Swipe, GetValue, LongPress,
DumpTree, Screenshot, SetTarget, FindElement, GetTargetInfo). `WaitFor`/
`WaitForNot` are not protocol opcodes — the Rust executor implements them by
polling `find_element*`/`dump_tree`, so no agent command is needed.

Errors surface as `Response::Error` with messages that distinguish
element-not-found, timeout, and target-not-running (A4).
