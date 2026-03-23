//! USB tunnel for communicating with physical iOS devices via usbmuxd.
//!
//! This module provides device discovery and port-forwarding through Apple's
//! `usbmuxd` daemon. When a physical device is connected over USB, [`connect`]
//! establishes a tunnel to the Swift agent's TCP port on the device and returns
//! a stream that can be used with [`AgentClient::from_stream`](crate::agent_client::AgentClient::from_stream).
//!
//! # Example
//!
//! ```no_run
//! use qorvex_core::usb_tunnel;
//! use qorvex_core::agent_client::AgentClient;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // List connected physical devices.
//! let devices = usb_tunnel::list_devices().await?;
//! let device = &devices[0];
//!
//! // Create a tunnel to the agent port on the device.
//! let stream = usb_tunnel::connect(&device.udid, 8080).await?;
//! let mut client = AgentClient::from_stream(stream);
//! client.heartbeat().await?;
//! # Ok(())
//! # }
//! ```

use std::fmt;
use std::net::IpAddr;

use idevice::usbmuxd::{Connection, UsbmuxdConnection};
use thiserror::Error;

use crate::agent_client::AgentStream;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during USB tunnel operations.
#[derive(Error, Debug)]
pub enum UsbTunnelError {
    /// Failed to connect to the local usbmuxd daemon.
    #[error("failed to connect to usbmuxd: {0}")]
    UsbmuxdUnavailable(String),

    /// No device with the given UDID was found.
    #[error("device not found: {0}")]
    DeviceNotFound(String),

    /// Failed to establish a tunnel to the device port.
    #[error("tunnel connection failed: {0}")]
    ConnectionFailed(String),

    /// The tunnel connection returned no usable socket.
    #[error("tunnel socket unavailable")]
    NoSocket,
}

impl From<idevice::IdeviceError> for UsbTunnelError {
    fn from(err: idevice::IdeviceError) -> Self {
        UsbTunnelError::ConnectionFailed(err.to_string())
    }
}

// ---------------------------------------------------------------------------
// PhysicalDevice
// ---------------------------------------------------------------------------

/// A physical iOS device discovered via usbmuxd.
#[derive(Debug, Clone)]
pub struct PhysicalDevice {
    /// Unique Device Identifier (UDID).
    pub udid: String,
    /// The usbmuxd-assigned numeric device ID (used internally for connections).
    pub device_id: u32,
    /// How the device is connected.
    pub connection: DeviceConnection,
}

/// How a physical device is connected to the host.
#[derive(Debug, Clone)]
pub enum DeviceConnection {
    /// Connected via USB cable.
    Usb,
    /// Connected via the network (WiFi).
    Network(IpAddr),
    /// Unknown connection type.
    Unknown(String),
}

impl fmt::Display for DeviceConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeviceConnection::Usb => write!(f, "USB"),
            DeviceConnection::Network(ip) => write!(f, "Network ({ip})"),
            DeviceConnection::Unknown(s) => write!(f, "Unknown ({s})"),
        }
    }
}

impl From<Connection> for DeviceConnection {
    fn from(conn: Connection) -> Self {
        match conn {
            Connection::Usb => DeviceConnection::Usb,
            Connection::Network(ip) => DeviceConnection::Network(ip),
            Connection::Unknown(s) => DeviceConnection::Unknown(s),
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// List all physical iOS devices currently connected via usbmuxd.
///
/// Returns an empty list if no devices are connected or usbmuxd is unavailable.
pub async fn list_devices() -> Result<Vec<PhysicalDevice>, UsbTunnelError> {
    let mut muxd = UsbmuxdConnection::default()
        .await
        .map_err(|e| UsbTunnelError::UsbmuxdUnavailable(e.to_string()))?;

    let devices = muxd
        .get_devices()
        .await
        .map_err(|e| UsbTunnelError::UsbmuxdUnavailable(e.to_string()))?;

    Ok(devices
        .into_iter()
        .map(|d| PhysicalDevice {
            udid: d.udid,
            device_id: d.device_id,
            connection: d.connection_type.into(),
        })
        .collect())
}

/// Establish a USB tunnel to a device port and return a stream.
///
/// Connects to the usbmuxd daemon, finds the device by UDID, and creates a
/// tunneled connection to the given port on the device. The returned stream
/// implements [`AgentStream`] and can be passed to
/// [`AgentClient::from_stream`](crate::agent_client::AgentClient::from_stream).
///
/// # Arguments
///
/// * `udid` - The UDID of the target device
/// * `port` - The TCP port on the device to tunnel to (e.g., 8080 for the agent)
pub async fn connect(udid: &str, port: u16) -> Result<Box<dyn AgentStream>, UsbTunnelError> {
    let mut muxd = UsbmuxdConnection::default()
        .await
        .map_err(|e| UsbTunnelError::UsbmuxdUnavailable(e.to_string()))?;

    let device = muxd
        .get_device(udid)
        .await
        .map_err(|_| UsbTunnelError::DeviceNotFound(udid.to_string()))?;

    let idevice = muxd
        .connect_to_device(device.device_id, port, "qorvex")
        .await?;

    let socket = idevice.get_socket().ok_or(UsbTunnelError::NoSocket)?;

    // socket is Box<dyn idevice::ReadWrite> which implements
    // AsyncRead + AsyncWrite + Unpin + Send (i.e., AgentStream).
    Ok(Box::new(socket))
}

// ---------------------------------------------------------------------------
// Tunneld support (pymobiledevice3)
// ---------------------------------------------------------------------------

/// A device available through pymobiledevice3's tunneld service.
#[derive(Debug, Clone)]
pub struct TunneldDeviceInfo {
    /// Device UDID.
    pub udid: String,
    /// Tunnel IP address (typically an IPv6 link-local address).
    pub tunnel_address: String,
    /// Tunnel port number.
    pub tunnel_port: u16,
    /// Network interface name.
    pub interface: String,
}

/// List physical devices available through pymobiledevice3's tunneld service.
///
/// Returns an empty list if tunneld is not running or unreachable.
pub async fn list_tunneld_devices() -> Result<Vec<TunneldDeviceInfo>, UsbTunnelError> {
    use std::net::SocketAddr;
    use std::str::FromStr;

    let addr = SocketAddr::new(
        IpAddr::from_str("127.0.0.1").unwrap(),
        idevice::tunneld::DEFAULT_PORT,
    );

    match idevice::tunneld::get_tunneld_devices(addr).await {
        Ok(devices) => Ok(devices
            .into_iter()
            .map(|(udid, td)| TunneldDeviceInfo {
                udid,
                tunnel_address: td.tunnel_address,
                tunnel_port: td.tunnel_port,
                interface: td.interface,
            })
            .collect()),
        Err(_) => Ok(Vec::new()), // tunneld not running — return empty, not error
    }
}

/// Connect to a device's agent port through a pymobiledevice3 tunnel.
///
/// This establishes a direct TCP connection to the tunnel address/port provided
/// by tunneld, then connects to the agent port on the device.
///
/// Note: tunneld already handles port forwarding — the `tunnel_address:agent_port`
/// gives us a direct TCP path to the device.
pub async fn connect_tunneld(
    tunnel_address: &str,
    agent_port: u16,
) -> Result<Box<dyn AgentStream>, UsbTunnelError> {
    use tokio::net::TcpStream;

    // IPv6 addresses must be wrapped in brackets for the socket address string.
    let addr = if tunnel_address.contains(':') {
        format!("[{}]:{}", tunnel_address, agent_port)
    } else {
        format!("{}:{}", tunnel_address, agent_port)
    };

    let stream = TcpStream::connect(&addr).await.map_err(|e| {
        UsbTunnelError::ConnectionFailed(format!(
            "Failed to connect to tunnel {}:{}: {}",
            tunnel_address, agent_port, e
        ))
    })?;

    Ok(Box::new(stream))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_connection_display_usb() {
        assert_eq!(DeviceConnection::Usb.to_string(), "USB");
    }

    #[test]
    fn device_connection_display_network() {
        let ip: IpAddr = "192.168.1.100".parse().unwrap();
        let conn = DeviceConnection::Network(ip);
        assert!(conn.to_string().contains("192.168.1.100"));
    }

    #[test]
    fn device_connection_display_unknown() {
        let conn = DeviceConnection::Unknown("bluetooth".into());
        assert!(conn.to_string().contains("bluetooth"));
    }

    #[test]
    fn device_connection_from_idevice_usb() {
        let conn: DeviceConnection = Connection::Usb.into();
        assert!(matches!(conn, DeviceConnection::Usb));
    }

    #[test]
    fn device_connection_from_idevice_network() {
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let conn: DeviceConnection = Connection::Network(ip).into();
        match conn {
            DeviceConnection::Network(addr) => assert_eq!(addr.to_string(), "10.0.0.1"),
            other => panic!("expected Network, got: {other:?}"),
        }
    }

    #[test]
    fn device_connection_from_idevice_unknown() {
        let conn: DeviceConnection = Connection::Unknown("zigbee".into()).into();
        match conn {
            DeviceConnection::Unknown(s) => assert_eq!(s, "zigbee"),
            other => panic!("expected Unknown, got: {other:?}"),
        }
    }

    #[test]
    fn usb_tunnel_error_display() {
        let err = UsbTunnelError::UsbmuxdUnavailable("not running".into());
        assert!(err.to_string().contains("usbmuxd"));

        let err = UsbTunnelError::DeviceNotFound("ABC123".into());
        assert!(err.to_string().contains("ABC123"));

        let err = UsbTunnelError::ConnectionFailed("refused".into());
        assert!(err.to_string().contains("refused"));

        let err = UsbTunnelError::NoSocket;
        assert!(err.to_string().contains("socket unavailable"));
    }

    #[test]
    fn physical_device_construction() {
        let device = PhysicalDevice {
            udid: "00008110-001A0C123456789A".into(),
            device_id: 42,
            connection: DeviceConnection::Usb,
        };
        assert_eq!(device.udid, "00008110-001A0C123456789A");
        assert_eq!(device.device_id, 42);
        assert!(matches!(device.connection, DeviceConnection::Usb));
    }
}
