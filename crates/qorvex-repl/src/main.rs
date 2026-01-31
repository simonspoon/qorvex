use qorvex_core::action::{ActionResult, ActionType};
use qorvex_core::axe::Axe;
use qorvex_core::session::Session;
use qorvex_core::simctl::Simctl;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct ReplState {
    session: Option<Arc<Session>>,
    simulator_udid: Option<String>,
}

impl ReplState {
    fn new() -> Self {
        Self {
            session: None,
            simulator_udid: None,
        }
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);
    let mut writer = stdout;

    let mut state = ReplState::new();
    let mut line = String::new();

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
            state.session = Some(Session::new(state.simulator_udid.clone()));
            "success".to_string()
        }
        "end_session" => {
            state.session = None;
            "success".to_string()
        }
        "quit" => {
            state.session = None;
            std::process::exit(0);
        }
        "tap_element" => {
            let id = args.first().map(|s| s.as_str()).unwrap_or("");
            if id.is_empty() {
                return "fail: missing element id".to_string();
            }
            match &state.simulator_udid {
                Some(udid) => match Axe::tap_element(udid, id) {
                    Ok(_) => "success".to_string(),
                    Err(e) => format!("fail: {}", e),
                },
                None => "fail: no simulator booted".to_string(),
            }
        }
        "tap_location" => {
            if args.len() < 2 {
                return "fail: missing x,y coordinates".to_string();
            }
            let x: i32 = match args[0].parse() {
                Ok(v) => v,
                Err(_) => return "fail: invalid x coordinate".to_string(),
            };
            let y: i32 = match args[1].parse() {
                Ok(v) => v,
                Err(_) => return "fail: invalid y coordinate".to_string(),
            };
            match &state.simulator_udid {
                Some(udid) => match Axe::tap(udid, x, y) {
                    Ok(_) => "success".to_string(),
                    Err(e) => format!("fail: {}", e),
                },
                None => "fail: no simulator booted".to_string(),
            }
        }
        "log_comment" => {
            let message = args.join(" ");
            if let Some(session) = &state.session {
                let screenshot = state
                    .simulator_udid
                    .as_ref()
                    .and_then(|udid| Simctl::screenshot(udid).ok())
                    .map(|bytes| {
                        use base64::Engine;
                        base64::engine::general_purpose::STANDARD.encode(&bytes)
                    });

                session
                    .log_action(
                        ActionType::LogComment {
                            message: message.clone(),
                        },
                        ActionResult::Success,
                        screenshot,
                    )
                    .await;
            }
            "success".to_string()
        }
        "get_screenshot" => match &state.simulator_udid {
            Some(udid) => match Simctl::screenshot(udid) {
                Ok(bytes) => {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD.encode(&bytes)
                }
                Err(e) => format!("fail: {}", e),
            },
            None => "fail: no simulator booted".to_string(),
        },
        "get_screen_info" => match &state.simulator_udid {
            Some(udid) => match Axe::dump_hierarchy(udid) {
                Ok(hierarchy) => {
                    let elements = Axe::list_elements(&hierarchy);
                    serde_json::to_string(&elements).unwrap_or_else(|e| format!("fail: {}", e))
                }
                Err(e) => format!("fail: {}", e),
            },
            None => "fail: no simulator booted".to_string(),
        },
        "get_element_value" => {
            let id = args.first().map(|s| s.as_str()).unwrap_or("");
            if id.is_empty() {
                return "fail: missing element id".to_string();
            }
            match &state.simulator_udid {
                Some(udid) => match Axe::get_element_value(udid, id) {
                    Ok(Some(value)) => value,
                    Ok(None) => "null".to_string(),
                    Err(e) => format!("fail: {}", e),
                },
                None => "fail: no simulator booted".to_string(),
            }
        }
        "send_keys" => {
            let text = args.join(" ");
            if text.is_empty() {
                return "fail: missing text".to_string();
            }
            match &state.simulator_udid {
                Some(udid) => match Simctl::send_keys(udid, &text) {
                    Ok(_) => "success".to_string(),
                    Err(e) => format!("fail: {}", e),
                },
                None => "fail: no simulator booted".to_string(),
            }
        }
        "help" => {
            "commands: start_session, end_session, quit, tap_element(id), tap_location(x,y), log_comment(message), get_screenshot(), get_screen_info(), get_element_value(id), send_keys(text)".to_string()
        }
        _ => format!("fail: unknown command '{}'", cmd),
    }
}

fn parse_command(input: &str) -> (String, Vec<String>) {
    if let Some(paren_idx) = input.find('(') {
        let cmd = input[..paren_idx].trim().to_string();
        let args_str = input[paren_idx + 1..].trim_end_matches(')').trim();
        let args: Vec<String> = if args_str.is_empty() {
            vec![]
        } else {
            args_str.split(',').map(|s| s.trim().to_string()).collect()
        };
        (cmd, args)
    } else {
        (input.to_string(), vec![])
    }
}
