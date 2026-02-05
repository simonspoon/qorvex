use std::io::{self, Read};
use std::path::Path;

use qorvex_core::action::{ActionLog, ActionType};

use crate::error::AutoError;

pub struct LogConverter;

impl LogConverter {
    pub fn convert_file(path: &Path) -> Result<String, AutoError> {
        let content = std::fs::read_to_string(path)?;
        Self::convert_str(&content)
    }

    pub fn convert_stdin() -> Result<String, AutoError> {
        let mut content = String::new();
        io::stdin().read_to_string(&mut content)?;
        Self::convert_str(&content)
    }

    fn convert_str(content: &str) -> Result<String, AutoError> {
        let mut lines = Vec::new();
        lines.push("start_session".to_string());

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let log: ActionLog = serde_json::from_str(line).map_err(|e| AutoError::Io(
                io::Error::new(io::ErrorKind::InvalidData, format!("Invalid JSONL: {}", e)),
            ))?;

            if let Some(cmd) = Self::action_to_command(&log.action) {
                lines.push(cmd);
            }
        }

        lines.push("end_session".to_string());
        Ok(lines.join("\n") + "\n")
    }

    fn action_to_command(action: &ActionType) -> Option<String> {
        match action {
            ActionType::Tap { selector, by_label, element_type } => {
                let mut args = vec![format!("\"{}\"", selector)];
                if *by_label {
                    args.push("\"label\"".to_string());
                }
                if let Some(t) = element_type {
                    if !*by_label {
                        args.push("\"\"".to_string()); // placeholder for label arg
                    }
                    args.push(format!("\"{}\"", t));
                }
                Some(format!("tap({})", args.join(", ")))
            }
            ActionType::TapLocation { x, y } => {
                Some(format!("tap_location({}, {})", x, y))
            }
            ActionType::SendKeys { text } => {
                Some(format!("send_keys(\"{}\")", escape_string(text)))
            }
            ActionType::GetScreenshot => {
                Some("get_screenshot()".to_string())
            }
            ActionType::GetScreenInfo => {
                Some("get_screen_info()".to_string())
            }
            ActionType::GetValue { selector, by_label, element_type } => {
                let mut args = vec![format!("\"{}\"", selector)];
                if *by_label {
                    args.push("\"label\"".to_string());
                }
                if let Some(t) = element_type {
                    if !*by_label {
                        args.push("\"\"".to_string());
                    }
                    args.push(format!("\"{}\"", t));
                }
                Some(format!("get_value({})", args.join(", ")))
            }
            ActionType::WaitFor { selector, by_label, element_type, timeout_ms } => {
                let mut args = vec![format!("\"{}\"", selector), timeout_ms.to_string()];
                if *by_label {
                    args.push("\"label\"".to_string());
                }
                if let Some(t) = element_type {
                    if !*by_label {
                        args.push("\"\"".to_string());
                    }
                    args.push(format!("\"{}\"", t));
                }
                Some(format!("wait_for({})", args.join(", ")))
            }
            ActionType::LogComment { message } => {
                Some(format!("# {}", message))
            }
            // Skip session management actions
            ActionType::StartSession | ActionType::EndSession | ActionType::Quit => None,
        }
    }
}

fn escape_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tap_to_command() {
        let action = ActionType::Tap {
            selector: "login-button".to_string(),
            by_label: false,
            element_type: None,
        };
        assert_eq!(
            LogConverter::action_to_command(&action),
            Some("tap(\"login-button\")".to_string())
        );
    }

    #[test]
    fn test_tap_by_label_to_command() {
        let action = ActionType::Tap {
            selector: "Login".to_string(),
            by_label: true,
            element_type: None,
        };
        assert_eq!(
            LogConverter::action_to_command(&action),
            Some("tap(\"Login\", \"label\")".to_string())
        );
    }

    #[test]
    fn test_tap_with_type_to_command() {
        let action = ActionType::Tap {
            selector: "Submit".to_string(),
            by_label: true,
            element_type: Some("Button".to_string()),
        };
        assert_eq!(
            LogConverter::action_to_command(&action),
            Some("tap(\"Submit\", \"label\", \"Button\")".to_string())
        );
    }

    #[test]
    fn test_send_keys_to_command() {
        let action = ActionType::SendKeys { text: "hello".to_string() };
        assert_eq!(
            LogConverter::action_to_command(&action),
            Some("send_keys(\"hello\")".to_string())
        );
    }

    #[test]
    fn test_send_keys_escape() {
        let action = ActionType::SendKeys { text: "line1\nline2".to_string() };
        assert_eq!(
            LogConverter::action_to_command(&action),
            Some("send_keys(\"line1\\nline2\")".to_string())
        );
    }

    #[test]
    fn test_wait_for_to_command() {
        let action = ActionType::WaitFor {
            selector: "dashboard".to_string(),
            by_label: false,
            element_type: None,
            timeout_ms: 5000,
        };
        assert_eq!(
            LogConverter::action_to_command(&action),
            Some("wait_for(\"dashboard\", 5000)".to_string())
        );
    }

    #[test]
    fn test_tap_location_to_command() {
        let action = ActionType::TapLocation { x: 100, y: 200 };
        assert_eq!(
            LogConverter::action_to_command(&action),
            Some("tap_location(100, 200)".to_string())
        );
    }

    #[test]
    fn test_log_comment_to_command() {
        let action = ActionType::LogComment { message: "test step".to_string() };
        assert_eq!(
            LogConverter::action_to_command(&action),
            Some("# test step".to_string())
        );
    }

    #[test]
    fn test_session_actions_skipped() {
        assert!(LogConverter::action_to_command(&ActionType::StartSession).is_none());
        assert!(LogConverter::action_to_command(&ActionType::EndSession).is_none());
        assert!(LogConverter::action_to_command(&ActionType::Quit).is_none());
    }

    #[test]
    fn test_get_screenshot_to_command() {
        assert_eq!(
            LogConverter::action_to_command(&ActionType::GetScreenshot),
            Some("get_screenshot()".to_string())
        );
    }

    #[test]
    fn test_get_screen_info_to_command() {
        assert_eq!(
            LogConverter::action_to_command(&ActionType::GetScreenInfo),
            Some("get_screen_info()".to_string())
        );
    }

    #[test]
    fn test_get_value_to_command() {
        let action = ActionType::GetValue {
            selector: "status-label".to_string(),
            by_label: false,
            element_type: None,
        };
        assert_eq!(
            LogConverter::action_to_command(&action),
            Some("get_value(\"status-label\")".to_string())
        );
    }

    #[test]
    fn test_convert_jsonl() {
        use qorvex_core::action::ActionResult;

        let log1 = ActionLog::new(ActionType::StartSession, ActionResult::Success, None);
        let log2 = ActionLog::new(
            ActionType::Tap { selector: "btn".to_string(), by_label: false, element_type: None },
            ActionResult::Success,
            None,
        );
        let log3 = ActionLog::new(ActionType::EndSession, ActionResult::Success, None);

        let jsonl = format!(
            "{}\n{}\n{}\n",
            serde_json::to_string(&log1).unwrap(),
            serde_json::to_string(&log2).unwrap(),
            serde_json::to_string(&log3).unwrap(),
        );

        let result = LogConverter::convert_str(&jsonl).unwrap();
        assert!(result.contains("start_session"));
        assert!(result.contains("tap(\"btn\")"));
        assert!(result.contains("end_session"));
        // StartSession/EndSession from log should be skipped, but the wrapper adds them
        assert_eq!(result.lines().count(), 3); // start_session, tap, end_session
    }
}
