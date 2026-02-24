use std::io::{self, Read};
use std::path::Path;

use qorvex_core::action::{ActionLog, ActionType};

/// Convert JSONL action logs to shell scripts that call `qorvex` CLI commands.
pub struct LogConverter;

impl LogConverter {
    pub fn convert_file(path: &Path) -> Result<String, io::Error> {
        let content = std::fs::read_to_string(path)?;
        Self::convert_str(&content)
    }

    pub fn convert_stdin() -> Result<String, io::Error> {
        let mut content = String::new();
        io::stdin().read_to_string(&mut content)?;
        Self::convert_str(&content)
    }

    fn convert_str(content: &str) -> Result<String, io::Error> {
        let mut lines = vec![
            "#!/usr/bin/env bash".to_string(),
            "set -euo pipefail".to_string(),
            String::new(),
        ];

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let log: ActionLog = serde_json::from_str(line).map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidData, format!("Invalid JSONL: {}", e))
            })?;

            if let Some(cmd) = Self::action_to_command(&log.action, log.tag.as_deref()) {
                lines.push(cmd);
            }
        }

        Ok(lines.join("\n") + "\n")
    }

    fn action_to_command(action: &ActionType, tag: Option<&str>) -> Option<String> {
        let base = match action {
            ActionType::Tap { selector, by_label, element_type, .. } => {
                let mut cmd = format!("qorvex tap {}", shell_escape(selector));
                if *by_label {
                    cmd.push_str(" --label");
                }
                if let Some(t) = element_type {
                    cmd.push_str(&format!(" -T {}", shell_escape(t)));
                }
                Some(cmd)
            }
            ActionType::TapLocation { x, y } => {
                Some(format!("qorvex tap-location {} {}", x, y))
            }
            ActionType::Swipe { direction } => {
                Some(format!("qorvex swipe {}", shell_escape(direction)))
            }
            ActionType::SendKeys { text } => {
                Some(format!("qorvex send-keys {}", shell_escape(text)))
            }
            ActionType::GetScreenshot => {
                Some("qorvex screenshot".to_string())
            }
            ActionType::GetScreenInfo => {
                Some("qorvex screen-info".to_string())
            }
            ActionType::GetValue { selector, by_label, element_type, .. } => {
                let mut cmd = format!("qorvex get-value {}", shell_escape(selector));
                if *by_label {
                    cmd.push_str(" --label");
                }
                if let Some(t) = element_type {
                    cmd.push_str(&format!(" -T {}", shell_escape(t)));
                }
                Some(cmd)
            }
            ActionType::WaitFor { selector, by_label, element_type, timeout_ms, .. } => {
                let mut cmd = format!("qorvex wait-for {}", shell_escape(selector));
                if *by_label {
                    cmd.push_str(" --label");
                }
                if let Some(t) = element_type {
                    cmd.push_str(&format!(" -T {}", shell_escape(t)));
                }
                cmd.push_str(&format!(" -o {}", timeout_ms));
                Some(cmd)
            }
            ActionType::WaitForNot { selector, by_label, element_type, timeout_ms } => {
                let mut cmd = format!("qorvex wait-for-not {}", shell_escape(selector));
                if *by_label {
                    cmd.push_str(" --label");
                }
                if let Some(t) = element_type {
                    cmd.push_str(&format!(" -T {}", shell_escape(t)));
                }
                cmd.push_str(&format!(" -o {}", timeout_ms));
                Some(cmd)
            }
            ActionType::LongPress { x, y, duration } => {
                Some(format!("# TODO: long-press {} {} {} (not yet supported)", x, y, duration))
            }
            ActionType::SetTarget { bundle_id } => {
                Some(format!("qorvex set-target {}", shell_escape(bundle_id)))
            }
            ActionType::LogComment { message } => {
                Some(format!("# {}", message))
            }
            // Skip session management actions
            ActionType::StartSession | ActionType::EndSession | ActionType::Quit => None,
        };
        match base {
            Some(mut cmd) => {
                if let Some(t) = tag {
                    cmd.push_str(&format!(" --tag {}", shell_escape(t)));
                }
                Some(cmd)
            }
            None => None,
        }
    }
}

/// Shell-escape a string using single quotes. Internal single quotes become `'\''`.
fn shell_escape(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/') {
        // Safe to use unquoted
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
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
            timeout_ms: None,
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex tap login-button".to_string())
        );
    }

    #[test]
    fn test_tap_by_label_to_command() {
        let action = ActionType::Tap {
            selector: "Login".to_string(),
            by_label: true,
            element_type: None,
            timeout_ms: None,
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex tap Login --label".to_string())
        );
    }

    #[test]
    fn test_tap_with_type_to_command() {
        let action = ActionType::Tap {
            selector: "Submit".to_string(),
            by_label: true,
            element_type: Some("Button".to_string()),
            timeout_ms: None,
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex tap Submit --label -T Button".to_string())
        );
    }

    #[test]
    fn test_tap_selector_with_spaces() {
        let action = ActionType::Tap {
            selector: "Sign In".to_string(),
            by_label: true,
            element_type: None,
            timeout_ms: None,
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex tap 'Sign In' --label".to_string())
        );
    }

    #[test]
    fn test_tap_location_to_command() {
        let action = ActionType::TapLocation { x: 100, y: 200 };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex tap-location 100 200".to_string())
        );
    }

    #[test]
    fn test_swipe_to_command() {
        let action = ActionType::Swipe { direction: "up".to_string() };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex swipe up".to_string())
        );
    }

    #[test]
    fn test_send_keys_to_command() {
        let action = ActionType::SendKeys { text: "hello".to_string() };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex send-keys hello".to_string())
        );
    }

    #[test]
    fn test_send_keys_with_spaces() {
        let action = ActionType::SendKeys { text: "hello world".to_string() };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex send-keys 'hello world'".to_string())
        );
    }

    #[test]
    fn test_send_keys_with_single_quotes() {
        let action = ActionType::SendKeys { text: "it's".to_string() };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex send-keys 'it'\\''s'".to_string())
        );
    }

    #[test]
    fn test_screenshot_to_command() {
        assert_eq!(
            LogConverter::action_to_command(&ActionType::GetScreenshot, None),
            Some("qorvex screenshot".to_string())
        );
    }

    #[test]
    fn test_screen_info_to_command() {
        assert_eq!(
            LogConverter::action_to_command(&ActionType::GetScreenInfo, None),
            Some("qorvex screen-info".to_string())
        );
    }

    #[test]
    fn test_get_value_to_command() {
        let action = ActionType::GetValue {
            selector: "status-label".to_string(),
            by_label: false,
            element_type: None,
            timeout_ms: None,
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex get-value status-label".to_string())
        );
    }

    #[test]
    fn test_wait_for_to_command() {
        let action = ActionType::WaitFor {
            selector: "dashboard".to_string(),
            by_label: false,
            element_type: None,
            timeout_ms: 5000,
            require_stable: true,
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex wait-for dashboard -o 5000".to_string())
        );
    }

    #[test]
    fn test_wait_for_not_to_command() {
        let action = ActionType::WaitForNot {
            selector: "spinner".to_string(),
            by_label: true,
            element_type: Some("ActivityIndicator".to_string()),
            timeout_ms: 10000,
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex wait-for-not spinner --label -T ActivityIndicator -o 10000".to_string())
        );
    }

    #[test]
    fn test_long_press_to_command() {
        let action = ActionType::LongPress { x: 100, y: 200, duration: 1.5 };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("# TODO: long-press 100 200 1.5 (not yet supported)".to_string())
        );
    }

    #[test]
    fn test_set_target_to_command() {
        let action = ActionType::SetTarget { bundle_id: "com.example.App".to_string() };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex set-target com.example.App".to_string())
        );
    }

    #[test]
    fn test_log_comment_to_command() {
        let action = ActionType::LogComment { message: "test step".to_string() };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("# test step".to_string())
        );
    }

    #[test]
    fn test_session_actions_skipped() {
        assert!(LogConverter::action_to_command(&ActionType::StartSession, None).is_none());
        assert!(LogConverter::action_to_command(&ActionType::EndSession, None).is_none());
        assert!(LogConverter::action_to_command(&ActionType::Quit, None).is_none());
    }

    #[test]
    fn test_convert_jsonl() {
        use qorvex_core::action::ActionResult;

        let log1 = ActionLog::new(ActionType::StartSession, ActionResult::Success, None, None, None);
        let log2 = ActionLog::new(
            ActionType::Tap { selector: "btn".to_string(), by_label: false, element_type: None, timeout_ms: None },
            ActionResult::Success,
            None,
            None,
            None,
        );
        let log3 = ActionLog::new(ActionType::EndSession, ActionResult::Success, None, None, None);

        let jsonl = format!(
            "{}\n{}\n{}\n",
            serde_json::to_string(&log1).unwrap(),
            serde_json::to_string(&log2).unwrap(),
            serde_json::to_string(&log3).unwrap(),
        );

        let result = LogConverter::convert_str(&jsonl).unwrap();
        assert!(result.starts_with("#!/usr/bin/env bash\n"));
        assert!(result.contains("set -euo pipefail"));
        assert!(result.contains("qorvex tap btn"));
        assert!(!result.contains("start_session"));
        assert!(!result.contains("end_session"));
    }

    #[test]
    fn test_shell_escape_simple() {
        assert_eq!(shell_escape("hello"), "hello");
        assert_eq!(shell_escape("login-button"), "login-button");
        assert_eq!(shell_escape("com.example.App"), "com.example.App");
    }

    #[test]
    fn test_shell_escape_spaces() {
        assert_eq!(shell_escape("hello world"), "'hello world'");
    }

    #[test]
    fn test_shell_escape_single_quotes() {
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
    }

    #[test]
    fn test_tag_round_trip() {
        use qorvex_core::action::ActionResult;

        let log = ActionLog::new(
            ActionType::Tap { selector: "btn".to_string(), by_label: false, element_type: None, timeout_ms: None },
            ActionResult::Success,
            None,
            None,
            Some("hit-main-widget".to_string()),
        );

        let jsonl = serde_json::to_string(&log).unwrap();
        let parsed: ActionLog = serde_json::from_str(&jsonl).unwrap();
        assert_eq!(parsed.tag.as_deref(), Some("hit-main-widget"));

        let cmd = LogConverter::action_to_command(&parsed.action, parsed.tag.as_deref()).unwrap();
        assert_eq!(cmd, "qorvex tap btn --tag hit-main-widget");
    }

    #[test]
    fn test_tag_with_spaces() {
        let cmd = LogConverter::action_to_command(
            &ActionType::Tap { selector: "btn".to_string(), by_label: false, element_type: None, timeout_ms: None },
            Some("my tag"),
        ).unwrap();
        assert_eq!(cmd, "qorvex tap btn --tag 'my tag'");
    }
}
