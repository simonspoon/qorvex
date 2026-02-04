use clap::Parser;
use qorvex_core::action::{ActionResult, ActionType};
use qorvex_core::axe::Axe;
use qorvex_core::ipc::{socket_path, IpcServer};
use qorvex_core::session::Session;
use qorvex_core::simctl::Simctl;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::task::JoinHandle;

#[derive(Parser, Debug)]
#[command(name = "qorvex-repl")]
#[command(about = "Interactive REPL for iOS Simulator automation")]
struct Args {
    /// Session name for IPC socket
    #[arg(short, long, default_value = "default")]
    session: String,
}

/// Validates a simulator UDID format.
/// Accepts standard UUID format (8-4-4-4-12 hex digits) or iOS Simulator UDIDs.
fn is_valid_udid(udid: &str) -> bool {
    // Standard UUID format: 8-4-4-4-12 (36 characters with hyphens)
    // Example: XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX
    if udid.len() != 36 {
        return false;
    }

    let parts: Vec<&str> = udid.split('-').collect();
    if parts.len() != 5 {
        return false;
    }

    let expected_lengths = [8, 4, 4, 4, 12];
    for (part, &expected_len) in parts.iter().zip(expected_lengths.iter()) {
        if part.len() != expected_len {
            return false;
        }
        if !part.chars().all(|c| c.is_ascii_hexdigit()) {
            return false;
        }
    }

    true
}

/// Validates that a coordinate value is non-negative.
fn validate_coordinate(value: i32, name: &str) -> Result<(), String> {
    if value < 0 {
        Err(format!(
            "{} coordinate must be non-negative (got {})",
            name, value
        ))
    } else {
        Ok(())
    }
}

struct ReplState {
    session: Option<Arc<Session>>,
    simulator_udid: Option<String>,
    ipc_server_handle: Option<JoinHandle<()>>,
    session_name: String,
}

impl ReplState {
    fn new(session_name: String) -> Self {
        Self {
            session: None,
            simulator_udid: None,
            ipc_server_handle: None,
            session_name,
        }
    }

    fn capture_screenshot(&self) -> Option<String> {
        self.simulator_udid.as_ref().and_then(|udid| {
            Simctl::screenshot(udid).ok().map(|bytes| {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.encode(&bytes)
            })
        })
    }

    async fn log_action(&self, action: ActionType, result: ActionResult) {
        if let Some(session) = &self.session {
            let screenshot = self.capture_screenshot();
            eprintln!("[repl] Logging action: {:?}", action);
            session.log_action(action, result, screenshot).await;
        } else {
            eprintln!("[repl] Warning: No session active, action not logged");
        }
    }
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);
    let mut writer = stdout;

    let mut state = ReplState::new(args.session);
    let mut line = String::new();

    // Print session name for visibility
    eprintln!("[repl] Session name: {}", state.session_name);
    eprintln!("[repl] IPC socket: {:?}", socket_path(&state.session_name));

    // Try to get booted simulator
    state.simulator_udid = Simctl::get_booted_udid().ok();

    loop {
        line.clear();
        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
            break;
        }

        let input = line.trim();
        if input.is_empty() {
            continue;
        }

        let response = process_command(input, &mut state).await;
        let output = format!("{}\n", response);
        let _ = writer.write_all(output.as_bytes()).await;
        let _ = writer.flush().await;
    }
}

async fn process_command(input: &str, state: &mut ReplState) -> String {
    let (cmd, args) = parse_command(input);

    match cmd.as_str() {
        "start_session" => {
            let session = Session::new(state.simulator_udid.clone(), &state.session_name);
            state.session = Some(session.clone());

            // Start IPC server for watcher connections
            let server = IpcServer::new(session, &state.session_name);
            let handle = tokio::spawn(async move {
                if let Err(e) = server.run().await {
                    eprintln!("[repl] IPC server error: {}", e);
                }
            });
            state.ipc_server_handle = Some(handle);

            "success".to_string()
        }
        "end_session" => {
            // Stop IPC server
            if let Some(handle) = state.ipc_server_handle.take() {
                handle.abort();
            }
            state.session = None;
            "success".to_string()
        }
        "quit" => {
            if let Some(handle) = state.ipc_server_handle.take() {
                handle.abort();
            }
            state.session = None;
            std::process::exit(0);
        }
        "tap_element" => {
            let id = args.first().map(|s| s.as_str()).unwrap_or("");
            if id.is_empty() {
                return "fail: tap_element requires 1 argument: tap_element(element_id)".to_string();
            }
            match &state.simulator_udid {
                Some(udid) => match Axe::tap_element(udid, id) {
                    Ok(_) => {
                        state.log_action(
                            ActionType::TapElement { id: id.to_string() },
                            ActionResult::Success,
                        ).await;
                        "success".to_string()
                    }
                    Err(e) => {
                        state.log_action(
                            ActionType::TapElement { id: id.to_string() },
                            ActionResult::Failure(e.to_string()),
                        ).await;
                        format!("fail: {}", e)
                    }
                },
                None => "fail: no simulator selected - use list_devices and use_device first".to_string(),
            }
        }
        "tap_location" => {
            if args.len() < 2 {
                return "fail: tap_location requires 2 arguments: tap_location(x, y)".to_string();
            }
            let x: i32 = match args[0].parse() {
                Ok(v) => v,
                Err(_) => {
                    return format!(
                        "fail: invalid x coordinate '{}' - must be an integer",
                        args[0]
                    )
                }
            };
            let y: i32 = match args[1].parse() {
                Ok(v) => v,
                Err(_) => {
                    return format!(
                        "fail: invalid y coordinate '{}' - must be an integer",
                        args[1]
                    )
                }
            };
            if let Err(e) = validate_coordinate(x, "x") {
                return format!("fail: {}", e);
            }
            if let Err(e) = validate_coordinate(y, "y") {
                return format!("fail: {}", e);
            }
            match &state.simulator_udid {
                Some(udid) => match Axe::tap(udid, x, y) {
                    Ok(_) => {
                        state.log_action(
                            ActionType::TapLocation { x, y },
                            ActionResult::Success,
                        ).await;
                        "success".to_string()
                    }
                    Err(e) => {
                        state.log_action(
                            ActionType::TapLocation { x, y },
                            ActionResult::Failure(e.to_string()),
                        ).await;
                        format!("fail: {}", e)
                    }
                },
                None => "fail: no simulator selected - use list_devices and use_device first".to_string(),
            }
        }
        "log_comment" => {
            let message = args.join(" ");
            if message.is_empty() {
                return "fail: log_comment requires a message: log_comment(your message here)"
                    .to_string();
            }
            state.log_action(
                ActionType::LogComment { message },
                ActionResult::Success,
            ).await;
            "success".to_string()
        }
        "get_screenshot" => match &state.simulator_udid {
            Some(udid) => match Simctl::screenshot(udid) {
                Ok(bytes) => {
                    use base64::Engine;
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                    // Log with the captured screenshot
                    if let Some(session) = &state.session {
                        session.log_action(
                            ActionType::GetScreenshot,
                            ActionResult::Success,
                            Some(b64.clone()),
                        ).await;
                    }
                    b64
                }
                Err(e) => {
                    state.log_action(ActionType::GetScreenshot, ActionResult::Failure(e.to_string())).await;
                    format!("fail: {}", e)
                }
            },
            None => "fail: no simulator selected - use list_devices and use_device first".to_string(),
        },
        "get_screen_info" => match &state.simulator_udid {
            Some(udid) => match Axe::dump_hierarchy(udid) {
                Ok(hierarchy) => {
                    let elements = Axe::list_elements(&hierarchy);
                    state.log_action(ActionType::GetScreenInfo, ActionResult::Success).await;
                    serde_json::to_string(&elements).unwrap_or_else(|e| format!("fail: {}", e))
                }
                Err(e) => {
                    state.log_action(ActionType::GetScreenInfo, ActionResult::Failure(e.to_string())).await;
                    format!("fail: {}", e)
                }
            },
            None => "fail: no simulator selected - use list_devices and use_device first".to_string(),
        },
        "get_element_value" => {
            let id = args.first().map(|s| s.as_str()).unwrap_or("");
            if id.is_empty() {
                return "fail: get_element_value requires 1 argument: get_element_value(element_id)"
                    .to_string();
            }
            match &state.simulator_udid {
                Some(udid) => match Axe::get_element_value(udid, id) {
                    Ok(Some(value)) => {
                        state.log_action(
                            ActionType::GetElementValue { id: id.to_string() },
                            ActionResult::Success,
                        ).await;
                        value
                    }
                    Ok(None) => {
                        state.log_action(
                            ActionType::GetElementValue { id: id.to_string() },
                            ActionResult::Success,
                        ).await;
                        "null".to_string()
                    }
                    Err(e) => {
                        state.log_action(
                            ActionType::GetElementValue { id: id.to_string() },
                            ActionResult::Failure(e.to_string()),
                        ).await;
                        format!("fail: {}", e)
                    }
                },
                None => "fail: no simulator selected - use list_devices and use_device first".to_string(),
            }
        }
        "send_keys" => {
            let text = args.join(" ");
            if text.is_empty() {
                return "fail: send_keys requires text: send_keys(text to type)".to_string();
            }
            match &state.simulator_udid {
                Some(udid) => match Simctl::send_keys(udid, &text) {
                    Ok(_) => {
                        state.log_action(
                            ActionType::SendKeys { text: text.clone() },
                            ActionResult::Success,
                        ).await;
                        "success".to_string()
                    }
                    Err(e) => {
                        state.log_action(
                            ActionType::SendKeys { text: text.clone() },
                            ActionResult::Failure(e.to_string()),
                        ).await;
                        format!("fail: {}", e)
                    }
                },
                None => "fail: no simulator selected - use list_devices and use_device first".to_string(),
            }
        }
        "wait_for" => {
            let id = args.first().map(|s| s.as_str()).unwrap_or("");
            if id.is_empty() {
                return "fail: wait_for requires at least 1 argument: wait_for(element_id) or wait_for(element_id, timeout_ms)".to_string();
            }
            let timeout_ms: u64 = args.get(1)
                .map(|s| s.parse().unwrap_or(5000))
                .unwrap_or(5000);
            match &state.simulator_udid {
                Some(udid) => {
                    use qorvex_core::executor::ActionExecutor;
                    let executor = ActionExecutor::new(udid.clone());
                    let result = executor.execute(ActionType::WaitFor {
                        id: id.to_string(),
                        timeout_ms,
                    }).await;
                    let action_result = if result.success {
                        ActionResult::Success
                    } else {
                        ActionResult::Failure(result.message.clone())
                    };
                    state.log_action(
                        ActionType::WaitFor { id: id.to_string(), timeout_ms },
                        action_result,
                    ).await;
                    if result.success {
                        format!("success: {} ({})", result.message, result.data.unwrap_or_default())
                    } else {
                        format!("fail: {}", result.message)
                    }
                }
                None => "fail: no simulator selected - use list_devices and use_device first".to_string(),
            }
        }
        "list_devices" => match Simctl::list_devices() {
            Ok(devices) => {
                serde_json::to_string(&devices).unwrap_or_else(|e| format!("fail: {}", e))
            }
            Err(e) => format!("fail: {}", e),
        },
        "use_device" => {
            let udid = args.first().map(|s| s.as_str()).unwrap_or("");
            if udid.is_empty() {
                return "fail: use_device requires 1 argument: use_device(simulator_udid)"
                    .to_string();
            }
            if !is_valid_udid(udid) {
                return format!(
                    "fail: invalid UDID format '{}' - expected format: XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX",
                    udid
                );
            }
            state.simulator_udid = Some(udid.to_string());
            "success".to_string()
        }
        "boot_device" => {
            let udid = args.first().map(|s| s.as_str()).unwrap_or("");
            if udid.is_empty() {
                return "fail: boot_device requires 1 argument: boot_device(simulator_udid)"
                    .to_string();
            }
            if !is_valid_udid(udid) {
                return format!(
                    "fail: invalid UDID format '{}' - expected format: XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX",
                    udid
                );
            }
            match Simctl::boot(udid) {
                Ok(_) => {
                    state.simulator_udid = Some(udid.to_string());
                    "success".to_string()
                }
                Err(e) => format!("fail: {}", e),
            }
        }
        "get_session_info" => match &state.session {
            Some(session) => {
                let action_log = session.get_action_log().await;
                let info = serde_json::json!({
                    "active": true,
                    "simulator_udid": state.simulator_udid,
                    "action_count": action_log.len()
                });
                serde_json::to_string(&info).unwrap_or_else(|e| format!("fail: {}", e))
            }
            None => {
                let info = serde_json::json!({
                    "active": false,
                    "simulator_udid": state.simulator_udid
                });
                serde_json::to_string(&info).unwrap_or_else(|e| format!("fail: {}", e))
            }
        },
        "list_elements" => match &state.simulator_udid {
            Some(udid) => match Axe::dump_hierarchy(udid) {
                Ok(hierarchy) => {
                    let elements = Axe::list_elements(&hierarchy);
                    serde_json::to_string(&elements).unwrap_or_else(|e| format!("fail: {}", e))
                }
                Err(e) => format!("fail: {}", e),
            },
            None => "fail: no simulator selected - use list_devices and use_device first".to_string(),
        },
        "help" => {
            r#"Available Commands:

Session:
  start_session          Start a new session
  end_session            End the current session
  get_session_info       Get current session information

Device:
  list_devices           List available simulators
  use_device(udid)       Select a simulator by UDID
  boot_device(udid)      Boot a simulator

Screen:
  get_screenshot         Capture a screenshot (base64 PNG)
  get_screen_info        Get UI hierarchy as JSON

UI:
  list_elements          List all UI elements
  get_element_value(id)  Get an element's value by ID
  tap_element(id)        Tap an element by ID
  tap_location(x, y)     Tap at screen coordinates
  wait_for(id)           Wait for element to appear (default 5s timeout)
  wait_for(id, ms)       Wait for element with custom timeout

Input:
  send_keys(text)        Send keyboard input
  log_comment(message)   Log a comment to the session

General:
  help                   Show this help message
  quit                   Exit the REPL"#.to_string()
        }
        _ => format!("fail: unknown command '{}'", cmd),
    }
}

fn parse_command(input: &str) -> (String, Vec<String>) {
    let Some(paren_idx) = input.find('(') else {
        return (input.to_string(), vec![]);
    };

    let cmd = input[..paren_idx].trim().to_string();

    // Find the matching closing paren by counting depth
    let after_paren = &input[paren_idx + 1..];
    let args_str = find_matching_paren_content(after_paren);
    let args_str = args_str.trim();

    if args_str.is_empty() {
        return (cmd, vec![]);
    }

    let args = split_args(args_str);
    (cmd, args)
}

/// Extract content up to the matching closing parenthesis
fn find_matching_paren_content(s: &str) -> &str {
    let mut depth = 1;
    let mut in_double_quote = false;
    let mut in_single_quote = false;
    let mut prev_was_escape = false;

    for (i, c) in s.char_indices() {
        if prev_was_escape {
            prev_was_escape = false;
            continue;
        }

        match c {
            '\\' if in_double_quote || in_single_quote => {
                prev_was_escape = true;
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
            }
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
            }
            '(' if !in_double_quote && !in_single_quote => {
                depth += 1;
            }
            ')' if !in_double_quote && !in_single_quote => {
                depth -= 1;
                if depth == 0 {
                    return &s[..i];
                }
            }
            _ => {}
        }
    }

    // No matching paren found, return everything (backwards compatible)
    s.trim_end_matches(')')
}

/// Split arguments by commas, respecting quotes and nested parentheses
fn split_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    let mut in_double_quote = false;
    let mut in_single_quote = false;
    let mut prev_was_escape = false;

    for c in s.chars() {
        if prev_was_escape {
            current.push(c);
            prev_was_escape = false;
            continue;
        }

        match c {
            '\\' if in_double_quote || in_single_quote => {
                prev_was_escape = true;
                current.push(c);
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
                current.push(c);
            }
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
                current.push(c);
            }
            '(' if !in_double_quote && !in_single_quote => {
                depth += 1;
                current.push(c);
            }
            ')' if !in_double_quote && !in_single_quote => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 && !in_double_quote && !in_single_quote => {
                args.push(current.trim().to_string());
                current = String::new();
            }
            _ => {
                current.push(c);
            }
        }
    }

    // Push the last argument
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        args.push(trimmed);
    }

    args
}

#[cfg(test)]
mod tests {
    use super::*;

    // === Basic commands without arguments ===

    #[test]
    fn test_parse_help_command() {
        let (cmd, args) = parse_command("help");
        assert_eq!(cmd, "help");
        assert!(args.is_empty());
    }

    #[test]
    fn test_parse_quit_command() {
        let (cmd, args) = parse_command("quit");
        assert_eq!(cmd, "quit");
        assert!(args.is_empty());
    }

    #[test]
    fn test_parse_start_session_command() {
        let (cmd, args) = parse_command("start_session");
        assert_eq!(cmd, "start_session");
        assert!(args.is_empty());
    }

    #[test]
    fn test_parse_end_session_command() {
        let (cmd, args) = parse_command("end_session");
        assert_eq!(cmd, "end_session");
        assert!(args.is_empty());
    }

    #[test]
    fn test_parse_get_screenshot_no_parens() {
        let (cmd, args) = parse_command("get_screenshot");
        assert_eq!(cmd, "get_screenshot");
        assert!(args.is_empty());
    }

    #[test]
    fn test_parse_get_screenshot_with_empty_parens() {
        let (cmd, args) = parse_command("get_screenshot()");
        assert_eq!(cmd, "get_screenshot");
        assert!(args.is_empty());
    }

    #[test]
    fn test_parse_get_screen_info_with_empty_parens() {
        let (cmd, args) = parse_command("get_screen_info()");
        assert_eq!(cmd, "get_screen_info");
        assert!(args.is_empty());
    }

    // === Commands with single argument ===

    #[test]
    fn test_parse_tap_element_single_arg() {
        let (cmd, args) = parse_command("tap_element(button_login)");
        assert_eq!(cmd, "tap_element");
        assert_eq!(args, vec!["button_login"]);
    }

    #[test]
    fn test_parse_get_element_value_single_arg() {
        let (cmd, args) = parse_command("get_element_value(text_field_1)");
        assert_eq!(cmd, "get_element_value");
        assert_eq!(args, vec!["text_field_1"]);
    }

    #[test]
    fn test_parse_send_keys_single_word() {
        let (cmd, args) = parse_command("send_keys(hello)");
        assert_eq!(cmd, "send_keys");
        assert_eq!(args, vec!["hello"]);
    }

    // === Commands with multiple arguments ===

    #[test]
    fn test_parse_tap_location_two_args() {
        let (cmd, args) = parse_command("tap_location(100, 200)");
        assert_eq!(cmd, "tap_location");
        assert_eq!(args, vec!["100", "200"]);
    }

    #[test]
    fn test_parse_tap_location_no_spaces() {
        let (cmd, args) = parse_command("tap_location(100,200)");
        assert_eq!(cmd, "tap_location");
        assert_eq!(args, vec!["100", "200"]);
    }

    #[test]
    fn test_parse_tap_location_extra_spaces() {
        let (cmd, args) = parse_command("tap_location(  100  ,  200  )");
        assert_eq!(cmd, "tap_location");
        assert_eq!(args, vec!["100", "200"]);
    }

    // === Edge cases: empty and whitespace ===

    #[test]
    fn test_parse_empty_string() {
        let (cmd, args) = parse_command("");
        assert_eq!(cmd, "");
        assert!(args.is_empty());
    }

    #[test]
    fn test_parse_whitespace_only() {
        let (cmd, args) = parse_command("   ");
        assert_eq!(cmd, "   ");
        assert!(args.is_empty());
    }

    #[test]
    fn test_parse_command_with_leading_whitespace() {
        // Note: the current implementation doesn't trim leading whitespace on command without parens
        let (cmd, args) = parse_command("  help");
        assert_eq!(cmd, "  help");
        assert!(args.is_empty());
    }

    #[test]
    fn test_parse_command_with_parens_and_leading_whitespace() {
        // With parens, leading whitespace is preserved but space before ( is trimmed
        let (cmd, args) = parse_command("  tap_element(id)");
        assert_eq!(cmd, "tap_element");
        assert_eq!(args, vec!["id"]);
    }

    // === Edge cases: unknown commands ===

    #[test]
    fn test_parse_unknown_command() {
        let (cmd, args) = parse_command("unknown_command");
        assert_eq!(cmd, "unknown_command");
        assert!(args.is_empty());
    }

    #[test]
    fn test_parse_unknown_command_with_args() {
        let (cmd, args) = parse_command("foo_bar(arg1, arg2, arg3)");
        assert_eq!(cmd, "foo_bar");
        assert_eq!(args, vec!["arg1", "arg2", "arg3"]);
    }

    // === Edge cases: strings with spaces ===

    #[test]
    fn test_parse_send_keys_with_spaces() {
        // Note: spaces within a single arg are preserved, but commas still split
        let (cmd, args) = parse_command("send_keys(hello world)");
        assert_eq!(cmd, "send_keys");
        assert_eq!(args, vec!["hello world"]);
    }

    #[test]
    fn test_parse_log_comment_with_spaces() {
        let (cmd, args) = parse_command("log_comment(this is a test message)");
        assert_eq!(cmd, "log_comment");
        assert_eq!(args, vec!["this is a test message"]);
    }

    #[test]
    fn test_parse_args_with_commas_split() {
        // Commas always split arguments - no quote handling
        let (cmd, args) = parse_command("log_comment(hello, world, how are you)");
        assert_eq!(cmd, "log_comment");
        assert_eq!(args, vec!["hello", "world", "how are you"]);
    }

    // === Edge cases: parentheses handling ===

    #[test]
    fn test_parse_nested_parentheses() {
        // Nested parens are now properly preserved
        let (cmd, args) = parse_command("cmd(arg(nested))");
        assert_eq!(cmd, "cmd");
        assert_eq!(args, vec!["arg(nested)"]);
    }

    #[test]
    fn test_parse_deeply_nested_parentheses() {
        let (cmd, args) = parse_command("cmd(a(b(c)))");
        assert_eq!(cmd, "cmd");
        assert_eq!(args, vec!["a(b(c))"]);
    }

    #[test]
    fn test_parse_multiple_nested_args() {
        let (cmd, args) = parse_command("cmd(func(a), func(b))");
        assert_eq!(cmd, "cmd");
        assert_eq!(args, vec!["func(a)", "func(b)"]);
    }

    #[test]
    fn test_parse_no_closing_paren() {
        // Missing closing paren - trim_end_matches does nothing
        let (cmd, args) = parse_command("tap_element(button");
        assert_eq!(cmd, "tap_element");
        assert_eq!(args, vec!["button"]);
    }

    #[test]
    fn test_parse_only_opening_paren() {
        let (cmd, args) = parse_command("cmd(");
        assert_eq!(cmd, "cmd");
        assert!(args.is_empty());
    }

    #[test]
    fn test_parse_empty_parens_with_spaces() {
        let (cmd, args) = parse_command("cmd(   )");
        assert_eq!(cmd, "cmd");
        assert!(args.is_empty());
    }

    // === Edge cases: quotes handling ===

    #[test]
    fn test_parse_quoted_string_preserved() {
        // Quotes are preserved as part of the string
        let (cmd, args) = parse_command("send_keys(\"hello world\")");
        assert_eq!(cmd, "send_keys");
        assert_eq!(args, vec!["\"hello world\""]);
    }

    #[test]
    fn test_parse_single_quotes_preserved() {
        let (cmd, args) = parse_command("send_keys('test')");
        assert_eq!(cmd, "send_keys");
        assert_eq!(args, vec!["'test'"]);
    }

    #[test]
    fn test_parse_quoted_string_with_comma() {
        // Commas inside quotes should not split arguments
        let (cmd, args) = parse_command("send_keys(\"hello, world\")");
        assert_eq!(cmd, "send_keys");
        assert_eq!(args, vec!["\"hello, world\""]);
    }

    #[test]
    fn test_parse_single_quoted_string_with_comma() {
        let (cmd, args) = parse_command("send_keys('hello, world')");
        assert_eq!(cmd, "send_keys");
        assert_eq!(args, vec!["'hello, world'"]);
    }

    #[test]
    fn test_parse_mixed_quoted_and_unquoted_args() {
        let (cmd, args) = parse_command("cmd(\"hello, world\", 123, 'foo, bar')");
        assert_eq!(cmd, "cmd");
        assert_eq!(args, vec!["\"hello, world\"", "123", "'foo, bar'"]);
    }

    #[test]
    fn test_parse_quoted_string_with_parens() {
        let (cmd, args) = parse_command("send_keys(\"func(x)\")");
        assert_eq!(cmd, "send_keys");
        assert_eq!(args, vec!["\"func(x)\""]);
    }

    #[test]
    fn test_parse_escaped_quote_in_string() {
        // Escaped quotes should not end the string
        let (cmd, args) = parse_command(r#"send_keys("hello \"world\"")"#);
        assert_eq!(cmd, "send_keys");
        assert_eq!(args, vec![r#""hello \"world\"""#]);
    }

    #[test]
    fn test_parse_escaped_backslash() {
        let (cmd, args) = parse_command(r#"send_keys("path\\to\\file")"#);
        assert_eq!(cmd, "send_keys");
        assert_eq!(args, vec![r#""path\\to\\file""#]);
    }

    // === Edge cases: special characters ===

    #[test]
    fn test_parse_arg_with_numbers() {
        let (cmd, args) = parse_command("tap_element(button_123)");
        assert_eq!(cmd, "tap_element");
        assert_eq!(args, vec!["button_123"]);
    }

    #[test]
    fn test_parse_arg_with_dashes() {
        let (cmd, args) = parse_command("tap_element(my-button-id)");
        assert_eq!(cmd, "tap_element");
        assert_eq!(args, vec!["my-button-id"]);
    }

    #[test]
    fn test_parse_arg_with_dots() {
        let (cmd, args) = parse_command("tap_element(com.example.button)");
        assert_eq!(cmd, "tap_element");
        assert_eq!(args, vec!["com.example.button"]);
    }

    #[test]
    fn test_parse_negative_numbers() {
        let (cmd, args) = parse_command("tap_location(-10, -20)");
        assert_eq!(cmd, "tap_location");
        assert_eq!(args, vec!["-10", "-20"]);
    }

    // === Edge cases: extra arguments ===

    #[test]
    fn test_parse_extra_args_preserved() {
        // Parser doesn't validate arg count - all args are returned
        let (cmd, args) = parse_command("tap_element(id1, id2, id3)");
        assert_eq!(cmd, "tap_element");
        assert_eq!(args, vec!["id1", "id2", "id3"]);
    }

    #[test]
    fn test_parse_many_args() {
        let (cmd, args) = parse_command("custom(a, b, c, d, e, f)");
        assert_eq!(cmd, "custom");
        assert_eq!(args, vec!["a", "b", "c", "d", "e", "f"]);
    }

    // === Edge cases: command name variations ===

    #[test]
    fn test_parse_uppercase_command() {
        let (cmd, args) = parse_command("HELP");
        assert_eq!(cmd, "HELP");
        assert!(args.is_empty());
    }

    #[test]
    fn test_parse_mixed_case_command() {
        let (cmd, args) = parse_command("TapElement(id)");
        assert_eq!(cmd, "TapElement");
        assert_eq!(args, vec!["id"]);
    }

    // === Edge cases: multiple closing parens ===

    #[test]
    fn test_parse_multiple_closing_parens() {
        // Extra closing parens after the matching one are ignored
        let (cmd, args) = parse_command("cmd(arg)))");
        assert_eq!(cmd, "cmd");
        assert_eq!(args, vec!["arg"]);
    }

    // === Regression tests for real-world usage ===

    #[test]
    fn test_parse_use_device_with_udid() {
        let (cmd, args) = parse_command("use_device(ABCD1234-5678-90EF-GHIJ-KLMNOPQRSTUV)");
        assert_eq!(cmd, "use_device");
        assert_eq!(args, vec!["ABCD1234-5678-90EF-GHIJ-KLMNOPQRSTUV"]);
    }

    #[test]
    fn test_parse_list_devices() {
        let (cmd, args) = parse_command("list_devices");
        assert_eq!(cmd, "list_devices");
        assert!(args.is_empty());
    }

    #[test]
    fn test_parse_list_devices_with_parens() {
        let (cmd, args) = parse_command("list_devices()");
        assert_eq!(cmd, "list_devices");
        assert!(args.is_empty());
    }

    // === UDID validation tests ===

    #[test]
    fn test_valid_udid_standard_format() {
        // Standard UUID format with uppercase hex
        assert!(is_valid_udid("12345678-1234-1234-1234-123456789ABC"));
    }

    #[test]
    fn test_valid_udid_lowercase() {
        // Lowercase hex is valid
        assert!(is_valid_udid("abcdef12-3456-7890-abcd-ef1234567890"));
    }

    #[test]
    fn test_valid_udid_mixed_case() {
        // Mixed case is valid
        assert!(is_valid_udid("ABCDEF12-3456-7890-abcd-EF1234567890"));
    }

    #[test]
    fn test_invalid_udid_too_short() {
        assert!(!is_valid_udid("12345678-1234-1234-1234-12345678"));
    }

    #[test]
    fn test_invalid_udid_too_long() {
        assert!(!is_valid_udid("12345678-1234-1234-1234-123456789ABCDEF"));
    }

    #[test]
    fn test_invalid_udid_wrong_segment_lengths() {
        assert!(!is_valid_udid("1234567-12345-1234-1234-123456789ABC"));
    }

    #[test]
    fn test_invalid_udid_no_hyphens() {
        assert!(!is_valid_udid("123456781234123412341234567890AB"));
    }

    #[test]
    fn test_invalid_udid_non_hex_chars() {
        assert!(!is_valid_udid("GHIJKLMN-1234-1234-1234-123456789ABC"));
    }

    #[test]
    fn test_invalid_udid_empty() {
        assert!(!is_valid_udid(""));
    }

    #[test]
    fn test_invalid_udid_random_string() {
        assert!(!is_valid_udid("not-a-valid-udid"));
    }

    // === Coordinate validation tests ===

    #[test]
    fn test_validate_coordinate_zero() {
        assert!(validate_coordinate(0, "x").is_ok());
    }

    #[test]
    fn test_validate_coordinate_positive() {
        assert!(validate_coordinate(100, "x").is_ok());
        assert!(validate_coordinate(999, "y").is_ok());
    }

    #[test]
    fn test_validate_coordinate_negative_x() {
        let result = validate_coordinate(-1, "x");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("x coordinate must be non-negative"));
    }

    #[test]
    fn test_validate_coordinate_negative_y() {
        let result = validate_coordinate(-50, "y");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("y coordinate must be non-negative"));
    }
}
