use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{info, debug};

use qorvex_core::action::{ActionResult, ActionType};
use qorvex_core::driver::DriverConfig;
use qorvex_core::executor::ActionExecutor;

use qorvex_core::session::Session;
use qorvex_core::simctl::Simctl;
use qorvex_core::watcher::{ScreenWatcher, WatcherConfig, WatcherHandle};

use crate::ast::*;
use crate::error::AutoError;
use crate::parser;
use crate::runtime::{Runtime, Value};

pub struct ScriptExecutor {
    runtime: Runtime,
    session: Arc<Session>,
    executor: Option<ActionExecutor>,
    simulator_udid: Option<String>,
    watcher_handle: Option<WatcherHandle>,
    default_timeout_ms: u64,
    base_dir: PathBuf,
    include_stack: HashSet<PathBuf>,
    driver_config: DriverConfig,
}

impl ScriptExecutor {
    pub async fn new(session: Arc<Session>, simulator_udid: Option<String>, base_dir: PathBuf, driver_config: DriverConfig) -> Self {
        let executor = if simulator_udid.is_some() {
            ActionExecutor::from_config_connected(driver_config.clone()).await.ok()
        } else {
            None
        };
        Self {
            runtime: Runtime::new(),
            session,
            executor,
            simulator_udid,
            watcher_handle: None,
            default_timeout_ms: 5000,
            base_dir,
            include_stack: HashSet::new(),
            driver_config,
        }
    }

    pub async fn execute_script(&mut self, script: &Script) -> Result<(), AutoError> {
        for stmt in &script.statements {
            self.execute_statement(stmt).await?;
        }
        Ok(())
    }

    fn execute_statement<'a>(
        &'a mut self,
        stmt: &'a Statement,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), AutoError>> + 'a>> {
        Box::pin(async move {
            match stmt {
                Statement::Command(call) => {
                    self.execute_command(call).await?;
                    Ok(())
                }
                Statement::Assignment { variable, value } => {
                    let val = self.eval_expression(value, 0).await?;
                    self.runtime.set(variable.clone(), val);
                    Ok(())
                }
                Statement::Foreach { variable, collection, body } => {
                    let line = match collection {
                        Expression::CommandCapture(call) => call.line,
                        _ => 0,
                    };
                    let coll = self.eval_expression(collection, line).await?;
                    let items = match coll {
                        Value::List(items) => items,
                        _ => return Err(AutoError::Runtime {
                            message: "foreach requires a list".to_string(),
                            line: 0,
                        }),
                    };

                    for item in items {
                        self.runtime.set(variable.clone(), item);
                        for stmt in body {
                            self.execute_statement(stmt).await?;
                        }
                    }
                    Ok(())
                }
                Statement::For { variable, from, to, body } => {
                    for i in *from..=*to {
                        self.runtime.set(variable.clone(), Value::Number(i));
                        for stmt in body {
                            self.execute_statement(stmt).await?;
                        }
                    }
                    Ok(())
                }
                Statement::If { condition, then_block, else_block } => {
                    let line = match condition {
                        Expression::CommandCapture(call) => call.line,
                        _ => 0,
                    };
                    let cond_val = self.eval_expression(condition, line).await?;
                    if cond_val.is_truthy() {
                        for stmt in then_block {
                            self.execute_statement(stmt).await?;
                        }
                    } else if let Some(else_stmts) = else_block {
                        for stmt in else_stmts {
                            self.execute_statement(stmt).await?;
                        }
                    }
                    Ok(())
                }
                Statement::Set { key, value, line } => {
                    let val = self.eval_expression(value, *line).await?;
                    match key.as_str() {
                        "timeout" => {
                            let ms: u64 = val.as_string().parse().map_err(|_| AutoError::Runtime {
                                message: format!("Invalid timeout value: {}", val.as_string()),
                                line: *line,
                            })?;
                            self.default_timeout_ms = ms;
                            info!(line, ms, "default timeout set");
                        }
                        _ => {
                            return Err(AutoError::Runtime {
                                message: format!("Unknown setting: {}", key),
                                line: *line,
                            });
                        }
                    }
                    Ok(())
                }
                Statement::Include { path, line } => {
                    let val = self.eval_expression(path, *line).await?;
                    let raw_path = val.as_string();
                    let resolved = self.resolve_include_path(&raw_path);
                    let canonical = resolved.canonicalize().map_err(|e| AutoError::Runtime {
                        message: format!("Cannot resolve include path '{}': {}", raw_path, e),
                        line: *line,
                    })?;

                    if self.include_stack.contains(&canonical) {
                        return Err(AutoError::Runtime {
                            message: format!("Circular include detected: {}", canonical.display()),
                            line: *line,
                        });
                    }

                    let source = std::fs::read_to_string(&canonical).map_err(|e| AutoError::Runtime {
                        message: format!("Cannot read '{}': {}", canonical.display(), e),
                        line: *line,
                    })?;
                    let included_script = parser::parse(&source).map_err(|e| AutoError::Runtime {
                        message: format!("In included file '{}': {}", raw_path, e),
                        line: *line,
                    })?;

                    let prev_base = self.base_dir.clone();
                    if let Some(parent) = canonical.parent() {
                        self.base_dir = parent.to_path_buf();
                    }
                    self.include_stack.insert(canonical.clone());

                    info!(line, path = %canonical.display(), "including file");
                    let result = self.execute_script(&included_script).await;

                    self.include_stack.remove(&canonical);
                    self.base_dir = prev_base;

                    result
                }
            }
        })
    }

    fn eval_expression<'a>(
        &'a mut self,
        expr: &'a Expression,
        fallback_line: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, AutoError>> + 'a>> {
        Box::pin(async move {
            match expr {
                Expression::CommandCapture(call) => {
                    let result = self.execute_command(call).await?;
                    Ok(result)
                }
                Expression::BinaryOp { op, left, right } => {
                    let lhs = self.eval_expression(left, fallback_line).await?;
                    let rhs = self.eval_expression(right, fallback_line).await?;
                    match op {
                        BinOp::Add => {
                            if let (Value::Number(a), Value::Number(b)) = (&lhs, &rhs) {
                                Ok(Value::Number(a + b))
                            } else {
                                Ok(Value::String(format!("{}{}", lhs.as_string(), rhs.as_string())))
                            }
                        }
                        BinOp::Eq => Ok(Value::Number(if lhs == rhs { 1 } else { 0 })),
                        BinOp::NotEq => Ok(Value::Number(if lhs != rhs { 1 } else { 0 })),
                    }
                }
                other => self.runtime.eval_expression(other, fallback_line),
            }
        })
    }

    fn eval_args_to_strings(&self, args: &[Expression], line: usize) -> Result<Vec<String>, AutoError> {
        args.iter()
            .map(|arg| {
                let val = self.runtime.eval_expression(arg, line)?;
                Ok(val.as_string())
            })
            .collect()
    }

    async fn execute_command(&mut self, call: &CommandCall) -> Result<Value, AutoError> {
        let line = call.line;
        let args = self.eval_args_to_strings(&call.args, line)?;

        match call.name.as_str() {
            "start_session" => {
                debug!(line, "start_session is handled automatically");
                Ok(Value::String("Session started".to_string()))
            }
            "end_session" => {
                debug!(line, "end_session is handled automatically");
                Ok(Value::String("Session ended".to_string()))
            }
            "use_device" => {
                let udid = args.first().ok_or_else(|| AutoError::Runtime {
                    message: "use_device requires 1 argument: use_device(udid)".to_string(),
                    line,
                })?;
                self.simulator_udid = Some(udid.clone());
                let mut executor = ActionExecutor::from_config_connected(self.driver_config.clone())
                    .await
                    .map_err(|e| AutoError::ActionFailed {
                        message: format!("Failed to connect to automation backend: {}", e),
                        line,
                    })?;
                executor.set_capture_screenshots(false);
                self.executor = Some(executor);
                info!(line, udid = %udid, "using device");
                Ok(Value::String(format!("Using device {}", udid)))
            }
            "boot_device" => {
                let udid = args.first().ok_or_else(|| AutoError::Runtime {
                    message: "boot_device requires 1 argument: boot_device(udid)".to_string(),
                    line,
                })?;
                Simctl::boot(udid).map_err(|e| AutoError::ActionFailed {
                    message: e.to_string(),
                    line,
                })?;
                self.simulator_udid = Some(udid.clone());
                let mut executor = ActionExecutor::from_config_connected(self.driver_config.clone())
                    .await
                    .map_err(|e| AutoError::ActionFailed {
                        message: format!("Failed to connect to automation backend: {}", e),
                        line,
                    })?;
                executor.set_capture_screenshots(false);
                self.executor = Some(executor);
                info!(line, udid = %udid, "booted device");
                Ok(Value::String(format!("Booted device {}", udid)))
            }
            "start_watcher" => {
                if self.watcher_handle.is_some() {
                    return Err(AutoError::Runtime {
                        message: "Watcher already running".to_string(),
                        line,
                    });
                }
                let _udid = self.simulator_udid.as_ref().ok_or_else(|| AutoError::Runtime {
                    message: "No simulator selected".to_string(),
                    line,
                })?;
                let interval_ms: u64 = args.first()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(500);
                let config = WatcherConfig {
                    interval_ms,
                    capture_screenshots: true,
                    visual_change_threshold: 5,
                };
                let driver = self.executor.as_ref()
                    .ok_or_else(|| AutoError::Runtime {
                        message: "No executor available for watcher".to_string(),
                        line,
                    })?
                    .driver()
                    .clone();
                let handle = ScreenWatcher::spawn(self.session.clone(), driver, config);
                self.watcher_handle = Some(handle);
                info!(line, interval_ms, "watcher started");
                Ok(Value::String("Watcher started".to_string()))
            }
            "stop_watcher" => {
                if let Some(handle) = self.watcher_handle.take() {
                    handle.cancel();
                    info!(line, "watcher stopped");
                    Ok(Value::String("Watcher stopped".to_string()))
                } else {
                    Err(AutoError::Runtime {
                        message: "No watcher running".to_string(),
                        line,
                    })
                }
            }
            "set_target" => {
                let bundle_id = args.first().ok_or_else(|| AutoError::Runtime {
                    message: "set_target requires 1 argument: set_target(bundle_id)".to_string(),
                    line,
                })?;
                let executor = self.require_executor(line)?;
                executor.driver().set_target(bundle_id).await.map_err(|e| AutoError::ActionFailed {
                    message: e.to_string(),
                    line,
                })?;
                info!(line, bundle_id = %bundle_id, "target set");
                Ok(Value::String(format!("Target set to {}", bundle_id)))
            }
            "list_devices" => {
                let devices = Simctl::list_devices().map_err(|e| AutoError::ActionFailed {
                    message: e.to_string(),
                    line,
                })?;
                for d in &devices {
                    debug!(name = %d.name, udid = %d.udid, state = %d.state, "device");
                }
                info!(line, count = devices.len(), "listed devices");
                Ok(Value::String(format!("{} devices", devices.len())))
            }
            "list_elements" | "get_screen_info" => {
                let action_type = ActionType::GetScreenInfo;
                let executor = self.require_executor(line)?;
                let result = executor.execute(action_type.clone()).await;

                let action_result = if result.success {
                    ActionResult::Success
                } else {
                    ActionResult::Failure(result.message.clone())
                };
                self.session.log_action(action_type, action_result, result.screenshot.clone(), None).await;

                if result.success {
                    if let Some(ref data) = result.data {
                        info!(line, msg = %result.message, "screen info");
                        Ok(Value::String(data.clone()))
                    } else {
                        Ok(Value::String(result.message))
                    }
                } else {
                    Err(AutoError::ActionFailed { message: result.message, line })
                }
            }
            "get_session_info" => {
                let action_log = self.session.get_action_log().await;
                let info = format!("Session: {} actions", action_log.len());
                info!(line, msg = %info, "session info");
                Ok(Value::String(info))
            }
            "help" => {
                info!("available commands: start_session, end_session, tap, swipe, send_keys, wait_for, get_value, get_screenshot, get_screen_info, list_elements, list_devices, use_device, boot_device, set_target, tap_location, log, log_comment, start_watcher, stop_watcher, get_session_info, help");
                Ok(Value::String("help".to_string()))
            }
            "tap" => {
                let no_wait = args.iter().any(|s| s.trim() == "--no-wait");
                let args: Vec<String> = args.iter().filter(|s| s.trim() != "--no-wait").cloned().collect();
                let selector = args.first().ok_or_else(|| AutoError::Runtime {
                    message: "tap requires at least 1 argument".to_string(),
                    line,
                })?.clone();
                let by_label = args.get(1).map(|s| s.to_lowercase() == "label").unwrap_or(false);
                let element_type = args.get(2).cloned().filter(|s| !s.is_empty());

                if !no_wait {
                    self.wait_for_element_exists(&selector, by_label, element_type.as_deref(), line).await?;
                }

                let action = ActionType::Tap { selector, by_label, element_type };
                self.execute_action(action, line).await
            }
            "swipe" => {
                let direction = args.first().cloned().unwrap_or_else(|| "up".to_string());
                let action = ActionType::Swipe { direction };
                self.execute_action(action, line).await
            }
            "tap_location" => {
                if args.len() < 2 {
                    return Err(AutoError::Runtime {
                        message: "tap_location requires 2 arguments: tap_location(x, y)".to_string(),
                        line,
                    });
                }
                let x: i32 = args[0].parse().map_err(|_| AutoError::Runtime {
                    message: format!("Invalid x coordinate: {}", args[0]),
                    line,
                })?;
                let y: i32 = args[1].parse().map_err(|_| AutoError::Runtime {
                    message: format!("Invalid y coordinate: {}", args[1]),
                    line,
                })?;
                let action = ActionType::TapLocation { x, y };
                self.execute_action(action, line).await
            }
            "send_keys" => {
                let text = args.first().ok_or_else(|| AutoError::Runtime {
                    message: "send_keys requires 1 argument".to_string(),
                    line,
                })?.clone();
                let action = ActionType::SendKeys { text };
                self.execute_action(action, line).await
            }
            "wait_for" => {
                let selector = args.first().ok_or_else(|| AutoError::Runtime {
                    message: "wait_for requires at least 1 argument".to_string(),
                    line,
                })?.clone();
                let timeout_ms: u64 = args.get(1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(self.default_timeout_ms);
                let by_label = args.get(2).map(|s| s.to_lowercase() == "label").unwrap_or(false);
                let element_type = args.get(3).cloned().filter(|s| !s.is_empty());

                let action = ActionType::WaitFor { selector, by_label, element_type, timeout_ms };
                self.execute_action(action, line).await
            }
            "get_value" => {
                let no_wait = args.iter().any(|s| s.trim() == "--no-wait");
                let args: Vec<String> = args.iter().filter(|s| s.trim() != "--no-wait").cloned().collect();
                let selector = args.first().ok_or_else(|| AutoError::Runtime {
                    message: "get_value requires at least 1 argument".to_string(),
                    line,
                })?.clone();
                let by_label = args.get(1).map(|s| s.to_lowercase() == "label").unwrap_or(false);
                let element_type = args.get(2).cloned().filter(|s| !s.is_empty());

                if !no_wait {
                    let wait_action = ActionType::WaitFor {
                        selector: selector.clone(),
                        by_label,
                        element_type: element_type.clone(),
                        timeout_ms: self.default_timeout_ms,
                    };
                    self.execute_action(wait_action, line).await?;
                }

                let action = ActionType::GetValue { selector, by_label, element_type };
                let executor = self.require_executor(line)?;
                let result = executor.execute(action.clone()).await;

                let action_result = if result.success {
                    ActionResult::Success
                } else {
                    ActionResult::Failure(result.message.clone())
                };
                self.session.log_action(action, action_result, result.screenshot.clone(), None).await;

                if result.success {
                    let data = result.data.unwrap_or_else(|| "null".to_string());
                    info!(line, value = %data, "got value");
                    Ok(Value::String(data))
                } else {
                    Err(AutoError::ActionFailed { message: result.message, line })
                }
            }
            "get_screenshot" => {
                let action = ActionType::GetScreenshot;
                self.execute_action(action, line).await
            }
            "log" | "log_comment" => {
                let message = args.first().ok_or_else(|| AutoError::Runtime {
                    message: "log_comment requires 1 argument".to_string(),
                    line,
                })?.clone();
                let action = ActionType::LogComment { message: message.clone() };
                self.session.log_action(action, ActionResult::Success, None, None).await;
                info!(line, msg = %message, "logged comment");
                Ok(Value::String(message))
            }
            _ => Err(AutoError::Runtime {
                message: format!("Unknown command: {}", call.name),
                line,
            }),
        }
    }

    /// Extract elapsed_ms from execution result data JSON, if present.
    fn extract_elapsed_ms(data: &Option<String>) -> Option<u64> {
        let d = data.as_ref()?;
        let parsed: serde_json::Value = serde_json::from_str(d).ok()?;
        parsed.get("elapsed_ms").and_then(|v| v.as_u64())
    }

    fn resolve_include_path(&self, path: &str) -> PathBuf {
        let p = Path::new(path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.base_dir.join(p)
        }
    }

    fn require_executor(&self, line: usize) -> Result<&ActionExecutor, AutoError> {
        self.executor.as_ref().ok_or_else(|| AutoError::Runtime {
            message: "No simulator selected. Use use_device(udid) or boot_device(udid) first.".to_string(),
            line,
        })
    }

    /// Polls until the element exists, without requiring frame stability.
    /// Returns as soon as the element is found once, letting XCUIElement's
    /// native tap handle hittability and animations.
    async fn wait_for_element_exists(
        &self,
        selector: &str,
        by_label: bool,
        element_type: Option<&str>,
        line: usize,
    ) -> Result<(), AutoError> {
        let driver = self.require_executor(line)?.driver();
        let timeout = Duration::from_millis(self.default_timeout_ms);
        let poll_interval = Duration::from_millis(100);
        let start = Instant::now();

        loop {
            if let Ok(Some(_)) = driver.find_element_with_type(selector, by_label, element_type).await {
                return Ok(());
            }
            if start.elapsed() >= timeout {
                let msg = if by_label {
                    format!("Timeout after {}ms waiting for element with label '{}'", self.default_timeout_ms, selector)
                } else {
                    format!("Timeout after {}ms waiting for element '{}'", self.default_timeout_ms, selector)
                };
                return Err(AutoError::ActionFailed { message: msg, line });
            }
            tokio::time::sleep(poll_interval).await;
        }
    }

    async fn execute_action(&mut self, action: ActionType, line: usize) -> Result<Value, AutoError> {
        let executor = self.require_executor(line)?;
        let result = executor.execute(action.clone()).await;

        let action_result = if result.success {
            ActionResult::Success
        } else {
            ActionResult::Failure(result.message.clone())
        };
        let duration_ms = Self::extract_elapsed_ms(&result.data);
        self.session.log_action(action, action_result, result.screenshot.clone(), duration_ms).await;

        if result.success {
            info!(line, msg = %result.message, "action executed");
            Ok(Value::String(result.data.unwrap_or(result.message)))
        } else {
            Err(AutoError::ActionFailed { message: result.message, line })
        }
    }

    pub fn cleanup(&mut self) {
        if let Some(handle) = self.watcher_handle.take() {
            handle.cancel();
        }
    }
}

impl Drop for ScriptExecutor {
    fn drop(&mut self) {
        self.cleanup();
    }
}
