use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::action::ActionType;
use crate::session::{Session, SessionEvent};

#[derive(Error, Debug)]
pub enum IpcError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Session not found")]
    SessionNotFound,
}

/// Request from REPL client to server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IpcRequest {
    /// Execute an action
    Execute { action: ActionType },
    /// Subscribe to session events (for watcher)
    Subscribe,
    /// Get current session state
    GetState,
    /// Get action log
    GetLog,
}

/// Response from server to client
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IpcResponse {
    /// Action execution result
    ActionResult { success: bool, message: String, screenshot: Option<String> },
    /// Session state
    State { session_id: String, screenshot: Option<String> },
    /// Action log
    Log { entries: Vec<crate::action::ActionLog> },
    /// Session event (for subscribers)
    Event { event: SessionEvent },
    /// Error
    Error { message: String },
}

/// Get the socket path for a session
pub fn socket_path(session_name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("qorvex_{}.sock", session_name));
    path
}

/// IPC Server that manages a session
pub struct IpcServer {
    session: Arc<Session>,
    socket_path: PathBuf,
}

impl IpcServer {
    pub fn new(session: Arc<Session>, session_name: &str) -> Self {
        Self {
            session,
            socket_path: socket_path(session_name),
        }
    }

    /// Start the IPC server
    pub async fn run(&self) -> Result<(), IpcError> {
        // Remove existing socket
        let _ = std::fs::remove_file(&self.socket_path);

        let listener = UnixListener::bind(&self.socket_path)?;

        loop {
            let (stream, _) = listener.accept().await?;
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
                    // For now, just log the action - actual execution will be added
                    let log = session.log_action(
                        action,
                        crate::action::ActionResult::Success,
                        None
                    ).await;

                    let response = IpcResponse::ActionResult {
                        success: true,
                        message: format!("Executed: {:?}", log.action),
                        screenshot: log.screenshot,
                    };
                    let json = serde_json::to_string(&response)? + "\n";
                    writer.write_all(json.as_bytes()).await?;
                }
                IpcRequest::Subscribe => {
                    // Send events as they occur
                    let mut rx = session.subscribe();
                    while let Ok(event) = rx.recv().await {
                        let response = IpcResponse::Event { event };
                        let json = serde_json::to_string(&response)? + "\n";
                        if writer.write_all(json.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                }
                IpcRequest::GetState => {
                    let response = IpcResponse::State {
                        session_id: session.id.to_string(),
                        screenshot: session.get_screenshot().await,
                    };
                    let json = serde_json::to_string(&response)? + "\n";
                    writer.write_all(json.as_bytes()).await?;
                }
                IpcRequest::GetLog => {
                    let response = IpcResponse::Log {
                        entries: session.get_action_log().await,
                    };
                    let json = serde_json::to_string(&response)? + "\n";
                    writer.write_all(json.as_bytes()).await?;
                }
            }
        }
        Ok(())
    }

    pub fn socket_path(&self) -> &PathBuf {
        &self.socket_path
    }
}

/// IPC Client for REPL/Watcher
pub struct IpcClient {
    stream: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

impl IpcClient {
    pub async fn connect(session_name: &str) -> Result<Self, IpcError> {
        let path = socket_path(session_name);
        let stream = UnixStream::connect(&path).await?;
        let (reader, writer) = stream.into_split();
        Ok(Self {
            stream: BufReader::new(reader),
            writer,
        })
    }

    pub async fn send(&mut self, request: &IpcRequest) -> Result<IpcResponse, IpcError> {
        let json = serde_json::to_string(request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;

        let mut line = String::new();
        self.stream.read_line(&mut line).await?;
        let response: IpcResponse = serde_json::from_str(line.trim())?;
        Ok(response)
    }

    /// Subscribe and return receiver for events
    pub async fn subscribe(&mut self) -> Result<(), IpcError> {
        let request = IpcRequest::Subscribe;
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        Ok(())
    }

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
