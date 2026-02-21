//! Inter-process communication for server and client coordination.
//!
//! This module provides Unix socket-based IPC using a JSON-over-newlines protocol.
//! The server runs an [`IpcServer`] that accepts connections from watchers (TUI clients),
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
use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use tracing::{debug, info_span, Instrument};

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

    // --- Session Management ---
    /// Start a new automation session.
    StartSession,
    /// End the current session.
    EndSession,

    // --- Device Management ---
    /// List available simulator devices.
    ListDevices,
    /// Select a simulator device by UDID.
    UseDevice { udid: String },
    /// Boot a simulator device.
    BootDevice { udid: String },

    // --- Agent Management ---
    /// Start or connect to the automation agent.
    StartAgent { project_dir: Option<String> },
    /// Stop the managed agent process.
    StopAgent,
    /// Connect to agent at a specific host/port.
    Connect { host: String, port: u16 },

    // --- Configuration ---
    /// Set the target app bundle ID.
    SetTarget { bundle_id: String },
    /// Set the default wait timeout in milliseconds.
    SetTimeout { timeout_ms: u64 },
    /// Get the current default wait timeout.
    GetTimeout,

    // --- Watcher ---
    /// Start the screen change watcher.
    StartWatcher { interval_ms: Option<u64> },
    /// Stop the screen change watcher.
    StopWatcher,

    // --- Info ---
    /// Get current session information.
    GetSessionInfo,
    /// Get cached elements and devices for client-side tab completion.
    GetCompletionData,

    // --- Server Lifecycle ---
    /// Request the server to shut down cleanly.
    ///
    /// The server will stop the agent, remove the socket, and exit.
    Shutdown,
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

    /// Generic success/failure result for management commands.
    CommandResult {
        /// Whether the command succeeded.
        success: bool,
        /// Human-readable description of the result.
        message: String,
    },

    /// List of simulator devices.
    DeviceList {
        /// Available simulator devices.
        devices: Vec<crate::simctl::SimulatorDevice>,
    },

    /// Current session information.
    SessionInfo {
        /// Session name.
        session_name: String,
        /// Whether a session is currently active.
        active: bool,
        /// Connected device UDID, if any.
        device_udid: Option<String>,
        /// Number of actions logged this session.
        action_count: usize,
    },

    /// Cached completion data for client-side tab completion.
    CompletionData {
        /// Cached UI elements from the last screen info.
        elements: Vec<crate::element::UIElement>,
        /// Cached simulator devices.
        devices: Vec<crate::simctl::SimulatorDevice>,
    },

    /// Current timeout value.
    TimeoutValue {
        /// Default timeout in milliseconds.
        timeout_ms: u64,
    },

    /// Acknowledgement that the server is shutting down.
    ShutdownAck,
}

/// Trait for handling IPC requests.
///
/// Implement this trait to provide custom request handling logic for the IPC server.
/// The handler receives parsed requests and writes responses directly to the client.
#[async_trait]
pub trait RequestHandler: Send + Sync + 'static {
    /// Handle a single IPC request.
    ///
    /// For streaming requests like Subscribe, the handler should write multiple
    /// responses to the writer. For single-response requests, write one response
    /// and return.
    async fn handle(
        &self,
        request: IpcRequest,
        session: Arc<Session>,
        writer: &mut tokio::net::unix::OwnedWriteHalf,
    ) -> Result<(), IpcError>;
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
    /// Shared driver slot, populated when the automation backend connects.
    /// IPC Execute requests use this driver instead of creating new connections.
    shared_driver: Arc<tokio::sync::Mutex<Option<Arc<dyn crate::driver::AutomationDriver>>>>,
    /// Optional pluggable request handler. When set, all requests are delegated
    /// to this handler instead of the built-in logic.
    handler: Option<Arc<dyn RequestHandler>>,
}

impl IpcServer {
    /// Creates a new IPC server for the given session.
    ///
    /// The server starts without a connected driver. Call [`set_driver`](Self::set_driver)
    /// or use the returned [`shared_driver`](Self::shared_driver) handle to provide
    /// a connected driver before Execute requests will work.
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
            shared_driver: Arc::new(tokio::sync::Mutex::new(None)),
            handler: None,
        }
    }

    /// Sets a pluggable request handler on this server.
    ///
    /// When a handler is set, all incoming IPC requests are delegated to it
    /// instead of the built-in hardcoded logic. This allows external crates
    /// (e.g., `qorvex-server`) to provide their own request handling.
    ///
    /// # Arguments
    ///
    /// * `handler` - The request handler implementation
    ///
    /// # Returns
    ///
    /// The server instance (builder pattern).
    pub fn with_handler(mut self, handler: Arc<dyn RequestHandler>) -> Self {
        self.handler = Some(handler);
        self
    }

    /// Returns the shared driver slot.
    ///
    /// Callers can clone this handle and populate it with a connected driver
    /// so that IPC Execute requests use the same backend connection.
    pub fn shared_driver(&self) -> Arc<tokio::sync::Mutex<Option<Arc<dyn crate::driver::AutomationDriver>>>> {
        self.shared_driver.clone()
    }

    /// Sets the automation driver used by IPC Execute requests.
    pub async fn set_driver(&self, driver: Arc<dyn crate::driver::AutomationDriver>) {
        *self.shared_driver.lock().await = Some(driver);
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
        // Remove existing socket (ignore errors)
        let _ = std::fs::remove_file(&self.socket_path);

        let listener = UnixListener::bind(&self.socket_path)?;

        loop {
            let (stream, _) = listener.accept().await?;
            let session = self.session.clone();
            let shared_driver = self.shared_driver.clone();
            let handler = self.handler.clone();

            tokio::spawn(async move {
                let span = info_span!("ipc_client");
                let _ = Self::handle_client(stream, session, shared_driver, handler).instrument(span).await;
            });
        }
    }

    async fn handle_client(
        stream: UnixStream,
        session: Arc<Session>,
        shared_driver: Arc<tokio::sync::Mutex<Option<Arc<dyn crate::driver::AutomationDriver>>>>,
        handler: Option<Arc<dyn RequestHandler>>,
    ) -> Result<(), IpcError> {
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

            if let Some(ref handler) = handler {
                handler.handle(request, session.clone(), &mut writer).await?;
                continue;
            }

            // Fallback: built-in hardcoded logic (backward compatibility)
            match request {
                IpcRequest::Execute { action } => {
                    debug!(action = %action.name(), "executing action via IPC");
                    // Execute the action using the ActionExecutor
                    // LogComment doesn't require a driver
                    let response = if let ActionType::LogComment { ref message } = action {
                        let msg = format!("Logged: {}", message);
                        session.log_action(action, ActionResult::Success, None, None).await;

                        IpcResponse::ActionResult {
                            success: true,
                            message: msg,
                            screenshot: None,
                            data: None,
                        }
                    } else {
                        let driver_guard = shared_driver.lock().await;
                        match driver_guard.as_ref() {
                            Some(driver) => {
                                let executor = ActionExecutor::new(driver.clone());
                                drop(driver_guard); // release lock before executing
                                let result = executor.execute(action.clone()).await;

                                // Log to session
                                let action_result = if result.success {
                                    ActionResult::Success
                                } else {
                                    ActionResult::Failure(result.message.clone())
                                };
                                let duration_ms = result.data.as_ref()
                                    .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
                                    .and_then(|v| v.get("elapsed_ms").and_then(|e| e.as_u64()));
                                session.log_action(action, action_result, result.screenshot.clone(), duration_ms).await;

                                IpcResponse::ActionResult {
                                    success: result.success,
                                    message: result.message,
                                    screenshot: result.screenshot.map(Arc::new),
                                    data: result.data,
                                }
                            }
                            None => {
                                IpcResponse::Error {
                                    message: "No automation backend connected for this session".to_string(),
                                }
                            }
                        }
                    };

                    let json = serde_json::to_string(&response)? + "\n";
                    writer.write_all(json.as_bytes()).await?;
                    writer.flush().await?;
                }
                IpcRequest::Subscribe => {
                    debug!("client subscribing to events");
                    // Send events as they occur
                    let mut rx = session.subscribe();
                    while let Ok(event) = rx.recv().await {
                        let response = IpcResponse::Event { event };
                        let json = serde_json::to_string(&response)? + "\n";
                        if writer.write_all(json.as_bytes()).await.is_err() {
                            break;
                        }
                        if writer.flush().await.is_err() {
                            break;
                        }
                    }
                }
                IpcRequest::GetState => {
                    debug!("client requesting state");
                    let response = IpcResponse::State {
                        session_id: session.id.to_string(),
                        screenshot: session.get_screenshot().await,
                    };
                    let json = serde_json::to_string(&response)? + "\n";
                    writer.write_all(json.as_bytes()).await?;
                    writer.flush().await?;
                }
                IpcRequest::GetLog => {
                    debug!("client requesting log");
                    let response = IpcResponse::Log {
                        entries: session.get_action_log().await,
                    };
                    let json = serde_json::to_string(&response)? + "\n";
                    writer.write_all(json.as_bytes()).await?;
                    writer.flush().await?;
                }
                _ => {
                    let response = IpcResponse::Error {
                        message: "This server does not support management commands. Use qorvex-server instead.".to_string(),
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
        let _ = std::fs::remove_file(&self.socket_path);
    }
}
