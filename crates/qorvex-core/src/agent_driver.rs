//! [`AutomationDriver`] implementation backed by a Swift agent connection.
//!
//! This module provides [`AgentDriver`], which implements the [`AutomationDriver`]
//! trait by communicating with a Swift accessibility agent using the binary
//! protocol defined in [`crate::protocol`].
//!
//! [`AgentDriver`] is a thin alias over the transport-generic
//! [`AgentSession`](crate::agent_session::AgentSession): all protocol plumbing,
//! the recovery ladder, and the [`AutomationDriver`] trait surface live in
//! [`crate::agent_session`]; this module supplies only the iOS/macOS transport
//! ([`IosTransport`]) and the driver's constructors/accessors.
//!
//! The driver supports four connection modes (see [`ConnectionTarget`]):
//!
//! - **Direct TCP** (simulators): connects to a host:port via TCP socket
//! - **USB tunnel** (physical devices): tunnels through usbmuxd to a port on
//!   the device, using the [`usb_tunnel`](crate::usb_tunnel) module
//! - **Tunneld** (pymobiledevice3): connects through a pymobiledevice3 tunnel
//!   to the agent on the device
//! - **CoreDevice** (iOS 17+): connects via the native CoreDevice tunnel
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

use async_trait::async_trait;
use tracing::{info, warn};

use crate::agent_client::AgentClient;
use crate::agent_lifecycle::AgentLifecycle;
use crate::agent_session::{map_client_error, AgentSession, AgentTransport, Recovered};
use crate::driver::DriverError;

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
    /// Connect via a pymobiledevice3 tunnel (CoreDevice devices).
    Tunneld {
        /// Tunnel IP address (typically IPv6 link-local from tunneld).
        tunnel_address: String,
        /// The agent TCP port on the device.
        agent_port: u16,
    },
    /// Connect via the native CoreDevice tunnel (iOS 17+, no pymobiledevice3 required).
    CoreDevice {
        /// The device UDID.
        udid: String,
        /// The TCP port the agent listens on inside the tunnel.
        port: u16,
    },
}

// ---------------------------------------------------------------------------
// IosTransport
// ---------------------------------------------------------------------------

/// The iOS/macOS transport half of an [`AgentSession`]: opens a Swift-agent
/// connection for a [`ConnectionTarget`] and, when a lifecycle is attached,
/// recovers a dead connection by a cheap TCP reconnect or a full agent respawn.
#[doc(hidden)]
pub struct IosTransport {
    target: ConnectionTarget,
    lifecycle: Option<Arc<AgentLifecycle>>,
}

#[async_trait]
impl AgentTransport for IosTransport {
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
            ConnectionTarget::Tunneld {
                tunnel_address,
                agent_port,
            } => {
                let stream =
                    crate::usb_tunnel::connect_tunneld(tunnel_address, *agent_port).await?;
                AgentClient::from_stream(stream)
            }
            ConnectionTarget::CoreDevice { udid, port } => {
                let stream = crate::core_device_tunnel::connect_coredevice(udid, *port)
                    .await
                    .map_err(|e| DriverError::ConnectionLost(e.to_string()))?;
                AgentClient::from_stream(stream)
            }
        };

        client.heartbeat().await.map_err(map_client_error)?;
        Ok(client)
    }

    /// Recovery is only attempted when a lifecycle manager is attached; without
    /// one, a connection error propagates unchanged (matching the original
    /// behavior).
    fn recovery_enabled(&self) -> bool {
        self.lifecycle.is_some()
    }

    async fn recover(&self) -> Result<Recovered, DriverError> {
        // Cheap path first: a fresh TCP connect. After a read timeout the stream
        // is dropped, but the agent process may still be alive — just slow — and
        // still holds the target, so a successful reconnect needs no SetTarget
        // restore. A fresh connect is far cheaper than a kill-and-respawn cycle.
        info!("attempting TCP reconnect (agent may still be alive)");
        match self.create_client().await {
            Ok(client) => {
                info!("TCP reconnect succeeded");
                return Ok(Recovered {
                    client,
                    restore_target: false,
                });
            }
            Err(e) => warn!(error = %e, "TCP reconnect failed"),
        }

        // Full recovery: terminate the dead agent, respawn it (skip rebuild —
        // the XCTest bundle is still on disk), wait for ready, then reconnect.
        // The fresh agent has no target, so the caller restores it.
        let lifecycle = self.lifecycle.as_ref().ok_or(DriverError::NotConnected)?;
        info!("agent connection lost, attempting recovery");
        lifecycle
            .terminate_agent()
            .map_err(|e| DriverError::CommandFailed(format!("recovery: terminate failed: {e}")))?;
        lifecycle
            .spawn_agent()
            .map_err(|e| DriverError::CommandFailed(format!("recovery: spawn failed: {e}")))?;
        lifecycle
            .wait_for_ready()
            .await
            .map_err(|e| DriverError::CommandFailed(format!("recovery: agent not ready: {e}")))?;
        let client = self.create_client().await?;
        Ok(Recovered {
            client,
            restore_target: true,
        })
    }
}

// ---------------------------------------------------------------------------
// AgentDriver
// ---------------------------------------------------------------------------

/// An [`AutomationDriver`] backed by a connection to a Swift agent.
///
/// This is a type alias for [`AgentSession`] specialized to the iOS/macOS
/// transport. The constructors below pick a [`ConnectionTarget`]; the session
/// lazily opens an [`AgentClient`] when [`connect`](AutomationDriver::connect)
/// is called.
pub type AgentDriver = AgentSession<IosTransport>;

impl AgentSession<IosTransport> {
    /// Creates a driver with a direct TCP connection target.
    ///
    /// No connection is established until [`connect`](AutomationDriver::connect) is called.
    pub fn direct(host: impl Into<String>, port: u16) -> Self {
        Self::from_transport(IosTransport {
            target: ConnectionTarget::Direct {
                host: host.into(),
                port,
            },
            lifecycle: None,
        })
    }

    /// Creates a driver that will tunnel to a physical device via USB.
    ///
    /// No connection is established until [`connect`](AutomationDriver::connect) is called.
    pub fn usb_device(udid: impl Into<String>, device_port: u16) -> Self {
        Self::from_transport(IosTransport {
            target: ConnectionTarget::UsbDevice {
                udid: udid.into(),
                device_port,
            },
            lifecycle: None,
        })
    }

    /// Creates a driver that will connect through a pymobiledevice3 tunnel.
    ///
    /// No connection is established until [`connect`](AutomationDriver::connect) is called.
    pub fn tunneld(tunnel_address: impl Into<String>, agent_port: u16) -> Self {
        Self::from_transport(IosTransport {
            target: ConnectionTarget::Tunneld {
                tunnel_address: tunnel_address.into(),
                agent_port,
            },
            lifecycle: None,
        })
    }

    /// Creates a driver that will connect via the native CoreDevice tunnel (iOS 17+).
    ///
    /// No connection is established until [`connect`](AutomationDriver::connect) is called.
    pub fn core_device(udid: impl Into<String>, port: u16) -> Self {
        Self::from_transport(IosTransport {
            target: ConnectionTarget::CoreDevice {
                udid: udid.into(),
                port,
            },
            lifecycle: None,
        })
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
    /// if a connection error is detected during a send. Called during
    /// construction, before `connect`.
    pub fn with_lifecycle(mut self, lifecycle: Arc<AgentLifecycle>) -> Self {
        self.transport.lifecycle = Some(lifecycle);
        self
    }

    /// Returns the connection target.
    pub fn target(&self) -> &ConnectionTarget {
        &self.transport.target
    }

    /// Returns the configured host (or device identifier for tunneled targets).
    pub fn host(&self) -> &str {
        match &self.transport.target {
            ConnectionTarget::Direct { host, .. } => host,
            ConnectionTarget::UsbDevice { udid, .. } => udid,
            ConnectionTarget::Tunneld { tunnel_address, .. } => tunnel_address,
            ConnectionTarget::CoreDevice { udid, .. } => udid,
        }
    }

    /// Returns the configured port.
    pub fn port(&self) -> u16 {
        match &self.transport.target {
            ConnectionTarget::Direct { port, .. } => *port,
            ConnectionTarget::UsbDevice { device_port, .. } => *device_port,
            ConnectionTarget::Tunneld { agent_port, .. } => *agent_port,
            ConnectionTarget::CoreDevice { port, .. } => *port,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_client::AgentClientError;
    use crate::agent_session::{expect_ok, map_client_error};
    use crate::driver::AutomationDriver;
    use crate::protocol::{encode_response, Response};
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

    #[test]
    fn tunneld_creates_tunneld_target() {
        let driver = AgentDriver::tunneld("fd00::1", 8080);
        assert!(matches!(
            driver.target(),
            ConnectionTarget::Tunneld { tunnel_address, agent_port }
                if tunnel_address == "fd00::1" && *agent_port == 8080
        ));
        assert_eq!(driver.host(), "fd00::1");
        assert_eq!(driver.port(), 8080);
        assert!(!driver.is_connected());
    }

    #[test]
    fn core_device_creates_core_device_target() {
        let driver = AgentDriver::core_device("00008140-000A15911AE3001C", 8080);
        match driver.target() {
            ConnectionTarget::CoreDevice { udid, port } => {
                assert_eq!(udid, "00008140-000A15911AE3001C");
                assert_eq!(*port, 8080);
            }
            other => panic!("Expected CoreDevice, got: {:?}", other),
        }
        assert_eq!(driver.host(), "00008140-000A15911AE3001C");
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

        let value = driver.get_element_value_by_label("Email").await.unwrap();
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
        let err = map_client_error(AgentClientError::ConnectionFailed("refused".to_string()));
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
        let err = map_client_error(AgentClientError::Protocol(ProtocolError::InvalidOpCode(
            0xFF,
        )));
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
        assert!(AgentDriver::is_connection_error(
            &DriverError::ConnectionLost("reset".to_string())
        ));
        assert!(AgentDriver::is_connection_error(&DriverError::Io(
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken")
        )));

        // Timeout — now treated as connection error (stream is dropped on timeout)
        assert!(AgentDriver::is_connection_error(&DriverError::Timeout));

        // Non-connection errors — should return false
        assert!(!AgentDriver::is_connection_error(
            &DriverError::CommandFailed("element not found".to_string())
        ));
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
        assert!(driver.transport.lifecycle.is_none());
        driver.connect().await.unwrap();

        // This should fail with a connection error and propagate directly
        let result = driver.tap_element("btn").await;
        assert!(result.is_err());
        // The error should be a connection-type error
        assert!(AgentDriver::is_connection_error(&result.unwrap_err()));
    }

    // -----------------------------------------------------------------------
    // try_reconnect path
    // -----------------------------------------------------------------------

    /// Helper: handle one connection (heartbeat + one request/response).
    async fn handle_one_connection(stream: &mut tokio::net::TcpStream, response: &Response) {
        let mut header = [0u8; 4];

        // Heartbeat
        stream.read_exact(&mut header).await.unwrap();
        let len = crate::protocol::read_frame_length(&header) as usize;
        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload).await.unwrap();
        let ok_bytes = encode_response(&Response::Ok);
        stream.write_all(&ok_bytes).await.unwrap();
        stream.flush().await.unwrap();

        // Actual request
        stream.read_exact(&mut header).await.unwrap();
        let len = crate::protocol::read_frame_length(&header) as usize;
        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload).await.unwrap();
        let response_bytes = encode_response(response);
        stream.write_all(&response_bytes).await.unwrap();
        stream.flush().await.unwrap();
    }

    /// When the first send fails with a connection error and a lifecycle is
    /// attached, the driver should try a TCP reconnect before falling through
    /// to full agent recovery. If the reconnect succeeds, the retry should
    /// succeed without recovery.
    #[tokio::test]
    async fn reconnect_succeeds_avoids_recovery() {
        use crate::agent_lifecycle::{AgentLifecycle, AgentLifecycleConfig};
        use std::path::PathBuf;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            // --- First connection (initial connect) ---
            let (mut stream, _) = listener.accept().await.unwrap();

            // Heartbeat during connect()
            let mut header = [0u8; 4];
            stream.read_exact(&mut header).await.unwrap();
            let len = crate::protocol::read_frame_length(&header) as usize;
            let mut payload = vec![0u8; len];
            stream.read_exact(&mut payload).await.unwrap();
            let ok_bytes = encode_response(&Response::Ok);
            stream.write_all(&ok_bytes).await.unwrap();
            stream.flush().await.unwrap();

            // Read the tap request then drop the connection (simulate timeout drop)
            let _ = stream.read_exact(&mut header).await;
            drop(stream);

            // --- Second connection (reconnect attempt) ---
            let (mut stream2, _) = listener.accept().await.unwrap();
            handle_one_connection(&mut stream2, &Response::Ok).await;
        });

        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());

        // Attach a dummy lifecycle so the reconnect path is entered.
        // Recovery will never be called if reconnect succeeds.
        let dummy_lifecycle = Arc::new(AgentLifecycle::new(
            "FAKE-UDID".to_string(),
            AgentLifecycleConfig::new(PathBuf::from("/nonexistent")),
        ));
        driver = driver.with_lifecycle(dummy_lifecycle);

        driver.connect().await.unwrap();

        // First send_raw fails (dropped connection) → try_reconnect succeeds
        // → retry via send_raw succeeds → no recovery needed.
        driver.tap_location(50, 50).await.unwrap();
    }

    /// When the first send fails and the reconnect also fails (nothing
    /// listening), the driver falls through to attempt_recovery. With a
    /// dummy lifecycle that can't actually spawn an agent, recovery fails
    /// and the error propagates.
    #[tokio::test]
    async fn reconnect_fails_falls_through_to_recovery() {
        use crate::agent_lifecycle::{AgentLifecycle, AgentLifecycleConfig};
        use std::path::PathBuf;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            // Only one connection — the initial connect.
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

            // Read the tap request then drop.
            let _ = stream.read_exact(&mut header).await;
            drop(stream);

            // Drop the listener so reconnect gets "connection refused".
            drop(listener);
        });

        // Small delay to ensure the listener is ready
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        let dummy_lifecycle = Arc::new(AgentLifecycle::new(
            "FAKE-UDID".to_string(),
            AgentLifecycleConfig::new(PathBuf::from("/nonexistent")),
        ));
        driver = driver.with_lifecycle(dummy_lifecycle);

        driver.connect().await.unwrap();

        // send_raw fails → try_reconnect fails (listener dropped) →
        // attempt_recovery fails (dummy lifecycle) → error propagates.
        let result = driver.tap_location(50, 50).await;
        assert!(result.is_err());
    }

    /// Same as reconnect_succeeds_avoids_recovery but exercises the
    /// send_with_read_timeout path (used by dump_tree).
    #[tokio::test]
    async fn reconnect_succeeds_with_read_timeout_path() {
        use crate::agent_lifecycle::{AgentLifecycle, AgentLifecycleConfig};
        use std::path::PathBuf;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let tree_json = r#"[{"type":"Application","frame":{"x":0,"y":0,"width":390,"height":844},"children":[]}]"#;
        let tree_response = Response::Tree {
            json: tree_json.to_string(),
        };

        tokio::spawn(async move {
            // --- First connection (initial connect) ---
            let (mut stream, _) = listener.accept().await.unwrap();

            // Heartbeat during connect()
            let mut header = [0u8; 4];
            stream.read_exact(&mut header).await.unwrap();
            let len = crate::protocol::read_frame_length(&header) as usize;
            let mut payload = vec![0u8; len];
            stream.read_exact(&mut payload).await.unwrap();
            let ok_bytes = encode_response(&Response::Ok);
            stream.write_all(&ok_bytes).await.unwrap();
            stream.flush().await.unwrap();

            // Read the DumpTree request then drop (simulate timeout → stream dropped)
            let _ = stream.read_exact(&mut header).await;
            drop(stream);

            // --- Second connection (reconnect) ---
            let (mut stream2, _) = listener.accept().await.unwrap();
            handle_one_connection(&mut stream2, &tree_response).await;
        });

        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        let dummy_lifecycle = Arc::new(AgentLifecycle::new(
            "FAKE-UDID".to_string(),
            AgentLifecycleConfig::new(PathBuf::from("/nonexistent")),
        ));
        driver = driver.with_lifecycle(dummy_lifecycle);

        driver.connect().await.unwrap();

        // dump_tree uses send_with_read_timeout internally.
        let elements = driver.dump_tree().await.unwrap();
        assert_eq!(elements.len(), 1);
    }
}
