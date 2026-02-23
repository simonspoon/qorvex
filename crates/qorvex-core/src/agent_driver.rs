//! [`AutomationDriver`] implementation backed by a Swift agent connection.
//!
//! This module provides [`AgentDriver`], which implements the [`AutomationDriver`]
//! trait by communicating with a Swift accessibility agent using the binary
//! protocol defined in [`crate::protocol`].
//!
//! The driver supports two connection modes:
//!
//! - **Direct TCP** (simulators): connects to a host:port via TCP socket
//! - **USB tunnel** (physical devices): tunnels through usbmuxd to a port on
//!   the device, using the [`usb_tunnel`](crate::usb_tunnel) module
//!
//! # Example
//!
//! ```no_run
//! use qorvex_core::agent_driver::{AgentDriver, ConnectionTarget};
//! use qorvex_core::driver::AutomationDriver;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Direct TCP (simulator)
//! let mut driver = AgentDriver::direct("localhost", 9800);
//! driver.connect().await?;
//!
//! // USB tunnel (physical device)
//! let mut driver = AgentDriver::usb_device("00008110-001A0C123456789A", 8080);
//! driver.connect().await?;
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;

use tracing::{debug, info, warn, instrument};

use crate::agent_client::{AgentClient, AgentClientError};
use crate::agent_lifecycle::AgentLifecycle;
use crate::element::UIElement;
use crate::driver::{AutomationDriver, DriverError};
use crate::protocol::{Request, Response};

// ---------------------------------------------------------------------------
// Error mapping
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

/// Checks that the response is [`Response::Ok`] and returns a
/// [`DriverError::CommandFailed`] if it is not.
fn expect_ok(response: Response) -> Result<(), DriverError> {
    match response {
        Response::Ok => Ok(()),
        other => Err(DriverError::CommandFailed(format!(
            "unexpected response: {other:?}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// ConnectionTarget
// ---------------------------------------------------------------------------

/// Specifies how the driver should reach the Swift agent.
#[derive(Debug, Clone)]
pub enum ConnectionTarget {
    /// Connect via direct TCP (typically for simulators on localhost).
    Direct {
        /// Hostname or IP address.
        host: String,
        /// TCP port.
        port: u16,
    },
    /// Connect via USB tunnel through usbmuxd (physical device).
    UsbDevice {
        /// The device UDID.
        udid: String,
        /// The TCP port the agent listens on *on the device*.
        device_port: u16,
    },
}

// ---------------------------------------------------------------------------
// AgentDriver
// ---------------------------------------------------------------------------

/// An [`AutomationDriver`] backed by a connection to a Swift agent.
///
/// The driver holds a [`ConnectionTarget`] and lazily creates an
/// [`AgentClient`] when [`connect`](AutomationDriver::connect) is called.
/// For [`ConnectionTarget::Direct`], this opens a TCP socket; for
/// [`ConnectionTarget::UsbDevice`], it creates a USB tunnel through usbmuxd.
///
/// The client is wrapped in a [`tokio::sync::Mutex`] so that the `&self`
/// trait methods can acquire mutable access for sending requests.
pub struct AgentDriver {
    target: ConnectionTarget,
    client: Mutex<Option<AgentClient>>,
    lifecycle: Option<Arc<AgentLifecycle>>,
}

impl AgentDriver {
    /// Creates a driver with a direct TCP connection target.
    ///
    /// No connection is established until [`connect`](AutomationDriver::connect) is called.
    pub fn direct(host: impl Into<String>, port: u16) -> Self {
        Self {
            target: ConnectionTarget::Direct {
                host: host.into(),
                port,
            },
            client: Mutex::new(None),
            lifecycle: None,
        }
    }

    /// Creates a driver that will tunnel to a physical device via USB.
    ///
    /// No connection is established until [`connect`](AutomationDriver::connect) is called.
    pub fn usb_device(udid: impl Into<String>, device_port: u16) -> Self {
        Self {
            target: ConnectionTarget::UsbDevice {
                udid: udid.into(),
                device_port,
            },
            client: Mutex::new(None),
            lifecycle: None,
        }
    }

    /// Creates a driver for the given host and port (direct TCP).
    ///
    /// This is a convenience alias for [`direct`](Self::direct) that maintains
    /// backward compatibility with existing code.
    pub fn new(host: String, port: u16) -> Self {
        Self::direct(host, port)
    }

    /// Attaches a lifecycle manager for automatic crash recovery.
    ///
    /// When set, the driver will attempt to restart the agent and reconnect
    /// if a connection error is detected during [`send`].
    pub fn with_lifecycle(mut self, lifecycle: Arc<AgentLifecycle>) -> Self {
        self.lifecycle = Some(lifecycle);
        self
    }

    /// Returns the connection target.
    pub fn target(&self) -> &ConnectionTarget {
        &self.target
    }

    /// Returns the configured host, if this is a direct TCP connection.
    pub fn host(&self) -> &str {
        match &self.target {
            ConnectionTarget::Direct { host, .. } => host,
            ConnectionTarget::UsbDevice { udid, .. } => udid,
        }
    }

    /// Returns the configured port.
    pub fn port(&self) -> u16 {
        match &self.target {
            ConnectionTarget::Direct { port, .. } => *port,
            ConnectionTarget::UsbDevice { device_port, .. } => *device_port,
        }
    }

    /// Creates a new [`AgentClient`] for the current [`ConnectionTarget`]
    /// and verifies it with a heartbeat.
    async fn create_client(&self) -> Result<AgentClient, DriverError> {
        let mut client = match &self.target {
            ConnectionTarget::Direct { host, port } => {
                let host_port = format!("{host}:{port}");
                let addr = tokio::net::lookup_host(&host_port)
                    .await
                    .map_err(|e| DriverError::ConnectionLost(e.to_string()))?
                    .next()
                    .ok_or_else(|| {
                        DriverError::ConnectionLost(format!("could not resolve {host_port}"))
                    })?;
                let mut c = AgentClient::new(addr);
                c.connect().await.map_err(map_client_error)?;
                c
            }
            ConnectionTarget::UsbDevice { udid, device_port } => {
                let stream = crate::usb_tunnel::connect(udid, *device_port).await?;
                AgentClient::from_stream(stream)
            }
        };

        client.heartbeat().await.map_err(map_client_error)?;
        Ok(client)
    }

    /// Returns `true` if the error indicates a broken connection that may
    /// be recoverable by restarting the agent.
    fn is_connection_error(err: &DriverError) -> bool {
        matches!(
            err,
            DriverError::NotConnected | DriverError::ConnectionLost(_) | DriverError::Io(_)
        )
    }

    /// Attempt to recover from a dead agent connection.
    ///
    /// Terminates the agent, respawns it (skipping rebuild), waits for it to
    /// become ready, then reconnects and replaces the stored client.
    async fn attempt_recovery(&self) -> Result<(), DriverError> {
        let lifecycle = self
            .lifecycle
            .as_ref()
            .ok_or(DriverError::NotConnected)?;

        info!("agent connection lost, attempting recovery");

        // Terminate the old agent process.
        lifecycle
            .terminate_agent()
            .map_err(|e| DriverError::CommandFailed(format!("recovery: terminate failed: {e}")))?;

        // Respawn (skip rebuild — the XCTest bundle is still on disk).
        lifecycle
            .spawn_agent()
            .map_err(|e| DriverError::CommandFailed(format!("recovery: spawn failed: {e}")))?;

        // Wait for the new agent to become ready.
        lifecycle
            .wait_for_ready()
            .await
            .map_err(|e| DriverError::CommandFailed(format!("recovery: agent not ready: {e}")))?;

        // Reconnect.
        let client = self.create_client().await?;
        *self.client.lock().await = Some(client);

        info!("agent recovery successful");
        Ok(())
    }

    /// Sends a request via the inner [`AgentClient`], mapping errors to
    /// [`DriverError`].
    ///
    /// On connection error, if a lifecycle manager is attached, attempts
    /// automatic recovery and retries once.
    async fn send(&self, request: &Request) -> Result<Response, DriverError> {
        let result = self.send_raw(request).await;

        match &result {
            Err(e) if Self::is_connection_error(e) && self.lifecycle.is_some() => {
                warn!(error = %e, "connection error, attempting recovery");
                self.attempt_recovery().await?;
                self.send_raw(request).await
            }
            _ => result,
        }
    }

    /// Sends a request without recovery wrapping.
    async fn send_raw(&self, request: &Request) -> Result<Response, DriverError> {
        let mut guard = self.client.lock().await;
        let client = guard.as_mut().ok_or(DriverError::NotConnected)?;
        client.send(request).await.map_err(map_client_error)
    }

    /// Sends a request with a custom read timeout.
    ///
    /// When `timeout_ms` is `Some`, the read deadline is set to `timeout_ms + 5s`
    /// so the Rust side waits longer than the agent's internal retry window.
    /// When `None`, falls back to the default `send()`.
    ///
    /// On connection error, if a lifecycle manager is attached, attempts
    /// automatic recovery and retries once.
    async fn send_with_read_timeout(
        &self,
        request: &Request,
        timeout_ms: Option<u64>,
    ) -> Result<Response, DriverError> {
        let result = self.send_raw_with_read_timeout(request, timeout_ms).await;

        match &result {
            Err(e) if Self::is_connection_error(e) && self.lifecycle.is_some() => {
                warn!(error = %e, "connection error (with timeout), attempting recovery");
                self.attempt_recovery().await?;
                self.send_raw_with_read_timeout(request, timeout_ms).await
            }
            _ => result,
        }
    }

    /// Sends a request with a custom read timeout, without recovery wrapping.
    async fn send_raw_with_read_timeout(
        &self,
        request: &Request,
        timeout_ms: Option<u64>,
    ) -> Result<Response, DriverError> {
        match timeout_ms {
            Some(ms) => {
                let read_timeout = Duration::from_millis(ms + 5000);
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
impl AutomationDriver for AgentDriver {
    #[instrument(skip(self), level = "debug")]
    async fn connect(&mut self) -> Result<(), DriverError> {
        let client = self.create_client().await?;
        *self.client.lock().await = Some(client);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.client.try_lock().map(|g| g.is_some()).unwrap_or(false)
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
        let response = self.send(&Request::DumpTree).await?;
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

    async fn get_element_value(
        &self,
        identifier: &str,
    ) -> Result<Option<String>, DriverError> {
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

    async fn get_element_value_by_label(
        &self,
        label: &str,
    ) -> Result<Option<String>, DriverError> {
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
    async fn set_target(&self, bundle_id: &str) -> Result<(), DriverError> {
        let response = self.send(&Request::SetTarget {
            bundle_id: bundle_id.to_string(),
        }).await?;
        expect_ok(response)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::encode_response;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Start a mock TCP server that accepts one connection, reads one request
    /// frame, and replies with the given response. Returns the local address.
    async fn mock_server(response: Response) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();

            // Read request frame.
            let mut header = [0u8; 4];
            stream.read_exact(&mut header).await.unwrap();
            let len = crate::protocol::read_frame_length(&header) as usize;
            let mut payload = vec![0u8; len];
            stream.read_exact(&mut payload).await.unwrap();

            // Send response.
            let response_bytes = encode_response(&response);
            stream.write_all(&response_bytes).await.unwrap();
            stream.flush().await.unwrap();
        });

        addr
    }

    /// Start a mock TCP server that handles the initial heartbeat (during
    /// connect) and then one subsequent request, replying to both.
    async fn mock_server_with_connect(response: Response) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();

            // 1) Read the heartbeat request from connect().
            let mut header = [0u8; 4];
            stream.read_exact(&mut header).await.unwrap();
            let len = crate::protocol::read_frame_length(&header) as usize;
            let mut payload = vec![0u8; len];
            stream.read_exact(&mut payload).await.unwrap();

            // Reply Ok to heartbeat.
            let ok_bytes = encode_response(&Response::Ok);
            stream.write_all(&ok_bytes).await.unwrap();
            stream.flush().await.unwrap();

            // 2) Read the actual request.
            stream.read_exact(&mut header).await.unwrap();
            let len = crate::protocol::read_frame_length(&header) as usize;
            let mut payload = vec![0u8; len];
            stream.read_exact(&mut payload).await.unwrap();

            // Reply with the provided response.
            let response_bytes = encode_response(&response);
            stream.write_all(&response_bytes).await.unwrap();
            stream.flush().await.unwrap();
        });

        addr
    }

    // -----------------------------------------------------------------------
    // Construction
    // -----------------------------------------------------------------------

    #[test]
    fn new_creates_disconnected_driver() {
        let driver = AgentDriver::new("localhost".to_string(), 9800);
        assert_eq!(driver.host(), "localhost");
        assert_eq!(driver.port(), 9800);
        assert!(!driver.is_connected());
    }

    #[test]
    fn new_with_custom_host_port() {
        let driver = AgentDriver::new("192.168.1.100".to_string(), 5555);
        assert_eq!(driver.host(), "192.168.1.100");
        assert_eq!(driver.port(), 5555);
        assert!(!driver.is_connected());
    }

    #[test]
    fn direct_creates_direct_target() {
        let driver = AgentDriver::direct("localhost", 9800);
        assert!(matches!(
            driver.target(),
            ConnectionTarget::Direct { host, port } if host == "localhost" && *port == 9800
        ));
        assert!(!driver.is_connected());
    }

    #[test]
    fn usb_device_creates_usb_target() {
        let driver = AgentDriver::usb_device("ABC-123", 8080);
        assert!(matches!(
            driver.target(),
            ConnectionTarget::UsbDevice { udid, device_port }
                if udid == "ABC-123" && *device_port == 8080
        ));
        assert_eq!(driver.host(), "ABC-123");
        assert_eq!(driver.port(), 8080);
        assert!(!driver.is_connected());
    }

    // -----------------------------------------------------------------------
    // Connection
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn connect_establishes_connection() {
        let addr = mock_server(Response::Ok).await;
        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());

        driver.connect().await.unwrap();
        assert!(driver.is_connected());
    }

    #[tokio::test]
    async fn connect_fails_with_bad_address() {
        // Use a port that (almost certainly) has nothing listening.
        let mut driver = AgentDriver::new("127.0.0.1".to_string(), 1);
        let result = driver.connect().await;
        assert!(result.is_err());
        assert!(!driver.is_connected());
    }

    // -----------------------------------------------------------------------
    // Operations without connection
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn tap_location_returns_not_connected_when_disconnected() {
        let driver = AgentDriver::new("localhost".to_string(), 9800);
        let result = driver.tap_location(100, 200).await;
        assert!(matches!(result, Err(DriverError::NotConnected)));
    }

    #[tokio::test]
    async fn tap_element_returns_not_connected_when_disconnected() {
        let driver = AgentDriver::new("localhost".to_string(), 9800);
        let result = driver.tap_element("btn").await;
        assert!(matches!(result, Err(DriverError::NotConnected)));
    }

    #[tokio::test]
    async fn dump_tree_returns_not_connected_when_disconnected() {
        let driver = AgentDriver::new("localhost".to_string(), 9800);
        let result = driver.dump_tree().await;
        assert!(matches!(result, Err(DriverError::NotConnected)));
    }

    #[tokio::test]
    async fn screenshot_returns_not_connected_when_disconnected() {
        let driver = AgentDriver::new("localhost".to_string(), 9800);
        let result = driver.screenshot().await;
        assert!(matches!(result, Err(DriverError::NotConnected)));
    }

    // -----------------------------------------------------------------------
    // Tap operations
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn tap_location_sends_tap_coord() {
        let addr = mock_server_with_connect(Response::Ok).await;
        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        driver.connect().await.unwrap();

        driver.tap_location(100, 200).await.unwrap();
    }

    #[tokio::test]
    async fn tap_element_sends_tap_element() {
        let addr = mock_server_with_connect(Response::Ok).await;
        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        driver.connect().await.unwrap();

        driver.tap_element("login-button").await.unwrap();
    }

    #[tokio::test]
    async fn tap_by_label_sends_tap_by_label() {
        let addr = mock_server_with_connect(Response::Ok).await;
        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        driver.connect().await.unwrap();

        driver.tap_by_label("Log In").await.unwrap();
    }

    #[tokio::test]
    async fn tap_with_type_sends_request() {
        let addr = mock_server_with_connect(Response::Ok).await;
        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        driver.connect().await.unwrap();

        driver
            .tap_with_type("submit", false, "Button")
            .await
            .unwrap();
    }

    // -----------------------------------------------------------------------
    // Swipe
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn swipe_sends_request() {
        let addr = mock_server_with_connect(Response::Ok).await;
        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        driver.connect().await.unwrap();

        driver.swipe(0, 800, 0, 200, Some(0.5)).await.unwrap();
    }

    #[tokio::test]
    async fn swipe_without_duration() {
        let addr = mock_server_with_connect(Response::Ok).await;
        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        driver.connect().await.unwrap();

        driver.swipe(50, 100, 50, 500, None).await.unwrap();
    }

    // -----------------------------------------------------------------------
    // Long press
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn long_press_sends_request() {
        let addr = mock_server_with_connect(Response::Ok).await;
        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        driver.connect().await.unwrap();

        driver.long_press(150, 300, 1.5).await.unwrap();
    }

    #[tokio::test]
    async fn long_press_returns_not_connected_when_disconnected() {
        let driver = AgentDriver::new("localhost".to_string(), 9800);
        let result = driver.long_press(100, 200, 1.0).await;
        assert!(matches!(result, Err(DriverError::NotConnected)));
    }

    // -----------------------------------------------------------------------
    // Type text
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn type_text_sends_request() {
        let addr = mock_server_with_connect(Response::Ok).await;
        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        driver.connect().await.unwrap();

        driver.type_text("hello@example.com").await.unwrap();
    }

    // -----------------------------------------------------------------------
    // Dump tree
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn dump_tree_parses_json_response() {
        let json = r#"[{
            "AXUniqueId": "main-view",
            "AXLabel": "Main View",
            "type": "View",
            "children": [
                {
                    "AXUniqueId": "login-button",
                    "AXLabel": "Log In",
                    "type": "Button",
                    "children": []
                }
            ]
        }]"#;
        let addr = mock_server_with_connect(Response::Tree {
            json: json.to_string(),
        })
        .await;

        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        driver.connect().await.unwrap();

        let tree = driver.dump_tree().await.unwrap();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].identifier.as_deref(), Some("main-view"));
        assert_eq!(tree[0].label.as_deref(), Some("Main View"));
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(
            tree[0].children[0].identifier.as_deref(),
            Some("login-button")
        );
    }

    #[tokio::test]
    async fn dump_tree_empty_hierarchy() {
        let addr = mock_server_with_connect(Response::Tree {
            json: "[]".to_string(),
        })
        .await;

        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        driver.connect().await.unwrap();

        let tree = driver.dump_tree().await.unwrap();
        assert!(tree.is_empty());
    }

    #[tokio::test]
    async fn dump_tree_invalid_json_returns_error() {
        let addr = mock_server_with_connect(Response::Tree {
            json: "not valid json".to_string(),
        })
        .await;

        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        driver.connect().await.unwrap();

        let result = driver.dump_tree().await;
        assert!(matches!(result, Err(DriverError::JsonParse(_))));
    }

    #[tokio::test]
    async fn dump_tree_unexpected_response_type() {
        let addr = mock_server_with_connect(Response::Ok).await;
        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        driver.connect().await.unwrap();

        let result = driver.dump_tree().await;
        match result {
            Err(DriverError::CommandFailed(msg)) => {
                assert!(msg.contains("unexpected response"));
            }
            other => panic!("expected CommandFailed, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Get value
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn get_element_value_returns_some() {
        let addr = mock_server_with_connect(Response::Value {
            value: Some("hello@example.com".to_string()),
        })
        .await;

        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        driver.connect().await.unwrap();

        let value = driver.get_element_value("email-field").await.unwrap();
        assert_eq!(value.as_deref(), Some("hello@example.com"));
    }

    #[tokio::test]
    async fn get_element_value_returns_none() {
        let addr = mock_server_with_connect(Response::Value { value: None }).await;

        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        driver.connect().await.unwrap();

        let value = driver.get_element_value("empty-field").await.unwrap();
        assert!(value.is_none());
    }

    #[tokio::test]
    async fn get_element_value_by_label_returns_value() {
        let addr = mock_server_with_connect(Response::Value {
            value: Some("typed text".to_string()),
        })
        .await;

        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        driver.connect().await.unwrap();

        let value = driver
            .get_element_value_by_label("Email")
            .await
            .unwrap();
        assert_eq!(value.as_deref(), Some("typed text"));
    }

    #[tokio::test]
    async fn get_value_with_type_returns_value() {
        let addr = mock_server_with_connect(Response::Value {
            value: Some("field value".to_string()),
        })
        .await;

        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        driver.connect().await.unwrap();

        let value = driver
            .get_value_with_type("Email", true, "TextField")
            .await
            .unwrap();
        assert_eq!(value.as_deref(), Some("field value"));
    }

    #[tokio::test]
    async fn get_element_value_unexpected_response() {
        let addr = mock_server_with_connect(Response::Ok).await;
        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        driver.connect().await.unwrap();

        let result = driver.get_element_value("some-field").await;
        match result {
            Err(DriverError::CommandFailed(msg)) => {
                assert!(msg.contains("unexpected response"));
            }
            other => panic!("expected CommandFailed, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Screenshot
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn screenshot_returns_raw_bytes() {
        let png_header = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let addr = mock_server_with_connect(Response::Screenshot {
            data: png_header.clone(),
        })
        .await;

        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        driver.connect().await.unwrap();

        let data = driver.screenshot().await.unwrap();
        assert_eq!(data, png_header);
    }

    #[tokio::test]
    async fn set_target_sends_request() {
        let addr = mock_server_with_connect(Response::Ok).await;
        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        driver.connect().await.unwrap();

        driver.set_target("com.example.myapp").await.unwrap();
    }

    #[tokio::test]
    async fn screenshot_unexpected_response() {
        let addr = mock_server_with_connect(Response::Ok).await;
        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        driver.connect().await.unwrap();

        let result = driver.screenshot().await;
        match result {
            Err(DriverError::CommandFailed(msg)) => {
                assert!(msg.contains("unexpected response"));
            }
            other => panic!("expected CommandFailed, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Error mapping
    // -----------------------------------------------------------------------

    #[test]
    fn map_not_connected() {
        let err = map_client_error(AgentClientError::NotConnected);
        assert!(matches!(err, DriverError::NotConnected));
    }

    #[test]
    fn map_connection_failed() {
        let err = map_client_error(AgentClientError::ConnectionFailed(
            "refused".to_string(),
        ));
        match err {
            DriverError::ConnectionLost(msg) => assert_eq!(msg, "refused"),
            other => panic!("expected ConnectionLost, got: {other:?}"),
        }
    }

    #[test]
    fn map_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken");
        let err = map_client_error(AgentClientError::Io(io_err));
        match err {
            DriverError::Io(e) => assert_eq!(e.kind(), std::io::ErrorKind::BrokenPipe),
            other => panic!("expected Io, got: {other:?}"),
        }
    }

    #[test]
    fn map_protocol_error() {
        use crate::protocol::ProtocolError;
        let err = map_client_error(AgentClientError::Protocol(
            ProtocolError::InvalidOpCode(0xFF),
        ));
        match err {
            DriverError::CommandFailed(msg) => assert!(msg.contains("0xFF")),
            other => panic!("expected CommandFailed, got: {other:?}"),
        }
    }

    #[test]
    fn map_agent_error() {
        let err = map_client_error(AgentClientError::AgentError(
            "element not found".to_string(),
        ));
        match err {
            DriverError::CommandFailed(msg) => assert_eq!(msg, "element not found"),
            other => panic!("expected CommandFailed, got: {other:?}"),
        }
    }

    #[test]
    fn map_timeout() {
        let err = map_client_error(AgentClientError::Timeout);
        assert!(matches!(err, DriverError::Timeout));
    }

    // -----------------------------------------------------------------------
    // expect_ok
    // -----------------------------------------------------------------------

    #[test]
    fn expect_ok_with_ok_response() {
        assert!(expect_ok(Response::Ok).is_ok());
    }

    #[test]
    fn expect_ok_with_tree_response() {
        let result = expect_ok(Response::Tree {
            json: "[]".to_string(),
        });
        match result {
            Err(DriverError::CommandFailed(msg)) => {
                assert!(msg.contains("unexpected response"));
            }
            other => panic!("expected CommandFailed, got: {other:?}"),
        }
    }

    #[test]
    fn expect_ok_with_value_response() {
        let result = expect_ok(Response::Value { value: None });
        match result {
            Err(DriverError::CommandFailed(msg)) => {
                assert!(msg.contains("unexpected response"));
            }
            other => panic!("expected CommandFailed, got: {other:?}"),
        }
    }

    #[test]
    fn expect_ok_with_screenshot_response() {
        let result = expect_ok(Response::Screenshot { data: vec![] });
        match result {
            Err(DriverError::CommandFailed(msg)) => {
                assert!(msg.contains("unexpected response"));
            }
            other => panic!("expected CommandFailed, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // is_connection_error classifier
    // -----------------------------------------------------------------------

    #[test]
    fn test_is_connection_error_classifier() {
        // Connection errors — should return true
        assert!(AgentDriver::is_connection_error(&DriverError::NotConnected));
        assert!(AgentDriver::is_connection_error(&DriverError::ConnectionLost(
            "reset".to_string()
        )));
        assert!(AgentDriver::is_connection_error(&DriverError::Io(
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken")
        )));

        // Non-connection errors — should return false
        assert!(!AgentDriver::is_connection_error(&DriverError::Timeout));
        assert!(!AgentDriver::is_connection_error(&DriverError::CommandFailed(
            "element not found".to_string()
        )));
        assert!(!AgentDriver::is_connection_error(&DriverError::JsonParse(
            "bad json".to_string()
        )));
    }

    // -----------------------------------------------------------------------
    // send without lifecycle propagates error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_send_without_lifecycle_propagates_error() {
        // Create a mock server that handles heartbeat then drops
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();

            // Heartbeat
            let mut header = [0u8; 4];
            stream.read_exact(&mut header).await.unwrap();
            let len = crate::protocol::read_frame_length(&header) as usize;
            let mut payload = vec![0u8; len];
            stream.read_exact(&mut payload).await.unwrap();
            let ok_bytes = encode_response(&Response::Ok);
            stream.write_all(&ok_bytes).await.unwrap();
            stream.flush().await.unwrap();

            // Read next request then drop (simulating crash)
            let _ = stream.read_exact(&mut header).await;
            drop(stream);
        });

        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        // No lifecycle attached — recovery should NOT be attempted
        assert!(driver.lifecycle.is_none());
        driver.connect().await.unwrap();

        // This should fail with a connection error and propagate directly
        let result = driver.tap_element("btn").await;
        assert!(result.is_err());
        // The error should be a connection-type error
        assert!(AgentDriver::is_connection_error(&result.unwrap_err()));
    }
}
