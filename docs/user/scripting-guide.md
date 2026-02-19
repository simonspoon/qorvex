# Scripting Guide (.qvx)

Qorvex automation scripts use the `.qvx` format -- a simple scripting language for recording and replaying UI interactions.

## Running Scripts

```bash
# Run a script
qorvex-auto run test-login.qvx

# Run against a named session
qorvex-auto -s my-session run test-login.qvx
```

The runner auto-starts the Swift agent if `install.sh` was used to install qorvex.

## Converting Action Logs to Scripts

Record actions in the REPL, then convert the log:

```bash
# Print to stdout
qorvex-auto convert ~/.qorvex/logs/default_20250101_120000.jsonl --stdout

# Save to file
qorvex-auto convert session.jsonl --name login-flow
```

## Basic Syntax

Commands use REPL-compatible syntax:

```
# Comments start with #
use_device("UDID-HERE")
set_target("com.example.myapp")
tap("login-button")
send_keys("user@example.com")
swipe("down")
get_screenshot
```

Note: string arguments must be quoted. Commands without arguments don't need parentheses.

## Variables

```
# Assign a value
username = "testuser"

# Use in commands
send_keys(username)

# Capture command output
value = get_value("status-label")
```

## String Interpolation

Double-quoted strings support `$variable` interpolation:

```
name = "world"
log_comment("Hello $name")    # logs "Hello world"
```

Single-quoted strings have no interpolation:

```
log_comment('literal $name')  # logs "literal $name"
```

Escapes in double-quoted strings: `\n`, `\t`, `\\`, `\$`

## Control Flow

### If/Else

```
value = get_value("status-label")
if value == "Success" {
    log_comment("Test passed")
} else {
    tap("retry-button")
}
```

### Foreach

```
foreach item in ["tab1", "tab2", "tab3"] {
    tap(item)
    get_screenshot
}
```

### For (numeric range)

```
for i from 1 to 5 {
    tap("next-button")
    log_comment("Page $i")
}
```

## Settings

```
# Set default timeout for wait_for (milliseconds)
set timeout 10000
```

Currently `timeout` is the only recognized setting.

## Including Other Scripts

```
include "shared/login.qvx"
include "helpers/setup.qvx"
```

Paths are relative to the current script's directory. Circular includes are detected and produce a runtime error.

## Operators

| Operator | Description | Example |
|----------|-------------|---------|
| `+` | Addition (numbers) or concatenation (strings) | `"hello" + " world"` |
| `==` | Equality | `value == "done"` |
| `!=` | Inequality | `status != "error"` |

Cross-type comparison: a string is compared to a number's string representation (`"42" == 42` is true).

## Complete Example

```
# Login test script
use_device("XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX")
set_target("com.example.myapp")

set timeout 10000

# Login
tap("username-field")
send_keys("test@example.com")
tap("password-field")
send_keys("password123")
tap("login-button")

# Verify
wait_for("welcome-label")
value = get_value("welcome-label")

if value == "Welcome!" {
    log_comment("Login test passed")
} else {
    log_comment("Login test failed: got $value")
}

# Navigate tabs
foreach tab in ["home", "settings", "profile"] {
    tap(tab)
    get_screenshot
}
```

## Exit Codes

| Code | Meaning | When |
|------|---------|------|
| 0 | Success | Script completed normally |
| 1 | Action failed | A UI action failed (element not found, tap failed, etc.) |
| 2 | Parse error | Script syntax is invalid |
| 3 | Runtime error | Variable undefined, circular include, unknown setting, etc. |
| 4 | I/O error | File not found, permission denied, etc. |
