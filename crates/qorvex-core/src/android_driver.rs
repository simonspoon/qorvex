//! [`AutomationDriver`] implementation backed by the Android Kotlin agent.
//!
//! This module provides [`AndroidDriver`], the Android counterpart of
//! [`AgentDriver`](crate::agent_driver::AgentDriver). It implements the
//! [`AutomationDriver`] trait by communicating with the on-device Kotlin
//! UiAutomator agent (story #84) using the **same** binary protocol defined in
//! [`crate::protocol`] — no protocol changes, no iOS assumptions.
//!
//! # Connection path
//!
//! `adb` unifies emulator, USB, and `adb connect` network devices behind a
//! single `serial` (ADR-3). The driver establishes one
//! [`AdbForward`](crate::adb_forward::AdbForward) tunnel
//! (`adb -s <serial> forward tcp:<local_port> tcp:<device_port>`) and then
//! connects to `127.0.0.1:<local_port>` — **identical** to the simulator
//! `Direct` path in [`AgentDriver`]. This lets the driver reuse
//! [`AgentClient::new`](crate::agent_client::AgentClient::new) verbatim, with no
//! `from_stream` plumbing (ADR-3 §1).
//!
//! ```text
//!   AndroidDriver ──Request/Response──▶ AgentClient
//!        │                                   │ TCP 127.0.0.1:<local_port>
//!        │ owns AdbForward ──────────────────┤
//!        ▼                                   ▼
//!   adb forward rule ───────────▶ adb ───▶ Kotlin agent (device_port on device)
//! ```
//!
//! # Reconnect & recovery (ADR-3 §4 / ADR-2)
//!
//! The `adb forward` rule lives in the adb server, independent of the agent
//! process and of any one TCP connection, so a dropped *TCP* connection (agent
//! stall, transient drop) is recovered by simply re-opening the socket. On a
//! *forward-level* failure (device re-plug, emulator reboot, adb server bounce)
//! the driver re-issues the forward via [`AdbForward::ensure`] before
//! reconnecting. Both primitives exist today, so the driver's
//! socket-reconnect-then-re-issue-forward ladder is fully wired here.
//!
//! The heavier terminate+respawn rung of the ladder (ADR-2 §4) depends on
//! `AndroidLifecycle`, which lands in story #88. See the
//! [`AndroidDriver::attempt_recovery`] note for the exact seam left for #88.
//!
//! # Example
//!
//! ```no_run
//! use qorvex_core::android_driver::AndroidDriver;
//! use qorvex_core::driver::AutomationDriver;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // emulator-5554, host port auto-picked, agent on device port 8080
//! let mut driver = AndroidDriver::new("emulator-5554", None, 8080);
//! driver.connect().await?;
//! # Ok(())
//! # }
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{debug, info, instrument, warn};

use crate::adb_forward::{AdbForward, AdbForwardError};
use crate::agent_client::{AgentClient, AgentClientError};
use crate::driver::{AutomationDriver, DriverError, TargetInfo};
use crate::element::UIElement;
use crate::protocol::{Request, Response};

/// The default device-side TCP port the Kotlin agent listens on (matches the
/// agent's `qorvex_port` default in story #84 / ADR-2).
pub const DEFAULT_ANDROID_AGENT_PORT: u16 = 8080;

// ---------------------------------------------------------------------------
// Error mapping (mirrors agent_driver.rs)
// ---------------------------------------------------------------------------

/// Maps an [`AgentClientError`] to a [`DriverError`].
fn map_client_error(err: AgentClientError) -> DriverError {
    match err {
        AgentClientError::NotConnected => DriverError::NotConnected,
        AgentClientError::ConnectionFailed(msg) => DriverError::ConnectionLost(msg),
        AgentClientError::Io(e) => DriverError::Io(e),
        AgentClientError::Protocol(e) => DriverError::CommandFailed(e.to_string()),
        AgentClientError::AgentError(msg) => DriverError::CommandFailed(msg),
        AgentClientError::Timeout => DriverError::Timeout,
    }
}

/// Maps an [`AdbForwardError`] to a [`DriverError`].
///
/// All forward failures surface as connection-class errors so the executor and
/// the driver's own reconnect classifier treat them like a lost link.
fn map_forward_error(err: AdbForwardError) -> DriverError {
    DriverError::ConnectionLost(err.to_string())
}

/// Checks that the response is [`Response::Ok`], else returns
/// [`DriverError::CommandFailed`].
fn expect_ok(response: Response) -> Result<(), DriverError> {
    match response {
        Response::Ok => Ok(()),
        other => Err(DriverError::CommandFailed(format!(
            "unexpected response: {other:?}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// AndroidDriver
// ---------------------------------------------------------------------------

/// An [`AutomationDriver`] backed by the on-device Kotlin agent, reached over an
/// [`AdbForward`] tunnel.
///
/// The driver owns the forward and lazily creates an [`AgentClient`] when
/// [`connect`](AutomationDriver::connect) is called. The client is wrapped in a
/// [`tokio::sync::Mutex`] so the `&self` trait methods can acquire mutable
/// access for sending requests; the forward is likewise behind a `Mutex` so the
/// reconnect path can re-issue the rule via [`AdbForward::ensure`].
pub struct AndroidDriver {
    /// adb serial of the target device (emulator-5554 | host:port | USB serial).
    serial: String,
    /// Requested host loopback port; `None` lets adb auto-pick on first connect.
    local_port: Option<u16>,
    /// Agent's TCP port inside the device.
    device_port: u16,
    /// The established forward, set on [`connect`](AutomationDriver::connect).
    forward: Mutex<Option<AdbForward>>,
    /// The protocol client over the forwarded socket.
    client: Mutex<Option<AgentClient>>,
    /// Number of successful reconnect/recovery events since creation.
    recovery_count: AtomicU64,
    /// Remembered target package so it can be re-sent after recovery.
    target_bundle_id: Mutex<Option<String>>,
}

impl AndroidDriver {
    /// Create a driver for the given adb `serial`.
    ///
    /// * `serial` - adb device serial (`emulator-5554`, `host:port` for
    ///   `adb connect`, or a USB serial).
    /// * `local_port` - the host loopback port to bind; pass `None` (or
    ///   `Some(0)`) to let adb pick a free port.
    /// * `device_port` - the agent's device-side TCP port (default
    ///   [`DEFAULT_ANDROID_AGENT_PORT`]).
    ///
    /// No connection is established until [`connect`](AutomationDriver::connect)
    /// is called.
    pub fn new(serial: impl Into<String>, local_port: Option<u16>, device_port: u16) -> Self {
        Self {
            serial: serial.into(),
            local_port,
            device_port,
            forward: Mutex::new(None),
            client: Mutex::new(None),
            recovery_count: AtomicU64::new(0),
            target_bundle_id: Mutex::new(None),
        }
    }

    /// **Test-support only.** Build a driver with a pre-connected
    /// [`AgentClient`], bypassing the `adb forward` step that requires a real
    /// device.
    ///
    /// This is the out-of-crate analog of the in-crate `with_test_client`
    /// helper: it lets the parity harness drive the **production** trait surface
    /// (`tap_*`, `dump_tree`, `get_*`, `find_*`, `screenshot`, `set_target`, …)
    /// against a loopback mock agent, exactly as a real caller would, just
    /// without adb. Gated behind the `test-support` feature so it never compiles
    /// into a normal build.
    #[doc(hidden)]
    #[cfg(feature = "test-support")]
    pub async fn with_connected_client(
        serial: impl Into<String>,
        device_port: u16,
        client: AgentClient,
    ) -> Self {
        let driver = Self::new(serial, None, device_port);
        *driver.client.lock().await = Some(client);
        driver
    }

    /// The adb serial this driver targets.
    pub fn serial(&self) -> &str {
        &self.serial
    }

    /// The agent's device-side TCP port.
    pub fn device_port(&self) -> u16 {
        self.device_port
    }

    /// Returns the number of successful recovery events since creation.
    ///
    /// The executor polls this to detect a mid-action reconnect and reset its
    /// wait timer accordingly (parity with [`AgentDriver`]).
    pub fn recovery_count(&self) -> u64 {
        self.recovery_count.load(Ordering::Relaxed)
    }

    /// Establish (or re-establish) the `adb forward` and open an
    /// [`AgentClient`] against `127.0.0.1:<local_port>`, verified by heartbeat.
    ///
    /// On the first call the forward is created with the requested
    /// `local_port`; the bound port is then pinned so subsequent reconnects
    /// reach the same loopback address.
    async fn create_client(&self) -> Result<AgentClient, DriverError> {
        let addr = self.ensure_forward().await?;
        let mut client = AgentClient::new(addr);
        client.connect().await.map_err(map_client_error)?;
        client.heartbeat().await.map_err(map_client_error)?;
        Ok(client)
    }

    /// Ensure a forward rule exists and return the loopback `SocketAddr` to
    /// connect to.
    ///
    /// First call establishes the rule (adb auto-picks a port when
    /// `local_port` is `None`). Later calls re-issue the existing rule via
    /// [`AdbForward::ensure`] (idempotent; preserves the bound port), covering
    /// the forward-level reconnect of ADR-3 §4.
    ///
    /// `adb` is a blocking CLI, so the call runs inside `spawn_blocking` to
    /// avoid stalling the async runtime.
    async fn ensure_forward(&self) -> Result<std::net::SocketAddr, DriverError> {
        let mut guard = self.forward.lock().await;
        match guard.take() {
            Some(mut forward) => {
                // `AdbForward::ensure` shells out to the blocking `adb` CLI, so
                // run it off the async runtime (parity with the `None` branch).
                // Move the forward into the blocking task and hand it back so
                // the re-issued rule's ownership stays in this driver.
                let forward = tokio::task::spawn_blocking(move || {
                    forward.ensure().map(|()| forward)
                })
                .await
                .map_err(|e| DriverError::CommandFailed(format!("adb forward task failed: {e}")))?
                .map_err(map_forward_error)?;
                *guard = Some(forward);
            }
            None => {
                let serial = self.serial.clone();
                let local_port = self.local_port;
                let device_port = self.device_port;
                let forward = tokio::task::spawn_blocking(move || {
                    AdbForward::establish(&serial, local_port, device_port)
                })
                .await
                .map_err(|e| DriverError::CommandFailed(format!("adb forward task failed: {e}")))?
                .map_err(map_forward_error)?;
                *guard = Some(forward);
            }
        }
        let addr_str = guard
            .as_ref()
            .expect("forward set above")
            .local_addr();
        addr_str
            .parse()
            .map_err(|e| DriverError::ConnectionLost(format!("bad forward address {addr_str}: {e}")))
    }

    /// Returns `true` if the error indicates a broken connection that a
    /// reconnect may recover.
    fn is_connection_error(err: &DriverError) -> bool {
        matches!(
            err,
            DriverError::NotConnected
                | DriverError::ConnectionLost(_)
                | DriverError::Io(_)
                | DriverError::Timeout
        )
    }

    /// Attempt to recover a dead connection.
    ///
    /// The realized ladder (ADR-3 §4): re-issue the `adb forward` rule (covered
    /// inside [`create_client`] → [`ensure_forward`]) and re-open the TCP
    /// socket, then restore the target package. On success the recovery counter
    /// is bumped.
    ///
    /// **Seam for story #88:** the heavier `terminate_agent` + `spawn_agent` +
    /// `wait_for_ready` rung (ADR-2 §4) requires `AndroidLifecycle`, which is
    /// introduced in #88. When that lands, #88 attaches an
    /// `Option<Arc<AndroidLifecycle>>` (mirroring
    /// [`AgentDriver::with_lifecycle`](crate::agent_driver::AgentDriver)) and
    /// inserts the respawn step here, after this socket-level reconnect fails.
    /// No protocol or trait change is needed for that addition.
    async fn attempt_recovery(&self) -> Result<(), DriverError> {
        info!(serial = %self.serial, "android agent connection lost, re-establishing forward + socket");
        let client = self.create_client().await?;
        *self.client.lock().await = Some(client);
        self.restore_target().await?;
        self.recovery_count.fetch_add(1, Ordering::Relaxed);
        info!("android reconnect successful");
        Ok(())
    }

    /// Re-send `SetTarget` if a target package was previously set, so the
    /// reconnected agent automates the right package.
    async fn restore_target(&self) -> Result<(), DriverError> {
        let bundle_id = self.target_bundle_id.lock().await.clone();
        if let Some(bid) = bundle_id {
            info!(bundle_id = %bid, "restoring target after android reconnect");
            let response = self
                .send_raw(&Request::SetTarget { bundle_id: bid })
                .await?;
            expect_ok(response)?;
        }
        Ok(())
    }

    /// Send a request, retrying once via [`attempt_recovery`] on a connection
    /// error.
    async fn send(&self, request: &Request) -> Result<Response, DriverError> {
        let result = self.send_raw(request).await;
        match &result {
            Err(e) if Self::is_connection_error(e) => {
                warn!(error = %e, opcode = request.opcode_name(), "android connection error, attempting recovery");
                self.attempt_recovery().await?;
                self.send_raw(request).await
            }
            _ => result,
        }
    }

    /// Send a request without recovery wrapping.
    async fn send_raw(&self, request: &Request) -> Result<Response, DriverError> {
        let mut guard = self.client.lock().await;
        let client = guard.as_mut().ok_or(DriverError::NotConnected)?;
        client.send(request).await.map_err(map_client_error)
    }

    /// Send a request with a custom read timeout, retrying once on a connection
    /// error. When `timeout_ms` is `None`, falls back to [`send`](Self::send).
    async fn send_with_read_timeout(
        &self,
        request: &Request,
        timeout_ms: Option<u64>,
    ) -> Result<Response, DriverError> {
        let result = self.send_raw_with_read_timeout(request, timeout_ms).await;
        match &result {
            Err(e) if Self::is_connection_error(e) => {
                warn!(error = %e, opcode = request.opcode_name(), "android connection error (with timeout), attempting recovery");
                self.attempt_recovery().await?;
                self.send_raw_with_read_timeout(request, timeout_ms).await
            }
            _ => result,
        }
    }

    /// Send a request with a custom read timeout, without recovery wrapping.
    async fn send_raw_with_read_timeout(
        &self,
        request: &Request,
        timeout_ms: Option<u64>,
    ) -> Result<Response, DriverError> {
        match timeout_ms {
            Some(ms) => {
                // Wait longer than the agent's internal retry window so the Rust
                // side does not drop the socket before the agent replies
                // (parity with AgentDriver).
                let read_timeout = Duration::from_millis(ms + 15_000);
                let mut guard = self.client.lock().await;
                let client = guard.as_mut().ok_or(DriverError::NotConnected)?;
                client
                    .send_with_timeout(request, read_timeout)
                    .await
                    .map_err(map_client_error)
            }
            None => self.send_raw(request).await,
        }
    }
}

#[async_trait]
impl AutomationDriver for AndroidDriver {
    #[instrument(skip(self), level = "debug")]
    async fn connect(&mut self) -> Result<(), DriverError> {
        let client = self.create_client().await?;
        *self.client.lock().await = Some(client);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.client.try_lock().map(|g| g.is_some()).unwrap_or(false)
    }

    fn recovery_count(&self) -> u64 {
        self.recovery_count.load(Ordering::Relaxed)
    }

    async fn tap_location(&self, x: i32, y: i32) -> Result<(), DriverError> {
        let response = self.send(&Request::TapCoord { x, y }).await?;
        expect_ok(response)
    }

    #[instrument(skip(self), level = "debug")]
    async fn tap_element(&self, identifier: &str) -> Result<(), DriverError> {
        let response = self
            .send(&Request::TapElement {
                selector: identifier.to_string(),
                timeout_ms: None,
            })
            .await?;
        expect_ok(response)
    }

    #[instrument(skip(self), level = "debug")]
    async fn tap_by_label(&self, label: &str) -> Result<(), DriverError> {
        let response = self
            .send(&Request::TapByLabel {
                label: label.to_string(),
                timeout_ms: None,
            })
            .await?;
        expect_ok(response)
    }

    #[instrument(skip(self), level = "debug")]
    async fn tap_with_type(
        &self,
        selector: &str,
        by_label: bool,
        element_type: &str,
    ) -> Result<(), DriverError> {
        let response = self
            .send(&Request::TapWithType {
                selector: selector.to_string(),
                by_label,
                element_type: element_type.to_string(),
                timeout_ms: None,
            })
            .await?;
        expect_ok(response)
    }

    #[instrument(skip(self), level = "debug")]
    async fn swipe(
        &self,
        start_x: i32,
        start_y: i32,
        end_x: i32,
        end_y: i32,
        duration: Option<f64>,
    ) -> Result<(), DriverError> {
        let response = self
            .send(&Request::Swipe {
                start_x,
                start_y,
                end_x,
                end_y,
                duration,
            })
            .await?;
        expect_ok(response)
    }

    async fn long_press(&self, x: i32, y: i32, duration: f64) -> Result<(), DriverError> {
        let response = self.send(&Request::LongPress { x, y, duration }).await?;
        expect_ok(response)
    }

    #[instrument(skip(self), level = "debug")]
    async fn type_text(&self, text: &str) -> Result<(), DriverError> {
        let response = self
            .send(&Request::TypeText {
                text: text.to_string(),
            })
            .await?;
        expect_ok(response)
    }

    #[instrument(skip(self), level = "debug")]
    async fn dump_tree(&self) -> Result<Vec<UIElement>, DriverError> {
        // Large hierarchies can take well over 30s to snapshot; use a generous
        // timeout to avoid dropping the connection mid-snapshot (parity with
        // AgentDriver).
        const DUMP_TREE_TIMEOUT_MS: u64 = 120_000;
        let response = self
            .send_with_read_timeout(&Request::DumpTree, Some(DUMP_TREE_TIMEOUT_MS))
            .await?;
        match response {
            Response::Tree { json } => {
                let elements: Vec<UIElement> = serde_json::from_str(&json)
                    .map_err(|e| DriverError::JsonParse(e.to_string()))?;
                debug!(element_count = elements.len(), "android tree dumped");
                Ok(elements)
            }
            other => Err(DriverError::CommandFailed(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    async fn get_element_value(&self, identifier: &str) -> Result<Option<String>, DriverError> {
        let response = self
            .send(&Request::GetValue {
                selector: identifier.to_string(),
                by_label: false,
                element_type: None,
                timeout_ms: None,
            })
            .await?;
        match response {
            Response::Value { value } => Ok(value),
            other => Err(DriverError::CommandFailed(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    async fn get_element_value_by_label(&self, label: &str) -> Result<Option<String>, DriverError> {
        let response = self
            .send(&Request::GetValue {
                selector: label.to_string(),
                by_label: true,
                element_type: None,
                timeout_ms: None,
            })
            .await?;
        match response {
            Response::Value { value } => Ok(value),
            other => Err(DriverError::CommandFailed(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    async fn get_value_with_type(
        &self,
        selector: &str,
        by_label: bool,
        element_type: &str,
    ) -> Result<Option<String>, DriverError> {
        let response = self
            .send(&Request::GetValue {
                selector: selector.to_string(),
                by_label,
                element_type: Some(element_type.to_string()),
                timeout_ms: None,
            })
            .await?;
        match response {
            Response::Value { value } => Ok(value),
            other => Err(DriverError::CommandFailed(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    #[instrument(skip(self), level = "debug")]
    async fn screenshot(&self) -> Result<Vec<u8>, DriverError> {
        let response = self.send(&Request::Screenshot).await?;
        match response {
            Response::Screenshot { data } => {
                debug!(bytes = data.len(), "android screenshot captured");
                Ok(data)
            }
            other => Err(DriverError::CommandFailed(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    async fn tap_element_with_timeout(
        &self,
        identifier: &str,
        timeout_ms: Option<u64>,
    ) -> Result<(), DriverError> {
        let response = self
            .send_with_read_timeout(
                &Request::TapElement {
                    selector: identifier.to_string(),
                    timeout_ms,
                },
                timeout_ms,
            )
            .await?;
        expect_ok(response)
    }

    async fn tap_by_label_with_timeout(
        &self,
        label: &str,
        timeout_ms: Option<u64>,
    ) -> Result<(), DriverError> {
        let response = self
            .send_with_read_timeout(
                &Request::TapByLabel {
                    label: label.to_string(),
                    timeout_ms,
                },
                timeout_ms,
            )
            .await?;
        expect_ok(response)
    }

    async fn tap_with_type_with_timeout(
        &self,
        selector: &str,
        by_label: bool,
        element_type: &str,
        timeout_ms: Option<u64>,
    ) -> Result<(), DriverError> {
        let response = self
            .send_with_read_timeout(
                &Request::TapWithType {
                    selector: selector.to_string(),
                    by_label,
                    element_type: element_type.to_string(),
                    timeout_ms,
                },
                timeout_ms,
            )
            .await?;
        expect_ok(response)
    }

    async fn get_value_with_timeout(
        &self,
        selector: &str,
        by_label: bool,
        element_type: Option<&str>,
        timeout_ms: Option<u64>,
    ) -> Result<Option<String>, DriverError> {
        let response = self
            .send_with_read_timeout(
                &Request::GetValue {
                    selector: selector.to_string(),
                    by_label,
                    element_type: element_type.map(|s| s.to_string()),
                    timeout_ms,
                },
                timeout_ms,
            )
            .await?;
        match response {
            Response::Value { value } => Ok(value),
            other => Err(DriverError::CommandFailed(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    #[instrument(skip(self), level = "debug")]
    async fn find_element(&self, identifier: &str) -> Result<Option<UIElement>, DriverError> {
        self.find_element_with_type(identifier, false, None).await
    }

    #[instrument(skip(self), level = "debug")]
    async fn find_element_by_label(&self, label: &str) -> Result<Option<UIElement>, DriverError> {
        self.find_element_with_type(label, true, None).await
    }

    #[instrument(skip(self), level = "debug")]
    async fn find_element_with_type(
        &self,
        selector: &str,
        by_label: bool,
        element_type: Option<&str>,
    ) -> Result<Option<UIElement>, DriverError> {
        let response = self
            .send(&Request::FindElement {
                selector: selector.to_string(),
                by_label,
                element_type: element_type.map(|s| s.to_string()),
            })
            .await?;
        match response {
            Response::Element { json } => {
                let element: Option<UIElement> = serde_json::from_str(&json)
                    .map_err(|e| DriverError::JsonParse(e.to_string()))?;
                Ok(element)
            }
            other => Err(DriverError::CommandFailed(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    #[instrument(skip(self), level = "debug")]
    async fn find_element_with_read_timeout(
        &self,
        selector: &str,
        by_label: bool,
        element_type: Option<&str>,
        read_timeout_ms: Option<u64>,
    ) -> Result<Option<UIElement>, DriverError> {
        let response = self
            .send_with_read_timeout(
                &Request::FindElement {
                    selector: selector.to_string(),
                    by_label,
                    element_type: element_type.map(|s| s.to_string()),
                },
                read_timeout_ms,
            )
            .await?;
        match response {
            Response::Element { json } => {
                let element: Option<UIElement> = serde_json::from_str(&json)
                    .map_err(|e| DriverError::JsonParse(e.to_string()))?;
                Ok(element)
            }
            other => Err(DriverError::CommandFailed(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }

    #[instrument(skip(self), level = "debug")]
    async fn set_target(&self, bundle_id: &str) -> Result<(), DriverError> {
        let response = self
            .send(&Request::SetTarget {
                bundle_id: bundle_id.to_string(),
            })
            .await?;
        expect_ok(response)?;
        // Remember for restore after reconnect.
        *self.target_bundle_id.lock().await = Some(bundle_id.to_string());
        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_target_info(&self) -> Result<TargetInfo, DriverError> {
        let bundle_id = self
            .target_bundle_id
            .lock()
            .await
            .clone()
            .unwrap_or_default();
        let response = self.send(&Request::GetTargetInfo).await?;
        match response {
            Response::TargetInfo { json } => {
                // Deserialize the agent's JSON; bundle_id falls back to what we
                // tracked locally if the agent omits it.
                #[derive(serde::Deserialize, Default)]
                struct AgentTargetInfo {
                    #[serde(default)]
                    state: String,
                    #[serde(default)]
                    display_name: String,
                    #[serde(default)]
                    version: String,
                    #[serde(default)]
                    build: String,
                    #[serde(default)]
                    bundle_id: String,
                }
                let partial: AgentTargetInfo = serde_json::from_str(&json)
                    .map_err(|e| DriverError::JsonParse(e.to_string()))?;
                Ok(TargetInfo {
                    bundle_id: if partial.bundle_id.is_empty() {
                        bundle_id
                    } else {
                        partial.bundle_id
                    },
                    display_name: partial.display_name,
                    version: partial.version,
                    build: partial.build,
                    state: partial.state,
                })
            }
            other => Err(DriverError::CommandFailed(format!(
                "unexpected response: {other:?}"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
//
// These exercise the trait-impl request/response logic against a loopback mock
// agent. We bypass the `adb forward` step (which needs a real device) by
// injecting a pre-connected `AgentClient` and a sentinel forward via the
// test-only `with_test_client` helper — so every code path below runs the same
// public trait methods a production caller would, just without adb.
//
// The live emulator round-trip (real adb forward → real Kotlin agent) is
// deferred to integration story #90.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::encode_response;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    impl AndroidDriver {
        /// Test-only: install a pre-connected client, skipping the adb forward.
        ///
        /// This lets unit tests drive the full trait surface over a loopback
        /// mock without a device. Production always goes through
        /// [`connect`](AutomationDriver::connect) → [`create_client`].
        async fn with_test_client(self, client: AgentClient) -> Self {
            *self.client.lock().await = Some(client);
            self
        }
    }

    /// A mock agent that handles one connection: one request frame in, the
    /// supplied response out. Returns the bound loopback address.
    async fn mock_agent(response: Response) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut header = [0u8; 4];
            stream.read_exact(&mut header).await.unwrap();
            let len = crate::protocol::read_frame_length(&header) as usize;
            let mut payload = vec![0u8; len];
            stream.read_exact(&mut payload).await.unwrap();
            let bytes = encode_response(&response);
            stream.write_all(&bytes).await.unwrap();
            stream.flush().await.unwrap();
        });
        addr
    }

    /// Build a driver whose client is connected to a loopback mock that will
    /// reply with `response` to the next request.
    async fn driver_with_mock(response: Response) -> AndroidDriver {
        let addr = mock_agent(response).await;
        let mut client = AgentClient::new(addr);
        client.connect().await.unwrap();
        AndroidDriver::new("emulator-5554", Some(43217), 8080)
            .with_test_client(client)
            .await
    }

    // --- construction ---

    #[test]
    fn new_creates_disconnected_driver() {
        let driver = AndroidDriver::new("emulator-5554", None, 8080);
        assert_eq!(driver.serial(), "emulator-5554");
        assert_eq!(driver.device_port(), 8080);
        assert_eq!(driver.recovery_count(), 0);
        assert!(!driver.is_connected());
    }

    #[test]
    fn new_with_default_port_constant() {
        let driver = AndroidDriver::new("host:5555", None, DEFAULT_ANDROID_AGENT_PORT);
        assert_eq!(driver.device_port(), 8080);
    }

    // --- operations without a connection return NotConnected ---

    #[tokio::test]
    async fn tap_location_not_connected_when_disconnected() {
        let driver = AndroidDriver::new("emulator-5554", Some(1), 8080);
        // No client installed and adb is absent in CI, so ensure_forward fails
        // → recovery fails → a connection-class error propagates.
        let result = driver.tap_location(10, 20).await;
        assert!(result.is_err());
        assert!(AndroidDriver::is_connection_error(&result.unwrap_err()));
    }

    // --- tap family ---

    #[tokio::test]
    async fn tap_location_sends_tap_coord() {
        let driver = driver_with_mock(Response::Ok).await;
        driver.tap_location(100, 200).await.unwrap();
    }

    #[tokio::test]
    async fn tap_element_sends_request() {
        let driver = driver_with_mock(Response::Ok).await;
        driver.tap_element("login-button").await.unwrap();
    }

    #[tokio::test]
    async fn tap_by_label_sends_request() {
        let driver = driver_with_mock(Response::Ok).await;
        driver.tap_by_label("Log In").await.unwrap();
    }

    #[tokio::test]
    async fn tap_with_type_sends_request() {
        let driver = driver_with_mock(Response::Ok).await;
        driver.tap_with_type("submit", false, "Button").await.unwrap();
    }

    #[tokio::test]
    async fn tap_element_with_timeout_sends_request() {
        let driver = driver_with_mock(Response::Ok).await;
        driver
            .tap_element_with_timeout("btn", Some(1000))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn tap_by_label_with_timeout_sends_request() {
        let driver = driver_with_mock(Response::Ok).await;
        driver
            .tap_by_label_with_timeout("Log In", Some(1000))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn tap_with_type_with_timeout_sends_request() {
        let driver = driver_with_mock(Response::Ok).await;
        driver
            .tap_with_type_with_timeout("submit", true, "Button", Some(1000))
            .await
            .unwrap();
    }

    // --- swipe / long press ---

    #[tokio::test]
    async fn swipe_sends_request() {
        let driver = driver_with_mock(Response::Ok).await;
        driver.swipe(0, 800, 0, 200, Some(0.5)).await.unwrap();
    }

    #[tokio::test]
    async fn swipe_without_duration() {
        let driver = driver_with_mock(Response::Ok).await;
        driver.swipe(50, 100, 50, 500, None).await.unwrap();
    }

    #[tokio::test]
    async fn long_press_sends_request() {
        let driver = driver_with_mock(Response::Ok).await;
        driver.long_press(150, 300, 1.5).await.unwrap();
    }

    // --- type text ---

    #[tokio::test]
    async fn type_text_sends_request() {
        let driver = driver_with_mock(Response::Ok).await;
        driver.type_text("hello@example.com").await.unwrap();
    }

    // --- dump tree (uses the read-timeout path) ---

    #[tokio::test]
    async fn dump_tree_parses_android_json() {
        // Mirrors ADR-1 mapping: short element_type, FQCN role, hittable bool.
        let json = r#"[{
            "AXUniqueId": "login_button",
            "AXLabel": "Log In",
            "type": "Button",
            "role": "android.widget.Button",
            "hittable": true,
            "frame": {"x": 0.0, "y": 100.0, "width": 200.0, "height": 48.0},
            "children": [
                {
                    "AXUniqueId": "email_field",
                    "AXValue": "user@example.com",
                    "type": "EditText",
                    "role": "android.widget.EditText",
                    "children": []
                }
            ]
        }]"#;
        let driver = driver_with_mock(Response::Tree {
            json: json.to_string(),
        })
        .await;

        let tree = driver.dump_tree().await.unwrap();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].identifier.as_deref(), Some("login_button"));
        assert_eq!(tree[0].label.as_deref(), Some("Log In"));
        assert_eq!(tree[0].element_type.as_deref(), Some("Button"));
        assert_eq!(tree[0].role.as_deref(), Some("android.widget.Button"));
        assert_eq!(tree[0].hittable, Some(true));
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].value.as_deref(), Some("user@example.com"));
        assert_eq!(tree[0].children[0].element_type.as_deref(), Some("EditText"));
    }

    #[tokio::test]
    async fn dump_tree_empty() {
        let driver = driver_with_mock(Response::Tree {
            json: "[]".to_string(),
        })
        .await;
        assert!(driver.dump_tree().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn dump_tree_invalid_json_errors() {
        let driver = driver_with_mock(Response::Tree {
            json: "not json".to_string(),
        })
        .await;
        assert!(matches!(
            driver.dump_tree().await,
            Err(DriverError::JsonParse(_))
        ));
    }

    #[tokio::test]
    async fn dump_tree_unexpected_response_type() {
        let driver = driver_with_mock(Response::Ok).await;
        match driver.dump_tree().await {
            Err(DriverError::CommandFailed(msg)) => assert!(msg.contains("unexpected response")),
            other => panic!("expected CommandFailed, got {other:?}"),
        }
    }

    // --- get value family ---

    #[tokio::test]
    async fn get_element_value_returns_some() {
        let driver = driver_with_mock(Response::Value {
            value: Some("user@example.com".to_string()),
        })
        .await;
        assert_eq!(
            driver.get_element_value("email_field").await.unwrap().as_deref(),
            Some("user@example.com")
        );
    }

    #[tokio::test]
    async fn get_element_value_returns_none() {
        let driver = driver_with_mock(Response::Value { value: None }).await;
        assert!(driver.get_element_value("empty").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn get_element_value_by_label_returns_value() {
        let driver = driver_with_mock(Response::Value {
            value: Some("typed".to_string()),
        })
        .await;
        assert_eq!(
            driver
                .get_element_value_by_label("Email")
                .await
                .unwrap()
                .as_deref(),
            Some("typed")
        );
    }

    #[tokio::test]
    async fn get_value_with_type_returns_value() {
        let driver = driver_with_mock(Response::Value {
            value: Some("v".to_string()),
        })
        .await;
        assert_eq!(
            driver
                .get_value_with_type("Email", true, "EditText")
                .await
                .unwrap()
                .as_deref(),
            Some("v")
        );
    }

    #[tokio::test]
    async fn get_value_with_timeout_returns_value() {
        let driver = driver_with_mock(Response::Value {
            value: Some("v".to_string()),
        })
        .await;
        assert_eq!(
            driver
                .get_value_with_timeout("Email", true, Some("EditText"), Some(500))
                .await
                .unwrap()
                .as_deref(),
            Some("v")
        );
    }

    // --- screenshot ---

    #[tokio::test]
    async fn screenshot_returns_png_bytes() {
        let png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let driver = driver_with_mock(Response::Screenshot { data: png.clone() }).await;
        assert_eq!(driver.screenshot().await.unwrap(), png);
    }

    // --- find element ---

    #[tokio::test]
    async fn find_element_returns_some() {
        let json = r#"{"AXUniqueId":"btn","type":"Button","children":[]}"#;
        let driver = driver_with_mock(Response::Element {
            json: json.to_string(),
        })
        .await;
        let found = driver.find_element("btn").await.unwrap();
        assert_eq!(found.unwrap().identifier.as_deref(), Some("btn"));
    }

    #[tokio::test]
    async fn find_element_returns_none_for_null() {
        let driver = driver_with_mock(Response::Element {
            json: "null".to_string(),
        })
        .await;
        assert!(driver.find_element("missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn find_element_by_label_returns_some() {
        let json = r#"{"AXLabel":"Log In","type":"Button","children":[]}"#;
        let driver = driver_with_mock(Response::Element {
            json: json.to_string(),
        })
        .await;
        let found = driver.find_element_by_label("Log In").await.unwrap();
        assert_eq!(found.unwrap().label.as_deref(), Some("Log In"));
    }

    #[tokio::test]
    async fn find_element_with_read_timeout_returns_some() {
        let json = r#"{"AXUniqueId":"btn","type":"Button","children":[]}"#;
        let driver = driver_with_mock(Response::Element {
            json: json.to_string(),
        })
        .await;
        let found = driver
            .find_element_with_read_timeout("btn", false, Some("Button"), Some(500))
            .await
            .unwrap();
        assert_eq!(found.unwrap().identifier.as_deref(), Some("btn"));
    }

    // --- set target / target info ---

    #[tokio::test]
    async fn set_target_sends_request_and_remembers() {
        let driver = driver_with_mock(Response::Ok).await;
        driver.set_target("com.example.app").await.unwrap();
        assert_eq!(
            driver.target_bundle_id.lock().await.as_deref(),
            Some("com.example.app")
        );
    }

    #[tokio::test]
    async fn get_target_info_parses_json() {
        let json = r#"{
            "bundle_id": "com.example.app",
            "display_name": "Example",
            "version": "1.2.3",
            "build": "42",
            "state": "running"
        }"#;
        let driver = driver_with_mock(Response::TargetInfo {
            json: json.to_string(),
        })
        .await;
        let info = driver.get_target_info().await.unwrap();
        assert_eq!(info.bundle_id, "com.example.app");
        assert_eq!(info.display_name, "Example");
        assert_eq!(info.version, "1.2.3");
        assert_eq!(info.build, "42");
        assert_eq!(info.state, "running");
    }

    #[tokio::test]
    async fn get_target_info_falls_back_to_tracked_bundle_id() {
        // Agent omits bundle_id → driver fills it from what set_target tracked.
        let driver = AndroidDriver::new("emulator-5554", Some(43217), 8080);
        *driver.target_bundle_id.lock().await = Some("com.tracked.pkg".to_string());
        let json = r#"{"display_name":"X","version":"1","build":"1","state":"running"}"#;
        let addr = mock_agent(Response::TargetInfo {
            json: json.to_string(),
        })
        .await;
        let mut client = AgentClient::new(addr);
        client.connect().await.unwrap();
        *driver.client.lock().await = Some(client);

        let info = driver.get_target_info().await.unwrap();
        assert_eq!(info.bundle_id, "com.tracked.pkg");
    }

    // --- error mapping ---

    #[test]
    fn map_client_error_variants() {
        assert!(matches!(
            map_client_error(AgentClientError::NotConnected),
            DriverError::NotConnected
        ));
        assert!(matches!(
            map_client_error(AgentClientError::Timeout),
            DriverError::Timeout
        ));
        match map_client_error(AgentClientError::AgentError("element not found".into())) {
            DriverError::CommandFailed(m) => assert_eq!(m, "element not found"),
            other => panic!("expected CommandFailed, got {other:?}"),
        }
        match map_client_error(AgentClientError::ConnectionFailed("refused".into())) {
            DriverError::ConnectionLost(m) => assert_eq!(m, "refused"),
            other => panic!("expected ConnectionLost, got {other:?}"),
        }
    }

    #[test]
    fn map_forward_error_is_connection_lost() {
        let err = map_forward_error(AdbForwardError::ForwardFailed("device offline".into()));
        match &err {
            DriverError::ConnectionLost(m) => assert!(m.contains("device offline")),
            other => panic!("expected ConnectionLost, got {other:?}"),
        }
        // Forward failures must classify as connection errors so the recovery
        // path is entered.
        assert!(AndroidDriver::is_connection_error(&err));
    }

    #[test]
    fn is_connection_error_classifier() {
        assert!(AndroidDriver::is_connection_error(&DriverError::NotConnected));
        assert!(AndroidDriver::is_connection_error(&DriverError::ConnectionLost(
            "x".into()
        )));
        assert!(AndroidDriver::is_connection_error(&DriverError::Timeout));
        assert!(AndroidDriver::is_connection_error(&DriverError::Io(
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken")
        )));
        assert!(!AndroidDriver::is_connection_error(&DriverError::CommandFailed(
            "not found".into()
        )));
        assert!(!AndroidDriver::is_connection_error(&DriverError::JsonParse(
            "bad".into()
        )));
    }

    // --- expect_ok ---

    #[test]
    fn expect_ok_ok() {
        assert!(expect_ok(Response::Ok).is_ok());
    }

    #[test]
    fn expect_ok_non_ok() {
        match expect_ok(Response::Value { value: None }) {
            Err(DriverError::CommandFailed(m)) => assert!(m.contains("unexpected response")),
            other => panic!("expected CommandFailed, got {other:?}"),
        }
    }

    // --- recovery: agent-error responses are NOT connection errors and do not
    //     trigger reconnect (they map to CommandFailed and propagate). ---

    #[tokio::test]
    async fn agent_error_propagates_without_recovery() {
        // The mock replies with a protocol Error → AgentClient surfaces it as
        // AgentError → CommandFailed (not a connection error), so send() does
        // NOT attempt recovery and the error reaches the caller verbatim.
        let driver = driver_with_mock(Response::Error {
            message: "element not found".to_string(),
        })
        .await;
        match driver.tap_element("missing").await {
            Err(DriverError::CommandFailed(m)) => assert_eq!(m, "element not found"),
            other => panic!("expected CommandFailed, got {other:?}"),
        }
        // No recovery was attempted.
        assert_eq!(driver.recovery_count(), 0);
    }
}
