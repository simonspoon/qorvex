use std::sync::Arc;

use clap::Parser;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::oneshot;
use tokio::sync::Mutex;
use tracing::{debug, info, info_span, Instrument};

mod server;
use server::ServerState;

use qorvex_core::ipc::{socket_path, IpcError, IpcRequest, IpcResponse};

#[derive(Parser)]
#[command(name = "qorvex-server")]
#[command(about = "Standalone automation server for iOS Simulator")]
struct Args {
    /// Session name for IPC socket
    #[arg(short, long, default_value = "default", env = "QORVEX_SESSION")]
    session: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Setup logging
    let log_dir = qorvex_core::session::logs_dir();
    let file_appender = tracing_appender::rolling::never(&log_dir, "qorvex-server.log");
    tracing_subscriber::fmt()
        .with_writer(file_appender)
        .with_ansi(false)
        .init();

    info!(session = %args.session, "Starting qorvex-server");

    let state = Arc::new(Mutex::new(ServerState::new(args.session.clone())));

    // Remove existing socket
    let sock_path = socket_path(&args.session);
    let _ = std::fs::remove_file(&sock_path);

    let listener = UnixListener::bind(&sock_path)?;
    info!(path = %sock_path.display(), "Listening on socket");

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let shutdown_tx = Arc::new(Mutex::new(Some(shutdown_tx)));

    let mut sigterm = signal(SignalKind::terminate())?;

    tokio::select! {
        result = run_accept_loop(&listener, state.clone(), shutdown_tx.clone()) => {
            if let Err(e) = result {
                info!(error = %e, "Accept loop exited");
            }
        }
        _ = shutdown_rx => {
            info!("Shutdown requested via IPC");
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received SIGINT");
        }
        _ = sigterm.recv() => {
            info!("Received SIGTERM");
        }
    }

    cleanup(state, &sock_path).await;

    Ok(())
}

async fn run_accept_loop(
    listener: &UnixListener,
    state: Arc<Mutex<ServerState>>,
    shutdown_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
) -> Result<(), IpcError> {
    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        let shutdown_tx = shutdown_tx.clone();
        tokio::spawn(async move {
            let span = info_span!("ipc_client");
            if let Err(e) = handle_client(stream, state, shutdown_tx).instrument(span).await {
                debug!(error = %e, "Client disconnected");
            }
        });
    }
}

async fn cleanup(state: Arc<Mutex<ServerState>>, sock_path: &std::path::Path) {
    info!("Cleaning up");
    {
        let mut s = state.lock().await;
        if let Some(handle) = s.watcher_handle.take() {
            handle.cancel();
        }
        // AgentLifecycle::Drop will kill the agent child process
    }
    // drop state so ServerState destructors run
    drop(state);
    let _ = std::fs::remove_file(sock_path);
    info!("Server stopped");
}

async fn handle_client(
    stream: tokio::net::UnixStream,
    state: Arc<Mutex<ServerState>>,
    shutdown_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
) -> Result<(), IpcError> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }

        let request: IpcRequest = serde_json::from_str(line.trim())?;

        match request {
            IpcRequest::Shutdown => {
                info!("Shutdown requested by client");
                let response = IpcResponse::ShutdownAck;
                let json = serde_json::to_string(&response)? + "\n";
                writer.write_all(json.as_bytes()).await?;
                writer.flush().await?;
                if let Some(tx) = shutdown_tx.lock().await.take() {
                    let _ = tx.send(());
                }
                return Ok(());
            }
            IpcRequest::Subscribe => {
                // Subscribe is streaming — get session and stream events
                let session = {
                    let s = state.lock().await;
                    s.session.clone()
                };
                if let Some(session) = session {
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
                } else {
                    let response = IpcResponse::Error {
                        message: "No active session".to_string(),
                    };
                    let json = serde_json::to_string(&response)? + "\n";
                    writer.write_all(json.as_bytes()).await?;
                    writer.flush().await?;
                }
            }
            other => {
                // Check for element updates before handling
                {
                    let mut s = state.lock().await;
                    s.check_element_updates();
                }
                let response = {
                    let mut s = state.lock().await;
                    s.handle_request(other).await
                };
                let json = serde_json::to_string(&response)? + "\n";
                writer.write_all(json.as_bytes()).await?;
                writer.flush().await?;
            }
        }
    }
    Ok(())
}
