# Scripting Engine Internals

The `qorvex-auto` crate implements a small scripting language for automation scripts (`.qvx` files). This document covers the parser, AST, runtime, and executor internals.

**Source:** `crates/qorvex-auto/src/`

---

## AST Types

### Script

```rust
pub struct Script {
    pub statements: Vec<Statement>,
}
```

### Statement

```rust
enum Statement {
    Command { call: CommandCall, line: usize },
    Assignment { name: String, value: Expression, line: usize },
    Foreach { var: String, iter: Expression, body: Vec<Statement>, line: usize },
    For { var: String, from: Expression, to: Expression, body: Vec<Statement>, line: usize },
    If { condition: Expression, then_body: Vec<Statement>, else_body: Option<Vec<Statement>>, line: usize },
    Set { key: String, value: Expression, line: usize },
    Include { path: Expression, line: usize },
}
```

Every variant carries a `line: usize` for error reporting.

### CommandCall

```rust
pub struct CommandCall {
    pub name: String,
    pub args: Vec<Expression>,
    pub line: usize,
}
```

### Expression

```rust
enum Expression {
    String(String),
    Number(i64),
    Variable(String),
    List(Vec<Expression>),
    BinaryOp { op: BinOp, left: Box<Expression>, right: Box<Expression> },
    CommandCapture(CommandCall),  // var = get_value(...)
}
```

`CommandCapture` enables assignment from command output, e.g., `var = get_value("element-id")`.

### BinOp

```rust
enum BinOp { Add, Eq, NotEq }
```

---

## Parser: Two-Phase Approach

Public entry point:

```rust
pub fn parse(source: &str) -> Result<Script, AutoError>
```

### Phase 1 -- Tokenizer

Character-by-character scan producing `Located<Token>` values with line numbers.

**Token types:**

| Token | Description |
|-------|-------------|
| `Ident` | Identifier (command names, variable names) |
| `String` | Single- or double-quoted string literal |
| `InterpolatedString(Vec<StringSegment>)` | Double-quoted string containing `$varname` interpolation |
| `Number(i64)` | Integer literal |
| `LParen` / `RParen` | `(` / `)` |
| `LBrace` / `RBrace` | `{` / `}` |
| `LBracket` / `RBracket` | `[` / `]` |
| `Comma` | `,` |
| `Equals` | `=` |
| `DoubleEquals` | `==` |
| `NotEquals` | `!=` |
| `Plus` | `+` |
| `Newline` | Line terminator |

**Keywords:** `Foreach`, `In`, `For`, `From`, `To`, `If`, `Else`, `Set`, `Include`

**Key behaviors:**

- `#` starts a line comment. Everything from `#` to the next newline is skipped.
- **Double-quoted strings** support variable interpolation via `$varname` and escape sequences: `\n`, `\t`, `\\`, `\$`. Strings containing interpolation are emitted as `InterpolatedString`.
- **Single-quoted strings** have no interpolation and no escape processing.
- `InterpolatedString` segments are folded into a `BinaryOp::Add` chain during parsing (e.g., `"hello $name"` becomes `Add("hello ", Variable("name"))`).
- **Assignment detection** uses look-ahead: if the token stream matches `IDENT Equals`, it is parsed as an `Assignment`; otherwise as a `Command`.

### Phase 2 -- Recursive Descent Parser

Consumes the token stream produced by Phase 1 and builds the AST. Statement boundaries are newlines. Block bodies (for `foreach`, `for`, `if`/`else`) are delimited by `{` and `}`.

---

## Runtime

### Value Types

```rust
enum Value {
    String(String),
    Number(i64),
    List(Vec<Value>),
}
```

**Truthiness rules:**

| Type | Truthy when |
|------|-------------|
| `String` | Non-empty |
| `Number` | Non-zero |
| `List` | Non-empty |

**Cross-type equality:** When comparing `String == Number` (or vice versa), the number is converted to its `to_string()` representation and compared as strings.

**`BinOp::Add` behavior:**

| Left | Right | Result |
|------|-------|--------|
| `Number` | `Number` | Numeric addition |
| Any | Any | String concatenation (both sides converted via `to_string()`) |

### Runtime Struct

```rust
struct Runtime {
    variables: HashMap<String, Value>,
}
```

**Methods:**

| Method | Signature | Description |
|--------|-----------|-------------|
| `new` | `() -> Runtime` | Empty variable environment |
| `set` | `(&mut self, name: &str, value: Value)` | Set or overwrite a variable |
| `get` | `(&self, name: &str) -> Option<&Value>` | Look up a variable |
| `eval_expression` | `(&self, expr: &Expression, line: usize) -> Result<Value, AutoError>` | Evaluate an expression in the current environment |

---

## `ScriptExecutor`

### Fields

| Field | Type | Default |
|-------|------|---------|
| `runtime` | `Runtime` | Empty |
| `session` | `Arc<Session>` | Provided at construction |
| `executor` | `Option<ActionExecutor>` | Pre-connected if UDID given |
| `simulator_udid` | `Option<String>` | Provided at construction |
| `watcher_handle` | `Option<WatcherHandle>` | `None` |
| `default_timeout_ms` | `u64` | `5000` |
| `base_dir` | `PathBuf` | Provided at construction |
| `include_stack` | `HashSet<PathBuf>` | Empty |
| `driver_config` | `DriverConfig` | Provided at construction |

### Include Resolution

Include resolution follows these steps:

1. Evaluate the path expression to a raw path string.
2. If the path is relative, join it with `base_dir`.
3. Canonicalize to an absolute path.
4. Check `include_stack` for the canonicalized path. If present, return an error (circular include detected).
5. Parse the included file.
6. Save the current `base_dir`, then set `base_dir` to the included file's parent directory.
7. Push the path onto `include_stack`, execute the included script's statements, pop from `include_stack`, restore the original `base_dir`.

### `set` Command

Only the `timeout` key is recognized:

```
set timeout 10000
```

Any other key returns:

```
AutoError::Runtime { message: "Unknown setting: {key}" }
```

### Command Dispatch

The executor handles the following commands:

**Session/device commands:** `start_session`, `end_session`, `use_device`, `boot_device`, `set_target`, `list_devices`

**Watcher commands:** `start_watcher`, `stop_watcher`

**Action commands:** `tap`, `swipe`, `tap_location`, `send_keys`, `wait_for`, `get_value`, `get_screenshot`, `get_screen_info`, `list_elements`

**Logging commands:** `log`, `log_comment`

### Per-Phase Tap Timing

When executing tap commands, the executor records timing for two distinct phases:

- `wait_ms` -- time spent on element lookup (finding the element, waiting for hittability)
- `tap_ms` -- time spent on agent execution (the actual tap command over the wire)

These are logged via `session.log_action_timed()` and appear in the JSONL output as separate fields.

---

## `AutoError` Exit Codes

| Variant | Exit Code | Display Format |
|---------|-----------|----------------|
| `ActionFailed { message, line }` | 1 | `Action failed at line {line}: {message}` |
| `Parse { message, line }` | 2 | `Parse error at line {line}: {message}` |
| `Runtime { message, line }` | 3 | `Runtime error at line {line}: {message}` |
| `Io(std::io::Error)` | 4 | `IO error: {e}` |

Exit codes are used by `qorvex-auto`'s `main.rs` to set the process exit status, enabling reliable error detection in CI/CD pipelines.
