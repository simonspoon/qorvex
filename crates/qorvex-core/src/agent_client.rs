//! Async client for communicating with the Swift accessibility agent.
//!
//! This module provides [`AgentClient`], a low-level transport layer that sends
//! [`Request`]s and receives [`Response`]s over a bidirectional async stream
//! using the binary protocol defined in [`crate::protocol`].
//!
//! The client can connect via direct TCP (for simulators on localhost) or accept
//! a pre-connected stream (for USB tunnels to physical devices).
//!
//! # Example
//!
//! ```no_run
//! use std::net::SocketAddr;
//! use qorvex_core::agent_client::AgentClient;
//! use qorvex_core::protocol::Request;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let addr: SocketAddr = "127.0.0.1:9800".parse()?;
//! let mut client = AgentClient::new(addr);
//!
//! client.connect().await?;
//! client.heartbeat().await?;
//! client.disconnect();
//! # Ok(())
//! # }
//! ```

use std::net::SocketAddr;
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use tracing::{debug, debug_span, trace, Instrument};

use crate::protocol::{
    ProtocolError, Request, Response, decode_response, encode_request, read_frame_length,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Timeout for establishing a TCP connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Timeout for reading a response frame from the agent.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// AgentStream trait
// ---------------------------------------------------------------------------

/// A bidirectional async stream suitable for agent communication.
///
/// Both [`TcpStream`] and USB tunnel connections (via `idevice::ReadWrite`)
/// satisfy these bounds, allowing [`AgentClient`] to work transparently over
/// either transport.
pub trait AgentStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> AgentStream for T {}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during agent communication.
#[derive(Error, Debug)]
pub enum AgentClientError {
    /// Attempted to send a request without an active connection.
    #[error("not connected to agent")]
    NotConnected,

    /// Failed to establish a TCP connection.
    #[error("connection failed: {0}")]
    ConnectionFailed(String),

    /// An I/O error occurred on the stream.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The response could not be decoded according to the protocol.
    #[error("protocol error: {0}")]
    Protocol(#[from] ProtocolError),

    /// The remote agent returned an error response.
    #[error("agent error: {0}")]
    AgentError(String),

    /// A read or connect operation exceeded its timeout.
    #[error("operation timed out")]
    Timeout,
}

// ---------------------------------------------------------------------------
// AgentClient
// ---------------------------------------------------------------------------

/// Async client for the Swift accessibility agent.
///
/// Manages a single connection and provides methods for sending protocol
/// requests and receiving responses. The connection can be established via
/// [`connect`](Self::connect) (direct TCP) or provided as a pre-connected
/// stream via [`from_stream`](Self::from_stream) (USB tunnel).
pub struct AgentClient {
    stream: Option<Box<dyn AgentStream>>,
    addr: Option<SocketAddr>,
}

impl AgentClient {
    /// Create a new client targeting the given address.
    ///
    /// No connection is established until [`connect`](Self::connect) is called.
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            stream: None,
            addr: Some(addr),
        }
    }

    /// Create a client from a pre-connected stream (e.g., a USB tunnel).
    ///
    /// The client is immediately usable for sending requests.
    pub fn from_stream(stream: impl AgentStream + 'static) -> Self {
        Self {
            stream: Some(Box::new(stream)),
            addr: None,
        }
    }

    /// Establish a TCP connection to the agent with a 5-second timeout.
    ///
    /// Only valid for clients created with [`new`](Self::new). Clients created
    /// with [`from_stream`](Self::from_stream) are already connected.
    pub async fn connect(&mut self) -> Result<(), AgentClientError> {
        let addr = self
            .addr
            .ok_or_else(|| AgentClientError::ConnectionFailed("no address configured".into()))?;

        debug!(%addr, "connecting to agent");

        let stream = timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
            .await
            .map_err(|_| AgentClientError::Timeout)?
            .map_err(|e| AgentClientError::ConnectionFailed(e.to_string()))?;

        self.stream = Some(Box::new(stream));
        debug!("connected to agent");
        Ok(())
    }

    /// Close the connection, if one is active.
    pub fn disconnect(&mut self) {
        self.stream.take();
    }

    /// Returns `true` if the client currently holds an open connection.
    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    /// Send a request to the agent and wait for the response.
    ///
    /// If the agent returns a [`Response::Error`], this method converts it into
    /// an [`AgentClientError::AgentError`] so callers can treat all failures
    /// uniformly via the error type.
    pub async fn send(&mut self, request: &Request) -> Result<Response, AgentClientError> {
        let opcode = request.opcode_name();
        let span = debug_span!("agent_send", opcode);
        async {
            let frame = encode_request(request);
            self.write_frame(&frame).await?;

            let payload = self.read_frame().await?;
            let response = decode_response(&payload)?;

            match response {
                Response::Error { message } => Err(AgentClientError::AgentError(message)),
                other => Ok(other),
            }
        }.instrument(span).await
    }

    /// Convenience method to send a heartbeat and verify the agent is alive.
    pub async fn heartbeat(&mut self) -> Result<(), AgentClientError> {
        self.send(&Request::Heartbeat).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Internal frame I/O
    // -----------------------------------------------------------------------

    /// Write a complete frame (header + payload) to the stream.
    ///
    /// The `data` parameter should already include the 4-byte length header
    /// (as produced by [`encode_request`]).
    async fn write_frame(&mut self, data: &[u8]) -> Result<(), AgentClientError> {
        let stream = self.stream.as_mut().ok_or(AgentClientError::NotConnected)?;
        trace!(frame_bytes = data.len(), "writing frame");
        stream.write_all(data).await?;
        stream.flush().await?;
        Ok(())
    }

    /// Read a complete response frame from the stream.
    ///
    /// Reads the 4-byte length header, then reads exactly that many bytes of
    /// payload. Returns the payload bytes (opcode + data, without the header).
    ///
    /// Applies a 30-second timeout to the entire read operation.
    async fn read_frame(&mut self) -> Result<Vec<u8>, AgentClientError> {
        let stream = self.stream.as_mut().ok_or(AgentClientError::NotConnected)?;

        let result = timeout(READ_TIMEOUT, async {
            // Read the 4-byte length header.
            let mut header = [0u8; 4];
            stream.read_exact(&mut header).await?;
            let len = read_frame_length(&header) as usize;

            // Read the payload.
            let mut payload = vec![0u8; len];
            trace!(payload_bytes = len, "reading frame");
            stream.read_exact(&mut payload).await?;

            Ok::<Vec<u8>, std::io::Error>(payload)
        })
        .await;

        match result {
            Ok(Ok(payload)) => Ok(payload),
            Ok(Err(io_err)) => {
                // I/O error — stream is likely broken, drop it to prevent reuse.
                self.stream.take();
                Err(AgentClientError::Io(io_err))
            }
            Err(_) => {
                // Timeout — the agent may still send a response later, leaving
                // stale bytes in the TCP buffer. Drop the stream so the next
                // caller gets NotConnected instead of reading a mismatched
                // response from a previous request.
                self.stream.take();
                Err(AgentClientError::Timeout)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::encode_response;
    use tokio::net::TcpListener;

    #[test]
    fn new_creates_disconnected_client() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let client = AgentClient::new(addr);
        assert!(client.stream.is_none());
        assert_eq!(client.addr, Some(addr));
    }

    #[test]
    fn is_connected_returns_false_initially() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let client = AgentClient::new(addr);
        assert!(!client.is_connected());
    }

    #[test]
    fn from_stream_creates_connected_client() {
        let (client_stream, _server_stream) = tokio::io::duplex(1024);
        let client = AgentClient::from_stream(client_stream);
        assert!(client.is_connected());
        assert!(client.addr.is_none());
    }

    #[tokio::test]
    async fn send_returns_not_connected_when_disconnected() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut client = AgentClient::new(addr);
        let result = client.send(&Request::Heartbeat).await;
        assert!(matches!(result, Err(AgentClientError::NotConnected)));
    }

    /// Helper: start a mock TCP server that accepts one connection, reads a
    /// request frame, and replies with the given response.
    async fn mock_server(response: Response) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();

            // Read the request frame (header + payload).
            let mut header = [0u8; 4];
            stream.read_exact(&mut header).await.unwrap();
            let len = crate::protocol::read_frame_length(&header) as usize;
            let mut payload = vec![0u8; len];
            stream.read_exact(&mut payload).await.unwrap();

            // Send the response.
            let response_bytes = encode_response(&response);
            stream.write_all(&response_bytes).await.unwrap();
            stream.flush().await.unwrap();
        });

        addr
    }

    #[tokio::test]
    async fn heartbeat_ok_via_mock_server() {
        let addr = mock_server(Response::Ok).await;

        let mut client = AgentClient::new(addr);
        client.connect().await.unwrap();
        assert!(client.is_connected());

        client.heartbeat().await.unwrap();
        client.disconnect();
        assert!(!client.is_connected());
    }

    #[tokio::test]
    async fn tap_coord_ok_via_mock_server() {
        let addr = mock_server(Response::Ok).await;

        let mut client = AgentClient::new(addr);
        client.connect().await.unwrap();

        let result = client.send(&Request::TapCoord { x: 100, y: 200 }).await;
        assert!(matches!(result, Ok(Response::Ok)));

        client.disconnect();
    }

    #[tokio::test]
    async fn agent_error_is_propagated() {
        let addr = mock_server(Response::Error {
            message: "element not found".into(),
        })
        .await;

        let mut client = AgentClient::new(addr);
        client.connect().await.unwrap();

        let result = client
            .send(&Request::TapElement {
                selector: "missing".into(),
            })
            .await;

        match result {
            Err(AgentClientError::AgentError(msg)) => {
                assert_eq!(msg, "element not found");
            }
            other => panic!("expected AgentError, got: {other:?}"),
        }

        client.disconnect();
    }

    #[tokio::test]
    async fn tree_response_via_mock_server() {
        let json = r#"{"type":"View","children":[]}"#.to_string();
        let addr = mock_server(Response::Tree { json: json.clone() }).await;

        let mut client = AgentClient::new(addr);
        client.connect().await.unwrap();

        let result = client.send(&Request::DumpTree).await.unwrap();
        assert_eq!(result, Response::Tree { json });

        client.disconnect();
    }

    #[tokio::test]
    async fn from_stream_send_and_receive() {
        let (client_stream, mut server_stream) = tokio::io::duplex(4096);

        // Spawn a mock "server" that reads a request and writes a response.
        tokio::spawn(async move {
            let mut header = [0u8; 4];
            server_stream.read_exact(&mut header).await.unwrap();
            let len = crate::protocol::read_frame_length(&header) as usize;
            let mut payload = vec![0u8; len];
            server_stream.read_exact(&mut payload).await.unwrap();

            let response_bytes = encode_response(&Response::Ok);
            server_stream.write_all(&response_bytes).await.unwrap();
            server_stream.flush().await.unwrap();
        });

        let mut client = AgentClient::from_stream(client_stream);
        assert!(client.is_connected());

        client.heartbeat().await.unwrap();
    }
}
