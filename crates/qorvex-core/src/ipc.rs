//! Inter-process communication for REPL and watcher coordination.
//!
//! This module provides Unix socket-based IPC using a JSON-over-newlines protocol.
//! The REPL runs an [`IpcServer`] that accepts connections from watchers (TUI clients),
//! which connect using [`IpcClient`].
//!
//! # Protocol
//!
//! Communication uses a simple line-based JSON protocol:
//! - Each message is a single line of JSON followed by a newline
//! - Requests are sent from client to server using [`IpcRequest`]
//! - Responses are sent from server to client using [`IpcResponse`]
//!
//! # Socket Location
//!
//! Sockets are created in `~/.qorvex/` with the naming pattern
//! `qorvex_{session_name}.sock`. Use [`socket_path`] to get the path for a session.
//!
//! # Example
//!
//! ```no_run
//! use qorvex_core::ipc::{IpcClient, IpcRequest};
//!
//! #[tokio::main]
//! async fn main() {
//!     // Connect to a session
//!     let mut client = IpcClient::connect("my-session").await.unwrap();
//!
//!     // Request current state
//!     let response = client.send(&IpcRequest::GetState).await.unwrap();
//!     println!("Response: {:?}", response);
//! }
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::action::{ActionResult, ActionType};
use crate::executor::ActionExecutor;
use crate::session::{Session, SessionEvent};

/// Errors that can occur during IPC operations.
#[derive(Error, Debug)]
pub enum IpcError {
    /// An I/O error occurred (connection, read, write).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Failed to serialize or deserialize JSON.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// The requested session was not found.
    #[error("Session not found")]
    SessionNotFound,
}

/// A request sent from client to server over the IPC connection.
///
/// Requests are serialized as JSON with a `type` tag discriminator.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IpcRequest {
    /// Execute an action on the simulator.
    Execute {
        /// The action to perform.
        action: ActionType,
    },

    /// Subscribe to session events.
    ///
    /// After sending this request, the server will stream [`IpcResponse::Event`]
    /// messages whenever the session state changes.
    Subscribe,

    /// Request the current session state.
    GetState,

    /// Request the action log history.
    GetLog,
}

/// A response sent from server to client over the IPC connection.
///
/// Responses are serialized as JSON with a `type` tag discriminator.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IpcResponse {
    /// Result of an action execution.
    ActionResult {
        /// Whether the action succeeded.
        success: bool,
        /// Human-readable description of the result.
        message: String,
        /// Screenshot taken after the action (base64-encoded PNG), wrapped in Arc for efficiency.
        screenshot: Option<Arc<String>>,
        /// Additional data returned by the action (JSON).
        data: Option<String>,
    },

    /// Current session state.
    State {
        /// The session's unique identifier.
        session_id: String,
        /// The current screenshot (base64-encoded PNG), wrapped in Arc for efficiency.
        screenshot: Option<Arc<String>>,
    },

    /// Action log history.
    Log {
        /// All logged actions in chronological order.
        entries: Vec<crate::action::ActionLog>,
    },

    /// A session event (sent to subscribers).
    Event {
        /// The event that occurred.
        event: SessionEvent,
    },

    /// An error occurred processing the request.
    Error {
        /// Human-readable error message.
        message: String,
    },
}

/// Returns the qorvex directory path (`~/.qorvex/`).
///
/// Creates the directory if it doesn't exist.
///
/// # Panics
///
/// Panics if the home directory cannot be determined.
pub fn qorvex_dir() -> PathBuf {
    let dir = dirs::home_dir()
        .expect("Could not determine home directory")
        .join(".qorvex");
    std::fs::create_dir_all(&dir).ok();
    dir
}

/// Returns the Unix socket path for a session.
///
/// The socket is created in `~/.qorvex/` with the pattern
/// `qorvex_{session_name}.sock`.
///
/// # Arguments
///
/// * `session_name` - The name of the session
///
/// # Returns
///
/// A `PathBuf` pointing to the socket location (e.g., `~/.qorvex/qorvex_my-session.sock`).
pub fn socket_path(session_name: &str) -> PathBuf {
    qorvex_dir().join(format!("qorvex_{}.sock", session_name))
}

/// Unix socket server for IPC communication.
///
/// The server accepts connections from clients (typically the watcher TUI)
/// and handles requests by interacting with the associated [`Session`].
///
/// The server automatically cleans up the socket file when dropped.
pub struct IpcServer {
    /// The session managed by this server.
    session: Arc<Session>,
    /// Path to the Unix socket file.
    socket_path: PathBuf,
}

impl IpcServer {
    /// Creates a new IPC server for the given session.
    ///
    /// # Arguments
    ///
    /// * `session` - The session to manage
    /// * `session_name` - The name used to determine the socket path
    ///
    /// # Returns
    ///
    /// A new `IpcServer` instance (not yet running).
    pub fn new(session: Arc<Session>, session_name: &str) -> Self {
        Self {
            session,
            socket_path: socket_path(session_name),
        }
    }

    /// Starts the IPC server and begins accepting connections.
    ///
    /// This method runs indefinitely, accepting client connections and spawning
    /// a handler task for each. Each client is handled independently.
    ///
    /// Any existing socket file at the path is removed before binding.
    ///
    /// # Errors
    ///
    /// - [`IpcError::Io`] if the socket cannot be bound or an accept fails
    ///
    /// # Note
    ///
    /// This method never returns under normal operation. Use it with
    /// `tokio::spawn` or `tokio::select!` for concurrent operation.
    pub async fn run(&self) -> Result<(), IpcError> {
        // Remove existing socket (ignore NotFound errors as socket may not exist)
        if let Err(e) = std::fs::remove_file(&self.socket_path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!("[ipc] Failed to remove existing socket: {}", e);
            }
        }

        let listener = UnixListener::bind(&self.socket_path)?;
        eprintln!("[ipc] Server listening on {:?}", self.socket_path);

        loop {
            let (stream, _) = listener.accept().await?;
            eprintln!("[ipc] Client connected");
            let session = self.session.clone();

            tokio::spawn(async move {
                if let Err(e) = Self::handle_client(stream, session).await {
                    eprintln!("Client error: {}", e);
                }
            });
        }
    }

    async fn handle_client(stream: UnixStream, session: Arc<Session>) -> Result<(), IpcError> {
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                break; // Client disconnected
            }

            let request: IpcRequest = serde_json::from_str(line.trim())?;

            match request {
                IpcRequest::Execute { action } => {
                    // Execute the action using the ActionExecutor
                    // LogComment doesn't require a simulator, handle it specially
                    let response = if matches!(action, ActionType::LogComment { .. }) {
                        // LogComment can run without a simulator
                        let executor = ActionExecutor::new(String::new());
                        let result = executor.execute(action.clone()).await;

                        session.log_action(action, ActionResult::Success, None).await;

                        IpcResponse::ActionResult {
                            success: result.success,
                            message: result.message,
                            screenshot: None,
                            data: result.data,
                        }
                    } else {
                        match &session.simulator_udid {
                            Some(udid) => {
                                let executor = ActionExecutor::new(udid.clone());
                                let result = executor.execute(action.clone()).await;

                                // Log to session
                                let action_result = if result.success {
                                    ActionResult::Success
                                } else {
                                    ActionResult::Failure(result.message.clone())
                                };
                                session.log_action(action, action_result, result.screenshot.clone()).await;

                                IpcResponse::ActionResult {
                                    success: result.success,
                                    message: result.message,
                                    screenshot: result.screenshot.map(Arc::new),
                                    data: result.data,
                                }
                            }
                            None => {
                                IpcResponse::Error {
                                    message: "No simulator selected for this session".to_string(),
                                }
                            }
                        }
                    };

                    let json = serde_json::to_string(&response)? + "\n";
                    writer.write_all(json.as_bytes()).await?;
                    writer.flush().await?;
                }
                IpcRequest::Subscribe => {
                    eprintln!("[ipc] Client subscribed to session events");
                    // Send events as they occur
                    let mut rx = session.subscribe();
                    while let Ok(event) = rx.recv().await {
                        eprintln!("[ipc] Broadcasting event to client: {:?}", std::mem::discriminant(&event));
                        let response = IpcResponse::Event { event };
                        let json = serde_json::to_string(&response)? + "\n";
                        if writer.write_all(json.as_bytes()).await.is_err() {
                            eprintln!("[ipc] Failed to write event to client");
                            break;
                        }
                        if writer.flush().await.is_err() {
                            eprintln!("[ipc] Failed to flush event to client");
                            break;
                        }
                    }
                    eprintln!("[ipc] Client subscription ended");
                }
                IpcRequest::GetState => {
                    let response = IpcResponse::State {
                        session_id: session.id.to_string(),
                        screenshot: session.get_screenshot().await,
                    };
                    let json = serde_json::to_string(&response)? + "\n";
                    writer.write_all(json.as_bytes()).await?;
                    writer.flush().await?;
                }
                IpcRequest::GetLog => {
                    let response = IpcResponse::Log {
                        entries: session.get_action_log().await,
                    };
                    let json = serde_json::to_string(&response)? + "\n";
                    writer.write_all(json.as_bytes()).await?;
                    writer.flush().await?;
                }
            }
        }
        Ok(())
    }

    /// Returns a reference to the socket path.
    pub fn socket_path(&self) -> &PathBuf {
        &self.socket_path
    }
}

/// Unix socket client for IPC communication.
///
/// Used by watchers (TUI clients) to connect to a running REPL session
/// and receive updates or send commands.
pub struct IpcClient {
    /// Buffered reader for the socket's read half.
    stream: BufReader<tokio::net::unix::OwnedReadHalf>,
    /// Writer for the socket's write half.
    writer: tokio::net::unix::OwnedWriteHalf,
}

impl IpcClient {
    /// Connects to an IPC server for the specified session.
    ///
    /// # Arguments
    ///
    /// * `session_name` - The name of the session to connect to
    ///
    /// # Returns
    ///
    /// A connected `IpcClient` instance.
    ///
    /// # Errors
    ///
    /// - [`IpcError::Io`] if the connection fails (e.g., server not running)
    pub async fn connect(session_name: &str) -> Result<Self, IpcError> {
        let path = socket_path(session_name);
        let stream = UnixStream::connect(&path).await?;
        let (reader, writer) = stream.into_split();
        Ok(Self {
            stream: BufReader::new(reader),
            writer,
        })
    }

    /// Sends a request and waits for the response.
    ///
    /// This method serializes the request, sends it to the server,
    /// and waits for a single response line.
    ///
    /// # Arguments
    ///
    /// * `request` - The request to send
    ///
    /// # Returns
    ///
    /// The server's response.
    ///
    /// # Errors
    ///
    /// - [`IpcError::Io`] if the send or receive fails
    /// - [`IpcError::Json`] if serialization or deserialization fails
    pub async fn send(&mut self, request: &IpcRequest) -> Result<IpcResponse, IpcError> {
        let json = serde_json::to_string(request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        self.writer.flush().await?;

        let mut line = String::new();
        self.stream.read_line(&mut line).await?;
        let response: IpcResponse = serde_json::from_str(line.trim())?;
        Ok(response)
    }

    /// Sends a subscribe request to the server.
    ///
    /// After calling this method, use [`Self::read_event`] to receive
    /// session events as they occur. The server will stream events until
    /// the connection is closed.
    ///
    /// # Errors
    ///
    /// - [`IpcError::Io`] if the send fails
    /// - [`IpcError::Json`] if serialization fails
    pub async fn subscribe(&mut self) -> Result<(), IpcError> {
        let request = IpcRequest::Subscribe;
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        self.writer.flush().await?;
        Ok(())
    }

    /// Reads the next event from the server.
    ///
    /// This method blocks until an event is received. It should be called
    /// in a loop after [`Self::subscribe`] to process incoming events.
    ///
    /// # Returns
    ///
    /// The next [`IpcResponse`] from the server (typically an `Event` variant).
    ///
    /// # Errors
    ///
    /// - [`IpcError::Io`] if the read fails (e.g., server disconnected)
    /// - [`IpcError::Json`] if deserialization fails
    pub async fn read_event(&mut self) -> Result<IpcResponse, IpcError> {
        let mut line = String::new();
        self.stream.read_line(&mut line).await?;
        let response: IpcResponse = serde_json::from_str(line.trim())?;
        Ok(response)
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.socket_path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!("[ipc] Failed to cleanup socket on drop: {}", e);
            }
        }
    }
}
