//! Transport-generic core shared by [`AgentDriver`](crate::agent_driver::AgentDriver)
//! (iOS/macOS) and [`AndroidDriver`](crate::android_driver::AndroidDriver).
//!
//! Both drivers speak the **same** binary protocol ([`crate::protocol`]) to an
//! on-device agent and differ only in *transport*: the iOS driver reaches the
//! Swift agent over a [`ConnectionTarget`](crate::agent_driver::ConnectionTarget)
//! (direct TCP / usbmuxd / tunneld / CoreDevice), while the Android driver
//! reaches the Kotlin agent over an [`AdbForward`](crate::adb_forward::AdbForward)
//! tunnel. Everything *above* the socket — request framing, response decoding,
//! the connection-error retry/recovery ladder, target restoration, and the
//! ~20 [`AutomationDriver`] trait-method bodies — is identical, so it lives here
//! once.
//!
//! # Shape
//!
//! - [`AgentTransport`] abstracts the one thing that differs: how to open (and,
//!   on failure, recover) a connected [`AgentClient`]. Each platform provides a
//!   small transport type.
//! - [`AgentSession<T>`] holds the shared state (the client, the recovery
//!   counter, the remembered target) and *all* the protocol plumbing. A blanket
//!   [`AutomationDriver`] impl over `AgentSession<T>` gives both platforms their
//!   trait surface for free.
//! - The public driver types are thin type aliases:
//!   `pub type AgentDriver = AgentSession<IosTransport>` and
//!   `pub type AndroidDriver = AgentSession<AndroidTransport>`, each with a
//!   transport-specific inherent impl for its constructors/accessors.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{debug, info, instrument, warn};

use crate::agent_client::{AgentClient, AgentClientError};
use crate::driver::{AutomationDriver, DriverError, TargetInfo};
use crate::element::UIElement;
use crate::protocol::{Request, Response};

/// The padding added to a request's `timeout_ms` to derive the socket read
/// deadline, so the Rust side always waits strictly longer than the agent's own
/// internal retry window and never drops a still-working connection.
const READ_TIMEOUT_PADDING_MS: u64 = 15_000;

/// Read deadline for [`DumpTree`](Request::DumpTree): large hierarchies can take
/// well over 30s to snapshot, so use a generous timeout.
const DUMP_TREE_TIMEOUT_MS: u64 = 120_000;

// ---------------------------------------------------------------------------
// Shared error mapping
// ---------------------------------------------------------------------------

/// Maps an [`AgentClientError`] to a [`DriverError`].
pub(crate) fn map_client_error(err: AgentClientError) -> DriverError {
    match err {
        AgentClientError::NotConnected => DriverError::NotConnected,
        AgentClientError::ConnectionFailed(msg) => DriverError::ConnectionLost(msg),
        AgentClientError::Io(e) => DriverError::Io(e),
        AgentClientError::Protocol(e) => DriverError::CommandFailed(e.to_string()),
        AgentClientError::AgentError(msg) => DriverError::CommandFailed(msg),
        AgentClientError::Timeout => DriverError::Timeout,
    }
}

/// Checks that the response is [`Response::Ok`] and returns a
/// [`DriverError::CommandFailed`] if it is not.
pub(crate) fn expect_ok(response: Response) -> Result<(), DriverError> {
    match response {
        Response::Ok => Ok(()),
        other => Err(DriverError::CommandFailed(format!(
            "unexpected response: {other:?}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// AgentTransport
// ---------------------------------------------------------------------------

/// The outcome of a successful [`AgentTransport::recover`]: a fresh,
/// heartbeat-verified client plus whether the target package must be re-sent.
///
/// A *cheap* reconnect to a still-alive agent process leaves the agent's target
/// intact (`restore_target = false`); a *full* recovery that respawns the agent
/// must re-send `SetTarget` (`restore_target = true`).
pub struct Recovered {
    /// The fresh, connected client to install on the session.
    pub client: AgentClient,
    /// Whether the session should re-send the remembered `SetTarget`.
    pub restore_target: bool,
}

/// Abstracts the transport-specific half of an agent connection: how to open a
/// client, and how to recover one after a connection error.
///
/// The session ([`AgentSession`]) owns the connection-error *policy* (when to
/// retry, counter bookkeeping, target restoration); the transport owns the
/// *mechanism* (open a socket; optionally respawn the agent).
#[async_trait]
pub trait AgentTransport: Send + Sync + 'static {
    /// Open and heartbeat-verify a fresh [`AgentClient`] for this transport.
    async fn create_client(&self) -> Result<AgentClient, DriverError>;

    /// Whether a connection-class error should trigger [`recover`](Self::recover).
    ///
    /// Defaults to `true` (always recover). The iOS transport returns `false`
    /// when no lifecycle is attached, so the error propagates unchanged.
    fn recovery_enabled(&self) -> bool {
        true
    }

    /// Produce a fresh client after a connection error.
    ///
    /// The default ladder is a single socket reconnect via
    /// [`create_client`](Self::create_client) — appropriate for transports whose
    /// `create_client` already re-establishes the link (e.g. Android re-issues
    /// the `adb forward` rule). Recovery here implies the connection was
    /// re-opened, so the remembered target is restored. The iOS transport
    /// overrides this to try a cheap reconnect first and fall through to a
    /// lifecycle-driven agent respawn.
    async fn recover(&self) -> Result<Recovered, DriverError> {
        Ok(Recovered {
            client: self.create_client().await?,
            restore_target: true,
        })
    }
}

// ---------------------------------------------------------------------------
// AgentSession
// ---------------------------------------------------------------------------

/// The transport-generic driver core.
///
/// Holds the protocol client, the recovery counter, and the remembered target,
/// and implements [`AutomationDriver`] for any [`AgentTransport`]. See the
/// [module docs](self) for how the public driver types alias this.
pub struct AgentSession<T: AgentTransport> {
    /// The transport-specific connector/recoverer.
    pub(crate) transport: T,
    /// The protocol client over the live socket; `None` until `connect`.
    pub(crate) client: Mutex<Option<AgentClient>>,
    /// Number of successful recovery events since creation.
    pub(crate) recovery_count: AtomicU64,
    /// Remembered target bundle/package so it can be re-sent after recovery.
    pub(crate) target_bundle_id: Mutex<Option<String>>,
}

impl<T: AgentTransport> AgentSession<T> {
    /// Build a disconnected session over `transport`. No connection is
    /// established until [`connect`](AutomationDriver::connect) is called.
    ///
    /// Named `from_transport` (not `new`) so the platform aliases can each
    /// expose their own `new` with a transport-appropriate signature.
    pub(crate) fn from_transport(transport: T) -> Self {
        Self {
            transport,
            client: Mutex::new(None),
            recovery_count: AtomicU64::new(0),
            target_bundle_id: Mutex::new(None),
        }
    }

    /// Returns the number of successful recovery events since creation.
    ///
    /// The executor polls this to detect a mid-action reconnect and reset its
    /// wait timer accordingly. Inherent alias of the trait method so concrete
    /// `AgentDriver`/`AndroidDriver` callers don't need the trait in scope;
    /// delegates to the single trait-method definition so the two never diverge.
    pub fn recovery_count(&self) -> u64 {
        AutomationDriver::recovery_count(self)
    }

    /// Returns `true` if the error indicates a broken connection that a recovery
    /// attempt may fix.
    pub(crate) fn is_connection_error(err: &DriverError) -> bool {
        matches!(
            err,
            DriverError::NotConnected
                | DriverError::ConnectionLost(_)
                | DriverError::Io(_)
                | DriverError::Timeout
        )
    }

    /// Run the transport's recovery ladder, install the fresh client, restore
    /// the target if required, and bump the recovery counter.
    async fn do_recover(&self) -> Result<(), DriverError> {
        let Recovered {
            client,
            restore_target,
        } = self.transport.recover().await?;
        *self.client.lock().await = Some(client);
        if restore_target {
            self.restore_target().await?;
        }
        self.recovery_count.fetch_add(1, Ordering::Relaxed);
        // Neutral wording: this fires for both a cheap same-process reconnect
        // and a full agent respawn, so it must not imply the heavier path. The
        // transport's own `recover` logs the rung-specific detail.
        info!("agent connection recovered");
        Ok(())
    }

    /// Re-send the `SetTarget` command if one was previously set, so a freshly
    /// (re)connected agent automates the right app.
    pub(crate) async fn restore_target(&self) -> Result<(), DriverError> {
        let bundle_id = self.target_bundle_id.lock().await.clone();
        if let Some(bid) = bundle_id {
            info!(bundle_id = %bid, "restoring target after recovery");
            let response = self.send_raw(&Request::SetTarget { bundle_id: bid }).await?;
            expect_ok(response)?;
        }
        Ok(())
    }

    /// Send a request, retrying once via the transport's recovery ladder on a
    /// connection error (when the transport opts in).
    async fn send(&self, request: &Request) -> Result<Response, DriverError> {
        let result = self.send_raw(request).await;
        match &result {
            Err(e) if Self::is_connection_error(e) && self.transport.recovery_enabled() => {
                warn!(error = %e, opcode = request.opcode_name(), "connection error, attempting recovery");
                self.do_recover().await?;
                self.send_raw(request).await
            }
            _ => result,
        }
    }

    /// Send a request without recovery wrapping.
    async fn send_raw(&self, request: &Request) -> Result<Response, DriverError> {
        let lock_start = Instant::now();
        let mut guard = self.client.lock().await;
        let lock_elapsed = lock_start.elapsed();
        if lock_elapsed > Duration::from_millis(500) {
            warn!(
                elapsed_ms = lock_elapsed.as_millis() as u64,
                "slow mutex acquisition on agent client"
            );
        }
        let client = guard.as_mut().ok_or(DriverError::NotConnected)?;
        client.send(request).await.map_err(map_client_error)
    }

    /// Send a request with a custom read timeout, retrying once via recovery on
    /// a connection error. When `timeout_ms` is `None`, falls back to
    /// [`send`](Self::send).
    async fn send_with_read_timeout(
        &self,
        request: &Request,
        timeout_ms: Option<u64>,
    ) -> Result<Response, DriverError> {
        let result = self.send_raw_with_read_timeout(request, timeout_ms).await;
        match &result {
            Err(e) if Self::is_connection_error(e) && self.transport.recovery_enabled() => {
                warn!(error = %e, opcode = request.opcode_name(), "connection error (with timeout), attempting recovery");
                self.do_recover().await?;
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
                // side does not drop the socket before the agent replies.
                let read_timeout = Duration::from_millis(ms + READ_TIMEOUT_PADDING_MS);
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

// ---------------------------------------------------------------------------
// Blanket AutomationDriver impl
// ---------------------------------------------------------------------------

#[async_trait]
impl<T: AgentTransport> AutomationDriver for AgentSession<T> {
    #[instrument(skip(self), level = "debug")]
    async fn connect(&mut self) -> Result<(), DriverError> {
        let client = self.transport.create_client().await?;
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
        let response = self
            .send_with_read_timeout(&Request::DumpTree, Some(DUMP_TREE_TIMEOUT_MS))
            .await?;
        match response {
            Response::Tree { json } => {
                let elements: Vec<UIElement> = serde_json::from_str(&json)
                    .map_err(|e| DriverError::JsonParse(e.to_string()))?;
                debug!(element_count = elements.len(), "tree dumped");
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
                debug!(bytes = data.len(), "screenshot captured");
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
        // Remember for restore after recovery.
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
                // Deserialize the agent's JSON response; bundle_id falls back to
                // what we tracked locally if the agent omits it.
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
