//! Shared test helpers for qorvex-core integration tests.
//!
//! This module provides reusable mock infrastructure for tests that exercise
//! the TCP agent protocol, IPC layer, and full session stack.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use qorvex_core::agent_driver::AgentDriver;
use qorvex_core::driver::AutomationDriver;
use qorvex_core::executor::ActionExecutor;
use qorvex_core::ipc::{IpcClient, IpcServer};
use qorvex_core::protocol::{encode_response, read_frame_length, Response};
use qorvex_core::session::Session;

// ---------------------------------------------------------------------------
// Basic mock helpers (extracted from driver_integration.rs)
// ---------------------------------------------------------------------------

/// Start a mock TCP agent that accepts one connection and handles a sequence
/// of request/response pairs. The first response is always consumed by the
/// heartbeat that `AgentDriver::connect()` sends.
pub async fn mock_agent(responses: Vec<Response>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();

        for response in responses {
            // Read one request frame: 4-byte LE header + payload.
            let mut header = [0u8; 4];
            stream.read_exact(&mut header).await.unwrap();
            let len = read_frame_length(&header) as usize;
            let mut payload = vec![0u8; len];
            stream.read_exact(&mut payload).await.unwrap();

            // Send the canned response.
            let resp_bytes = encode_response(&response);
            stream.write_all(&resp_bytes).await.unwrap();
            stream.flush().await.unwrap();
        }
    });

    addr
}

/// Convenience: create an AgentDriver connected to the mock, with screenshots
/// disabled, ready to use in an ActionExecutor.
pub async fn connected_executor(responses: Vec<Response>) -> ActionExecutor {
    let addr = mock_agent(responses).await;
    let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
    driver.connect().await.unwrap();
    let mut executor = ActionExecutor::new(Arc::new(driver));
    executor.set_capture_screenshots(false);
    executor
}

// ---------------------------------------------------------------------------
// Unique session name (extracted from ipc_integration.rs)
// ---------------------------------------------------------------------------

/// Generate a unique session name for test isolation.
///
/// Uses a UUID prefix to avoid collisions between concurrent test runs.
pub fn unique_session_name() -> String {
    format!(
        "test_{}",
        uuid::Uuid::new_v4()
            .to_string()
            .replace("-", "")[..8]
            .to_string()
    )
}

// ---------------------------------------------------------------------------
// Programmable mock agent
// ---------------------------------------------------------------------------

/// Describes the behavior a mock agent should exhibit for a single incoming
/// request frame.
pub enum MockBehavior {
    /// Read one request frame and reply with the given response.
    Respond(Response),
    /// Read one request frame, sleep for `Duration`, then reply.
    Delay(Duration, Response),
    /// Read one request frame and then close the connection.
    Drop,
    /// Read one request frame and send invalid (non-protocol) bytes.
    SendGarbage,
    /// Accept the connection but never read or write (blocks forever).
    Hang,
}

/// Start a mock TCP agent whose behavior is scripted per-request.
///
/// The agent accepts exactly one connection and processes each `MockBehavior`
/// entry in sequence. After all behaviors are exhausted the connection is
/// closed.
pub async fn programmable_mock_agent(behaviors: Vec<MockBehavior>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();

        for behavior in behaviors {
            match behavior {
                MockBehavior::Respond(response) => {
                    // Read one request frame.
                    let mut header = [0u8; 4];
                    if stream.read_exact(&mut header).await.is_err() {
                        return;
                    }
                    let len = read_frame_length(&header) as usize;
                    let mut payload = vec![0u8; len];
                    if stream.read_exact(&mut payload).await.is_err() {
                        return;
                    }

                    // Send response.
                    let resp_bytes = encode_response(&response);
                    let _ = stream.write_all(&resp_bytes).await;
                    let _ = stream.flush().await;
                }
                MockBehavior::Delay(duration, response) => {
                    // Read one request frame.
                    let mut header = [0u8; 4];
                    if stream.read_exact(&mut header).await.is_err() {
                        return;
                    }
                    let len = read_frame_length(&header) as usize;
                    let mut payload = vec![0u8; len];
                    if stream.read_exact(&mut payload).await.is_err() {
                        return;
                    }

                    tokio::time::sleep(duration).await;

                    let resp_bytes = encode_response(&response);
                    let _ = stream.write_all(&resp_bytes).await;
                    let _ = stream.flush().await;
                }
                MockBehavior::Drop => {
                    // Read one request frame then close.
                    let mut header = [0u8; 4];
                    let _ = stream.read_exact(&mut header).await;
                    let len = read_frame_length(&header) as usize;
                    let mut payload = vec![0u8; len];
                    let _ = stream.read_exact(&mut payload).await;
                    return; // close connection
                }
                MockBehavior::SendGarbage => {
                    // Read one request frame.
                    let mut header = [0u8; 4];
                    if stream.read_exact(&mut header).await.is_err() {
                        return;
                    }
                    let len = read_frame_length(&header) as usize;
                    let mut payload = vec![0u8; len];
                    if stream.read_exact(&mut payload).await.is_err() {
                        return;
                    }

                    // Send garbage bytes (not valid protocol).
                    let garbage = [0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0xFF, 0x42, 0x13];
                    let _ = stream.write_all(&garbage).await;
                    let _ = stream.flush().await;
                }
                MockBehavior::Hang => {
                    // Accept but never respond — block forever.
                    std::future::pending::<()>().await;
                }
            }
        }
    });

    addr
}

// ---------------------------------------------------------------------------
// TestHarness — full-stack test fixture
// ---------------------------------------------------------------------------

/// A full-stack test harness that wires up a Session, mock agent, IPC server,
/// and ActionExecutor for integration testing.
pub struct TestHarness {
    /// The session under test.
    pub session: Arc<Session>,
    /// The unique session name (used for IPC socket path).
    pub session_name: String,
    /// Background task running the IPC server.
    _server_handle: tokio::task::JoinHandle<()>,
}

impl TestHarness {
    /// Create a fully wired test harness.
    ///
    /// This:
    /// 1. Creates a Session backed by a temp directory (avoids polluting `~/.qorvex/logs`).
    /// 2. Starts a mock TCP agent with the given canned responses.
    /// 3. Connects an AgentDriver to the mock and wraps it in an ActionExecutor.
    /// 4. Starts an IPC server for the session and sets the driver on it.
    /// 5. Waits briefly for the IPC socket to become available.
    pub async fn start(responses: Vec<Response>) -> Self {
        let session_name = unique_session_name();

        // Use a temp directory for log files so tests don't pollute ~/.qorvex/logs.
        let tmp_dir = std::env::temp_dir().join(format!("qorvex_test_{}", &session_name));
        let session = Session::new_with_log_dir(None, &session_name, tmp_dir);

        // Stand up a mock agent and connect a driver.
        let addr = mock_agent(responses).await;
        let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
        driver.connect().await.unwrap();
        let driver: Arc<dyn AutomationDriver> = Arc::new(driver);

        // Start the IPC server with the driver attached.
        let ipc_server = IpcServer::new(session.clone(), &session_name);
        ipc_server.set_driver(driver).await;

        let server_handle = tokio::spawn(async move {
            let _ = ipc_server.run().await;
        });

        // Give the IPC server a moment to bind the Unix socket.
        tokio::time::sleep(Duration::from_millis(50)).await;

        Self {
            session,
            session_name,
            _server_handle: server_handle,
        }
    }

    /// Connect an IPC client to this harness's session.
    pub async fn connect_client(&self) -> IpcClient {
        IpcClient::connect(&self.session_name).await.unwrap()
    }
}

impl Drop for TestHarness {
    fn drop(&mut self) {
        // Abort the IPC server task so the socket is cleaned up.
        self._server_handle.abort();
    }
}
