use std::io::{self, Read};
use std::path::Path;

use qorvex_core::action::{ActionLog, ActionType, LabelMatch, LabelMatcher};

/// Convert JSONL action logs to shell scripts that call `qorvex` CLI commands.
pub struct LogConverter;

impl LogConverter {
    pub fn convert_file(path: &Path, batch: bool) -> Result<String, io::Error> {
        let content = std::fs::read_to_string(path)?;
        Self::convert_str_as(&content, batch)
    }

    pub fn convert_stdin(batch: bool) -> Result<String, io::Error> {
        let mut content = String::new();
        io::stdin().read_to_string(&mut content)?;
        Self::convert_str_as(&content, batch)
    }

    #[cfg(test)]
    fn convert_str(content: &str) -> Result<String, io::Error> {
        Self::convert_str_as(content, false)
    }

    /// Convert to a shell script, or with `batch` to a `qorvex batch` file:
    /// no shebang or `set`, no `qorvex ` prefix, comments as real `comment`
    /// lines. In batch mode a command that would span several lines is
    /// refused, naming the 1-based JSONL entry.
    fn convert_str_as(content: &str, batch: bool) -> Result<String, io::Error> {
        let mut lines = if batch {
            Vec::new()
        } else {
            vec![
                "#!/usr/bin/env bash".to_string(),
                "set -euo pipefail".to_string(),
                String::new(),
            ]
        };

        let mut entry = 0;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            entry += 1;

            let log: ActionLog = serde_json::from_str(line).map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidData, format!("Invalid JSONL: {}", e))
            })?;

            if let Some(cmd) = Self::action_to_command_as(&log.action, log.tag.as_deref(), batch) {
                if batch && cmd.contains(['\n', '\r']) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "entry {} contains a newline and cannot be one batch line",
                            entry
                        ),
                    ));
                }
                lines.push(cmd);
            }
        }

        Ok(lines.join("\n") + "\n")
    }

    #[cfg(test)]
    fn action_to_command(action: &ActionType, tag: Option<&str>) -> Option<String> {
        Self::action_to_command_as(action, tag, false)
    }

    fn action_to_command_as(action: &ActionType, tag: Option<&str>, batch: bool) -> Option<String> {
        let base = match action {
            ActionType::Tap {
                selector,
                by_label,
                element_type,
                fallback_frame,
                ..
            } => {
                let mut cmd = format!("qorvex tap {}", shell_escape(selector));
                if *by_label {
                    cmd.push_str(" --label");
                }
                if let Some(t) = element_type {
                    cmd.push_str(&format!(" -T {}", shell_escape(t)));
                }
                if *fallback_frame {
                    cmd.push_str(" --fallback-frame");
                }
                Some(cmd)
            }
            ActionType::TapLocation { x, y } => Some(format!("qorvex tap-location {} {}", x, y)),
            ActionType::Swipe {
                direction,
                duration_ms,
            } => Some(format!(
                "qorvex swipe {}{}",
                shell_escape(direction),
                duration_flag(*duration_ms)
            )),
            ActionType::SwipeFromElement {
                selector,
                by_label,
                element_type,
                label_match,
                direction,
                duration_ms,
            } => {
                let mut cmd = format!("qorvex swipe {} ", direction.as_str());
                match label_match {
                    Some(_) => cmd.push_str(&selector_args(selector, label_match)),
                    None => cmd.push_str(&format!("--from-element {}", shell_escape(selector))),
                }
                if *by_label {
                    cmd.push_str(" --label");
                }
                if let Some(t) = element_type {
                    cmd.push_str(&format!(" -T {}", shell_escape(t)));
                }
                cmd.push_str(&duration_flag(*duration_ms));
                Some(cmd)
            }
            ActionType::SwipeCoords {
                from_x,
                from_y,
                to_x,
                to_y,
                duration_ms,
            } => Some(format!(
                "qorvex swipe --from {},{} --to {},{}{}",
                from_x,
                from_y,
                to_x,
                to_y,
                duration_flag(*duration_ms)
            )),
            ActionType::Lock => Some("qorvex lock".to_string()),
            ActionType::Key { name } => Some(format!("qorvex key {}", name.as_str())),
            ActionType::HideKeyboard => Some("qorvex hide-keyboard".to_string()),
            ActionType::SendKeys { text, replace } => Some(format!(
                "qorvex send-keys {}{}",
                if *replace { "--replace " } else { "" },
                shell_escape(text)
            )),
            ActionType::GetScreenshot => Some("qorvex screenshot".to_string()),
            ActionType::GetScreenInfo => Some("qorvex screen-info".to_string()),
            ActionType::GetScreenTree => Some("qorvex screen-info --all".to_string()),
            ActionType::GetValue {
                selector,
                by_label,
                element_type,
                ..
            } => {
                let mut cmd = format!("qorvex get-value {}", shell_escape(selector));
                if *by_label {
                    cmd.push_str(" --label");
                }
                if let Some(t) = element_type {
                    cmd.push_str(&format!(" -T {}", shell_escape(t)));
                }
                Some(cmd)
            }
            ActionType::Exists {
                selector,
                by_label,
                element_type,
                label_match,
            } => {
                let mut cmd = format!("qorvex exists {}", selector_args(selector, label_match));
                if *by_label {
                    cmd.push_str(" --label");
                }
                if let Some(t) = element_type {
                    cmd.push_str(&format!(" -T {}", shell_escape(t)));
                }
                Some(cmd)
            }
            ActionType::ScrollTo {
                selector,
                by_label,
                element_type,
                label_match,
                direction,
                max_swipes,
                duration_ms,
            } => {
                let mut cmd = format!("qorvex scroll-to {}", selector_args(selector, label_match));
                if *by_label {
                    cmd.push_str(" --label");
                }
                if let Some(t) = element_type {
                    cmd.push_str(&format!(" -T {}", shell_escape(t)));
                }
                cmd.push_str(&format!(
                    " --direction {} --max-swipes {}",
                    direction.as_str(),
                    max_swipes
                ));
                if let Some(ms) = duration_ms {
                    cmd.push_str(&format!(" --duration-ms {}", ms));
                }
                Some(cmd)
            }
            ActionType::WaitFor {
                selector,
                by_label,
                element_type,
                label_match,
                timeout_ms,
                ..
            } => {
                let mut cmd = format!("qorvex wait-for {}", selector_args(selector, label_match));
                if *by_label {
                    cmd.push_str(" --label");
                }
                if let Some(t) = element_type {
                    cmd.push_str(&format!(" -T {}", shell_escape(t)));
                }
                cmd.push_str(&format!(" -o {}", timeout_ms));
                Some(cmd)
            }
            ActionType::WaitForNot {
                selector,
                by_label,
                element_type,
                label_match,
                timeout_ms,
            } => {
                let mut cmd = format!(
                    "qorvex wait-for-not {}",
                    selector_args(selector, label_match)
                );
                if *by_label {
                    cmd.push_str(" --label");
                }
                if let Some(t) = element_type {
                    cmd.push_str(&format!(" -T {}", shell_escape(t)));
                }
                cmd.push_str(&format!(" -o {}", timeout_ms));
                Some(cmd)
            }
            ActionType::WaitStable {
                stable_ms,
                timeout_ms,
            } => Some(format!(
                "qorvex wait-stable --for {} -o {}",
                stable_ms, timeout_ms
            )),
            ActionType::LongPress { x, y, duration } => Some(format!(
                "qorvex long-press {} {} --duration {}",
                x, y, duration
            )),
            ActionType::SetTarget { bundle_id } => {
                Some(format!("qorvex set-target {}", shell_escape(bundle_id)))
            }
            // Carry `--force` through: a replayed script that dropped it would
            // attach to whatever is already running instead of relaunching.
            ActionType::StartTarget { force } => Some(if *force {
                "qorvex start-target --force".to_string()
            } else {
                "qorvex start-target".to_string()
            }),
            ActionType::StopTarget => Some("qorvex stop-target".to_string()),
            ActionType::ForegroundTarget => Some("qorvex foreground-target".to_string()),
            ActionType::GetTargetInfo => Some("qorvex get-target-info".to_string()),
            ActionType::MemoryInfo { .. } => Some("qorvex memory-info".to_string()),
            ActionType::ShutdownDevice => Some("qorvex shutdown-device".to_string()),
            ActionType::DeleteDevice => Some("qorvex delete-device".to_string()),
            ActionType::InstallApp { path } => {
                Some(format!("qorvex install-app {}", shell_escape(path)))
            }
            ActionType::UninstallApp { bundle_id } => {
                Some(format!("qorvex uninstall-app {}", shell_escape(bundle_id)))
            }
            ActionType::ResetApp { bundle_id } => {
                Some(format!("qorvex reset-app {}", shell_escape(bundle_id)))
            }
            ActionType::AppContainer {
                bundle_id,
                container,
            } => {
                let mut cmd = format!("qorvex app-container {}", shell_escape(bundle_id));
                if let Some(kind) = container {
                    cmd.push_str(&format!(" {}", shell_escape(kind)));
                }
                Some(cmd)
            }
            ActionType::ListApps => Some("qorvex list-apps".to_string()),
            ActionType::CreateDevice {
                name,
                device_type,
                runtime,
            } => Some(format!(
                "qorvex create-device {} {} {}",
                shell_escape(name),
                shell_escape(device_type),
                shell_escape(runtime)
            )),
            ActionType::WaitForBoot => Some("qorvex wait-for-boot".to_string()),
            ActionType::DeviceLog { last, predicate } => {
                let mut cmd = format!("qorvex device-log --last {}", shell_escape(last));
                if let Some(p) = predicate {
                    cmd.push_str(&format!(" --predicate {}", shell_escape(p)));
                }
                Some(cmd)
            }
            ActionType::SetAppearance { appearance } => Some(format!(
                "qorvex set-appearance {}",
                shell_escape(appearance)
            )),
            ActionType::SetContentSize { size } => {
                Some(format!("qorvex set-content-size {}", shell_escape(size)))
            }
            ActionType::GrantPermission {
                verb,
                service,
                bundle_id,
            } => Some(format!(
                "qorvex grant-permission {} {} {}",
                shell_escape(verb),
                shell_escape(service),
                shell_escape(bundle_id)
            )),
            ActionType::Biometric { kind, touch_id } => Some(format!(
                "qorvex biometric {}{}",
                shell_escape(kind),
                if *touch_id { " --touch-id" } else { "" }
            )),
            ActionType::AddMedia { paths } => Some(format!(
                "qorvex add-media {}",
                paths
                    .iter()
                    .map(|p| shell_escape(p))
                    .collect::<Vec<_>>()
                    .join(" ")
            )),
            ActionType::OpenUrl { url } => Some(format!("qorvex open-url {}", shell_escape(url))),
            ActionType::LogComment { message } if batch => {
                // Quote an empty or `-`-leading message so clap reads it as the positional.
                let text = if message.is_empty() || message.starts_with('-') {
                    format!("'{}'", message.replace('\'', "'\\''"))
                } else {
                    shell_escape(message)
                };
                Some(format!("qorvex comment {}", text))
            }
            ActionType::LogComment { message } => Some(format!("# {}", message)),
            // Skip session management actions
            ActionType::StartSession | ActionType::EndSession | ActionType::Quit => None,
        };
        match base {
            Some(mut cmd) => {
                if let Some(t) = tag {
                    cmd.push_str(&format!(" --tag {}", shell_escape(t)));
                }
                if batch {
                    cmd = cmd.strip_prefix("qorvex ").unwrap_or(&cmd).to_string();
                }
                Some(cmd)
            }
            None => None,
        }
    }
}

/// The selector part of an element command: the escaped positional
/// selector, or the `--label-contains`/`--label-regex` [`--nth`] flags.
fn selector_args(selector: &str, label_match: &Option<Box<LabelMatch>>) -> String {
    let Some(m) = label_match else {
        return shell_escape(selector);
    };
    let mut out = match &m.matcher {
        LabelMatcher::Contains(t) => format!("--label-contains {}", shell_escape(t)),
        LabelMatcher::Regex(r) => format!("--label-regex {}", shell_escape(r)),
    };
    if let Some(n) = m.nth {
        out.push_str(&format!(" --nth {}", n));
    }
    out
}

fn duration_flag(duration_ms: Option<u32>) -> String {
    duration_ms.map_or_else(String::new, |ms| format!(" --duration-ms {}", ms))
}

/// Shell-escape a string using single quotes. Internal single quotes become `'\''`.
fn shell_escape(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/')
    {
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
            fallback_frame: false,
            report: false,
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
            fallback_frame: false,
            report: false,
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
            fallback_frame: false,
            report: false,
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex tap Submit --label -T Button".to_string())
        );
    }

    #[test]
    fn test_tap_fallback_frame_to_command() {
        let action = ActionType::Tap {
            selector: "Tap Me".to_string(),
            by_label: true,
            element_type: None,
            timeout_ms: Some(5000),
            fallback_frame: true,
            report: true,
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex tap 'Tap Me' --label --fallback-frame".to_string())
        );
    }

    #[test]
    fn test_tap_selector_with_spaces() {
        let action = ActionType::Tap {
            selector: "Sign In".to_string(),
            by_label: true,
            element_type: None,
            timeout_ms: None,
            fallback_frame: false,
            report: false,
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
        let action = ActionType::Swipe {
            direction: "up".to_string(),
            duration_ms: None,
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex swipe up".to_string())
        );
    }

    #[test]
    fn test_swipe_duration_to_command() {
        let action = ActionType::Swipe {
            direction: "up".to_string(),
            duration_ms: Some(800),
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex swipe up --duration-ms 800".to_string())
        );
        let action = ActionType::SwipeCoords {
            from_x: 10,
            from_y: 20,
            to_x: 30,
            to_y: 40,
            duration_ms: Some(1500),
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex swipe --from 10,20 --to 30,40 --duration-ms 1500".to_string())
        );
    }

    #[test]
    fn test_swipe_from_element_to_command() {
        use qorvex_core::action::ScrollDirection;
        let action = ActionType::SwipeFromElement {
            selector: "gesture-swipe-area".to_string(),
            by_label: false,
            element_type: Some("Other".to_string()),
            label_match: None,
            direction: ScrollDirection::Left,
            duration_ms: Some(800),
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some(
                "qorvex swipe left --from-element gesture-swipe-area -T Other --duration-ms 800"
                    .to_string()
            )
        );
        let action = ActionType::SwipeFromElement {
            selector: String::new(),
            by_label: false,
            element_type: None,
            label_match: Some(Box::new(LabelMatch {
                matcher: LabelMatcher::Contains("Privacy".to_string()),
                nth: Some(1),
            })),
            direction: ScrollDirection::Up,
            duration_ms: None,
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex swipe up --label-contains Privacy --nth 1".to_string())
        );
    }

    #[test]
    fn test_swipe_coords_to_command() {
        let action = ActionType::SwipeCoords {
            from_x: 10,
            from_y: 20,
            to_x: 30,
            to_y: 40,
            duration_ms: None,
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex swipe --from 10,20 --to 30,40".to_string())
        );
    }

    #[test]
    fn test_lock_to_command() {
        assert_eq!(
            LogConverter::action_to_command(&ActionType::Lock, None),
            Some("qorvex lock".to_string())
        );
    }

    #[test]
    fn test_key_to_command() {
        use qorvex_core::protocol::KeyName;
        for (name, want) in [
            (KeyName::Back, "qorvex key back"),
            (KeyName::Home, "qorvex key home"),
        ] {
            assert_eq!(
                LogConverter::action_to_command(&ActionType::Key { name }, None),
                Some(want.to_string())
            );
        }
    }

    #[test]
    fn test_send_keys_to_command() {
        let action = ActionType::SendKeys {
            text: "hello".to_string(),
            replace: false,
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex send-keys hello".to_string())
        );
    }

    #[test]
    fn test_send_keys_with_spaces() {
        let action = ActionType::SendKeys {
            text: "hello world".to_string(),
            replace: false,
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex send-keys 'hello world'".to_string())
        );
    }

    #[test]
    fn test_send_keys_with_single_quotes() {
        let action = ActionType::SendKeys {
            text: "it's".to_string(),
            replace: false,
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex send-keys 'it'\\''s'".to_string())
        );
    }

    #[test]
    fn test_send_keys_replace() {
        let action = ActionType::SendKeys {
            text: "hello world".to_string(),
            replace: true,
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex send-keys --replace 'hello world'".to_string())
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
    fn test_scroll_to_to_command() {
        let action = ActionType::ScrollTo {
            selector: "row 12".to_string(),
            by_label: true,
            element_type: Some("Cell".to_string()),
            label_match: None,
            direction: qorvex_core::action::ScrollDirection::Down,
            max_swipes: 5,
            duration_ms: Some(800),
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some(
                "qorvex scroll-to 'row 12' --label -T Cell --direction down --max-swipes 5 --duration-ms 800"
                    .to_string()
            )
        );
    }

    #[test]
    fn test_exists_to_command() {
        let action = ActionType::Exists {
            label_match: None,
            selector: "Tap Me".to_string(),
            by_label: true,
            element_type: Some("Button".to_string()),
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex exists 'Tap Me' --label -T Button".to_string())
        );
    }

    #[test]
    fn test_label_match_to_command() {
        let action = ActionType::Exists {
            selector: String::new(),
            by_label: false,
            element_type: Some("Button".to_string()),
            label_match: Some(Box::new(LabelMatch {
                matcher: LabelMatcher::Contains("Tap Me".to_string()),
                nth: None,
            })),
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex exists --label-contains 'Tap Me' -T Button".to_string())
        );
        let action = ActionType::WaitFor {
            selector: String::new(),
            by_label: false,
            element_type: None,
            timeout_ms: 5000,
            require_stable: true,
            label_match: Some(Box::new(LabelMatch {
                matcher: LabelMatcher::Regex("^Tapped: [0-9]+$".to_string()),
                nth: Some(1),
            })),
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex wait-for --label-regex '^Tapped: [0-9]+$' --nth 1 -o 5000".to_string())
        );
        let action = ActionType::WaitForNot {
            selector: String::new(),
            by_label: false,
            element_type: None,
            timeout_ms: 100,
            label_match: Some(Box::new(LabelMatch {
                matcher: LabelMatcher::Contains("spin".to_string()),
                nth: None,
            })),
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex wait-for-not --label-contains spin -o 100".to_string())
        );
    }

    #[test]
    fn test_wait_stable_to_command() {
        let action = ActionType::WaitStable {
            stable_ms: 500,
            timeout_ms: 5000,
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex wait-stable --for 500 -o 5000".to_string())
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
            label_match: None,
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
            label_match: None,
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
        let action = ActionType::LongPress {
            x: 100,
            y: 200,
            duration: 1.5,
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex long-press 100 200 --duration 1.5".to_string())
        );
    }

    #[test]
    fn test_set_target_to_command() {
        let action = ActionType::SetTarget {
            bundle_id: "com.example.App".to_string(),
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("qorvex set-target com.example.App".to_string())
        );
    }

    #[test]
    fn test_log_comment_to_command() {
        let action = ActionType::LogComment {
            message: "test step".to_string(),
        };
        assert_eq!(
            LogConverter::action_to_command(&action, None),
            Some("# test step".to_string())
        );
    }

    fn batch(jsonl: &str) -> Result<String, io::Error> {
        LogConverter::convert_str_as(jsonl, true)
    }

    #[test]
    fn test_batch_strips_prefix_and_header() {
        let jsonl = r#"{"id":"00000000000000000000000000000001","timestamp":"2025-01-01T00:00:00Z","action":{"type":"TapLocation","x":200,"y":400},"result":"Success","screenshot":null,"tag":"t1"}
{"id":"00000000000000000000000000000002","timestamp":"2025-01-01T00:00:01Z","action":{"type":"StartSession"},"result":"Success","screenshot":null}
"#;
        let out = batch(jsonl).unwrap();
        assert!(!out.contains("#!/"));
        assert!(!out.contains("set -e"));
        assert!(!out.contains("qorvex"));
        assert_eq!(
            out,
            format!(
                "{}\n",
                LogConverter::action_to_command(
                    &ActionType::TapLocation { x: 200, y: 400 },
                    Some("t1")
                )
                .unwrap()
                .strip_prefix("qorvex ")
                .unwrap()
            )
        );
        assert!(out.contains("--tag t1"));
    }

    #[test]
    fn test_batch_comment_is_real_line() {
        let jsonl = r#"{"id":"00000000000000000000000000000001","timestamp":"2025-01-01T00:00:00Z","action":{"type":"LogComment","message":"it's two"},"result":"Success","screenshot":null}
"#;
        assert_eq!(batch(jsonl).unwrap(), "comment 'it'\\''s two'\n");
        assert_eq!(
            crate::converter::LogConverter::convert_str(jsonl).unwrap(),
            "#!/usr/bin/env bash\nset -euo pipefail\n\n# it's two\n"
        );
    }

    #[test]
    fn test_batch_comment_empty_and_dash_are_quoted() {
        for (msg, want) in [("", "comment ''\n"), ("-x", "comment '-x'\n")] {
            let jsonl = format!(
                r#"{{"id":"00000000000000000000000000000001","timestamp":"2025-01-01T00:00:00Z","action":{{"type":"LogComment","message":"{msg}"}},"result":"Success","screenshot":null}}"#
            );
            assert_eq!(batch(&jsonl).unwrap(), want);
        }
    }

    #[test]
    fn test_batch_rejects_newline_with_entry_number() {
        let jsonl = r#"{"id":"00000000000000000000000000000001","timestamp":"2025-01-01T00:00:00Z","action":{"type":"LogComment","message":"ok"},"result":"Success","screenshot":null}

{"id":"00000000000000000000000000000002","timestamp":"2025-01-01T00:00:00Z","action":{"type":"LogComment","message":"a\nb"},"result":"Success","screenshot":null}
"#;
        let err = batch(jsonl).unwrap_err().to_string();
        assert!(err.contains("entry 2"), "{err}");
        assert!(LogConverter::convert_str(jsonl).is_ok());
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

        let log1 = ActionLog::new(
            ActionType::StartSession,
            ActionResult::Success,
            None,
            None,
            None,
        );
        let log2 = ActionLog::new(
            ActionType::Tap {
                selector: "btn".to_string(),
                by_label: false,
                element_type: None,
                timeout_ms: None,
                fallback_frame: false,
                report: false,
            },
            ActionResult::Success,
            None,
            None,
            None,
        );
        let log3 = ActionLog::new(
            ActionType::EndSession,
            ActionResult::Success,
            None,
            None,
            None,
        );

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
            ActionType::Tap {
                selector: "btn".to_string(),
                by_label: false,
                element_type: None,
                timeout_ms: None,
                fallback_frame: false,
                report: false,
            },
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
            &ActionType::Tap {
                selector: "btn".to_string(),
                by_label: false,
                element_type: None,
                timeout_ms: None,
                fallback_frame: false,
                report: false,
            },
            Some("my tag"),
        )
        .unwrap();
        assert_eq!(cmd, "qorvex tap btn --tag 'my tag'");
    }

    #[test]
    fn test_start_target_to_command() {
        assert_eq!(
            LogConverter::action_to_command(&ActionType::StartTarget { force: false }, None),
            Some("qorvex start-target".to_string())
        );
    }

    #[test]
    fn test_start_target_force_to_command() {
        assert_eq!(
            LogConverter::action_to_command(&ActionType::StartTarget { force: true }, None),
            Some("qorvex start-target --force".to_string())
        );
    }

    #[test]
    fn test_stop_target_to_command() {
        assert_eq!(
            LogConverter::action_to_command(&ActionType::StopTarget, None),
            Some("qorvex stop-target".to_string())
        );
    }

    #[test]
    fn test_foreground_target_to_command() {
        assert_eq!(
            LogConverter::action_to_command(&ActionType::ForegroundTarget, None),
            Some("qorvex foreground-target".to_string())
        );
    }
}
